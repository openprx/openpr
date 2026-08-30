//! The per-connection WebSocket actor loop: `hello` → `open` → `snapshot`, then a
//! `select!` between inbound client frames and this document's outbound broadcast channel, until
//! the socket closes.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

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
use super::limits::{
    CONNECTION_LIMIT_RETRY_AFTER_MS, FRAME_BURST_MAX, FRAMES_PER_CONNECTION_PER_SECOND,
    PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX, PRESENCE_PAYLOAD_BYTES_MAX,
    PRESENCE_TTL_SECONDS_DEFAULT, PRESENCE_TTL_SECONDS_MAX, RATE_LIMIT_RETRY_AFTER_MS, UPDATE_BURST_MAX,
    UPDATES_PER_CONNECTION_PER_SECOND, WEBSOCKET_FRAME_BYTES_MAX,
};
use super::registry::{OutboundEvent, PresenceLimit};
use super::runtime;
use super::ticket::ConsumedTicket;
use super::write::{self, AcceptOutcome, UpdateRequest};
use crate::error::ApiErrorKind;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A close code this module owns: policy violation, matching RFC 6455's registered meaning
/// closely enough (protocol error / auth failure at handshake) without colliding with the
/// contract's own frozen 4410 drain code. Used as [`ws_close_code_for`]'s fallback for every
/// `RejectedCode` `error-mapping-v1.md` leaves as "control frame, connection stays open"
/// (`invalid_update`, `stale_frontier`, `resync_required`, `server_draining{contention}`) when this
/// module still has to hang up on it anyway, at handshake time, before a steady-state loop exists
/// to keep the connection open for.
const CLOSE_POLICY_VIOLATION: u16 = 1008;

/// `limits-v1.md`'s connection/rate/slow-consumer close code — computed once from
/// [`ApiErrorKind::LimitExceeded`] so it can never drift from `error-mapping-v1.md`'s frozen table
/// (`flow::collab::registry` computes the identical value independently for its own slow-consumer
/// close path — both derive from this one source of truth, so they cannot disagree even though
/// each module owns its own constant).
const LIMIT_EXCEEDED_CLOSE_CODE: u16 = match ApiErrorKind::LimitExceeded.ws_close_code() {
    Some(code) => code,
    None => CLOSE_POLICY_VIOLATION,
};

/// `hello.capabilities` this server understands (`collab-protocol-v1.md`: "未知 required
/// capability... 必须失败关闭,不得部分应用"). The wire shape carries no required/optional
/// distinction, so every capability a client declares is treated as required — an empty list
/// (every real client today, and this module's own tests) trivially passes.
const KNOWN_CAPABILITIES: &[&str] = &["presence"];

/// The first capability in `capabilities` this server does not recognize, if any.
fn first_unknown_capability(capabilities: &[String]) -> Option<&str> {
    capabilities
        .iter()
        .map(String::as_str)
        .find(|capability| !KNOWN_CAPABILITIES.contains(capability))
}

/// `limits-v1.md`'s `open_documents_per_connection_max` (8) is met **structurally** in v0.4, not
/// by counting: `ADR-0007`'s `collab_tickets` row -- and therefore this connection's
/// `ConsumedTicket` -- already carries exactly one `document_id`; [`run`] consumes exactly one
/// ticket per socket and binds `document_id` once from it (`run`'s own `document_id =
/// consumed.document_id`, never reassigned for the life of the connection); `run`'s handshake
/// itself rejects an `open.document_id` that disagrees with the ticket's
/// (`RejectedCode::Forbidden`, before this function ever runs); and this function rejects every
/// `Frame::Open` a client sends afterward, in the steady-state loop, as `invalid_update` instead of
/// letting it register a second document. The real, enforced open-document count for any v0.4
/// connection is therefore always exactly `1` -- never approaching, let alone exceeding,
/// `OPEN_DOCUMENTS_PER_CONNECTION_MAX` -- so there is no runtime scenario a counter could reject
/// that this structural bound does not already foreclose (`limits-v1.md`'s own row: "v0.4 UI 主路径
/// 一次一个 object,保留少量 tab/prefetch 余量但禁止一连接扫描 workspace"; `frame.rs`'s doc comment:
/// "v0.4...scopes one WebSocket connection to exactly the one `document_id` its ticket was issued
/// for"). `open_documents_per_connection_is_bounded_to_one_by_rejecting_a_client_reopen` (below) is
/// this invariant's regression test: it goes red the moment a future change stops classifying a
/// steady-state `Frame::Open` as a reopen attempt.
const fn is_reopen_attempt(frame: &Frame) -> bool {
    matches!(frame, Frame::Open { .. })
}

/// A v0.4 connection's structural open-document bound (see [`is_reopen_attempt`]) can never exceed
/// the contract's counted ceiling -- checked once, at compile time, so this module would fail to
/// build before ever silently drifting past it.
const _: () = assert!(
    1 <= super::limits::OPEN_DOCUMENTS_PER_CONNECTION_MAX,
    "the structural open-document bound must never exceed OPEN_DOCUMENTS_PER_CONNECTION_MAX"
);

/// Maps this module's own wire [`RejectedCode`] to the richer [`ApiErrorKind`] so a close can use
/// [`ApiErrorKind::ws_close_code`] instead of a second, hand-maintained close-code table
/// (`error-mapping-v1.md`'s frozen mapping lives in exactly one place: `error.rs`).
const fn rejected_code_to_api_kind(code: RejectedCode) -> ApiErrorKind {
    match code {
        RejectedCode::Unauthenticated => ApiErrorKind::Unauthenticated,
        RejectedCode::Forbidden => ApiErrorKind::Forbidden,
        RejectedCode::FeatureDisabled => ApiErrorKind::FeatureDisabled,
        RejectedCode::NotFound => ApiErrorKind::NotFound,
        RejectedCode::UnsupportedProtocol => ApiErrorKind::UnsupportedProtocol,
        RejectedCode::StaleFrontier => ApiErrorKind::StaleFrontier,
        RejectedCode::InvalidUpdate => ApiErrorKind::InvalidUpdate,
        RejectedCode::PolicyRejected => ApiErrorKind::PolicyRejected,
        RejectedCode::LimitExceeded => ApiErrorKind::LimitExceeded,
        RejectedCode::ResyncRequired => ApiErrorKind::ResyncRequired,
        // A handshake-time close always reports `drain`, never `contention`: there is no document
        // lock/rebase/snapshot contention to report about a session that has not reached `open`
        // yet (`collab-protocol-v1.md`: "两者不得互换"). The one real `contention` rejection this
        // package sends (`write::accept_update`'s error branch, below) never calls this mapping —
        // it stays a `rejected` control frame and never closes the socket.
        RejectedCode::ServerDraining => ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::Drain),
    }
}

/// The WS close code to send right after a `rejected`/failed-handshake `code` when this connection
/// is being closed (`error-mapping-v1.md` via [`ApiErrorKind::ws_close_code`]).
fn ws_close_code_for(code: RejectedCode) -> u16 {
    rejected_code_to_api_kind(code)
        .ws_close_code()
        .unwrap_or(CLOSE_POLICY_VIOLATION)
}

/// Sends a `rejected` frame for `code` and immediately closes with the matching close code
/// (`ws_close_code_for`) — the shared shape every handshake-phase failure in [`run`]/
/// [`reverify_open`] uses. `recoverable` comes from [`ApiErrorKind::recoverable`], not a
/// per-call-site literal, so it stays in lockstep with `error-mapping-v1.md`'s own table (e.g.
/// `resync_required` is `true` there even though this module always closes right after sending it
/// during the handshake).
async fn reject_and_close(socket: &mut WebSocket, document_id: Uuid, code: RejectedCode, reason: &str) {
    let recoverable = rejected_code_to_api_kind(code).recoverable();
    send(socket, &rejected_frame(document_id, code, recoverable, None)).await;
    close(socket, ws_close_code_for(code), reason).await;
}

/// Builds a `limit_exceeded` `rejected` frame carrying the contract-mandated `details.limit_kind`
/// (`limits-v1.md`: "`limit_kind` 全集正是上表第三列的唯一值... 未知 kind 违反 contract") plus
/// `limit` and, when meaningful, `observed`/`retry_after_ms`.
fn limit_exceeded_frame(
    document_id: Uuid,
    limit_kind: &str,
    limit: u64,
    observed: Option<u64>,
    retry_after_ms: Option<u64>,
) -> Frame {
    let mut details = serde_json::Map::new();
    details.insert("limit_kind".to_string(), serde_json::json!(limit_kind));
    details.insert("limit".to_string(), serde_json::json!(limit));
    if let Some(observed) = observed {
        details.insert("observed".to_string(), serde_json::json!(observed));
    }
    if let Some(retry_after_ms) = retry_after_ms {
        details.insert("retry_after_ms".to_string(), serde_json::json!(retry_after_ms));
    }
    let details = serde_json::Value::Object(details);
    Frame::Rejected {
        protocol_version: PROTOCOL_VERSION,
        document_id,
        update_id: None,
        code: RejectedCode::LimitExceeded,
        recoverable: true,
        details: Some(details),
        current_seq: None,
        current_frontier: None,
        audit_event_id: None,
    }
}

/// A fixed sustained-rate token bucket with burst capacity (`limits-v1.md`: "Rate 使用 token
/// bucket"). Also tracks the "3 consecutive 1-second enforcement intervals over limit" close
/// trigger the same paragraph requires ("连续 3 个 1-second enforcement interval 超限...则关闭连接为
/// 4408") — a single denied request only ever produces a `limit_exceeded` control frame; only
/// sustained abuse across three whole enforcement windows escalates to a close.
struct RateLimiter {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
    window_start: Instant,
    window_exceeded: bool,
    consecutive_exceeded_windows: u32,
}

/// One [`RateLimiter::take`] outcome.
struct RateOutcome {
    /// Whether the just-evaluated frame consumed a token (and should proceed).
    admitted: bool,
    /// Whether 3 consecutive 1-second enforcement windows have now been over limit — the caller
    /// must close the connection at [`LIMIT_EXCEEDED_CLOSE_CODE`] regardless of `admitted`.
    force_close: bool,
}

impl RateLimiter {
    #[allow(clippy::cast_precision_loss)] // sustained/burst are small fixed contract constants
    fn new(sustained_per_sec: u64, burst: u64) -> Self {
        let now = Instant::now();
        Self {
            capacity: burst as f64,
            tokens: burst as f64,
            refill_per_sec: sustained_per_sec as f64,
            last_refill: now,
            window_start: now,
            window_exceeded: false,
            consecutive_exceeded_windows: 0,
        }
    }

    fn take(&mut self) -> RateOutcome {
        self.take_at(Instant::now())
    }

    /// Pure-logic core of [`take`](Self::take), parameterized on "now" instead of always reading
    /// the real monotonic clock. `take()` delegates here with `Instant::now()`; tests call this
    /// directly with deterministically-advanced `Instant`s (`t + Duration::from_secs(n)`, no real
    /// waiting) so the token-bucket refill and the 1-second enforcement-window rollover are fully
    /// controllable without `tokio::time::sleep` or any wall-clock dependency.
    fn take_at(&mut self, now: Instant) -> RateOutcome {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = elapsed.mul_add(self.refill_per_sec, self.tokens).min(self.capacity);

        let admitted = if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            self.window_exceeded = true;
            false
        };

        let force_close = if now.duration_since(self.window_start).as_secs_f64() >= 1.0 {
            self.consecutive_exceeded_windows = if self.window_exceeded {
                self.consecutive_exceeded_windows + 1
            } else {
                0
            };
            self.window_exceeded = false;
            self.window_start = now;
            self.consecutive_exceeded_windows >= 3
        } else {
            false
        };

        RateOutcome { admitted, force_close }
    }
}

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
    let Frame::Hello {
        protocol_version,
        capabilities,
        ..
    } = hello
    else {
        reject_and_close(
            &mut socket,
            document_id,
            RejectedCode::UnsupportedProtocol,
            "expected hello",
        )
        .await;
        return;
    };
    if protocol_version != PROTOCOL_VERSION {
        reject_and_close(
            &mut socket,
            document_id,
            RejectedCode::UnsupportedProtocol,
            "unsupported protocol_version",
        )
        .await;
        return;
    }
    if let Some(unknown) = first_unknown_capability(&capabilities) {
        tracing::debug!(capability = %unknown, "collab session: rejecting hello with an unknown capability");
        reject_and_close(
            &mut socket,
            document_id,
            RejectedCode::UnsupportedProtocol,
            "unknown hello capability",
        )
        .await;
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
        reject_and_close(&mut socket, document_id, RejectedCode::InvalidUpdate, "expected open").await;
        return;
    };
    if opened_document_id != document_id {
        reject_and_close(
            &mut socket,
            document_id,
            RejectedCode::Forbidden,
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
        reject_and_close(
            &mut socket,
            document_id,
            RejectedCode::ResyncRequired,
            "bootstrap failed",
        )
        .await;
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
    // `limits-v1.md`'s three connection ceilings (`connections_per_user_max`/`_per_document_max`/
    // `_per_workspace_max`) are checked and reserved atomically here, as late as possible (after
    // the ticket/permission/flag reverification above, so an over-ceiling caller never pays for a
    // database round trip whose result it cannot use). `open_documents_per_connection_max = 8` has
    // no counting to do: v0.4 scopes one WebSocket connection to exactly the one `document_id` its
    // ticket was issued for (`frame.rs`'s own doc comment), so that ceiling is met structurally by
    // every connection, not enforced by counting.
    let mut registered =
        match collab
            .registry
            .try_register(document_id, consumed.user_id, consumed.workspace_id, session_id)
        {
            Ok(registered) => registered,
            Err(limit) => {
                send(
                    &mut socket,
                    &limit_exceeded_frame(
                        document_id,
                        limit.limit_kind(),
                        limit.limit(),
                        None,
                        Some(CONNECTION_LIMIT_RETRY_AFTER_MS),
                    ),
                )
                .await;
                close(&mut socket, LIMIT_EXCEEDED_CLOSE_CODE, "connection limit exceeded").await;
                return;
            }
        };
    let checked_epoch = ctx.checked_epoch;
    let mut frame_limiter = RateLimiter::new(FRAMES_PER_CONNECTION_PER_SECOND, FRAME_BURST_MAX);
    let mut update_limiter = RateLimiter::new(UPDATES_PER_CONNECTION_PER_SECOND, UPDATE_BURST_MAX);
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
            event = registered.receiver.recv() => {
                match event {
                    Some(OutboundEvent::Frame(frame, encoded_len)) => {
                        // Releases this frame's slow-consumer queue charge (`registry.rs`'s
                        // `SessionHandle::deliver`) the moment it leaves the channel, regardless of
                        // whether it is forwarded immediately, buffered for pairing, or dropped as
                        // a stale duplicate below -- the ceiling bounds channel backlog, not any
                        // further in-process buffering.
                        registered.record_dequeued(encoded_len);
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
                            send(&mut socket, &limit_exceeded_frame(document_id, "websocket_frame_bytes", WEBSOCKET_FRAME_BYTES_MAX as u64, Some(text.len() as u64), None)).await;
                            continue;
                        }
                        // `limits-v1.md`: "frames_per_connection_per_second... 持续洪泛在 decode 前
                        // 限流" -- checked before the frame is even parsed.
                        let frame_outcome = frame_limiter.take();
                        if !frame_outcome.admitted {
                            send(&mut socket, &limit_exceeded_frame(document_id, "frame_rate", FRAMES_PER_CONNECTION_PER_SECOND, None, Some(RATE_LIMIT_RETRY_AFTER_MS))).await;
                        }
                        if frame_outcome.force_close {
                            close(&mut socket, LIMIT_EXCEEDED_CLOSE_CODE, "sustained frame rate exceeded").await;
                            break;
                        }
                        if !frame_outcome.admitted {
                            continue;
                        }
                        let Ok(frame) = serde_json::from_str::<Frame>(text.as_str()) else {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None)).await;
                            continue;
                        };
                        if matches!(frame, Frame::Update { .. }) {
                            let update_outcome = update_limiter.take();
                            if !update_outcome.admitted {
                                send(&mut socket, &limit_exceeded_frame(document_id, "update_rate", UPDATES_PER_CONNECTION_PER_SECOND, None, Some(RATE_LIMIT_RETRY_AFTER_MS))).await;
                            }
                            if update_outcome.force_close {
                                close(&mut socket, LIMIT_EXCEEDED_CLOSE_CODE, "sustained update rate exceeded").await;
                                break;
                            }
                            if !update_outcome.admitted {
                                continue;
                            }
                        }
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
                            send(&mut socket, &limit_exceeded_frame(document_id, "websocket_frame_bytes", WEBSOCKET_FRAME_BYTES_MAX as u64, Some(bytes.len() as u64), None)).await;
                        } else {
                            send(&mut socket, &rejected_frame(document_id, RejectedCode::UnsupportedProtocol, false, None)).await;
                            close(&mut socket, ws_close_code_for(RejectedCode::UnsupportedProtocol), "binary frames are not supported").await;
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
        reject_and_close(socket, document_id, RejectedCode::NotFound, "document not found").await;
        return None;
    };
    let Ok(flow_enabled) = crate::flow::repository::fetch_flow_enabled(&state.db, consumed.workspace_id).await else {
        reject_and_close(
            socket,
            document_id,
            RejectedCode::FeatureDisabled,
            "flow is not enabled",
        )
        .await;
        return None;
    };
    if !flow_enabled {
        reject_and_close(
            socket,
            document_id,
            RejectedCode::FeatureDisabled,
            "flow is not enabled",
        )
        .await;
        return None;
    }
    let Some(role) = fetch_role(&state.db, consumed.workspace_id, consumed.user_id).await else {
        reject_and_close(socket, document_id, RejectedCode::Forbidden, "not a workspace member").await;
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
        reject_and_close(socket, document_id, RejectedCode::Forbidden, "permission check failed").await;
        return None;
    };
    if level < PermissionLevel::Edit {
        reject_and_close(socket, document_id, RejectedCode::Forbidden, "insufficient permission").await;
        return None;
    }
    let Ok(checked_epoch) = authz::read_epoch(&state.db, consumed.workspace_id).await else {
        reject_and_close(socket, document_id, RejectedCode::Forbidden, "epoch read failed").await;
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
    // `open_documents_per_connection_max`'s structural guarantee (see [`is_reopen_attempt`]): a
    // client sending `open` again after the handshake must never be treated as opening a second
    // document. This does not change behavior from before this function was refactored to name it
    // -- `Frame::Open` landed in the same `invalid_update`-and-stay-open catch-all below either
    // way -- it only isolates the one decision that makes the ceiling structural into something
    // independently unit-testable.
    if is_reopen_attempt(&frame) {
        send(
            socket,
            &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
        )
        .await;
        return;
    }

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
            // `limits-v1.md`: "presence_payload_bytes_max... update/presence payload 在分配 engine
            // state 前检查" -- checked before anything else touches the payload.
            let Ok(encoded_payload) = serde_json::to_vec(&payload) else {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
                )
                .await;
                return;
            };
            #[allow(clippy::cast_possible_truncation)] // a WS frame is already bounded far below u64::MAX
            let payload_len = encoded_payload.len() as u64;
            if payload_len > PRESENCE_PAYLOAD_BYTES_MAX {
                send(
                    socket,
                    &limit_exceeded_frame(
                        document_id,
                        "presence_payload_bytes",
                        PRESENCE_PAYLOAD_BYTES_MAX,
                        Some(payload_len),
                        None,
                    ),
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
                    &limit_exceeded_frame(
                        document_id,
                        "presence_ttl_seconds",
                        u64::from(PRESENCE_TTL_SECONDS_MAX),
                        Some(u64::from(ttl_seconds)),
                        None,
                    ),
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
                Err(PresenceLimit::PerConnection) => {
                    send(
                        socket,
                        &limit_exceeded_frame(
                            document_id,
                            "presence_entries_per_connection",
                            PRESENCE_ENTRIES_PER_CONNECTION_MAX as u64,
                            None,
                            None,
                        ),
                    )
                    .await;
                }
                Err(PresenceLimit::PerDocument) => {
                    send(
                        socket,
                        &limit_exceeded_frame(
                            document_id,
                            "presence_entries_per_document",
                            PRESENCE_ENTRIES_PER_DOCUMENT_MAX as u64,
                            None,
                            None,
                        ),
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
        // `Frame::Open` is already handled by the early `is_reopen_attempt` return above, before
        // this match ever runs -- this arm can never actually observe one at runtime, but stays
        // listed here (rather than behind a `_` wildcard) so the compiler's own exhaustiveness
        // check still forces every future `Frame` variant to be an explicit decision somewhere in
        // this function, the same guarantee this match provided before the early return existed.
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
// Pure-logic unit tests for `RateLimiter` (no database, no real time). `limits-v1.md`'s
// `frames_per_connection_per_second` (30, burst 60) and `updates_per_connection_per_second` (10,
// burst 20): exact sustained-rate boundary accepted, boundary+1 rejected with the connection
// still open, and the connection is only force-closed after 3 *consecutive* 1-second enforcement
// windows are each over limit. Every test here drives `RateLimiter::take_at` with manually
// advanced `Instant`s (`t + Duration::from_secs(n)`) instead of `Instant::now()` + real sleeping,
// so window rollover is deterministic and the suite stays fast.
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{Frame, RateLimiter, bootstrap, is_reopen_attempt};
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::limits;

    use super::{
        FRAME_BURST_MAX, FRAMES_PER_CONNECTION_PER_SECOND, UPDATE_BURST_MAX, UPDATES_PER_CONNECTION_PER_SECOND,
    };

    /// Shared scenario for the sustained-rate exact/+1 boundary: starts the bucket empty (bypasses
    /// `RateLimiter::new`'s initial burst fill so this test isolates the *sustained* rate, not the
    /// burst capacity) and advances the clock by exactly one enforcement window, so token-bucket
    /// refill adds exactly `sustained` tokens (never more, since `sustained <= burst` for both
    /// frame and update rate). That admits exactly `sustained` requests before the
    /// `sustained + 1`-th is rejected, and the connection must stay open throughout -- a single
    /// exceeded window alone never force-closes.
    #[allow(clippy::cast_precision_loss)] // sustained/burst are small fixed contract constants
    fn assert_sustained_rate_exact_boundary_accepted_and_plus_one_rejected(sustained: u64, burst: u64) {
        let t0 = Instant::now();
        let mut limiter = RateLimiter {
            capacity: burst as f64,
            tokens: 0.0,
            refill_per_sec: sustained as f64,
            last_refill: t0,
            window_start: t0,
            window_exceeded: false,
            consecutive_exceeded_windows: 0,
        };
        let t1 = t0 + Duration::from_secs(1);

        for n in 0..sustained {
            let outcome = limiter.take_at(t1);
            assert!(
                outcome.admitted,
                "request {n} within the exact sustained-rate boundary ({sustained}) must be admitted"
            );
            assert!(
                !outcome.force_close,
                "a single window at/under the sustained rate must never force-close"
            );
        }

        let over = limiter.take_at(t1);
        assert!(
            !over.admitted,
            "the request one past the sustained-rate boundary ({sustained}) must be rejected"
        );
        assert!(
            !over.force_close,
            "a single exceeded window must not force-close the connection"
        );
    }

    #[test]
    fn frame_rate_exact_sustained_boundary_is_accepted_and_plus_one_is_rejected_without_closing() {
        assert_sustained_rate_exact_boundary_accepted_and_plus_one_rejected(
            FRAMES_PER_CONNECTION_PER_SECOND,
            FRAME_BURST_MAX,
        );
    }

    #[test]
    fn update_rate_exact_sustained_boundary_is_accepted_and_plus_one_is_rejected_without_closing() {
        assert_sustained_rate_exact_boundary_accepted_and_plus_one_rejected(
            UPDATES_PER_CONNECTION_PER_SECOND,
            UPDATE_BURST_MAX,
        );
    }

    /// The connection must be force-closed only once 3 *consecutive* 1-second enforcement windows
    /// were each over limit -- never on the 1st or 2nd. Uses a minimal sustained=1/burst=1 limiter
    /// (the escalation mechanism is shared by every rate `limit_kind`; the exact sustained/burst
    /// numbers are irrelevant to it) and, each window, admits the single refilled token and then
    /// gets rejected once (marking that window exceeded), before advancing exactly 1 second to
    /// roll into the next window.
    #[test]
    fn rate_limiter_force_closes_only_after_three_consecutive_exceeded_windows() {
        let t0 = Instant::now();
        let mut limiter = RateLimiter {
            capacity: 1.0,
            tokens: 1.0,
            refill_per_sec: 1.0,
            last_refill: t0,
            window_start: t0,
            window_exceeded: false,
            consecutive_exceeded_windows: 0,
        };

        // Window 0 [t0, t1): consume the only token, then get rejected -- marks window 0
        // exceeded. Still inside window 0, so no rollover is evaluated yet.
        assert!(limiter.take_at(t0).admitted);
        let rejected0 = limiter.take_at(t0);
        assert!(!rejected0.admitted);
        assert!(!rejected0.force_close);

        // Window 1 [t1, t2): resolves window 0 (exceeded) into the streak -> consecutive = 1.
        let t1 = t0 + Duration::from_secs(1);
        let rollover1 = limiter.take_at(t1);
        assert!(rollover1.admitted);
        assert!(
            !rollover1.force_close,
            "1st consecutive exceeded window alone must not close"
        );
        let rejected1 = limiter.take_at(t1);
        assert!(!rejected1.admitted);
        assert!(!rejected1.force_close);

        // Window 2 [t2, t3): resolves window 1 (exceeded) -> consecutive = 2.
        let t2 = t1 + Duration::from_secs(1);
        let rollover2 = limiter.take_at(t2);
        assert!(rollover2.admitted);
        assert!(
            !rollover2.force_close,
            "2nd consecutive exceeded window alone must not close"
        );
        let rejected2 = limiter.take_at(t2);
        assert!(!rejected2.admitted);
        assert!(!rejected2.force_close);

        // Window 3 [t3, ...): resolves window 2 (exceeded) -> consecutive = 3 -> force-close.
        let t3 = t2 + Duration::from_secs(1);
        let rollover3 = limiter.take_at(t3);
        assert!(rollover3.admitted);
        assert!(
            rollover3.force_close,
            "3rd consecutive exceeded window must force-close the connection (code 4408)"
        );
    }

    /// The consecutive-exceeded-window streak must reset to 0 (never carry over) once a window
    /// passes without being exceeded, so exceeding, recovering, and exceeding again never
    /// force-closes on the 2nd post-recovery window.
    #[test]
    fn rate_limiter_consecutive_exceeded_window_streak_resets_after_a_clean_window() {
        let t0 = Instant::now();
        let mut limiter = RateLimiter {
            capacity: 1.0,
            tokens: 1.0,
            refill_per_sec: 1.0,
            last_refill: t0,
            window_start: t0,
            window_exceeded: false,
            consecutive_exceeded_windows: 0,
        };

        // Window 0 [t0, t1): exceeded (consume the token, then get rejected).
        assert!(limiter.take_at(t0).admitted);
        assert!(!limiter.take_at(t0).admitted);

        // Window 1 [t1, t2): resolves window 0 (exceeded) -> consecutive = 1. Only one request is
        // made during window 1 itself (admitted, using the refilled token), so window 1 stays
        // clean -- nothing marks it exceeded.
        let t1 = t0 + Duration::from_secs(1);
        let admit1 = limiter.take_at(t1);
        assert!(admit1.admitted);
        assert!(!admit1.force_close);
        assert_eq!(limiter.consecutive_exceeded_windows, 1);

        // Window 2 [t2, t3): resolves window 1 -- since window 1 was clean, the streak resets to
        // 0 instead of continuing to 2.
        let t2 = t1 + Duration::from_secs(1);
        let admit2 = limiter.take_at(t2);
        assert!(admit2.admitted);
        assert!(!admit2.force_close, "a reset streak must never force-close");
        assert_eq!(
            limiter.consecutive_exceeded_windows, 0,
            "a clean window must reset the consecutive-exceeded streak"
        );
    }

    /// `open_documents_per_connection_max`'s structural guarantee (`is_reopen_attempt`'s own doc
    /// comment): a client re-sending `open` after the handshake must be classified as a reopen
    /// attempt (and therefore rejected without ever registering a second document), while every
    /// other frame type -- including the update/presence/ping frames a real steady-state
    /// connection actually processes -- must not be misclassified as one.
    #[test]
    fn open_documents_per_connection_is_bounded_to_one_by_rejecting_a_client_reopen() {
        let document_id = Uuid::new_v4();
        let reopen = Frame::Open {
            protocol_version: super::PROTOCOL_VERSION,
            document_id,
            known_seq: None,
            known_frontier: None,
        };
        assert!(
            is_reopen_attempt(&reopen),
            "a steady-state `open` frame must be classified as a reopen attempt"
        );

        let ping = Frame::Ping {
            protocol_version: super::PROTOCOL_VERSION,
            nonce: "n".to_string(),
        };
        assert!(
            !is_reopen_attempt(&ping),
            "frame types other than `open` must never be misclassified as a reopen attempt"
        );

        let update = Frame::Update {
            protocol_version: super::PROTOCOL_VERSION,
            document_id,
            update_id: Uuid::new_v4(),
            base_frontier: String::new(),
            bytes: String::new(),
            idempotency_key: None,
            origin: "test".to_string(),
            message: None,
        };
        assert!(
            !is_reopen_attempt(&update),
            "an `update` frame must never be misclassified as a reopen attempt"
        );
    }

    /// `check_bootstrap_decoded_bytes`'s exact/`+1` boundary: `BOOTSTRAP_DECODED_BYTES_MAX` itself
    /// is accepted; one byte over it is rejected `limit_exceeded` with `limit_kind =
    /// "bootstrap_decoded_bytes"` and the exact `limit`/`observed` values -- no document head or
    /// `event_dispatch` row exists on this read-only loader's path, so there is nothing further to
    /// assert unchanged (`bootstrap::load` never writes on this branch either).
    #[test]
    #[allow(clippy::panic, clippy::indexing_slicing)] // test-only: serde_json::Value field checks + the match's fallback arm; CLAUDE.md's ban is production-code-scoped
    fn bootstrap_decoded_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected() {
        assert!(bootstrap::check_bootstrap_decoded_bytes(limits::BOOTSTRAP_DECODED_BYTES_MAX).is_ok());

        match bootstrap::check_bootstrap_decoded_bytes(limits::BOOTSTRAP_DECODED_BYTES_MAX + 1) {
            Err(ApiError::Typed {
                kind: ApiErrorKind::LimitExceeded,
                details: Some(details),
                ..
            }) => {
                assert_eq!(details["limit_kind"], "bootstrap_decoded_bytes");
                assert_eq!(details["limit"], limits::BOOTSTRAP_DECODED_BYTES_MAX);
                assert_eq!(details["observed"], limits::BOOTSTRAP_DECODED_BYTES_MAX + 1);
            }
            other => panic!("expected a limit_exceeded(bootstrap_decoded_bytes) error, got {other:?}"),
        }
    }

    /// `check_bootstrap_response_bytes`'s exact/`+1` boundary, mirroring the decoded-bytes test
    /// above.
    #[test]
    #[allow(clippy::panic, clippy::indexing_slicing)] // test-only: serde_json::Value field checks + the match's fallback arm; CLAUDE.md's ban is production-code-scoped
    fn bootstrap_response_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected() {
        assert!(bootstrap::check_bootstrap_response_bytes(limits::BOOTSTRAP_RESPONSE_BYTES_MAX).is_ok());

        match bootstrap::check_bootstrap_response_bytes(limits::BOOTSTRAP_RESPONSE_BYTES_MAX + 1) {
            Err(ApiError::Typed {
                kind: ApiErrorKind::LimitExceeded,
                details: Some(details),
                ..
            }) => {
                assert_eq!(details["limit_kind"], "bootstrap_response_bytes");
                assert_eq!(details["limit"], limits::BOOTSTRAP_RESPONSE_BYTES_MAX);
                assert_eq!(details["observed"], limits::BOOTSTRAP_RESPONSE_BYTES_MAX + 1);
            }
            other => panic!("expected a limit_exceeded(bootstrap_response_bytes) error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Live-WebSocket boundary tests for `websocket_frame_bytes`/`presence_payload_bytes`/
    // `presence_ttl_seconds` (opt-in via `OPENPR_TEST_DATABASE_URL`, matching every other
    // real-database suite in this crate). Unlike `frame_rate`/`update_rate` above, these three
    // checks have no lower-level entry point this file exposes to call directly: they live inside
    // [`super::run`]'s live connection loop itself (the frame-length pre-decode gate, the presence
    // payload/ttl gates in `handle_client_frame`), reachable only through a real WebSocket upgrade
    // -- so this nested module spins up a real `axum::serve` listener and drives it with a real
    // `tokio-tungstenite` client, mirroring `routes/collab.rs`'s own `collab_database_tests`
    // harness (kept as an independent copy here rather than shared, the same way
    // `super::database_tests` already keeps its own `Scratch`/`seed_workspace`/`create_page`
    // instead of importing `routes::collab`'s private test-only copies).
    #[cfg(test)]
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::too_many_lines
    )]
    mod live_ws {
        use axum::Router;
        use axum::middleware as axum_middleware;
        use axum::routing::get;
        use futures_util::{SinkExt, StreamExt};
        use platform::{
            app::AppState,
            auth::JwtManager,
            config::{AppConfig, Secret},
        };
        use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
        use std::net::SocketAddr;
        use std::time::Duration;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use uuid::Uuid;

        use crate::flow::collab::frame::{Frame, PROTOCOL_VERSION, RejectedCode};
        use crate::routes::collab::{create_ticket, ws_upgrade};

        const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
        const TEST_ORIGIN: &str = "http://session-live-ws-test.local";
        const JWT_SECRET: &str = "session-live-ws-test-secret";

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

            let name = format!("openpr_session_live_ws_{label}");
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
                    app_name: "session-live-ws-test".to_string(),
                    bind_addr: "127.0.0.1:0".to_string(),
                    database_url: Secret::new("postgres://unused/unused"),
                    jwt_secret: Secret::new(JWT_SECRET),
                    jwt_access_ttl_seconds: 900,
                    jwt_refresh_ttl_seconds: 3600,
                    default_author_id: None,
                    allow_insecure_cookies: false,
                    collab_allowed_origins: vec![TEST_ORIGIN.to_string()],
                },
                db,
            }
        }

        async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
            state
                .db
                .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
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
                vec![owner_id.into(), format!("{owner_id}@session-live-ws.test").into()],
            )
            .await;
            exec(
                state,
                "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'session live ws test', $3)",
                vec![
                    workspace_id.into(),
                    format!("ws-{workspace_id}").into(),
                    owner_id.into(),
                ],
            )
            .await;
            exec(
                state,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
                vec![workspace_id.into(), owner_id.into()],
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
            use crate::flow::command::{CreateObjectInput, create_object};
            let accepted = create_object(
                state,
                CreateObjectInput {
                    workspace_id,
                    actor_id,
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Session Live WS Test Page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                },
            )
            .await
            .expect("object creation succeeds");
            (accepted.object.id, accepted.object.document_id)
        }

        fn jwt_for(user_id: Uuid) -> String {
            let manager = JwtManager::new(JWT_SECRET, 900, 3600);
            manager
                .issue_access_token(&user_id.to_string(), &format!("{user_id}@session-live-ws.test"))
                .expect("token issues")
        }

        /// Spins up a real listener serving only the two routes these tests need (ticket issuance
        /// plus the WS upgrade itself) — a strict subset of `routes/collab.rs`'s own
        /// `collab_database_tests::spawn_server`, which additionally wires diagnostics/verify/
        /// bootstrap this module has no use for.
        async fn spawn_server(state: AppState) -> SocketAddr {
            let auth_state = state.clone();
            let app = Router::new()
                .route(
                    "/api/v1/collab/tickets",
                    axum::routing::post(create_ticket).route_layer(axum_middleware::from_fn_with_state(
                        auth_state,
                        crate::middleware::bot_auth::bot_or_user_auth_middleware,
                    )),
                )
                .route("/api/v1/collab/ws", get(ws_upgrade))
                .with_state(state);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("binds an ephemeral port");
            let addr = listener.local_addr().expect("listener has a local address");
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            tokio::time::sleep(Duration::from_millis(20)).await;
            addr
        }

        async fn issue_ticket(
            addr: SocketAddr,
            token: &str,
            workspace_id: Uuid,
            document_id: Uuid,
            client_id: &str,
        ) -> String {
            let client = reqwest::Client::new();
            let response = client
                .post(format!("http://{addr}/api/v1/collab/tickets"))
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "workspace_id": workspace_id,
                    "document_id": document_id,
                    "client_id": client_id,
                    "origin": TEST_ORIGIN,
                }))
                .send()
                .await
                .expect("ticket request completes");
            let body: serde_json::Value = response.json().await.expect("ticket response is JSON");
            assert_eq!(body["code"], 0, "ticket issuance failed: {body}");
            body["data"]["ticket"].as_str().expect("ticket is a string").to_string()
        }

        type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

        async fn connect(addr: SocketAddr, ticket: &str, client_id: &str) -> WsStream {
            let url = format!("ws://{addr}/api/v1/collab/ws?ticket={ticket}&client_id={client_id}");
            let mut request = url.into_client_request().expect("builds a client request");
            request
                .headers_mut()
                .insert("Origin", TEST_ORIGIN.parse().expect("valid header value"));
            let (stream, response) = tokio_tungstenite::connect_async(request)
                .await
                .expect("upgrade succeeds");
            assert_eq!(response.status(), 101);
            stream
        }

        async fn send_frame(ws: &mut WsStream, frame: &Frame) {
            let text = serde_json::to_string(frame).expect("frame serializes");
            ws.send(TMessage::Text(text.into())).await.expect("send succeeds");
        }

        /// Sends a raw text WS message that is not necessarily a valid `Frame` -- used by the
        /// `websocket_frame_bytes` plus-one case, which must be rejected on length alone before
        /// ever being JSON-parsed (`read_frame`'s `text.len() > WEBSOCKET_FRAME_BYTES_MAX` check
        /// runs before `serde_json::from_str`).
        async fn send_raw_text(ws: &mut WsStream, text: String) {
            ws.send(TMessage::Text(text.into())).await.expect("send succeeds");
        }

        async fn recv_frame(ws: &mut WsStream) -> Frame {
            loop {
                let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                    .await
                    .expect("a frame arrives before the timeout")
                    .expect("the stream is not closed")
                    .expect("the frame is not a transport error");
                match message {
                    TMessage::Text(text) => return serde_json::from_str(text.as_str()).expect("frame deserializes"),
                    TMessage::Ping(_) | TMessage::Pong(_) => {}
                    other => panic!("unexpected non-text frame: {other:?}"),
                }
            }
        }

        /// Drives the real `hello`/`open`/`snapshot` handshake `super::run` requires before its
        /// steady-state loop (where the frame-length/presence gates live) is ever reached, and
        /// hands back the connected stream plus the document's starting `head_seq`.
        async fn open_session(addr: SocketAddr, ticket: &str, client_id: &str, document_id: Uuid) -> (WsStream, i64) {
            let mut ws = connect(addr, ticket, client_id).await;
            send_frame(
                &mut ws,
                &Frame::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    capabilities: vec![],
                    client_id: client_id.to_string(),
                    session_id: Uuid::new_v4(),
                },
            )
            .await;
            let hello_reply = recv_frame(&mut ws).await;
            assert!(
                matches!(hello_reply, Frame::Hello { .. }),
                "expected a hello reply, got {hello_reply:?}"
            );

            send_frame(
                &mut ws,
                &Frame::Open {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    known_seq: None,
                    known_frontier: None,
                },
            )
            .await;
            let snapshot_frame = recv_frame(&mut ws).await;
            let Frame::Snapshot { head_seq, .. } = snapshot_frame else {
                panic!("expected a snapshot frame, got {snapshot_frame:?}");
            };
            (ws, head_seq)
        }

        async fn count_event_dispatch(state: &AppState, document_id: Uuid) -> i64 {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                n: i64,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM event_dispatch WHERE document_id = $1",
                vec![document_id.into()],
            ))
            .one(&state.db)
            .await
            .expect("count query runs")
            .expect("count query returns a row")
            .n
        }

        async fn read_head_seq(state: &AppState, document_id: Uuid) -> i64 {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                head_seq: i64,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE id = $1",
                vec![document_id.into()],
            ))
            .one(&state.db)
            .await
            .expect("head_seq query runs")
            .expect("document row exists")
            .head_seq
        }

        /// `websocket_frame_bytes_max` (131,072 bytes) is checked pre-decode in `read_frame`,
        /// before the text is ever handed to `serde_json::from_str`. Proven with a real,
        /// well-formed `Frame::Ping` whose encoded length is exactly the ceiling (accepted and
        /// answered with a `pong`), and a 131,073-byte raw text message one byte over it
        /// (rejected on length alone, so it need not even be valid JSON) — both against the same
        /// still-open connection, and the oversized send provably advances neither the document
        /// head nor `event_dispatch`.
        #[tokio::test]
        async fn websocket_frame_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
            let scratch = scratch_or_skip!("frame-bytes");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let client_id = "frame-bytes-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, head_seq_before) = open_session(addr, &ticket, client_id, document_id).await;

            const WEBSOCKET_FRAME_BYTES_MAX: usize = 131_072;

            // ---- exact boundary: a real Frame::Ping padded to exactly the ceiling ----
            let base_len = serde_json::to_string(&Frame::Ping {
                protocol_version: PROTOCOL_VERSION,
                nonce: String::new(),
            })
            .expect("ping serializes")
            .len();
            assert!(WEBSOCKET_FRAME_BYTES_MAX >= base_len);
            let exact_ping = Frame::Ping {
                protocol_version: PROTOCOL_VERSION,
                nonce: "a".repeat(WEBSOCKET_FRAME_BYTES_MAX - base_len),
            };
            let exact_len = serde_json::to_string(&exact_ping).expect("ping serializes").len();
            assert_eq!(exact_len, WEBSOCKET_FRAME_BYTES_MAX);
            send_frame(&mut ws, &exact_ping).await;
            let pong = recv_frame(&mut ws).await;
            assert!(
                matches!(pong, Frame::Pong { .. }),
                "an exactly-at-ceiling frame must be processed normally, got {pong:?}"
            );

            let dispatch_before_plus_one = count_event_dispatch(&state, document_id).await;

            // ---- plus one: an arbitrary 131,073-byte text message, rejected on length alone ----
            send_raw_text(&mut ws, "a".repeat(WEBSOCKET_FRAME_BYTES_MAX + 1)).await;
            let rejected = recv_frame(&mut ws).await;
            let Frame::Rejected { code, details, .. } = rejected else {
                panic!("expected a rejected frame, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::LimitExceeded);
            let details = details.expect("a limit_exceeded rejection must carry details");
            assert_eq!(details["limit_kind"], "websocket_frame_bytes");
            assert_eq!(details["limit"], WEBSOCKET_FRAME_BYTES_MAX as u64);
            assert_eq!(details["observed"], (WEBSOCKET_FRAME_BYTES_MAX + 1) as u64);

            assert_eq!(
                read_head_seq(&state, document_id).await,
                head_seq_before,
                "an over-ceiling frame must never advance the document head"
            );
            assert_eq!(
                count_event_dispatch(&state, document_id).await,
                dispatch_before_plus_one,
                "an over-ceiling frame must never produce a new event_dispatch row"
            );

            scratch.drop_self().await;
        }

        /// `presence_payload_bytes_max` (8,192 bytes): checked in `handle_client_frame`'s
        /// `Frame::Presence` arm before the payload ever reaches `SessionRegistry::upsert_presence`.
        /// Proven with a real presence payload whose JSON-encoded length is exactly the ceiling
        /// (accepted and rebroadcast) and one byte over it (rejected).
        ///
        /// Two connections are needed, not one: `handle_client_frame`'s `Ok(())` arm broadcasts
        /// the accepted presence with `exclude: Some(session_id)` (`registry.rs`'s own doc
        /// comment on `broadcast`), i.e. the *sender* never sees its own accepted presence echoed
        /// back to itself -- only a `limit_exceeded` rejection is ever sent directly to the
        /// sender's own socket. `sender` proves the accept case indirectly too: reaching the
        /// plus-one send at all (rather than the connection having been dropped) already shows
        /// the exact-boundary send did not error out, but `observer` receiving the real broadcast
        /// is the actual proof the payload was accepted and stored.
        #[tokio::test]
        async fn presence_payload_bytes_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
            let scratch = scratch_or_skip!("presence-bytes");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let (mut sender, head_seq_before) = open_session(
                addr,
                &issue_ticket(addr, &token, workspace_id, document_id, "presence-bytes-sender").await,
                "presence-bytes-sender",
                document_id,
            )
            .await;
            let (mut observer, _) = open_session(
                addr,
                &issue_ticket(addr, &token, workspace_id, document_id, "presence-bytes-observer").await,
                "presence-bytes-observer",
                document_id,
            )
            .await;

            const PRESENCE_PAYLOAD_BYTES_MAX: usize = 8_192;

            // `encoded_payload = serde_json::to_vec(&payload)` in `handle_client_frame` re-encodes
            // just the `payload` value on its own (not the whole `Frame::Presence` envelope), so
            // the padding target is the payload's own encoded length, not the frame's.
            let session_id = Uuid::new_v4();
            let payload_of_len = |len: usize| {
                let base = serde_json::to_vec(&serde_json::json!({"cursor": ""}))
                    .expect("payload serializes")
                    .len();
                assert!(len >= base);
                serde_json::json!({"cursor": "a".repeat(len - base)})
            };
            let exact_payload = payload_of_len(PRESENCE_PAYLOAD_BYTES_MAX);
            assert_eq!(
                serde_json::to_vec(&exact_payload).expect("payload serializes").len(),
                PRESENCE_PAYLOAD_BYTES_MAX
            );

            send_frame(
                &mut sender,
                &Frame::Presence {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    session_id,
                    payload: exact_payload.clone(),
                    ttl_seconds: None,
                },
            )
            .await;
            let echoed = recv_frame(&mut observer).await;
            let Frame::Presence { payload, .. } = echoed else {
                panic!("an exactly-at-ceiling presence must be accepted and broadcast to peers, got {echoed:?}");
            };
            assert_eq!(payload, exact_payload);

            let dispatch_before_plus_one = count_event_dispatch(&state, document_id).await;

            // ---- plus one: one byte over the ceiling, rejected straight back to the sender ----
            let plus_one_payload = payload_of_len(PRESENCE_PAYLOAD_BYTES_MAX + 1);
            send_frame(
                &mut sender,
                &Frame::Presence {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    session_id,
                    payload: plus_one_payload,
                    ttl_seconds: None,
                },
            )
            .await;
            let rejected = recv_frame(&mut sender).await;
            let Frame::Rejected { code, details, .. } = rejected else {
                panic!("expected a rejected frame, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::LimitExceeded);
            let details = details.expect("a limit_exceeded rejection must carry details");
            assert_eq!(details["limit_kind"], "presence_payload_bytes");
            assert_eq!(details["limit"], PRESENCE_PAYLOAD_BYTES_MAX as u64);
            assert_eq!(details["observed"], (PRESENCE_PAYLOAD_BYTES_MAX + 1) as u64);

            assert_eq!(
                read_head_seq(&state, document_id).await,
                head_seq_before,
                "presence traffic must never touch the document head"
            );
            assert_eq!(
                count_event_dispatch(&state, document_id).await,
                dispatch_before_plus_one,
                "a rejected presence payload must never produce a new event_dispatch row"
            );

            scratch.drop_self().await;
        }

        /// `presence_ttl_seconds_max` (30): checked in `handle_client_frame`'s `Frame::Presence`
        /// arm, right after the payload-bytes gate. Proven with `ttl_seconds=30` (accepted and
        /// broadcast to a peer with the exact ttl echoed back) and `ttl_seconds=31` (rejected
        /// `limit_kind=presence_ttl_seconds`, straight back to the sender). Two connections for
        /// the same reason as the `presence_payload_bytes` test above: an accepted presence is
        /// broadcast excluding its own sender.
        #[tokio::test]
        async fn presence_ttl_seconds_exact_boundary_accepted_plus_one_rejected_zero_side_effects() {
            let scratch = scratch_or_skip!("presence-ttl");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let (mut sender, head_seq_before) = open_session(
                addr,
                &issue_ticket(addr, &token, workspace_id, document_id, "presence-ttl-sender").await,
                "presence-ttl-sender",
                document_id,
            )
            .await;
            let (mut observer, _) = open_session(
                addr,
                &issue_ticket(addr, &token, workspace_id, document_id, "presence-ttl-observer").await,
                "presence-ttl-observer",
                document_id,
            )
            .await;

            const PRESENCE_TTL_SECONDS_MAX: u32 = 30;
            let session_id = Uuid::new_v4();
            let payload = serde_json::json!({"cursor": "boundary"});

            send_frame(
                &mut sender,
                &Frame::Presence {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    session_id,
                    payload: payload.clone(),
                    ttl_seconds: Some(PRESENCE_TTL_SECONDS_MAX),
                },
            )
            .await;
            let echoed = recv_frame(&mut observer).await;
            let Frame::Presence { ttl_seconds, .. } = echoed else {
                panic!("a ttl_seconds exactly at the ceiling must be accepted and broadcast to peers, got {echoed:?}");
            };
            assert_eq!(ttl_seconds, Some(PRESENCE_TTL_SECONDS_MAX));

            let dispatch_before_plus_one = count_event_dispatch(&state, document_id).await;

            send_frame(
                &mut sender,
                &Frame::Presence {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    session_id,
                    payload,
                    ttl_seconds: Some(PRESENCE_TTL_SECONDS_MAX + 1),
                },
            )
            .await;
            let rejected = recv_frame(&mut sender).await;
            let Frame::Rejected { code, details, .. } = rejected else {
                panic!("expected a rejected frame, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::LimitExceeded);
            let details = details.expect("a limit_exceeded rejection must carry details");
            assert_eq!(details["limit_kind"], "presence_ttl_seconds");
            assert_eq!(details["limit"], u64::from(PRESENCE_TTL_SECONDS_MAX));
            assert_eq!(details["observed"], u64::from(PRESENCE_TTL_SECONDS_MAX + 1));

            assert_eq!(
                read_head_seq(&state, document_id).await,
                head_seq_before,
                "presence traffic must never touch the document head"
            );
            assert_eq!(
                count_event_dispatch(&state, document_id).await,
                dispatch_before_plus_one,
                "a rejected presence ttl must never produce a new event_dispatch row"
            );

            scratch.drop_self().await;
        }

        /// `open_documents_per_connection_max`'s structural guarantee (`session.rs`'s
        /// `is_reopen_attempt`), proven end to end rather than only against the pure classifier: a
        /// real connection that already completed its handshake sends a second `open` for the
        /// *same* document over the same socket. It must be rejected `invalid_update` (never
        /// treated as opening a second document), the connection must stay open (a still-usable
        /// `ping`/`pong` round trip proves it), and neither the document head nor `event_dispatch`
        /// may advance.
        #[tokio::test]
        async fn open_documents_per_connection_rejects_a_client_reopen_over_a_real_connection() {
            let scratch = scratch_or_skip!("reopen-attempt");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let client_id = "reopen-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, head_seq_before) = open_session(addr, &ticket, client_id, document_id).await;

            let dispatch_before = count_event_dispatch(&state, document_id).await;

            send_frame(
                &mut ws,
                &Frame::Open {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    known_seq: None,
                    known_frontier: None,
                },
            )
            .await;
            let rejected = recv_frame(&mut ws).await;
            let Frame::Rejected { code, .. } = rejected else {
                panic!("expected a rejected frame for a steady-state reopen, got {rejected:?}");
            };
            assert_eq!(
                code,
                RejectedCode::InvalidUpdate,
                "a second open on an already-open connection must never be treated as opening a document"
            );

            assert_eq!(
                read_head_seq(&state, document_id).await,
                head_seq_before,
                "a rejected reopen attempt must never advance the document head"
            );
            assert_eq!(
                count_event_dispatch(&state, document_id).await,
                dispatch_before,
                "a rejected reopen attempt must never produce a new event_dispatch row"
            );

            // The connection itself must stay open (`invalid_update` never closes at steady state)
            // -- proven with a real ping/pong round trip after the rejection.
            send_frame(
                &mut ws,
                &Frame::Ping {
                    protocol_version: PROTOCOL_VERSION,
                    nonce: "still-open".to_string(),
                },
            )
            .await;
            let pong = recv_frame(&mut ws).await;
            assert!(
                matches!(pong, Frame::Pong { .. }),
                "the connection must remain usable after a rejected reopen, got {pong:?}"
            );

            scratch.drop_self().await;
        }
    }
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
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::authz;
    use crate::flow::collab::cache::WarmCache;
    use crate::flow::collab::coordinator::DocumentCoordinator;
    use crate::flow::collab::registry::SessionRegistry;
    use crate::flow::collab::snapshot::SnapshotAdvancer;
    use crate::flow::collab::write::{self, AcceptOutcome, UpdateRequest};
    use crate::flow::collab::{bootstrap, limits};
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

    /// Proves `bootstrap_decoded_bytes_max` is wired into the real `bootstrap::load` call path,
    /// not only into `check_bootstrap_decoded_bytes` in isolation (`session::tests`'s own
    /// `bootstrap_decoded_bytes_exact_boundary_...` only calls that pure function directly, so it
    /// alone could not detect the wiring itself being removed from `load`). Writes the document's
    /// `snapshot` bytes directly (bypassing the real CRDT write path entirely -- `load` never
    /// hashes or otherwise validates `snapshot` bytes, only the tail's `content_hash` chain, and
    /// this document has zero tail rows) at the exact ceiling (accepted) and one byte over
    /// (rejected `limit_exceeded`), against the real `collab_documents` row through a real
    /// database.
    #[tokio::test]
    async fn bootstrap_load_enforces_the_decoded_bytes_ceiling_against_a_real_document_row() {
        let scratch = scratch_or_skip!("bootstrap-decoded-bytes-ceiling");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let at_ceiling = vec![0u8; usize::try_from(limits::BOOTSTRAP_DECODED_BYTES_MAX).expect("fits usize")];
        exec(
            &state,
            "UPDATE collab_documents SET snapshot = $1 WHERE id = $2",
            vec![at_ceiling.into(), document_id.into()],
        )
        .await;
        let accepted = bootstrap::load(&state.db, document_id).await;
        assert!(
            accepted.is_ok(),
            "a document with exactly BOOTSTRAP_DECODED_BYTES_MAX decoded bytes must be accepted, got {accepted:?}"
        );

        let over_ceiling = vec![0u8; usize::try_from(limits::BOOTSTRAP_DECODED_BYTES_MAX).expect("fits usize") + 1];
        exec(
            &state,
            "UPDATE collab_documents SET snapshot = $1 WHERE id = $2",
            vec![over_ceiling.into(), document_id.into()],
        )
        .await;
        match bootstrap::load(&state.db, document_id).await {
            Err(ApiError::Typed {
                kind: ApiErrorKind::LimitExceeded,
                details: Some(details),
                ..
            }) => {
                assert_eq!(details["limit_kind"], "bootstrap_decoded_bytes");
                assert_eq!(details["limit"], limits::BOOTSTRAP_DECODED_BYTES_MAX);
                assert_eq!(details["observed"], limits::BOOTSTRAP_DECODED_BYTES_MAX + 1);
            }
            other => panic!(
                "expected a real bootstrap::load call to reject with limit_exceeded(bootstrap_decoded_bytes), got {other:?}"
            ),
        }

        scratch.drop_self().await;
    }

    /// Proves `bootstrap_response_bytes_max` is wired into the real `bootstrap::load` call path,
    /// the same way the decoded-bytes test above does. `bootstrap_decoded_bytes` (snapshot + tail
    /// update bytes only, per its own definition) does not count `head_frontier`, so an oversized
    /// `head_frontier` pushes `estimated_response_bytes` over its ceiling while
    /// `check_bootstrap_decoded_bytes` still passes -- exercising `check_bootstrap_response_bytes`
    /// specifically, not `check_bootstrap_decoded_bytes` a second time. Zero tail rows means the
    /// only integrity requirement `load` imposes on the frontier is
    /// `snapshot_frontier == head_frontier` (`running_frontier` never advances past
    /// `doc.snapshot_frontier` when there is nothing to fold in), which setting both columns to
    /// the identical oversized value satisfies without needing a real CRDT frontier.
    #[tokio::test]
    async fn bootstrap_load_enforces_the_response_bytes_ceiling_against_a_real_document_row() {
        let scratch = scratch_or_skip!("bootstrap-response-bytes-ceiling");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        // Comfortably under `bootstrap_decoded_bytes_max` on its own (well under 1 MiB); large
        // enough that a genuinely wired response-bytes check, not merely luck, is what accepts it.
        let modest_frontier = vec![7u8; 4096];
        exec(
            &state,
            "UPDATE collab_documents SET snapshot_frontier = $1, head_frontier = $1 WHERE id = $2",
            vec![modest_frontier.into(), document_id.into()],
        )
        .await;
        let accepted = bootstrap::load(&state.db, document_id).await;
        assert!(
            accepted.is_ok(),
            "a modest head_frontier must not trip bootstrap_response_bytes, got {accepted:?}"
        );

        // `estimated_response_bytes` ~= base64_len(snapshot) + base64_len(head_frontier) + a fixed
        // per-tail-update allowance (zero tail rows here, so that term is zero). This document's
        // `snapshot` is a real but tiny CRDT export (well under 1 KiB), so an oversized
        // `head_frontier` alone must decide this: `BOOTSTRAP_DECODED_BYTES_MAX` never even sees
        // `head_frontier`'s length (its own definition is `snapshot.len() + sum(tail bytes)`), so
        // this exercises `check_bootstrap_response_bytes` without `check_bootstrap_decoded_bytes`
        // ever objecting first.
        let oversized_frontier = vec![7u8; 9_500_000];
        exec(
            &state,
            "UPDATE collab_documents SET snapshot_frontier = $1, head_frontier = $1 WHERE id = $2",
            vec![oversized_frontier.into(), document_id.into()],
        )
        .await;
        match bootstrap::load(&state.db, document_id).await {
            Err(ApiError::Typed {
                kind: ApiErrorKind::LimitExceeded,
                details: Some(details),
                ..
            }) => {
                assert_eq!(details["limit_kind"], "bootstrap_response_bytes");
                assert_eq!(details["limit"], limits::BOOTSTRAP_RESPONSE_BYTES_MAX);
            }
            other => panic!(
                "expected a real bootstrap::load call to reject with limit_exceeded(bootstrap_response_bytes), got {other:?}"
            ),
        }

        scratch.drop_self().await;
    }
}
