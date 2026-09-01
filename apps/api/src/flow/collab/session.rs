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
use super::frame::{DrainSignal, Frame, PROTOCOL_VERSION, RejectedCode, TailUpdate, WriteState};
use super::limits::{
    CONNECTION_LIMIT_RETRY_AFTER_MS, FRAME_BURST_MAX, FRAMES_PER_CONNECTION_PER_SECOND,
    OPEN_DOCUMENTS_PER_CONNECTION_MAX, PRESENCE_PAYLOAD_BYTES_MAX, PRESENCE_TTL_SECONDS_DEFAULT,
    PRESENCE_TTL_SECONDS_MAX, RATE_LIMIT_RETRY_AFTER_MS, UPDATE_BURST_MAX, UPDATES_PER_CONNECTION_PER_SECOND,
    WEBSOCKET_FRAME_BYTES_MAX,
};
use super::registry::{ConnectionLimit, OutboundEvent, PresenceLimit};
use super::runtime;
use super::ticket::ConsumedTicket;
use super::write::{self, AcceptOutcome, UpdateRequest};
use crate::error::ApiErrorKind;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a connection may go without producing an inbound frame before the server sends its
/// own `ping`, and — once that `ping` is outstanding — how long it then waits for any inbound
/// frame before hanging up (`collab-protocol-v1.md`'s `ping/pong nonce/time` heartbeat).
///
/// **Not a `limits-v1.md` row**: that contract freezes no heartbeat interval or pong deadline (the
/// same status [`super::limits::CONNECTION_LIMIT_RETRY_AFTER_MS`] and
/// [`super::limits::PROCESS_DRAIN_GRACE_MS`] carry, and recorded as a contract to-do rather than
/// invented here). The *number* is taken verbatim from the one frozen value that already defines
/// this system's crash-detection budget, [`PRESENCE_TTL_SECONDS_MAX`] = 30 s — `limits-v1.md`
/// justifies that value as "使 crash 后 cursor/selection 最迟 30 秒消失". A dead peer's presence
/// entry is therefore already gone by the time the first heartbeat fires, and the socket itself is
/// reclaimed within one further interval; picking anything larger would leave sockets outliving
/// the ephemeral state the contract says they own, and anything smaller would be a number this
/// package made up.
const HEARTBEAT_IDLE: Duration = Duration::from_secs(PRESENCE_TTL_SECONDS_MAX as u64);

/// RFC 6455's registered "going away" code, used when a peer stops answering the heartbeat. Not an
/// application close code: `error-mapping-v1.md` freezes the 44xx range for contract rejections,
/// and an unresponsive transport is not one of them — nothing was rejected, the peer simply
/// stopped being reachable.
const CLOSE_GOING_AWAY: u16 = 1001;

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

/// Sends the structured drain discriminator before the frozen 4410 close. Active sessions receive
/// the same frame through [`super::registry::SessionRegistry::drain_workspace`]; using it during
/// handshake too ensures a UI never has to infer maintenance from close prose.
async fn reject_drain_and_close(socket: &mut WebSocket, document_id: Uuid, signal: DrainSignal) {
    send(
        socket,
        &Frame::Rejected {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            update_id: None,
            code: RejectedCode::ServerDraining,
            recoverable: true,
            // Handshake-phase drain: the connection is refused before any `update` frame can
            // even be read, so no write of this session's was attempted.
            write_state: WriteState::NotApplied,
            details: Some(signal.details()),
            current_seq: None,
            current_frontier: None,
            audit_event_id: None,
        },
    )
    .await;
    close(
        socket,
        ws_close_code_for(RejectedCode::ServerDraining),
        &signal.close_reason(),
    )
    .await;
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
        // Every producer of this frame refuses *before* the payload reaches the write path: an
        // oversized/over-rate frame is rejected on the raw bytes before decode, and a registry
        // admission refusal happens before the session is even registered. None of them can have
        // written canonical state.
        write_state: WriteState::NotApplied,
        details: Some(details),
        current_seq: None,
        current_frontier: None,
        audit_event_id: None,
    }
}

/// The `limit_exceeded` frame a [`SessionRegistry::try_register`] refusal becomes on the wire.
///
/// No `observed`: `limits-v1.md`'s `limit_exceeded` details rule says "`observed` 只允许安全数值,
/// 不返回 content、bytes、**其他用户连接** 或过滤前结果" — `document_connections`/
/// `workspace_connections` are counts of *other people's* sessions, so reporting them back to the
/// refused client is exactly what that sentence forbids. `retry_after_ms` is what a connection
/// ceiling gives a caller instead ("rate/connection/queue 可按 `retry_after_ms` 重试").
fn connection_limit_frame(document_id: Uuid, limit: ConnectionLimit) -> Frame {
    limit_exceeded_frame(
        document_id,
        limit.limit_kind(),
        limit.limit(),
        None,
        Some(CONNECTION_LIMIT_RETRY_AFTER_MS),
    )
}

/// The `limit_exceeded` frame a refused `presence` upsert becomes on the wire. `observed` is
/// omitted for the same reason as [`connection_limit_frame`]: both presence ceilings count
/// entries owned by other sessions. Neither is retryable on a timer — the client must drop a
/// presence entry, not wait — so no `retry_after_ms` either.
fn presence_limit_frame(document_id: Uuid, limit: PresenceLimit) -> Frame {
    limit_exceeded_frame(document_id, limit.limit_kind(), limit.limit(), None, None)
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

/// What [`Heartbeat::evaluate_at`] wants the session loop to do at this instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeartbeatAction {
    /// The peer has been heard from recently enough, or an outstanding `ping` has not yet timed
    /// out. Do nothing.
    Idle,
    /// The connection has been silent for a full [`HEARTBEAT_IDLE`]. Send a server `ping`; the
    /// peer's `pong` (or literally any other inbound frame) clears it.
    SendPing,
    /// A `ping` has been outstanding for a full [`HEARTBEAT_IDLE`] with no inbound frame at all.
    /// The peer is gone; close at [`CLOSE_GOING_AWAY`].
    Close,
}

/// The server half of `collab-protocol-v1.md`'s `ping/pong` heartbeat: liveness detection for a
/// peer that has stopped sending anything at all (a half-open TCP connection produces no `Close`
/// frame and no read error, so the session loop would otherwise hold the connection slot — and the
/// `limits-v1.md` `user_connections`/`document_connections`/`workspace_connections` budget it
/// occupies — indefinitely).
///
/// Deliberately keyed on *any* inbound frame, not only `pong`: an actively editing client that
/// never implements `ping` handling is provably alive, and hanging up on it would be a regression
/// dressed up as a health check.
///
/// Split out as pure logic parameterized on "now", exactly like [`RateLimiter::take_at`], so the
/// 30-second interval is testable in microseconds with manually advanced `Instant`s instead of
/// real sleeping.
struct Heartbeat {
    idle_after: Duration,
    last_inbound: Instant,
    /// When the currently-outstanding server `ping` was sent, if one is outstanding.
    ping_sent_at: Option<Instant>,
}

impl Heartbeat {
    const fn new(idle_after: Duration, now: Instant) -> Self {
        Self {
            idle_after,
            last_inbound: now,
            ping_sent_at: None,
        }
    }

    /// Any inbound frame — `pong` included — proves the peer is alive and clears an outstanding
    /// heartbeat.
    const fn record_inbound(&mut self, now: Instant) {
        self.last_inbound = now;
        self.ping_sent_at = None;
    }

    fn evaluate_at(&mut self, now: Instant) -> HeartbeatAction {
        if let Some(sent_at) = self.ping_sent_at {
            if now.duration_since(sent_at) >= self.idle_after {
                return HeartbeatAction::Close;
            }
            return HeartbeatAction::Idle;
        }
        if now.duration_since(self.last_inbound) >= self.idle_after {
            self.ping_sent_at = Some(now);
            return HeartbeatAction::SendPing;
        }
        HeartbeatAction::Idle
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

/// The bare `rejected` frame for every *pre-write* refusal this module produces: handshake and
/// authorization failures, protocol/decode errors, and payload validation that runs before
/// `write::accept_update` is ever called. All of them are
/// [`WriteState::NotApplied`](super::frame::WriteState::NotApplied) by construction, which is why
/// this constructor pins the field rather than taking it as a parameter — a rejection produced
/// *after* a write attempt must not be built here at all, it must carry the `write_state` the
/// write path itself computed (see [`write::Rejected`](super::write::Rejected)).
const fn rejected_frame(document_id: Uuid, code: RejectedCode, recoverable: bool, update_id: Option<Uuid>) -> Frame {
    Frame::Rejected {
        protocol_version: PROTOCOL_VERSION,
        document_id,
        update_id,
        code,
        recoverable,
        write_state: WriteState::NotApplied,
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
        known_seq,
        known_frontier,
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
    // `collab-protocol-v1.md`'s `open known_seq/known_frontier`: a reconnecting client that still
    // holds the document up to a seq this server can continue from gets the accepted stream it
    // missed instead of the whole snapshot. Any refusal falls back to the full bootstrap below,
    // which is always a correct answer to `open`.
    match plan_resume(&state.db, document_id, &boot, known_seq, known_frontier.as_deref()).await {
        Ok(frames) => {
            for frame in &frames {
                send(&mut socket, frame).await;
            }
        }
        Err(refusal) => {
            if refusal != ResumeRefusal::NotRequested {
                tracing::debug!(
                    %document_id, ?refusal, ?known_seq,
                    "collab session: resume refused, falling back to a full snapshot"
                );
            }
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
        }
    }

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
                send(&mut socket, &connection_limit_frame(document_id, limit)).await;
                close(&mut socket, LIMIT_EXCEEDED_CLOSE_CODE, "connection limit exceeded").await;
                return;
            }
        };
    // The "join" half of presence fan-out. `SessionRegistry::broadcast` only reaches sessions that
    // were already connected when a peer's `presence` frame arrived, so without this a session
    // joining a document sees an empty document until every peer happens to refresh — up to a full
    // `presence_ttl_seconds_max`. Read-only: sending these neither creates nor refreshes any entry,
    // so a joining session cannot keep a departed peer's cursor alive. Sent directly on the socket
    // (not through the registry) because it is addressed to this one session, and before the loop
    // below starts so it can never overtake a live `presence` broadcast for the same peer.
    for (peer_session_id, payload) in collab.registry.presence_snapshot(document_id, Some(session_id)) {
        send(
            &mut socket,
            &Frame::Presence {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                session_id: peer_session_id,
                payload,
                ttl_seconds: None,
            },
        )
        .await;
    }

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
    // The ticket-bound document is the first attempted open. v0.4 never admits a second document
    // on this socket, but counting repeated `open` abuse still makes the frozen
    // `open_documents` limit_kind observable on the ninth attempted subscription.
    let mut open_attempts = 1u64;
    // `collab-protocol-v1.md`'s `ping/pong` heartbeat, server side. A half-open TCP connection
    // produces neither a `Close` frame nor a read error, so without this the `select!` below would
    // park forever on a peer that is already gone while still holding its `limits-v1.md` connection
    // slot. The ticker's period matches the idle threshold so at most two ticks (one to send the
    // `ping`, one to observe no answer) are ever needed.
    let mut heartbeat = Heartbeat::new(HEARTBEAT_IDLE, Instant::now());
    let mut heartbeat_ticks = tokio::time::interval(HEARTBEAT_IDLE);
    heartbeat_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The interval's first tick completes immediately; consume it here so the first real evaluation
    // happens one full period in, not at connection time.
    heartbeat_ticks.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = heartbeat_ticks.tick() => {
                match heartbeat.evaluate_at(Instant::now()) {
                    HeartbeatAction::Idle => {}
                    HeartbeatAction::SendPing => {
                        send(&mut socket, &Frame::Ping {
                            protocol_version: PROTOCOL_VERSION,
                            nonce: Uuid::new_v4().to_string(),
                        }).await;
                    }
                    HeartbeatAction::Close => {
                        close(&mut socket, CLOSE_GOING_AWAY, "heartbeat timeout").await;
                        break;
                    }
                }
            }
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
                    Some(OutboundEvent::ControlFrame(frame)) => {
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
                // Any inbound frame -- `pong`, `update`, `presence`, even a transport-level
                // ping/pong -- proves the peer is alive and clears an outstanding heartbeat.
                heartbeat.record_inbound(Instant::now());
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
                            // `error-mapping-v1.md`'s `drain` reason: "实例/workspace 正在停止接收
                            // 或排空连接". `reverify_open` already refuses a *handshake* against a
                            // draining workspace and `SessionRegistry::drain_workspace`/`drain_all`
                            // close the sessions live at the instant a drain starts, but neither
                            // covers the frame already in flight on this socket when that happened
                            // -- without this check it would still be committed to the canonical
                            // document by an instance that has announced it stopped accepting
                            // work. Checked only for `update` frames: those are the "new work" a
                            // drain refuses, and this is a process-global lock, not something to
                            // take on every `ping`/`presence` frame at 30/s per connection.
                            if let Some(signal) = collab.workspace_drain_signal(consumed.workspace_id) {
                                reject_drain_and_close(&mut socket, document_id, signal).await;
                                break;
                            }
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
                            &mut sequencer,
                            &mut pending_updates,
                            &mut open_attempts,
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
        SeqDecision::Duplicate | SeqDecision::ResyncPending => {}
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
                GapResolution::Resync { frame } => {
                    sequencer.give_up_and_resync();
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
    Resync { frame: Frame },
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

/// Why a client's `open{known_seq, known_frontier}` could not be resumed, and the connection has
/// to fall back to a full `snapshot` bootstrap instead. Every variant is a normal, expected
/// outcome — never an error the client is told about beyond receiving a `snapshot` rather than the
/// `ack` + replay a resume produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeRefusal {
    /// No `known_seq`/`known_frontier` pair was supplied: a first connection, not a reconnect.
    /// Both are required — resuming on `known_seq` alone would let a client whose local state had
    /// diverged (a partially-applied update, a crashed tab that never persisted its outbox)
    /// silently continue from a position it does not actually hold.
    NotRequested,
    /// `known_seq` is outside `[snapshot_seq, head_seq]`: either behind the retained history
    /// boundary (v0.8 compaction) or ahead of the canonical head, which no client may legitimately
    /// be.
    OutOfRange,
    /// The frontier the client claims at `known_seq` is not the one the server recorded there.
    FrontierMismatch,
    /// `(known_seq, head_seq]` is not fully readable from `collab_updates`.
    HistoryUnavailable,
}

/// `collab-protocol-v1.md`'s `open document_id, known_seq, known_frontier` → reconnect resume.
///
/// A client that reconnects already holding the document up to `known_seq` does not need the
/// snapshot back; it needs the accepted stream it missed. This plans exactly that: the frames
/// `(known_seq, head_seq]` would have produced had the connection never dropped, in strict seq
/// order, so the resumed subscription rejoins the live stream at `head_seq + 1` with the same
/// [`EgressSequencer`] invariant a fresh `snapshot` establishes.
///
/// Deliberately reuses [`bootstrap::fetch_update_range`] — the same persisted-receipt read the
/// outbound gap backfill uses — rather than re-applying CRDT bytes, and refuses (falling back to a
/// full bootstrap) on any doubt: `collab-protocol-v1.md` forbids guessing across a seq gap, and a
/// full `snapshot` is always a correct answer to `open`.
async fn plan_resume(
    db: &sea_orm::DatabaseConnection,
    document_id: Uuid,
    boot: &bootstrap::BootstrapResult,
    known_seq: Option<i64>,
    known_frontier: Option<&str>,
) -> Result<Vec<Frame>, ResumeRefusal> {
    let (Some(known_seq), Some(known_frontier)) = (known_seq, known_frontier) else {
        return Err(ResumeRefusal::NotRequested);
    };
    // `known_seq >= boot.snapshot_seq` is also what bounds the size of the replay, and that is a
    // load-bearing dependency on `bootstrap::load` rather than a check this function performs:
    //
    //   * this bound makes the replay range `(known_seq, head_seq]` a **subset** of the tail
    //     interval `(snapshot_seq, head_seq]` that `boot` already carries;
    //   * `bootstrap::load` has already enforced `BOOTSTRAP_DECODED_BYTES_MAX` (8 MiB) and
    //     `BOOTSTRAP_RESPONSE_BYTES_MAX` over exactly that tail interval, before this function is
    //     ever called (`bootstrap.rs`: the `(snapshot_seq,head_seq]` read, then
    //     `check_bootstrap_decoded_bytes` / `check_bootstrap_response_bytes`);
    //   * the replay's own re-read below is upper-bounded by `boot.head_seq`, not by the live
    //     head, so commits landing after the bootstrap cannot enlarge it either.
    //
    // So an unbounded replay is structurally impossible: a client cannot pick a `known_seq` whose
    // replay is larger than a bootstrap this server would already have refused to build. There is
    // deliberately no second byte ceiling here — a duplicated one could drift from the bootstrap
    // ceiling and would then either reject resumes the loader accepts or, worse, accept replays it
    // would not.
    //
    // ⚠️ If `bootstrap::load` ever stops enforcing those two ceilings over the whole
    // `(snapshot_seq, head_seq]` interval — or if this range check is ever loosened below
    // `snapshot_seq` — resume becomes an unbounded-response amplifier and needs its own ceiling.
    if known_seq < boot.snapshot_seq || known_seq > boot.head_seq {
        return Err(ResumeRefusal::OutOfRange);
    }

    // The `ack` that confirms the resume point back to the client. Sent whether or not there is
    // anything to replay, so "resumed, you are already at head" is distinguishable from a stalled
    // server -- a resumed `open` otherwise produces no response at all when `known_seq == head_seq`.
    let confirmation = Frame::Ack {
        protocol_version: PROTOCOL_VERSION,
        document_id,
        seq: known_seq,
        frontier: known_frontier.to_string(),
    };

    if known_seq == boot.head_seq {
        if known_frontier != BASE64.encode(&boot.head_frontier) {
            return Err(ResumeRefusal::FrontierMismatch);
        }
        return Ok(vec![confirmation]);
    }

    let expected_count = boot.head_seq - known_seq;
    let rows = match bootstrap::fetch_update_range(db, document_id, known_seq + 1, boot.head_seq).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, %document_id, known_seq, "collab session: resume backfill query failed");
            return Err(ResumeRefusal::HistoryUnavailable);
        }
    };
    if !i64::try_from(rows.len()).is_ok_and(|count| count == expected_count) {
        return Err(ResumeRefusal::HistoryUnavailable);
    }
    // The client's claimed frontier must be the one the first replayed update was authored on
    // top of, and the replay must land exactly on the head this bootstrap observed. Checking both
    // ends is what makes the replay a continuation of *this* client's state rather than a stream
    // of bytes it cannot apply.
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Err(ResumeRefusal::HistoryUnavailable);
    };
    if BASE64.encode(&first.before_frontier) != known_frontier {
        return Err(ResumeRefusal::FrontierMismatch);
    }
    if last.after_frontier != boot.head_frontier {
        return Err(ResumeRefusal::HistoryUnavailable);
    }

    let mut frames = Vec::with_capacity(rows.len().saturating_mul(2) + 1);
    frames.push(confirmation);
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
    Ok(frames)
}

async fn reverify_open(state: &AppState, consumed: &ConsumedTicket, socket: &mut WebSocket) -> Option<DocumentContext> {
    let document_id = consumed.document_id;
    let Some(object_id) = fetch_document_object_id(&state.db, document_id).await else {
        reject_and_close(socket, document_id, RejectedCode::NotFound, "document not found").await;
        return None;
    };
    if let Some(signal) = runtime::runtime().workspace_drain_signal(consumed.workspace_id) {
        reject_drain_and_close(socket, document_id, signal).await;
        return None;
    }
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
    // `ADR-0012` §3.1: `checked_epoch` is "the epoch permission was computed against", and every
    // update this connection later commits is fenced against it (`write::run_locked_phase` ->
    // `authz::fence_epoch_for_share`). It therefore has to be read no later than the permission
    // read it fences. Reading it afterwards -- as this did -- meant a revocation committing
    // between the two reads was folded into `checked_epoch` itself, so the commit-time fence
    // compared the post-revocation epoch against itself and every subsequent write on this
    // connection sailed through on permission that had already been taken away. The fence only
    // compares epochs, so the order of these two reads is the entire barrier.
    let Ok(checked_epoch) = authz::read_epoch(&state.db, consumed.workspace_id).await else {
        reject_and_close(socket, document_id, RejectedCode::Forbidden, "epoch read failed").await;
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
    sequencer: &mut EgressSequencer,
    pending_updates: &mut HashMap<Uuid, Frame>,
    open_attempts: &mut u64,
) {
    // `open_documents_per_connection_max`'s structural guarantee (see [`is_reopen_attempt`]): a
    // client sending `open` again after the handshake must never be treated as opening a second
    // document. This does not change behavior from before this function was refactored to name it
    // -- `Frame::Open` landed in the same `invalid_update`-and-stay-open catch-all below either
    // way -- it only isolates the one decision that makes the ceiling structural into something
    // independently unit-testable.
    if is_reopen_attempt(&frame) {
        *open_attempts = open_attempts.saturating_add(1);
        if *open_attempts > OPEN_DOCUMENTS_PER_CONNECTION_MAX {
            send(
                socket,
                &limit_exceeded_frame(
                    document_id,
                    "open_documents",
                    OPEN_DOCUMENTS_PER_CONNECTION_MAX,
                    Some(*open_attempts),
                    None,
                ),
            )
            .await;
            return;
        }
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
                    // Not the client's frame key: see `write::UpdateRequest::event_idempotency_key`
                    // for why the WebSocket surface must not record a free-form, per-document key
                    // in the workspace-scoped `business_events` index. This surface's replay key is
                    // `update_id`, which the protocol already requires a retrying client to reuse.
                    event_idempotency_key: None,
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
                    // call site, still inside the coordinator permit). The committer is excluded
                    // from that registry broadcast, so its receipt enters the same sequencer below
                    // directly; it still never bypasses sequence validation.
                    let frame = Frame::Accepted {
                        protocol_version: PROTOCOL_VERSION,
                        document_id,
                        update_id: accepted.update_id,
                        head_seq: accepted.head_seq,
                        head_frontier: BASE64.encode(&accepted.head_frontier),
                        projection_seq: accepted.projection_seq,
                        event_id: accepted.event_id,
                    };
                    // No accepted path may bypass the per-subscription sequencer. A peer commit
                    // can land while this write is waiting for the document coordinator/row lock;
                    // routing the submitter's own receipt through the same path preserves strict
                    // seq order and backfills that peer commit before acknowledging this one.
                    handle_outbound_frame(&state.db, document_id, socket, sequencer, pending_updates, frame).await;
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
                            // Never re-derived here: only the write path knows how far this
                            // update got before it was refused.
                            write_state: rejected.write_state,
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
                            // `write::accept_update` only returns `Err` from the phases that run
                            // *before* its locked phase opens a transaction (the dedup lookup,
                            // the tail-stats read, hydrate/apply); every failure the locked phase
                            // itself can observe is folded into an `Ok(Rejected)` carrying its own
                            // `write_state`. So an `Err` here provably wrote nothing.
                            write_state: WriteState::NotApplied,
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
                Err(limit) => {
                    send(socket, &presence_limit_frame(document_id, limit)).await;
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
        Frame::Ack {
            document_id: frame_document_id,
            seq,
            frontier,
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
            // The highest seq this subscription has actually been sent. `EgressSequencer` is
            // seeded at `snapshot.head_seq + 1` (or the resumed head + 1) and advances only when a
            // frame is forwarded, so `next_expected_seq - 1` is exactly "everything this client
            // could legitimately have applied". Acknowledging past it is a protocol violation, not
            // a race: no path forwards an `accepted` without advancing the sequencer first.
            let highest_forwarded = sequencer.next_expected_seq().saturating_sub(1);
            if seq < 0 || seq > highest_forwarded {
                send(
                    socket,
                    &rejected_frame(document_id, RejectedCode::InvalidUpdate, false, None),
                )
                .await;
                return;
            }
            // A repeat at or below the recorded position is ignored rather than rejected
            // (`collab-protocol-v1.md`: "`seq<=last_applied_seq` 是幂等重复,忽略但可重发 ack").
            collab.registry.record_ack(session_id, seq, frontier);
        }
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{Frame, RateLimiter, bootstrap, is_reopen_attempt};
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::limits;

    use super::{
        FRAME_BURST_MAX, FRAMES_PER_CONNECTION_PER_SECOND, UPDATE_BURST_MAX, UPDATES_PER_CONNECTION_PER_SECOND,
    };

    /// The three `limits-v1.md` connection ceilings, each driven to refusal by the *real*
    /// [`SessionRegistry::try_register`] admission path and then rendered through the *same*
    /// [`connection_limit_frame`] the session loop uses, so the `limit_kind`/`limit` a refused
    /// client actually reads is what this pins.
    ///
    /// Why the registry rather than 100/500 real sockets: `connections_per_document_max` (100) and
    /// `connections_per_workspace_max` (500) are counted in the process-wide registry, and the
    /// only way a WebSocket client reaches either is by holding that many sockets open at once —
    /// a fixture whose cost is entirely in the transport, while the decision under test (which
    /// ceiling was hit, and what it serializes to) is not in the transport at all.
    /// `user_connections` *is* additionally proven over 17 real sockets end to end
    /// (`live_ws::user_connections_ceiling_refuses_the_seventeenth_session_with_the_frozen_limit_kind`),
    /// which is what shows this rendering really is the one that reaches the wire.
    #[test]
    fn every_connection_ceiling_renders_its_frozen_limit_kind_and_limit_into_the_rejection_frame() {
        use crate::flow::collab::registry::{ConnectionLimit, SessionRegistry};

        let document_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();

        // `user_connections`: one user, one document -- the per-user check runs first.
        let per_user = SessionRegistry::new();
        let user_id = Uuid::new_v4();
        for _ in 0..limits::CONNECTIONS_PER_USER_MAX {
            per_user
                .try_register(document_id, user_id, workspace_id, Uuid::new_v4())
                .map_err(ConnectionLimit::limit_kind)
                .expect("registrations within the per-user ceiling are admitted");
        }
        let Err(user_limit) = per_user.try_register(document_id, user_id, workspace_id, Uuid::new_v4()) else {
            panic!("one past connections_per_user_max must be refused");
        };

        // `document_connections`: distinct users so the per-user ceiling is never the one hit.
        let per_document = SessionRegistry::new();
        for _ in 0..limits::CONNECTIONS_PER_DOCUMENT_MAX {
            per_document
                .try_register(document_id, Uuid::new_v4(), workspace_id, Uuid::new_v4())
                .map_err(ConnectionLimit::limit_kind)
                .expect("registrations within the per-document ceiling are admitted");
        }
        let Err(document_limit) = per_document.try_register(document_id, Uuid::new_v4(), workspace_id, Uuid::new_v4())
        else {
            panic!("one past connections_per_document_max must be refused");
        };

        // `workspace_connections`: distinct users *and* enough distinct documents that neither the
        // per-user nor the per-document ceiling is reached first.
        let per_workspace = SessionRegistry::new();
        for _ in 0..limits::CONNECTIONS_PER_WORKSPACE_MAX {
            per_workspace
                .try_register(Uuid::new_v4(), Uuid::new_v4(), workspace_id, Uuid::new_v4())
                .map_err(ConnectionLimit::limit_kind)
                .expect("registrations within the per-workspace ceiling are admitted");
        }
        let Err(workspace_limit) =
            per_workspace.try_register(Uuid::new_v4(), Uuid::new_v4(), workspace_id, Uuid::new_v4())
        else {
            panic!("one past connections_per_workspace_max must be refused");
        };

        let cases = [
            (user_limit, "user_connections", limits::CONNECTIONS_PER_USER_MAX),
            (
                document_limit,
                "document_connections",
                limits::CONNECTIONS_PER_DOCUMENT_MAX,
            ),
            (
                workspace_limit,
                "workspace_connections",
                limits::CONNECTIONS_PER_WORKSPACE_MAX,
            ),
        ];
        for (limit, expected_kind, expected_limit) in cases {
            let frame = super::connection_limit_frame(document_id, limit);
            let Frame::Rejected { code, details, .. } = frame else {
                panic!("{expected_kind} must render as a rejected frame");
            };
            assert_eq!(code, super::RejectedCode::LimitExceeded);
            let details = details.unwrap_or_else(|| panic!("{expected_kind} must carry details"));
            assert_eq!(details["limit_kind"], expected_kind);
            assert_eq!(details["limit"], expected_limit);
            assert_eq!(details["retry_after_ms"], limits::CONNECTION_LIMIT_RETRY_AFTER_MS);
            assert!(
                details.get("observed").is_none(),
                "{expected_kind}: `observed` would report other users' connections back to the caller, \
                 which limits-v1.md's details rule forbids"
            );
        }
    }

    /// The two presence ceilings, driven to refusal by the real
    /// [`SessionRegistry::upsert_presence`] accounting and rendered through the same
    /// [`presence_limit_frame`] `handle_client_frame` uses.
    ///
    /// Neither is reachable from a single v0.4 WebSocket connection: an entry is keyed
    /// `(document_id, session_id)` with `session_id` taken from the *connection*, never from the
    /// frame, so one connection owns exactly one entry — `presence_entries_per_connection` (8)
    /// would need a multi-document connection (which `frame.rs` documents as post-v0.4) and
    /// `presence_entries_per_document` (100) needs 100 simultaneous sockets. The enforcement and
    /// its wire rendering still have to be right, which is what this covers; the reachability
    /// caveat is recorded rather than papered over.
    #[test]
    fn both_presence_ceilings_render_their_frozen_limit_kind_and_limit_into_the_rejection_frame() {
        use crate::flow::collab::registry::SessionRegistry;

        let document_id = Uuid::new_v4();
        let payload = serde_json::json!({"cursor": 1});
        let ttl = Duration::from_secs(30);

        // Per connection: one session, distinct documents.
        let per_connection = SessionRegistry::new();
        let session_id = Uuid::new_v4();
        for _ in 0..limits::PRESENCE_ENTRIES_PER_CONNECTION_MAX {
            per_connection
                .upsert_presence(Uuid::new_v4(), session_id, payload.clone(), ttl)
                .expect("entries within the per-connection ceiling are accepted");
        }
        let connection_limit = per_connection
            .upsert_presence(Uuid::new_v4(), session_id, payload.clone(), ttl)
            .expect_err("one past presence_entries_per_connection_max is refused");

        // Per document: one document, distinct sessions.
        let per_document = SessionRegistry::new();
        for _ in 0..limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX {
            per_document
                .upsert_presence(document_id, Uuid::new_v4(), payload.clone(), ttl)
                .expect("entries within the per-document ceiling are accepted");
        }
        let document_limit = per_document
            .upsert_presence(document_id, Uuid::new_v4(), payload, ttl)
            .expect_err("one past presence_entries_per_document_max is refused");

        let cases = [
            (
                connection_limit,
                "presence_entries_per_connection",
                limits::PRESENCE_ENTRIES_PER_CONNECTION_MAX as u64,
            ),
            (
                document_limit,
                "presence_entries_per_document",
                limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX as u64,
            ),
        ];
        for (limit, expected_kind, expected_limit) in cases {
            let frame = super::presence_limit_frame(document_id, limit);
            let Frame::Rejected { code, details, .. } = frame else {
                panic!("{expected_kind} must render as a rejected frame");
            };
            assert_eq!(code, super::RejectedCode::LimitExceeded);
            let details = details.unwrap_or_else(|| panic!("{expected_kind} must carry details"));
            assert_eq!(details["limit_kind"], expected_kind);
            assert_eq!(details["limit"], expected_limit);
        }
    }

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

    /// The server heartbeat's decision core, on a virtual clock (the same technique the
    /// `RateLimiter` cases above use): after a full idle period the server pings, and a peer that
    /// answers nothing at all for one further period is hung up on. `HEARTBEAT_IDLE` is 30
    /// seconds, so exercising this against the real clock would cost a minute per assertion.
    #[test]
    fn heartbeat_pings_after_one_idle_period_and_closes_when_the_ping_goes_unanswered() {
        let t0 = Instant::now();
        let idle = super::HEARTBEAT_IDLE;
        let mut heartbeat = super::Heartbeat::new(idle, t0);

        assert_eq!(
            heartbeat.evaluate_at((t0 + idle).checked_sub(Duration::from_millis(1)).expect("in range")),
            super::HeartbeatAction::Idle,
            "one millisecond short of the idle period must not ping"
        );
        assert_eq!(
            heartbeat.evaluate_at(t0 + idle),
            super::HeartbeatAction::SendPing,
            "a full idle period with no inbound frame must produce exactly one ping"
        );
        assert_eq!(
            heartbeat.evaluate_at(t0 + idle + Duration::from_secs(1)),
            super::HeartbeatAction::Idle,
            "the outstanding ping must not be re-sent while it is still within its deadline"
        );
        assert_eq!(
            heartbeat.evaluate_at(
                (t0 + idle + idle)
                    .checked_sub(Duration::from_millis(1))
                    .expect("in range")
            ),
            super::HeartbeatAction::Idle
        );
        assert_eq!(
            heartbeat.evaluate_at(t0 + idle + idle),
            super::HeartbeatAction::Close,
            "a ping outstanding for a full further idle period means the peer is gone"
        );
    }

    /// Deliberately keyed on *any* inbound frame, not only `pong`: an actively editing client that
    /// never implements `ping` handling is provably alive and must not be hung up on.
    #[test]
    fn any_inbound_frame_clears_an_outstanding_heartbeat_ping() {
        let t0 = Instant::now();
        let idle = super::HEARTBEAT_IDLE;
        let mut heartbeat = super::Heartbeat::new(idle, t0);

        assert_eq!(heartbeat.evaluate_at(t0 + idle), super::HeartbeatAction::SendPing);
        heartbeat.record_inbound(t0 + idle + Duration::from_secs(1));
        assert_eq!(
            heartbeat.evaluate_at(t0 + idle + idle),
            super::HeartbeatAction::Idle,
            "the deadline must be cleared by the inbound frame, not merely postponed"
        );
        assert_eq!(
            heartbeat.evaluate_at(t0 + idle + idle + Duration::from_secs(1)),
            super::HeartbeatAction::SendPing,
            "and a fresh idle period after that inbound frame pings again"
        );
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

        #[tokio::test]
        async fn draining_workspace_handshake_emits_structured_rejection_before_4410_close() {
            let scratch = scratch_or_skip!("drain-handshake");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state).await;
            let client_id = "drain-handshake-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let guard = crate::flow::collab::runtime::runtime().begin_workspace_drain(workspace_id, 1_750);

            let mut ws = connect(addr, &ticket, client_id).await;
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
            assert!(matches!(recv_frame(&mut ws).await, Frame::Hello { .. }));
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

            let Frame::Rejected {
                code,
                recoverable,
                details,
                ..
            } = recv_frame(&mut ws).await
            else {
                panic!("expected a structured drain rejection");
            };
            assert_eq!(code, RejectedCode::ServerDraining);
            assert!(recoverable);
            let details = details.expect("drain details");
            assert_eq!(details["reason"], "drain");
            assert_eq!(details["retry_after_ms"], 1_750);

            let close_message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("close arrives")
                .expect("stream carries close")
                .expect("close is not a transport error");
            let TMessage::Close(Some(close)) = close_message else {
                panic!("expected a close frame, got {close_message:?}");
            };
            assert_eq!(u16::from(close.code), 4410);
            let close_reason: serde_json::Value = serde_json::from_str(&close.reason).expect("close reason JSON");
            assert_eq!(close_reason["reason"], "drain");
            assert_eq!(close_reason["retry_after_ms"], 1_750);

            drop(guard);
            scratch.drop_self().await;
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

        /// `open_documents` (`open_documents_per_connection_max` = 8), on the wire.
        ///
        /// The reopen test above proves the *structural* guarantee (a second `open` never opens a
        /// second document); this proves the counted ceiling `handle_client_frame` maintains on
        /// top of it, which is the only thing that makes the `open_documents` `limit_kind`
        /// observable at all. The handshake itself is attempt 1, so reopens 2..=8 are each refused
        /// `invalid_update` and the 8th reopen (attempt 9, the first past the ceiling) is refused
        /// `limit_exceeded` carrying the frozen `limit_kind`/`limit`/`observed`.
        #[tokio::test]
        async fn open_documents_ceiling_is_observable_as_limit_exceeded_on_the_ninth_attempt() {
            let scratch = scratch_or_skip!("open-documents-kind");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let client_id = "open-documents-kind-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, head_seq_before) = open_session(addr, &ticket, client_id, document_id).await;
            let dispatch_before = count_event_dispatch(&state, document_id).await;

            const OPEN_DOCUMENTS_PER_CONNECTION_MAX: u64 = 8;
            let reopen = Frame::Open {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                known_seq: None,
                known_frontier: None,
            };

            // Attempts 2..=OPEN_DOCUMENTS_PER_CONNECTION_MAX: still within the ceiling.
            for attempt in 2..=OPEN_DOCUMENTS_PER_CONNECTION_MAX {
                send_frame(&mut ws, &reopen).await;
                let rejected = recv_frame(&mut ws).await;
                let Frame::Rejected { code, .. } = rejected else {
                    panic!("attempt {attempt} must be rejected, got {rejected:?}");
                };
                assert_eq!(
                    code,
                    RejectedCode::InvalidUpdate,
                    "attempt {attempt} is within the ceiling and must stay invalid_update"
                );
            }

            // Attempt OPEN_DOCUMENTS_PER_CONNECTION_MAX + 1: the first past the ceiling.
            send_frame(&mut ws, &reopen).await;
            let rejected = recv_frame(&mut ws).await;
            let Frame::Rejected { code, details, .. } = rejected else {
                panic!("the ninth attempt must be rejected, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::LimitExceeded);
            let details = details.expect("a limit_exceeded rejection must carry details");
            assert_eq!(details["limit_kind"], "open_documents");
            assert_eq!(details["limit"], OPEN_DOCUMENTS_PER_CONNECTION_MAX);
            assert_eq!(details["observed"], OPEN_DOCUMENTS_PER_CONNECTION_MAX + 1);

            assert_eq!(read_head_seq(&state, document_id).await, head_seq_before);
            assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_before);

            scratch.drop_self().await;
        }

        /// `frame_rate` (`frames_per_connection_per_second` = 30, burst 60), on the wire.
        ///
        /// The bucket starts full (`RateLimiter::new`), so the rejection cannot land before frame
        /// 61; local round trips are far faster than the 30 tokens/second refill, so it also
        /// cannot be pushed out indefinitely. Both bounds are asserted rather than assuming an
        /// exact frame number, which would depend on how much wall time the round trips consume.
        /// A single exceeded window never closes the connection (the close needs 3 consecutive
        /// exceeded 1-second windows), and a `ping`/`pong` after the rejection proves it.
        #[tokio::test]
        async fn frame_rate_ceiling_is_observable_as_limit_exceeded_without_closing_the_connection() {
            let scratch = scratch_or_skip!("frame-rate-kind");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let client_id = "frame-rate-kind-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, head_seq_before) = open_session(addr, &ticket, client_id, document_id).await;
            let dispatch_before = count_event_dispatch(&state, document_id).await;

            const FRAMES_PER_CONNECTION_PER_SECOND: u64 = 30;
            const FRAME_BURST_MAX: u64 = 60;
            // Enough headroom that even a slow machine refilling tokens the whole time still runs
            // out, without being unbounded.
            const MAX_PINGS: u64 = 400;

            let mut admitted = 0u64;
            let mut rejection_details = None;
            for n in 0..MAX_PINGS {
                send_frame(
                    &mut ws,
                    &Frame::Ping {
                        protocol_version: PROTOCOL_VERSION,
                        nonce: format!("flood-{n}"),
                    },
                )
                .await;
                match recv_frame(&mut ws).await {
                    Frame::Pong { .. } => admitted += 1,
                    Frame::Rejected { code, details, .. } => {
                        assert_eq!(code, RejectedCode::LimitExceeded);
                        rejection_details = Some(details.expect("a limit_exceeded rejection must carry details"));
                        break;
                    }
                    other => panic!("unexpected frame while flooding: {other:?}"),
                }
            }

            let details = rejection_details.expect("flooding well past the burst must produce a frame_rate rejection");
            assert_eq!(details["limit_kind"], "frame_rate");
            assert_eq!(details["limit"], FRAMES_PER_CONNECTION_PER_SECOND);
            assert_eq!(
                details["retry_after_ms"], 1_000,
                "a rate rejection must tell the caller when to retry (`limits-v1.md`: rate/connection/queue 可按 retry_after_ms 重试)"
            );
            assert!(
                admitted >= FRAME_BURST_MAX,
                "the burst capacity must be honored before the first rejection: only {admitted} frames were admitted"
            );

            // A single exceeded window must not close the connection.
            send_frame(
                &mut ws,
                &Frame::Ping {
                    protocol_version: PROTOCOL_VERSION,
                    nonce: "still-open".to_string(),
                },
            )
            .await;
            let after = recv_frame(&mut ws).await;
            assert!(
                matches!(after, Frame::Pong { .. } | Frame::Rejected { .. }),
                "the connection must stay open after one exceeded rate window, got {after:?}"
            );

            assert_eq!(read_head_seq(&state, document_id).await, head_seq_before);
            assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_before);

            scratch.drop_self().await;
        }

        /// `update_rate` (`updates_per_connection_per_second` = 10, burst 20), on the wire.
        ///
        /// The update frames deliberately name a *different* `document_id` than the one this
        /// connection is scoped to: `handle_client_frame`'s `Frame::Update` arm rejects that
        /// `invalid_update` before any decode, hydrate, or write happens, so this test exercises
        /// the update-rate bucket (which `run`'s read loop charges *before* dispatching the frame
        /// at all) without submitting real CRDT work. The distinct rejection codes are what tell
        /// the two apart: `invalid_update` while tokens remain, `limit_exceeded`/`update_rate`
        /// once they are gone.
        #[tokio::test]
        async fn update_rate_ceiling_is_observable_as_limit_exceeded_separately_from_the_frame_rate() {
            let scratch = scratch_or_skip!("update-rate-kind");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let client_id = "update-rate-kind-client";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, head_seq_before) = open_session(addr, &ticket, client_id, document_id).await;
            let dispatch_before = count_event_dispatch(&state, document_id).await;

            const UPDATES_PER_CONNECTION_PER_SECOND: u64 = 10;
            const UPDATE_BURST_MAX: u64 = 20;
            // Must stay under the 60-frame burst so this can only ever trip `update_rate`.
            const MAX_UPDATES: u64 = 55;

            let mut admitted = 0u64;
            let mut rejection_details = None;
            for n in 0..MAX_UPDATES {
                send_frame(
                    &mut ws,
                    &Frame::Update {
                        protocol_version: PROTOCOL_VERSION,
                        document_id: Uuid::new_v4(),
                        update_id: Uuid::new_v4(),
                        base_frontier: String::new(),
                        bytes: String::new(),
                        idempotency_key: None,
                        origin: format!("update-flood-{n}"),
                        message: None,
                    },
                )
                .await;
                let Frame::Rejected { code, details, .. } = recv_frame(&mut ws).await else {
                    panic!("every frame in this flood must be rejected");
                };
                match code {
                    RejectedCode::InvalidUpdate => admitted += 1,
                    RejectedCode::LimitExceeded => {
                        rejection_details = Some(details.expect("a limit_exceeded rejection must carry details"));
                        break;
                    }
                    other => panic!("unexpected rejection code while flooding updates: {other:?}"),
                }
            }

            let details =
                rejection_details.expect("flooding well past the update burst must produce an update_rate rejection");
            assert_eq!(details["limit_kind"], "update_rate");
            assert_eq!(details["limit"], UPDATES_PER_CONNECTION_PER_SECOND);
            assert_eq!(details["retry_after_ms"], 1_000);
            assert!(
                admitted >= UPDATE_BURST_MAX,
                "the update burst capacity must be honored before the first rejection: only {admitted} were admitted"
            );

            assert_eq!(read_head_seq(&state, document_id).await, head_seq_before);
            assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_before);

            scratch.drop_self().await;
        }

        /// `user_connections` (`connections_per_user_max` = 16), on the wire and end to end.
        ///
        /// Sixteen real, simultaneously-open WebSocket sessions for one freshly-created user
        /// exhaust that user's slot budget in the process-wide session registry; the seventeenth
        /// handshake must be refused with the structured `limit_exceeded` frame and then closed at
        /// 4408. A fresh `user_id`/`workspace_id`/`document_id` per run is what keeps this from
        /// interacting with any other test sharing that registry -- the per-user ceiling is
        /// counted per `user_id`, and 17 sessions stay far below the 100-per-document and
        /// 500-per-workspace ceilings this same registration path also checks.
        #[tokio::test]
        async fn user_connections_ceiling_refuses_the_seventeenth_session_with_the_frozen_limit_kind() {
            let scratch = scratch_or_skip!("user-connections-kind");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            const CONNECTIONS_PER_USER_MAX: u64 = 16;

            // Held open for the whole test: dropping any of them would free a slot.
            let mut open_sessions = Vec::new();
            for n in 0..CONNECTIONS_PER_USER_MAX {
                let client_id = format!("user-connections-{n}");
                let ticket = issue_ticket(addr, &token, workspace_id, document_id, &client_id).await;
                let (ws, _) = open_session(addr, &ticket, &client_id, document_id).await;
                open_sessions.push(ws);
            }

            // The seventeenth completes the same `hello`/`open`/`snapshot` handshake: `run`
            // reserves the connection slot only after the snapshot has been sent (the ceiling
            // bounds *steady-state* sessions), so the refusal is the frame right after it.
            let client_id = "user-connections-overflow";
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
            let (mut ws, _) = open_session(addr, &ticket, client_id, document_id).await;

            let rejected = recv_frame(&mut ws).await;
            let Frame::Rejected { code, details, .. } = rejected else {
                panic!("the seventeenth session must be refused, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::LimitExceeded);
            let details = details.expect("a limit_exceeded rejection must carry details");
            assert_eq!(details["limit_kind"], "user_connections");
            assert_eq!(details["limit"], CONNECTIONS_PER_USER_MAX);
            assert_eq!(details["retry_after_ms"], 5_000);
            assert!(
                details.get("observed").is_none(),
                "`observed` must stay absent: limits-v1.md forbids reporting other connections back to the caller"
            );

            let close_message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("close arrives")
                .expect("stream carries close")
                .expect("close is not a transport error");
            let TMessage::Close(Some(close)) = close_message else {
                panic!("expected a close frame, got {close_message:?}");
            };
            assert_eq!(
                u16::from(close.code),
                4408,
                "a refused connection ceiling closes at the frozen limit_exceeded close code"
            );

            drop(open_sessions);
            scratch.drop_self().await;
        }

        // -----------------------------------------------------------------------------------
        // WP-16 (v0.5 session extension) helpers: presence fan-out, outbound-order injection,
        // reconnect resume, ack frontier, and the 10-client round-trip budget.
        // -----------------------------------------------------------------------------------

        /// Drives `hello` + `open` and stops there, handing back the stream and the
        /// server-assigned `session_id` (from the `hello` reply) without consuming whatever the
        /// server answers `open` with -- a resuming `open` answers with `ack` + replay, a fresh
        /// one with `snapshot`, and several tests below assert exactly which.
        async fn handshake(
            addr: SocketAddr,
            ticket: &str,
            client_id: &str,
            document_id: Uuid,
            known_seq: Option<i64>,
            known_frontier: Option<String>,
        ) -> (WsStream, Uuid) {
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
            let Frame::Hello { session_id, .. } = hello_reply else {
                panic!("expected a hello reply, got {hello_reply:?}");
            };
            send_frame(
                &mut ws,
                &Frame::Open {
                    protocol_version: PROTOCOL_VERSION,
                    document_id,
                    known_seq,
                    known_frontier,
                },
            )
            .await;
            (ws, session_id)
        }

        /// [`open_session`] plus the server-assigned `session_id` and the snapshot's
        /// `head_frontier` -- everything a test needs to address this subscription in the
        /// process-global registry and to reconnect against it later.
        async fn open_session_full(
            addr: SocketAddr,
            ticket: &str,
            client_id: &str,
            document_id: Uuid,
        ) -> (WsStream, i64, String, Uuid) {
            let (mut ws, session_id) = handshake(addr, ticket, client_id, document_id, None, None).await;
            let snapshot_frame = recv_frame(&mut ws).await;
            let Frame::Snapshot {
                head_seq,
                head_frontier,
                ..
            } = snapshot_frame
            else {
                panic!("expected a snapshot frame, got {snapshot_frame:?}");
            };
            (ws, head_seq, head_frontier, session_id)
        }

        /// A `ping`/`pong` round trip: the server's steady-state loop handles inbound frames
        /// strictly in arrival order, so a `pong` coming back proves every frame sent before the
        /// `ping` has already been fully processed. Used instead of sleeping, so the presence
        /// assertions below are deterministic rather than timing-dependent.
        async fn sync(ws: &mut WsStream) {
            let nonce = Uuid::new_v4().to_string();
            send_frame(
                ws,
                &Frame::Ping {
                    protocol_version: PROTOCOL_VERSION,
                    nonce: nonce.clone(),
                },
            )
            .await;
            let reply = recv_frame(ws).await;
            let Frame::Pong { nonce: echoed, .. } = reply else {
                panic!("expected a pong, got {reply:?}");
            };
            assert_eq!(echoed, nonce);
        }

        fn presence_frame(document_id: Uuid, cursor: i64, ttl_seconds: Option<u32>) -> Frame {
            Frame::Presence {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                // Ignored by the server: the entry key is `(document_id, server-assigned
                // session_id)`, never a client-supplied one.
                session_id: Uuid::new_v4(),
                payload: serde_json::json!({"cursor": cursor}),
                ttl_seconds,
            }
        }

        fn live_registry() -> &'static crate::flow::collab::registry::SessionRegistry {
            &crate::flow::collab::runtime::runtime().registry
        }

        async fn count_collab_updates(state: &AppState, document_id: Uuid) -> i64 {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                n: i64,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
                vec![document_id.into()],
            ))
            .one(&state.db)
            .await
            .expect("count query runs")
            .expect("count query returns a row")
            .n
        }

        /// Commits `count` real updates through the production write path but against a
        /// **local** [`crate::flow::collab::registry::SessionRegistry`], so none of them reach the
        /// live sessions this module's tests have open. That is what makes the outbound-order and
        /// resume tests deterministic: the rows exist in `collab_updates` (so backfill/replay can
        /// find them) while the connected client provably has not been told about them yet.
        async fn commit_updates_offline(
            state: &AppState,
            document_id: Uuid,
            workspace_id: Uuid,
            actor_id: Uuid,
            count: usize,
        ) -> Vec<crate::flow::collab::write::Accepted> {
            use collab_core::{CollabEngine, LoroCollabEngine};
            #[derive(sea_orm::FromQueryResult)]
            struct SnapshotRow {
                snapshot: Vec<u8>,
            }
            let snapshot_row = SnapshotRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT snapshot FROM collab_documents WHERE id = $1",
                vec![document_id.into()],
            ))
            .one(&state.db)
            .await
            .expect("query runs")
            .expect("row exists");
            let mut engine = LoroCollabEngine::load(&snapshot_row.snapshot).expect("loads");

            let cache = crate::flow::collab::cache::WarmCache::new();
            let coordinator = crate::flow::collab::coordinator::DocumentCoordinator::new();
            let registry = crate::flow::collab::registry::SessionRegistry::new();
            let advancer = crate::flow::collab::snapshot::SnapshotAdvancer::new();

            let mut committed = Vec::with_capacity(count);
            for i in 0..count {
                let base_frontier = engine.frontier();
                engine
                    .set_title(&format!("wp16-offline-{i}-{}", Uuid::new_v4()))
                    .expect("set_title succeeds");
                let bytes = engine.export_from(&base_frontier).expect("export succeeds");
                let checked_epoch = crate::flow::collab::authz::read_epoch(&state.db, workspace_id)
                    .await
                    .expect("epoch reads");
                let outcome = crate::flow::collab::write::accept_update(
                    &state.db,
                    &cache,
                    &coordinator,
                    &registry,
                    &advancer,
                    10,
                    None,
                    crate::flow::collab::write::UpdateRequest {
                        document_id,
                        update_id: Uuid::new_v4(),
                        bytes,
                        idempotency_key: None,
                        event_idempotency_key: None,
                        origin_client_id: Some("wp16-offline".to_string()),
                        message: None,
                        actor_id,
                        workspace_id,
                        checked_epoch,
                        expected_frontier: None,
                    },
                )
                .await
                .expect("accept_update does not hit a hard database error");
                let crate::flow::collab::write::AcceptOutcome::Accepted(accepted) = outcome else {
                    panic!("expected Accepted for offline commit {i}");
                };
                committed.push(accepted);
            }
            committed
        }

        fn encode_b64(bytes: &[u8]) -> String {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(bytes)
        }

        fn accepted_frame_for(document_id: Uuid, accepted: &crate::flow::collab::write::Accepted) -> Frame {
            Frame::Accepted {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id: accepted.update_id,
                head_seq: accepted.head_seq,
                head_frontier: encode_b64(&accepted.head_frontier),
                projection_seq: accepted.projection_seq,
                event_id: accepted.event_id,
            }
        }

        /// `versions/v0.5-collaboration.md`: "显示其他用户 cursor/selection". A `presence` frame must
        /// reach every *other* session on the document and never echo back to its sender
        /// (`SessionRegistry::broadcast`'s `exclude`), and a session joining afterwards must be
        /// handed the peers already present rather than waiting up to a full
        /// `presence_ttl_seconds_max` for their next refresh.
        #[tokio::test]
        async fn presence_fans_out_to_every_peer_and_never_echoes_to_its_sender() {
            let scratch = scratch_or_skip!("presence-fanout");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket_a = issue_ticket(addr, &token, workspace_id, document_id, "fanout-a").await;
            let (mut a, _, _, a_session) = open_session_full(addr, &ticket_a, "fanout-a", document_id).await;
            let ticket_b = issue_ticket(addr, &token, workspace_id, document_id, "fanout-b").await;
            let (mut b, _, _, _) = open_session_full(addr, &ticket_b, "fanout-b", document_id).await;
            let ticket_c = issue_ticket(addr, &token, workspace_id, document_id, "fanout-c").await;
            let (mut c, _, _, _) = open_session_full(addr, &ticket_c, "fanout-c", document_id).await;

            send_frame(&mut a, &presence_frame(document_id, 7, None)).await;

            for (label, peer) in [("b", &mut b), ("c", &mut c)] {
                let received = recv_frame(peer).await;
                let Frame::Presence {
                    session_id,
                    payload,
                    ttl_seconds,
                    ..
                } = received
                else {
                    panic!("peer {label} must receive the presence frame, got {received:?}");
                };
                assert_eq!(session_id, a_session, "presence must carry the *server* session id");
                assert_eq!(payload["cursor"], 7);
                assert_eq!(
                    ttl_seconds,
                    Some(crate::flow::collab::limits::PRESENCE_TTL_SECONDS_DEFAULT),
                    "an omitted ttl_seconds is filled in with the contract default"
                );
            }

            // The sender must not have been sent its own presence back. Proven without sleeping:
            // the server processes this connection's inbound frames in order, so if a presence
            // echo had been queued for `a` it would necessarily be delivered before the `pong`.
            sync(&mut a).await;

            // A session joining now is handed the presence already on the document.
            let ticket_d = issue_ticket(addr, &token, workspace_id, document_id, "fanout-d").await;
            let (mut d, d_session) = handshake(addr, &ticket_d, "fanout-d", document_id, None, None).await;
            assert!(matches!(recv_frame(&mut d).await, Frame::Snapshot { .. }));
            // A trailing `ping` bounds the wait: the join snapshot is written to the socket before
            // the steady-state loop reads anything, so if it were missing the next frame would be
            // this `pong` -- named by the assertion below instead of surfacing as a timeout.
            send_frame(
                &mut d,
                &Frame::Ping {
                    protocol_version: PROTOCOL_VERSION,
                    nonce: "join-presence".to_string(),
                },
            )
            .await;
            let joined = recv_frame(&mut d).await;
            let Frame::Presence {
                session_id, payload, ..
            } = joined
            else {
                panic!("a joining session must be handed the document's live presence, got {joined:?}");
            };
            assert_eq!(session_id, a_session);
            assert_ne!(session_id, d_session);
            assert_eq!(payload["cursor"], 7);

            scratch.drop_self().await;
        }

        /// `limits-v1.md`: "Entry key 固定为 `(document_id,session_id)`：同 key frame 只刷新 payload/
        /// expiry，不增加计数". Reported forty times from one session, the document must still hold
        /// exactly one entry -- an append-instead-of-upsert would both inflate the count and walk
        /// into `presence_entries_per_document_max`.
        #[tokio::test]
        async fn repeated_presence_from_one_session_refreshes_in_place_without_growing_the_count() {
            let scratch = scratch_or_skip!("presence-inplace");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "presence-inplace").await;
            let (mut ws, _, _, _) = open_session_full(addr, &ticket, "presence-inplace", document_id).await;

            // Forty is deliberately past `presence_entries_per_document_max` (100) / 2 and well
            // past `presence_entries_per_connection_max` (8): an append implementation trips a
            // ceiling long before the fortieth report, so this fails loudly either way.
            for cursor in 0..40i64 {
                send_frame(&mut ws, &presence_frame(document_id, cursor, Some(30))).await;
                sync(&mut ws).await;
                assert_eq!(
                    live_registry().presence_count(document_id),
                    1,
                    "report {cursor} must refresh the one (document_id, session_id) entry in place"
                );
            }

            scratch.drop_self().await;
        }

        /// `versions/v0.5-collaboration.md`: presence "不写 `collab_updates`"; `collab-protocol-v1.md`:
        /// "presence 永不进入 CRDT、history、event 或投递". Presence is not document content, so a
        /// burst of it must leave `collab_updates`, the document head, and `event_dispatch`
        /// byte-for-byte where they were.
        #[tokio::test]
        async fn presence_writes_neither_collab_updates_nor_the_document_head_nor_a_dispatch() {
            let scratch = scratch_or_skip!("presence-no-persist");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "presence-no-persist").await;
            let (mut ws, _, _, _) = open_session_full(addr, &ticket, "presence-no-persist", document_id).await;

            let updates_before = count_collab_updates(&state, document_id).await;
            let head_before = read_head_seq(&state, document_id).await;
            let dispatch_before = count_event_dispatch(&state, document_id).await;

            for cursor in 0..10i64 {
                send_frame(&mut ws, &presence_frame(document_id, cursor, Some(30))).await;
                sync(&mut ws).await;
            }
            assert_eq!(
                live_registry().presence_count(document_id),
                1,
                "the presence really was accepted -- otherwise the counts below prove nothing"
            );

            assert_eq!(
                count_collab_updates(&state, document_id).await,
                updates_before,
                "presence must never write a collab_updates row"
            );
            assert_eq!(
                read_head_seq(&state, document_id).await,
                head_before,
                "presence must never advance the document head"
            );
            assert_eq!(
                count_event_dispatch(&state, document_id).await,
                dispatch_before,
                "presence must never produce an event dispatch"
            );

            scratch.drop_self().await;
        }

        /// `limits-v1.md`: "Expiry 从服务端接受 frame 的单调时钟起算，30 秒无刷新自动删除，连接关闭或权限
        /// 撤销立即删除". Both halves against the live registry: a 1-second TTL that lapses, and a
        /// 30-second TTL whose entry must still vanish the moment the socket goes away.
        #[tokio::test]
        async fn presence_disappears_on_ttl_expiry_and_again_on_disconnect() {
            let scratch = scratch_or_skip!("presence-cleanup");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "presence-cleanup").await;
            let (mut ws, _, _, _) = open_session_full(addr, &ticket, "presence-cleanup", document_id).await;

            // ---- TTL expiry ----
            send_frame(&mut ws, &presence_frame(document_id, 1, Some(1))).await;
            sync(&mut ws).await;
            assert_eq!(live_registry().presence_count(document_id), 1);
            tokio::time::sleep(Duration::from_millis(1_400)).await;
            assert_eq!(
                live_registry().presence_count(document_id),
                0,
                "a 1-second presence TTL must have lapsed after 1.4 seconds"
            );

            // ---- disconnect ----
            send_frame(&mut ws, &presence_frame(document_id, 2, Some(30))).await;
            sync(&mut ws).await;
            assert_eq!(live_registry().presence_count(document_id), 1);
            drop(ws);

            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if live_registry().presence_count(document_id) == 0 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "a 30-second presence entry must be removed by the disconnect, not left to expire"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            scratch.drop_self().await;
        }

        /// `collab-protocol-v1.md` "accepted 出站顺序", injected directly at the boundary the
        /// sequencer guards (`SessionRegistry::broadcast`, the one path every outbound `accepted`
        /// takes): a repeat of an already-forwarded seq is dropped, and a notice that skips a seq
        /// is held back until the missing row has been backfilled from `collab_updates` and sent
        /// first. The commits are made against a *local* registry so the live session provably has
        /// not seen them, which is what lets the injection order be chosen freely.
        #[tokio::test]
        async fn outbound_duplicates_are_dropped_and_a_gap_is_backfilled_in_strict_seq_order() {
            let scratch = scratch_or_skip!("egress-dup-gap");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "egress-dup-gap").await;
            let (mut ws, head_seq, _, _) = open_session_full(addr, &ticket, "egress-dup-gap", document_id).await;

            let committed = commit_updates_offline(&state, document_id, workspace_id, owner_id, 3).await;
            assert_eq!(committed[0].head_seq, head_seq + 1);
            assert_eq!(committed[2].head_seq, head_seq + 3);

            let registry = live_registry();

            // ---- in order ----
            registry.broadcast(document_id, &accepted_frame_for(document_id, &committed[0]), None);
            let first = recv_frame(&mut ws).await;
            let Frame::Accepted { head_seq: got_seq, .. } = first else {
                panic!("expected the in-order accepted, got {first:?}");
            };
            assert_eq!(got_seq, head_seq + 1);

            // ---- duplicate: the identical notice again, then a marker that must arrive next ----
            registry.broadcast(document_id, &accepted_frame_for(document_id, &committed[0]), None);
            let marker = Frame::Presence {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                session_id: Uuid::new_v4(),
                payload: serde_json::json!({"marker": "after-duplicate"}),
                ttl_seconds: Some(30),
            };
            registry.broadcast(document_id, &marker, None);
            let after_duplicate = recv_frame(&mut ws).await;
            let Frame::Presence { payload, .. } = &after_duplicate else {
                panic!(
                    "the duplicate accepted must be dropped; the next frame must be the marker, got {after_duplicate:?}"
                );
            };
            assert_eq!(payload["marker"], "after-duplicate");

            // ---- gap: seq head+3 arrives while head+2 has never been forwarded ----
            registry.broadcast(document_id, &accepted_frame_for(document_id, &committed[2]), None);
            let backfilled_update = recv_frame(&mut ws).await;
            let Frame::Update {
                update_id: backfilled_id,
                ..
            } = backfilled_update
            else {
                panic!("the missing seq must be backfilled as an update first, got {backfilled_update:?}");
            };
            assert_eq!(backfilled_id, committed[1].update_id);
            let backfilled_accepted = recv_frame(&mut ws).await;
            let Frame::Accepted {
                head_seq: filled_seq,
                update_id: filled_id,
                ..
            } = backfilled_accepted
            else {
                panic!("expected the backfilled accepted, got {backfilled_accepted:?}");
            };
            assert_eq!(
                filled_seq,
                head_seq + 2,
                "the gap must be filled before the notice that revealed it"
            );
            assert_eq!(filled_id, committed[1].update_id);
            let revealing = recv_frame(&mut ws).await;
            let Frame::Accepted {
                head_seq: revealing_seq,
                ..
            } = revealing
            else {
                panic!("expected the revealing accepted last, got {revealing:?}");
            };
            assert_eq!(revealing_seq, head_seq + 3);

            scratch.drop_self().await;
        }

        /// `collab-protocol-v1.md`: "补不齐则发送 `resync(reason="outbound_gap")`，禁止先发 `N`". A
        /// notice for a seq that does not exist in `collab_updates` at all can never be backfilled,
        /// so the client must be told to resync and must never be handed the notice itself -- nor
        /// any later one, until it reconnects.
        #[tokio::test]
        async fn an_unfillable_outbound_gap_resyncs_and_never_forwards_the_revealing_notice() {
            let scratch = scratch_or_skip!("egress-unfillable");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "egress-unfillable").await;
            let (mut ws, head_seq, _, _) = open_session_full(addr, &ticket, "egress-unfillable", document_id).await;

            let registry = live_registry();
            let phantom = |seq: i64| Frame::Accepted {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id: Uuid::new_v4(),
                head_seq: seq,
                head_frontier: String::new(),
                projection_seq: seq,
                event_id: Uuid::new_v4(),
            };

            registry.broadcast(document_id, &phantom(head_seq + 50), None);
            let resync = recv_frame(&mut ws).await;
            let Frame::Resync {
                reason,
                minimum_snapshot_seq,
                ..
            } = resync
            else {
                panic!("an unfillable gap must produce a resync, got {resync:?}");
            };
            assert_eq!(reason, "outbound_gap");
            assert_eq!(minimum_snapshot_seq, Some(head_seq));

            // Every later notice stays frozen behind that resync; a marker proves the connection
            // is still live and that no accepted slipped through.
            registry.broadcast(document_id, &phantom(head_seq + 51), None);
            let marker = Frame::Presence {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                session_id: Uuid::new_v4(),
                payload: serde_json::json!({"marker": "after-resync"}),
                ttl_seconds: Some(30),
            };
            registry.broadcast(document_id, &marker, None);
            let after = recv_frame(&mut ws).await;
            let Frame::Presence { payload, .. } = &after else {
                panic!("no accepted may cross a pending resync, got {after:?}");
            };
            assert_eq!(payload["marker"], "after-resync");

            scratch.drop_self().await;
        }

        /// `versions/v0.5-collaboration.md`: "reconnect resume". A client that reconnects still
        /// holding the document at a seq/frontier the server can continue from receives the
        /// accepted stream it missed -- confirmed by an `ack` at its own resume point -- instead of
        /// a whole fresh `snapshot`.
        #[tokio::test]
        async fn reconnect_resume_replays_the_missed_accepted_stream_instead_of_a_snapshot() {
            let scratch = scratch_or_skip!("resume-replay");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "resume-a").await;
            let (first, head_seq, head_frontier, _) = open_session_full(addr, &ticket, "resume-a", document_id).await;
            drop(first);

            let committed = commit_updates_offline(&state, document_id, workspace_id, owner_id, 2).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "resume-b").await;
            let (mut ws, _) = handshake(
                addr,
                &ticket,
                "resume-b",
                document_id,
                Some(head_seq),
                Some(head_frontier.clone()),
            )
            .await;

            let confirmation = recv_frame(&mut ws).await;
            let Frame::Ack { seq, frontier, .. } = confirmation else {
                panic!("a resumed open must be confirmed with an ack, got {confirmation:?}");
            };
            assert_eq!(seq, head_seq);
            assert_eq!(frontier, head_frontier);

            for expected in &committed {
                let update = recv_frame(&mut ws).await;
                let Frame::Update { update_id, .. } = update else {
                    panic!("expected a replayed update, got {update:?}");
                };
                assert_eq!(update_id, expected.update_id);
                let accepted = recv_frame(&mut ws).await;
                let Frame::Accepted {
                    head_seq: seq,
                    update_id,
                    ..
                } = accepted
                else {
                    panic!("expected a replayed accepted, got {accepted:?}");
                };
                assert_eq!(seq, expected.head_seq);
                assert_eq!(update_id, expected.update_id);
            }

            // The replay is complete and the subscription is live: nothing else is pending.
            sync(&mut ws).await;

            scratch.drop_self().await;
        }

        /// The other half of resume: a `known_frontier` that is not the one the server recorded at
        /// `known_seq` means the client's local state has diverged, so it must be rebuilt from a
        /// full `snapshot` rather than have a replay applied on top of state it does not hold. The
        /// same must happen for a `known_seq` beyond the canonical head.
        #[tokio::test]
        async fn a_resume_with_a_stale_frontier_or_an_impossible_seq_falls_back_to_a_full_snapshot() {
            let scratch = scratch_or_skip!("resume-refused");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "refused-a").await;
            let (first, head_seq, head_frontier, _) = open_session_full(addr, &ticket, "refused-a", document_id).await;
            drop(first);
            commit_updates_offline(&state, document_id, workspace_id, owner_id, 2).await;

            // ---- stale frontier at a perfectly valid seq ----
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "refused-b").await;
            let (mut ws, _) = handshake(
                addr,
                &ticket,
                "refused-b",
                document_id,
                Some(head_seq),
                Some(encode_b64(b"not-the-recorded-frontier")),
            )
            .await;
            let answer = recv_frame(&mut ws).await;
            assert!(
                matches!(answer, Frame::Snapshot { .. }),
                "a stale known_frontier must fall back to a full snapshot, got {answer:?}"
            );
            drop(ws);

            // ---- a seq no client can legitimately hold ----
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "refused-c").await;
            let (mut ws, _) = handshake(
                addr,
                &ticket,
                "refused-c",
                document_id,
                Some(head_seq + 1_000),
                Some(head_frontier),
            )
            .await;
            let answer = recv_frame(&mut ws).await;
            assert!(
                matches!(answer, Frame::Snapshot { .. }),
                "a known_seq past the canonical head must fall back to a full snapshot, got {answer:?}"
            );

            scratch.drop_self().await;
        }

        /// `collab-protocol-v1.md`'s `ack document_id, seq, frontier`. The server records the
        /// acknowledged position, ignores an idempotent repeat rather than rewinding to it
        /// ("`seq<=last_applied_seq` 是幂等重复，忽略但可重发 ack"), and refuses an `ack` for a seq it
        /// never forwarded to this subscription.
        #[tokio::test]
        async fn ack_records_the_frontier_monotonically_and_refuses_a_seq_never_forwarded() {
            let scratch = scratch_or_skip!("ack-frontier");
            let state = state_for(scratch.db.clone());
            let (workspace_id, owner_id) = seed_workspace(&state).await;
            let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
            let token = jwt_for(owner_id);
            let addr = spawn_server(state.clone()).await;

            // One committed update before connecting, so the snapshot head is provably >= 1 and a
            // *regressing* ack (`head_seq - 1`) is expressible.
            commit_updates_offline(&state, document_id, workspace_id, owner_id, 1).await;
            let ticket = issue_ticket(addr, &token, workspace_id, document_id, "ack-frontier").await;
            let (mut ws, head_seq, head_frontier, session_id) =
                open_session_full(addr, &ticket, "ack-frontier", document_id).await;
            assert!(head_seq >= 1, "the fixture must leave at least one committed seq");

            let ack = |seq: i64, frontier: &str| Frame::Ack {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                seq,
                frontier: frontier.to_string(),
            };

            // ---- a seq this subscription was never sent ----
            // Followed immediately by a `ping`: the server answers inbound frames in order, so if
            // the illegal `ack` were silently accepted the next frame back would be the `pong`,
            // and the assertion below names exactly that.
            send_frame(&mut ws, &ack(head_seq + 1, "ahead")).await;
            send_frame(
                &mut ws,
                &Frame::Ping {
                    protocol_version: PROTOCOL_VERSION,
                    nonce: "ack-guard".to_string(),
                },
            )
            .await;
            let rejected = recv_frame(&mut ws).await;
            let Frame::Rejected { code, .. } = rejected else {
                panic!("an ack past the highest forwarded seq must be rejected, got {rejected:?}");
            };
            assert_eq!(code, RejectedCode::InvalidUpdate);
            let pong = recv_frame(&mut ws).await;
            assert!(
                matches!(pong, Frame::Pong { .. }),
                "expected the trailing pong, got {pong:?}"
            );
            assert!(
                live_registry().acked(session_id).is_none(),
                "a refused ack must never be recorded"
            );

            // ---- the snapshot head itself is ackable ----
            send_frame(&mut ws, &ack(head_seq, &head_frontier)).await;
            sync(&mut ws).await;
            let recorded = live_registry().acked(session_id).expect("the ack is recorded");
            assert_eq!(recorded.seq, head_seq);
            assert_eq!(recorded.frontier, head_frontier);

            // ---- an idempotent repeat behind it is ignored, not applied ----
            send_frame(&mut ws, &ack(head_seq - 1, "stale-replay")).await;
            sync(&mut ws).await;
            let recorded = live_registry().acked(session_id).expect("the ack is still recorded");
            assert_eq!(
                recorded.seq, head_seq,
                "a replayed older ack must never rewind the recorded frontier"
            );
            assert_eq!(recorded.frontier, head_frontier);

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
                    event_idempotency_key: None,
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

        let GapResolution::Resync { frame } = plan else {
            panic!("expected a resync -- only 1 of 5 requested rows exists");
        };
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
