//! The per-connection WebSocket actor loop: `hello` → `open` → `snapshot`, then a
//! `select!` between inbound client frames and this document's outbound broadcast channel, until
//! the socket closes.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::authz::{self, PermissionLevel};
use super::bootstrap;
use super::frame::{Frame, PROTOCOL_VERSION, RejectedCode, TailUpdate};
use super::limits::{PRESENCE_TTL_SECONDS_DEFAULT, PRESENCE_TTL_SECONDS_MAX, WEBSOCKET_FRAME_BYTES_MAX};
use super::registry::{OutboundEvent, PresenceLimit};
use super::runtime;
use super::ticket::ConsumedTicket;
use super::write::{self, AcceptOutcome, UpdateRequest};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A close code this module owns: policy violation, matching RFC 6455's registered meaning
/// closely enough (protocol error / auth failure at handshake) without colliding with the
/// contract's own frozen 4410 drain code.
const CLOSE_POLICY_VIOLATION: u16 = 1008;
const CLOSE_UNSUPPORTED_DATA: u16 = 1003;

struct DocumentContext {
    object_id: Uuid,
    checked_epoch: i64,
}

async fn fetch_document_object_id(db: &sea_orm::DatabaseConnection, document_id: Uuid) -> Option<Uuid> {
    #[derive(FromQueryResult)]
    struct Row {
        object_id: Uuid,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map(|r| r.object_id)
}

async fn fetch_role(db: &sea_orm::DatabaseConnection, workspace_id: Uuid, user_id: Uuid) -> Option<String> {
    #[derive(FromQueryResult)]
    struct Row {
        role: String,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map(|r| r.role)
}

fn encode(frame: &Frame) -> Option<Message> {
    serde_json::to_string(frame).ok().map(Message::text)
}

async fn send(socket: &mut WebSocket, frame: &Frame) {
    if let Some(msg) = encode(frame)
        && let Err(err) = socket.send(msg).await
    {
        tracing::debug!(error = %err, "collab session: send failed, connection is likely already gone");
    }
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

const fn rejected_frame(document_id: Uuid, code: RejectedCode, recoverable: bool, update_id: Option<Uuid>) -> Frame {
    Frame::Rejected {
        protocol_version: PROTOCOL_VERSION,
        document_id,
        update_id,
        code,
        recoverable,
        details: None,
        current_seq: None,
        current_frontier: None,
        audit_event_id: None,
    }
}

/// Runs one WebSocket session end to end. Consumes `socket` and never returns an error: every
/// failure this function can observe is either a clean protocol-level `rejected`/close (sent on
/// the wire) or a best-effort log, because by the time this runs the HTTP upgrade has already
/// completed and there is no more "the request failed" response left to give.
pub async fn run(mut socket: WebSocket, state: AppState, consumed: ConsumedTicket) {
    let session_id = Uuid::new_v4();
    let document_id = consumed.document_id;
    let collab = runtime::runtime();

    // ---- hello ----
    let Some(hello) = read_frame(&mut socket, HANDSHAKE_TIMEOUT).await else {
        return;
    };
    let Frame::Hello { protocol_version, .. } = hello else {
        send(
            &mut socket,
            &rejected_frame(document_id, RejectedCode::UnsupportedProtocol, false, None),
        )
        .await;
        close(&mut socket, CLOSE_POLICY_VIOLATION, "expected hello").await;
        return;
    };
    if protocol_version != PROTOCOL_VERSION {
        send(
            &mut socket,
            &rejected_frame(document_id, RejectedCode::UnsupportedProtocol, false, None),
        )
        .await;
        close(&mut socket, CLOSE_POLICY_VIOLATION, "unsupported protocol_version").await;
        return;
    }
    send(
        &mut socket,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec!["presence".to_string()],
            client_id: "server".to_string(),
            session_id,
        },
    )
    .await;

    // ---- open ----
    let Some(open) = read_frame(&mut socket, HANDSHAKE_TIMEOUT).await else {
        return;
    };
    let Frame::Open {
        document_id: opened_document_id,
        ..
    } = open
    else {
        send(
            &mut socket,
            &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
        )
        .await;
        close(&mut socket, CLOSE_POLICY_VIOLATION, "expected open").await;
        return;
    };
    if opened_document_id != document_id {
        send(
            &mut socket,
            &rejected_frame(document_id, RejectedCode::Forbidden, false, None),
        )
        .await;
        close(
            &mut socket,
            CLOSE_POLICY_VIOLATION,
            "open.document_id does not match the ticket",
        )
        .await;
        return;
    }

    let Some(ctx) = reverify_open(&state, &consumed, &mut socket).await else {
        return;
    };

    // ---- snapshot ----
    let Ok(boot) = bootstrap::load(&state.db, document_id).await else {
        send(
            &mut socket,
            &rejected_frame(document_id, RejectedCode::ResyncRequired, true, None),
        )
        .await;
        close(&mut socket, CLOSE_POLICY_VIOLATION, "bootstrap failed").await;
        return;
    };
    send(
        &mut socket,
        &Frame::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            snapshot_seq: boot.snapshot_seq,
            head_seq: boot.head_seq,
            snapshot: BASE64.encode(&boot.snapshot),
            tail_updates: boot
                .tail_updates
                .iter()
                .map(|u| TailUpdate {
                    seq: u.seq,
                    update_id: u.update_id,
                    bytes: BASE64.encode(&u.bytes),
                    before_frontier: BASE64.encode(&u.before_frontier),
                    after_frontier: BASE64.encode(&u.after_frontier),
                })
                .collect(),
            head_frontier: BASE64.encode(&boot.head_frontier),
        },
    )
    .await;

    // ---- steady state ----
    let mut outbound_rx = collab.registry.register(document_id, session_id);
    let checked_epoch = ctx.checked_epoch;

    loop {
        tokio::select! {
            biased;
            event = outbound_rx.recv() => {
                match event {
                    Some(OutboundEvent::Frame(frame)) => send(&mut socket, &frame).await,
                    Some(OutboundEvent::Close { code, reason }) => {
                        close(&mut socket, code, &reason).await;
                        break;
                    }
                    None => break,
                }
            }
            incoming = socket.recv() => {
                let Some(incoming) = incoming else { break };
                let Ok(message) = incoming else { break };
                match message {
                    Message::Close(_) => break,
                    Message::Text(text) => {
                        if text.len() > WEBSOCKET_FRAME_BYTES_MAX {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::LimitExceeded, false, None)).await;
                            continue;
                        }
                        let Ok(frame) = serde_json::from_str::<Frame>(text.as_str()) else {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None)).await;
                            continue;
                        };
                        handle_client_frame(
                            &state,
                            collab,
                            document_id,
                            ctx.object_id,
                            session_id,
                            consumed.user_id,
                            &consumed.client_id,
                            checked_epoch,
                            frame,
                            &mut socket,
                        )
                        .await;
                    }
                    Message::Binary(bytes) => {
                        if bytes.len() > WEBSOCKET_FRAME_BYTES_MAX {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::LimitExceeded, false, None)).await;
                        } else {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::UnsupportedProtocol, false, None)).await;
                            close(&mut socket, CLOSE_UNSUPPORTED_DATA, "binary frames are not supported").await;
                            break;
                        }
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        }
    }

    collab.registry.unregister(document_id, session_id);
}

async fn reverify_open(state: &AppState, consumed: &ConsumedTicket, socket: &mut WebSocket) -> Option<DocumentContext> {
    let document_id = consumed.document_id;
    let Some(object_id) = fetch_document_object_id(&state.db, document_id).await else {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::NotFound, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "document not found").await;
        return None;
    };
    let Ok(flow_enabled) = crate::flow::repository::fetch_flow_enabled(&state.db, consumed.workspace_id).await else {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::FeatureDisabled, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "flow is not enabled").await;
        return None;
    };
    if !flow_enabled {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::FeatureDisabled, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "flow is not enabled").await;
        return None;
    }
    let Some(role) = fetch_role(&state.db, consumed.workspace_id, consumed.user_id).await else {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::Forbidden, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "not a workspace member").await;
        return None;
    };
    let Ok(level) = authz::effective_permission(
        &state.db,
        consumed.workspace_id,
        object_id,
        "user",
        consumed.user_id,
        &role,
    )
    .await
    else {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::Forbidden, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "permission check failed").await;
        return None;
    };
    if level < PermissionLevel::Edit {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::Forbidden, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "insufficient permission").await;
        return None;
    }
    let Ok(checked_epoch) = authz::read_epoch(&state.db, consumed.workspace_id).await else {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::Forbidden, false, None),
        )
        .await;
        close(socket, CLOSE_POLICY_VIOLATION, "epoch read failed").await;
        return None;
    };
    Some(DocumentContext {
        object_id,
        checked_epoch,
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_client_frame(
    state: &AppState,
    collab: &runtime::CollabRuntime,
    document_id: Uuid,
    object_id: Uuid,
    session_id: Uuid,
    actor_id: Uuid,
    origin_client_id: &str,
    checked_epoch: i64,
    frame: Frame,
    socket: &mut WebSocket,
) {
    match frame {
        Frame::Update {
            document_id: frame_document_id,
            update_id,
            bytes,
            idempotency_key,
            message,
            ..
        } => {
            if frame_document_id != document_id {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, Some(update_id)),
                )
                .await;
                return;
            }
            let Ok(raw_bytes) = BASE64.decode(bytes) else {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, Some(update_id)),
                )
                .await;
                return;
            };
            // Kept for the peer relay below: `accept_update` takes ownership of its own copies,
            // and a successful commit does not otherwise hand the applied bytes back (`Accepted`
            // deliberately carries no `bytes` field — the whole point of relaying is to let *other*
            // sessions apply the same bytes this session already has locally).
            let relay_bytes = raw_bytes.clone();
            let relay_idempotency_key = idempotency_key.clone();
            let relay_message = message.clone();
            let outcome = write::accept_update(
                &state.db,
                &collab.cache,
                &collab.coordinator,
                crate::config::runtime().flow.dispatch_max_attempts,
                UpdateRequest {
                    document_id,
                    update_id,
                    bytes: raw_bytes,
                    idempotency_key,
                    origin_client_id: Some(origin_client_id.to_string()),
                    message,
                    actor_id,
                    workspace_id: if let Some(id) = fetch_object_workspace_id(state, object_id).await {
                        id
                    } else {
                        send(
                            socket,
                            &rejected_frame(document_id, RejectedCode::NotFound, false, Some(update_id)),
                        )
                        .await;
                        return;
                    },
                    checked_epoch,
                    expected_frontier: None,
                },
            )
            .await;

            match outcome {
                Ok(AcceptOutcome::Accepted(accepted)) => {
                    let frame = Frame::Accepted {
                        protocol_version: PROTOCOL_VERSION,
                        document_id,
                        update_id: accepted.update_id,
                        head_seq: accepted.head_seq,
                        head_frontier: BASE64.encode(&accepted.head_frontier),
                        projection_seq: accepted.projection_seq,
                        event_id: accepted.event_id,
                    };
                    send(socket, &frame).await;
                    // Peers other than the committer get the actual `update` bytes first — the
                    // frame `collab-protocol-v1.md`'s field table already defines for exactly this
                    // payload shape, relayed rather than invented — immediately followed by this
                    // same `accepted` (both sent over the one per-session channel `registry`
                    // already owns, so order is preserved: no second broadcast mechanism). A peer
                    // applies `update.bytes` (commutative CRDT import, order-independent) and uses
                    // the trailing `accepted.seq` as its gap-detection/`saved` anchor, exactly like
                    // `collab-protocol-v1.md`'s "客户端仅在 accepted.seq==last_applied_seq+1 时应用"
                    // rule already requires for its own commit ack.
                    let relay_frame = Frame::Update {
                        protocol_version: PROTOCOL_VERSION,
                        document_id,
                        update_id: accepted.update_id,
                        base_frontier: BASE64.encode(&accepted.before_frontier),
                        bytes: BASE64.encode(&relay_bytes),
                        idempotency_key: relay_idempotency_key,
                        origin: origin_client_id.to_string(),
                        message: relay_message,
                    };
                    broadcast_content_update(collab, document_id, Some(session_id), &relay_frame, &frame);
                }
                Ok(AcceptOutcome::Rejected(rejected)) => {
                    send(
                        socket,
                        &Frame::Rejected {
                            protocol_version: PROTOCOL_VERSION,
                            document_id,
                            update_id: rejected.update_id,
                            code: rejected.code,
                            recoverable: rejected.recoverable,
                            details: rejected.details,
                            current_seq: rejected.current_seq,
                            current_frontier: rejected.current_frontier.map(|f| BASE64.encode(f)),
                            audit_event_id: None,
                        },
                    )
                    .await;
                }
                Err(err) => {
                    tracing::error!(error = %err, "collab session: accept_update failed");
                    send(
                        socket,
                        &Frame::Rejected {
                            protocol_version: PROTOCOL_VERSION,
                            document_id,
                            update_id: Some(update_id),
                            code: RejectedCode::ServerDraining,
                            recoverable: true,
                            details: Some(serde_json::json!({"reason": "contention", "retry_after_ms": 500})),
                            current_seq: None,
                            current_frontier: None,
                            audit_event_id: None,
                        },
                    )
                    .await;
                }
            }
        }
        Frame::Presence {
            document_id: frame_document_id,
            payload,
            ttl_seconds,
            ..
        } => {
            if frame_document_id != document_id {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
                )
                .await;
                return;
            }
            let ttl_seconds = ttl_seconds.unwrap_or(PRESENCE_TTL_SECONDS_DEFAULT);
            if ttl_seconds == 0 {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
                )
                .await;
                return;
            }
            if ttl_seconds > PRESENCE_TTL_SECONDS_MAX {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::LimitExceeded, false, None),
                )
                .await;
                return;
            }
            let result = collab.registry.upsert_presence(
                document_id,
                session_id,
                payload.clone(),
                Duration::from_secs(u64::from(ttl_seconds)),
            );
            match result {
                Ok(()) => {
                    let frame = Frame::Presence {
                        protocol_version: PROTOCOL_VERSION,
                        document_id,
                        session_id,
                        payload,
                        ttl_seconds: Some(ttl_seconds),
                    };
                    collab.registry.broadcast(document_id, &frame, Some(session_id));
                }
                Err(PresenceLimit::PerConnection | PresenceLimit::PerDocument) => {
                    send(
                        socket,
                        &rejected_frame(document_id, RejectedCode::LimitExceeded, false, None),
                    )
                    .await;
                }
            }
        }
        Frame::Ping { nonce, .. } => {
            send(
                socket,
                &Frame::Pong {
                    protocol_version: PROTOCOL_VERSION,
                    nonce,
                },
            )
            .await;
        }
        Frame::Ack { .. } => {}
        Frame::Hello { .. }
        | Frame::Open { .. }
        | Frame::Snapshot { .. }
        | Frame::Accepted { .. }
        | Frame::Rejected { .. }
        | Frame::Resync { .. }
        | Frame::Pong { .. } => {
            send(
                socket,
                &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
            )
            .await;
        }
    }
}

/// Broadcasts one committed content write to every other session with `document_id` open: the
/// `update` frame carrying the actual CRDT bytes (so a peer can apply it incrementally, not just
/// learn that `head_seq` moved), immediately followed by the `accepted` frame carrying the
/// resulting seq/frontier/projection-seq anchor.
///
/// This is the *only* place either frame is broadcast from — the WebSocket write path above and
/// `flow::command::execute_command`'s REST `POST .../commands` path (`ADR-0010`'s "enqueue ordered
/// egress notice" step, `collab-protocol-v1.md`: "复用 ADR-0010 的 ordered egress / invalidation
/// 通道，不另起一套") both call this instead of touching `collab.registry` directly, so there is
/// exactly one broadcast call site regardless of which write path produced the commit. Both calls
/// happen strictly after their write already committed; ordering across commits for one document
/// on this instance falls out of the per-document coordinator serializing writers and every commit
/// broadcasting exactly once, in commit order (see `registry::SessionRegistry::broadcast`'s own doc
/// comment for why that is sufficient in a single-instance deployment).
pub(crate) fn broadcast_content_update(
    collab: &runtime::CollabRuntime,
    document_id: Uuid,
    exclude: Option<Uuid>,
    update_frame: &Frame,
    accepted_frame: &Frame,
) {
    collab.registry.broadcast(document_id, update_frame, exclude);
    collab.registry.broadcast(document_id, accepted_frame, exclude);
}

async fn fetch_object_workspace_id(state: &AppState, object_id: Uuid) -> Option<Uuid> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM flow_objects WHERE id = $1",
        vec![object_id.into()],
    ))
    .one(&state.db)
    .await
    .ok()
    .flatten()
    .map(|r| r.workspace_id)
}

async fn read_frame(socket: &mut WebSocket, timeout: Duration) -> Option<Frame> {
    let message = tokio::time::timeout(timeout, socket.recv()).await.ok()??.ok()?;
    let Message::Text(text) = message else { return None };
    if text.len() > WEBSOCKET_FRAME_BYTES_MAX {
        return None;
    }
    serde_json::from_str(text.as_str()).ok()
}
