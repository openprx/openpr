//! The per-connection WebSocket actor loop: `hello` → `open` → `snapshot`, then a
//! `select!` between inbound client frames and this document's outbound broadcast channel, until
//! the socket closes.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::authz::{self, PermissionLevel};
use super::bootstrap;
use super::egress::{EgressSequencer, SeqDecision};
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
    // `collab-protocol-v1.md` "accepted 出站顺序": "snapshot.head_seq=H 后第一条 accepted 只能是
    // H+1". Registration happens strictly after the snapshot above was already loaded, so a commit
    // landing in that (necessarily nonzero) gap would otherwise reach this session's channel as an
    // unannounced jump straight to some seq > H+1 -- this sequencer is what turns that into a
    // detected, resolved gap instead of a silent one.
    let mut sequencer = EgressSequencer::after_snapshot(boot.head_seq);
    // `Frame::Update` and its paired `Frame::Accepted` (same `update_id`) always arrive back to
    // back on this channel -- `write::accept_update` is their one broadcast call site and always
    // sends both for one commit -- but a `presence` broadcast from another task can still land
    // between them (`SessionRegistry::broadcast` locks/unlocks per call, not across the pair), so
    // pairing is done by `update_id`, not "the very next frame".
    let mut pending_updates: HashMap<Uuid, Frame> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            event = outbound_rx.recv() => {
                match event {
                    Some(OutboundEvent::Frame(frame)) => {
                        handle_outbound_frame(
                            &state.db,
                            document_id,
                            &mut socket,
                            &mut sequencer,
                            &mut pending_updates,
                            *frame,
                        )
                        .await;
                    }
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

/// One frame off this session's outbound channel, run through [`EgressSequencer`]
/// (`collab-protocol-v1.md` "accepted 出站顺序").
///
/// `Frame::Update` never carries a seq itself -- its paired `Frame::Accepted` (same `update_id`,
/// always broadcast immediately after it, `write::accept_update`'s one broadcast call site) does
/// -- so an incoming `Update` is only ever buffered in `pending_updates` here, never forwarded on
/// its own; the decision of whether (and what else) to forward is made entirely when its `Accepted`
/// arrives. Every other frame type (`presence`, etc.) passes straight through untouched.
async fn handle_outbound_frame(
    db: &sea_orm::DatabaseConnection,
    document_id: Uuid,
    socket: &mut WebSocket,
    sequencer: &mut EgressSequencer,
    pending_updates: &mut HashMap<Uuid, Frame>,
    frame: Frame,
) {
    if let Frame::Update { update_id, .. } = &frame {
        pending_updates.insert(*update_id, frame);
        return;
    }
    let Frame::Accepted {
        head_seq, update_id, ..
    } = &frame
    else {
        send(socket, &frame).await;
        return;
    };
    let head_seq = *head_seq;
    let paired_update = pending_updates.remove(update_id);

    match sequencer.evaluate(head_seq) {
        SeqDecision::InOrder => {
            if let Some(paired_update) = &paired_update {
                send(socket, paired_update).await;
            }
            send(socket, &frame).await;
        }
        // Already forwarded (or never will be, past a prior resync) -- drop silently.
        // `collab-protocol-v1.md`: "seq<=last_applied_seq 是幂等重复，忽略".
        SeqDecision::Duplicate => {}
        SeqDecision::Gap {
            missing_from,
            missing_to,
        } => {
            let plan = plan_gap_resolution(
                db,
                document_id,
                missing_from,
                missing_to,
                head_seq,
                paired_update,
                frame,
            )
            .await;
            match plan {
                GapResolution::Backfilled { frames, advance_to } => {
                    sequencer.resolve_gap(advance_to);
                    for f in &frames {
                        send(socket, f).await;
                    }
                }
                GapResolution::Resync { frame, advance_to } => {
                    sequencer.give_up_and_resync(advance_to);
                    send(socket, &frame).await;
                }
            }
        }
    }
}

/// What [`plan_gap_resolution`] decided to do about one [`SeqDecision::Gap`], separated from
/// actually sending anything so the decision itself -- including the real `collab_updates` read --
/// is testable without a live [`WebSocket`].
enum GapResolution {
    /// Every frame to send, in order: one synthesized `update`+`accepted` pair per backfilled row,
    /// then the paired `update` (if any) and the notice that revealed the gap. `advance_to` is the
    /// seq [`EgressSequencer::resolve_gap`] should be called with.
    Backfilled { frames: Vec<Frame>, advance_to: i64 },
    /// Backfill came up short (compacted rows, or a query failure): the one `resync` frame to
    /// send, and the seq [`EgressSequencer::give_up_and_resync`] should be called with. Per
    /// `collab-protocol-v1.md` ("禁止先发 N"), the notice that revealed the gap is never included.
    Resync { frame: Frame, advance_to: i64 },
}

/// The `SeqDecision::Gap` branch of [`handle_outbound_frame`]: backfill `[missing_from,
/// missing_to]` from `collab_updates` and plan forwarding it followed by the notice that revealed
/// the gap, or -- if backfill comes up short -- plan a `resync` instead. Pure decision-making plus
/// one database read; no socket I/O, so a real-database test can call this directly.
async fn plan_gap_resolution(
    db: &sea_orm::DatabaseConnection,
    document_id: Uuid,
    missing_from: i64,
    missing_to: i64,
    revealing_seq: i64,
    paired_update: Option<Frame>,
    revealing_frame: Frame,
) -> GapResolution {
    let expected_count = missing_to - missing_from + 1;
    let backfill = bootstrap::fetch_update_range(db, document_id, missing_from, missing_to).await;
    let complete_backfill = match backfill {
        Ok(rows) if i64::try_from(rows.len()).is_ok_and(|count| count == expected_count) => Some(rows),
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(error = %err, %document_id, missing_from, missing_to, "collab session: egress gap backfill query failed");
            None
        }
    };

    let Some(rows) = complete_backfill else {
        tracing::warn!(
            %document_id, missing_from, missing_to,
            "collab session: egress gap could not be fully backfilled, sending resync(outbound_gap)"
        );
        return GapResolution::Resync {
            frame: Frame::Resync {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                reason: "outbound_gap".to_string(),
                minimum_snapshot_seq: Some(missing_from.saturating_sub(1)),
            },
            advance_to: revealing_seq,
        };
    };

    let mut frames = Vec::with_capacity(rows.len().saturating_mul(2) + 2);
    for row in rows {
        frames.push(Frame::Update {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: row.update_id,
            base_frontier: BASE64.encode(&row.before_frontier),
            bytes: BASE64.encode(&row.bytes),
            idempotency_key: None,
            origin: row.origin_client_id.unwrap_or_default(),
            message: None,
        });
        frames.push(Frame::Accepted {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: row.update_id,
            head_seq: row.seq,
            head_frontier: BASE64.encode(&row.after_frontier),
            projection_seq: row.projection_seq,
            event_id: row.event_id,
        });
    }
    if let Some(paired_update) = paired_update {
        frames.push(paired_update);
    }
    frames.push(revealing_frame);
    GapResolution::Backfilled {
        frames,
        advance_to: revealing_seq,
    }
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
            let outcome = write::accept_update(
                &state.db,
                &collab.cache,
                &collab.coordinator,
                &collab.registry,
                &collab.snapshot,
                crate::config::runtime().flow.dispatch_max_attempts,
                Some(session_id),
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
                    if accepted.should_advance_snapshot {
                        crate::flow::collab::snapshot::spawn_background(
                            &collab.snapshot,
                            state.db.clone(),
                            document_id,
                        );
                    }
                    // Peers other than the committer already received the `update`+`accepted`
                    // pair from `write::accept_update` itself (this instance's single broadcast
                    // call site, still inside the coordinator permit — see that module's doc
                    // comment) with this session excluded; only the committer's own ack is left to
                    // send, directly on its own socket, bypassing the registry entirely (it is not
                    // registered as its own frame's recipient).
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

// ---------------------------------------------------------------------------------------------
// Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`, matching
// `flow::collab::write::database_tests`'s scratch-per-run convention). Colocated here (not only
// exercised end-to-end via `routes::collab`'s real WebSocket harness) so `plan_gap_resolution` --
// the `SeqDecision::Gap` backfill/resync decision -- is directly, deterministically testable: the
// startup race it exists to cover (a commit landing between this session's snapshot load and its
// `collab.registry.register` call) is a genuine timing race in the real server, not something a
// black-box WebSocket test can reliably force.
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod database_tests {
    use base64::Engine;
    use collab_core::{CollabEngine, LoroCollabEngine};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, FromQueryResult};
    use uuid::Uuid;

    use super::{Frame, GapResolution, PROTOCOL_VERSION, plan_gap_resolution};
    use crate::flow::collab::authz;
    use crate::flow::collab::cache::WarmCache;
    use crate::flow::collab::coordinator::DocumentCoordinator;
    use crate::flow::collab::registry::SessionRegistry;
    use crate::flow::collab::snapshot::SnapshotAdvancer;
    use crate::flow::collab::write::{self, AcceptOutcome, UpdateRequest};
    use crate::flow::command::{CreateObjectInput, create_object};

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_session_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }

        Some(Scratch { db, name, admin_url })
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "session-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("session-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
        }
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@session-test.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'session test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid) -> (Uuid, Uuid) {
        let accepted = create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "Gap Resolution Test Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object creation succeeds");
        (accepted.object.id, accepted.object.document_id)
    }

    /// Commits `count` real, sequential updates through the exact production write path, returning
    /// each commit's `write::Accepted`.
    async fn commit_n_updates(
        state: &AppState,
        document_id: Uuid,
        workspace_id: Uuid,
        actor_id: Uuid,
        count: usize,
    ) -> Vec<write::Accepted> {
        #[derive(FromQueryResult)]
        struct SnapshotRow {
            snapshot: Vec<u8>,
        }
        let snapshot_row = SnapshotRow::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists");
        let mut engine = LoroCollabEngine::load(&snapshot_row.snapshot).expect("loads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let registry = SessionRegistry::new();
        let snapshot_advancer = SnapshotAdvancer::new();

        let mut committed = Vec::with_capacity(count);
        for i in 0..count {
            let base_frontier = engine.frontier();
            engine.set_title(&format!("gap-test-{i}")).expect("set_title succeeds");
            let bytes = engine.export_from(&base_frontier).expect("export succeeds");
            let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
            let outcome = write::accept_update(
                &state.db,
                &cache,
                &coordinator,
                &registry,
                &snapshot_advancer,
                10,
                None,
                UpdateRequest {
                    document_id,
                    update_id: Uuid::new_v4(),
                    bytes,
                    idempotency_key: None,
                    origin_client_id: Some("gap-test".to_string()),
                    message: None,
                    actor_id,
                    workspace_id,
                    checked_epoch,
                    expected_frontier: None,
                },
            )
            .await
            .expect("accept_update does not hit a hard database error");
            let AcceptOutcome::Accepted(accepted) = outcome else {
                panic!("expected Accepted for commit {i}");
            };
            committed.push(accepted);
        }
        committed
    }

    fn accepted_frame(document_id: Uuid, accepted: &write::Accepted) -> Frame {
        Frame::Accepted {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: accepted.update_id,
            head_seq: accepted.head_seq,
            head_frontier: super::BASE64.encode(&accepted.head_frontier),
            projection_seq: accepted.projection_seq,
            event_id: accepted.event_id,
        }
    }

    #[tokio::test]
    async fn plan_gap_resolution_backfills_a_full_gap_and_orders_frames_seq_then_paired_then_revealing() {
        let scratch = scratch_or_skip!("gap-plan-backfill");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        // Simulates: this session's egress next expected seq 1, but the notice that reached it
        // is seq 4 -- exactly the shape a startup race (commits 1..=3 landing between this
        // session's snapshot load and its registry registration) produces.
        let committed = commit_n_updates(&state, document_id, workspace_id, owner_id, 4).await;
        let revealing = &committed[3];
        let revealing_frame = accepted_frame(document_id, revealing);

        let plan = plan_gap_resolution(
            &state.db,
            document_id,
            1,
            3,
            revealing.head_seq,
            None,
            revealing_frame.clone(),
        )
        .await;

        let GapResolution::Backfilled { frames, advance_to } = plan else {
            panic!("expected a full backfill, got a resync");
        };
        assert_eq!(
            advance_to, 4,
            "sequencer must advance to the revealing seq, not just past the gap"
        );
        // 3 backfilled (update, accepted) pairs + the revealing frame itself.
        assert_eq!(frames.len(), 7);

        let backfilled_seqs: Vec<i64> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::Accepted { head_seq, .. } => Some(*head_seq),
                _ => None,
            })
            .collect();
        assert_eq!(
            backfilled_seqs,
            vec![1, 2, 3, 4],
            "backfilled accepted frames must be seq-ordered, ending with the revealing notice itself"
        );
        // update/accepted pairing: frames[0]/[1] is seq 1, frames[2]/[3] is seq 2, etc.
        for (i, expected_seq) in [1i64, 2, 3].into_iter().enumerate() {
            let Frame::Update { update_id, .. } = &frames[i * 2] else {
                panic!("frame {} must be an Update", i * 2);
            };
            let Frame::Accepted {
                head_seq,
                update_id: accepted_update_id,
                ..
            } = &frames[i * 2 + 1]
            else {
                panic!("frame {} must be an Accepted", i * 2 + 1);
            };
            assert_eq!(*head_seq, expected_seq);
            assert_eq!(
                update_id, accepted_update_id,
                "the Update/Accepted pair must share update_id"
            );
        }
        assert_eq!(
            frames[6], revealing_frame,
            "the last frame must be the notice that revealed the gap"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn plan_gap_resolution_includes_the_paired_update_before_the_revealing_accepted() {
        let scratch = scratch_or_skip!("gap-plan-paired");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let committed = commit_n_updates(&state, document_id, workspace_id, owner_id, 2).await;
        let revealing = &committed[1];
        let revealing_frame = accepted_frame(document_id, revealing);
        let paired_update = Frame::Update {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: revealing.update_id,
            base_frontier: super::BASE64.encode(&revealing.before_frontier),
            bytes: "cGFpcmVk".to_string(),
            idempotency_key: None,
            origin: "gap-test".to_string(),
            message: None,
        };

        let plan = plan_gap_resolution(
            &state.db,
            document_id,
            1,
            1,
            revealing.head_seq,
            Some(paired_update.clone()),
            revealing_frame.clone(),
        )
        .await;

        let GapResolution::Backfilled { frames, .. } = plan else {
            panic!("expected a full backfill");
        };
        // 1 backfilled pair (seq 1) + the paired update + the revealing accepted.
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[2], paired_update);
        assert_eq!(frames[3], revealing_frame);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn plan_gap_resolution_resyncs_and_never_forwards_the_revealing_notice_when_backfill_is_short() {
        let scratch = scratch_or_skip!("gap-plan-resync");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        // Only one real commit exists; a gap claiming to cover seq 1..=5 cannot be backfilled.
        let committed = commit_n_updates(&state, document_id, workspace_id, owner_id, 1).await;
        let revealing_frame = Frame::Accepted {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: Uuid::new_v4(),
            head_seq: 6,
            head_frontier: super::BASE64.encode(&committed[0].head_frontier),
            projection_seq: 6,
            event_id: Uuid::new_v4(),
        };

        let plan = plan_gap_resolution(&state.db, document_id, 1, 5, 6, None, revealing_frame).await;

        let GapResolution::Resync { frame, advance_to } = plan else {
            panic!("expected a resync -- only 1 of 5 requested rows exists");
        };
        assert_eq!(advance_to, 6);
        let Frame::Resync {
            reason,
            minimum_snapshot_seq,
            ..
        } = frame
        else {
            panic!("expected a Resync frame");
        };
        assert_eq!(reason, "outbound_gap");
        assert_eq!(minimum_snapshot_seq, Some(0));

        scratch.drop_self().await;
    }
}
