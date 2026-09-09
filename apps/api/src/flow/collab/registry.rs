//! The single-instance session registry: which sessions have which document open, for presence
//! fan-out and for pushing `accepted`/`resync`/forced-close frames to every session watching a
//! document; and, since this package's limits-enforcement pass, the accounting for
//! `limits-v1.md`'s connection ceilings (`connections_per_user_max`/`_per_document_max`/
//! `_per_workspace_max`) and slow-consumer outbound queue ceiling
//! (`slow_consumer_queue_frames_max`/`_bytes_max`).
//!
//! `ADR-0012` §5's v0.5 scope is single-instance: this module maintains both document fan-out and
//! a distinct `object_id → active sessions` reverse index. [`super::revocation`] re-evaluates the
//! workspace's bounded live-session set after grant, inheritance, parent, membership, baseline,
//! and feature changes; it deletes revoked presence independently, removes the session from
//! fan-out, then best-effort sends the frozen close. Cross-instance
//! broadcast and durable revocation logs belong to v0.8/ADR-0016 and are intentionally absent.
//! Correctness never depends on the close path: `authz::fence_epoch_for_share` remains the
//! commit-time barrier.
//!
//! [`SessionRegistry::drain_all`]/[`SessionRegistry::drain_workspace`] carry the same
//! documented drain shape: they are the tested, callable `server_draining{reason:"drain"}`
//! primitive (`collab-protocol-v1.md` "服务端主动排空").
//! [`super::runtime::CollabRuntime::begin_workspace_drain`] is the shared controlled producer used
//! by REST admission, MCP/CLI (through REST), and WebSocket sessions. An instance-shutdown hook is
//! still outside this v0.4 surface.

#![allow(clippy::too_long_first_doc_paragraph, clippy::struct_field_names)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::frame::{DrainSignal, Frame, PROTOCOL_VERSION, RejectedCode, WriteState};
use super::limits::{
    CONNECTIONS_PER_DOCUMENT_MAX, CONNECTIONS_PER_USER_MAX, CONNECTIONS_PER_WORKSPACE_MAX,
    PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX, SLOW_CONSUMER_QUEUE_BYTES_MAX,
    SLOW_CONSUMER_QUEUE_FRAMES_MAX,
};
use crate::error::{ApiErrorKind, ServerDrainingReason};

/// `limits-v1.md`: "...或 slow-consumer queue 达限则关闭连接为 4408". Computed once from
/// [`ApiErrorKind::LimitExceeded`] so it can never drift from `error-mapping-v1.md`'s frozen table
/// (`flow::collab::session` computes the identical value independently for its own rate-limiter
/// close path -- both derive from this one source of truth, so they cannot disagree even though
/// each module owns its own constant).
const SLOW_CONSUMER_CLOSE_CODE: u16 = match ApiErrorKind::LimitExceeded.ws_close_code() {
    Some(code) => code,
    None => 1008,
};

/// `collab-protocol-v1.md`: "实例/workspace 显式排空使用 close 4410". Computed from
/// [`ApiErrorKind::ServerDraining`] with [`ServerDrainingReason::Drain`] -- never `Contention`,
/// which never closes the socket (`flow::collab::session`'s write-error path sends a `rejected`
/// control frame with `details.reason="contention"` instead and keeps the connection open; the two
/// must never be collapsed, `collab-protocol-v1.md`: "两者不得互换").
const DRAIN_CLOSE_CODE: u16 = match ApiErrorKind::ServerDraining(ServerDrainingReason::Drain).ws_close_code() {
    Some(code) => code,
    None => 1008,
};

/// Builds the structured control frame from the same [`DrainSignal`] that supplies the safe close
/// reason, keeping the active-session and handshake wire shapes identical.
fn drain_rejected_frame(document_id: Uuid, signal: DrainSignal) -> Frame {
    Frame::Rejected {
        protocol_version: PROTOCOL_VERSION,
        document_id,
        update_id: None,
        code: RejectedCode::ServerDraining,
        recoverable: true,
        // A drain is an admission decision about the *connection*, not about any one update: it
        // is announced without a write having been attempted at all (`update_id` is `None` for
        // the same reason), so nothing this session submitted can have been applied by it.
        write_state: WriteState::NotApplied,
        details: Some(signal.details()),
        current_seq: None,
        current_frontier: None,
        audit_event_id: None,
    }
}

/// What a session's outbound writer task actually receives; the writer decides how to encode each
/// variant onto the socket. `Frame`'s `usize` is that frame's encoded byte length at send time —
/// charged against [`QueueState`] by [`SessionHandle::deliver`] and released by the session loop's
/// own [`RegisteredSession::record_dequeued`] once it has been taken off this channel.
pub enum OutboundEvent {
    Frame(Box<Frame>, usize),
    /// An unmetered server control frame that must get through even when the data queue is full
    /// (currently the structured `server_draining{drain}` notice immediately before close).
    ControlFrame(Box<Frame>),
    Close {
        code: u16,
        reason: String,
    },
}

/// The slow-consumer queue accounting for one session's outbound channel (`limits-v1.md`'s
/// `slow_consumer_queue_frames_max`/`slow_consumer_queue_bytes_max`) — shared (via `Arc`) between
/// the [`SessionHandle`] every [`SessionRegistry::broadcast`] call reaches through and the
/// [`RegisteredSession`] the owning session loop holds directly, so charging (on send) and
/// releasing (on dequeue) never need to re-take the registry's `sessions` lock.
#[derive(Default)]
struct QueueState {
    frames: AtomicUsize,
    bytes: AtomicUsize,
    /// Set exactly once, the moment this session is force-closed for exceeding either ceiling —
    /// guards against sending more than one `OutboundEvent::Close` and against continuing to
    /// charge (and therefore never un-charge) frames to a session already being torn down.
    close_sent: AtomicBool,
}

#[derive(Clone)]
pub struct SessionHandle {
    pub session_id: Uuid,
    sender: mpsc::UnboundedSender<OutboundEvent>,
    queue: Arc<QueueState>,
}

impl SessionHandle {
    /// Best-effort, unconditional send: a session whose outbound channel is gone (already
    /// disconnecting) is silently skipped rather than treated as an error — the registry does not
    /// own the socket's lifecycle, the session loop does. Not subject to the slow-consumer queue
    /// ceiling (used only for `Close` events and by [`Self::deliver`] once that ceiling is already
    /// exceeded) — a session already being force-closed must still receive its close notice.
    fn send(&self, event: OutboundEvent) {
        let _ = self.sender.send(event);
    }

    /// The slow-consumer-aware path every [`SessionRegistry::broadcast`] recipient goes through:
    /// charges `encoded_len` against this session's [`QueueState`] before sending, and — if either
    /// ceiling would be exceeded — refuses to send the frame at all, sends one `Close` at
    /// [`SLOW_CONSUMER_CLOSE_CODE`] instead, and marks the session so no further frame is ever
    /// queued for it again (`limits-v1.md`: "frames/bytes 任一先到即断开并要求 resume/resync").
    fn deliver(&self, frame: &Frame, encoded_len: usize) {
        if self.queue.close_sent.load(Ordering::Acquire) {
            return;
        }
        let frames = self.queue.frames.fetch_add(1, Ordering::AcqRel) + 1;
        let bytes = self.queue.bytes.fetch_add(encoded_len, Ordering::AcqRel) + encoded_len;
        let observed_frames = u64::try_from(frames).unwrap_or(u64::MAX);
        let observed_bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        let exceeded = if observed_frames > SLOW_CONSUMER_QUEUE_FRAMES_MAX {
            Some(("slow_consumer_queue_frames", SLOW_CONSUMER_QUEUE_FRAMES_MAX))
        } else if observed_bytes > SLOW_CONSUMER_QUEUE_BYTES_MAX {
            Some(("slow_consumer_queue_bytes", SLOW_CONSUMER_QUEUE_BYTES_MAX))
        } else {
            None
        };
        if let Some((limit_kind, limit)) = exceeded {
            // Roll back this frame's own contribution — it is never actually sent — then close.
            self.queue.frames.fetch_sub(1, Ordering::AcqRel);
            self.queue.bytes.fetch_sub(encoded_len, Ordering::AcqRel);
            if !self.queue.close_sent.swap(true, Ordering::AcqRel) {
                self.send(OutboundEvent::Close {
                    code: SLOW_CONSUMER_CLOSE_CODE,
                    reason: format!(r#"{{"code":"limit_exceeded","limit_kind":"{limit_kind}","limit":{limit}}}"#),
                });
            }
            return;
        }
        self.send(OutboundEvent::Frame(Box::new(frame.clone()), encoded_len));
    }
}

pub struct PresenceEntry {
    pub payload: Value,
    expires_at: Instant,
}

/// One subscription's acknowledged position (`collab-protocol-v1.md`'s `ack document_id, seq,
/// frontier`). Recorded per session, never per document: two sessions on the same document
/// acknowledge independently, and `collab-protocol-v1.md` makes the client's `last_applied_seq`
/// the thing an `ack` reports ("`seq<=last_applied_seq` 是幂等重复,忽略但可重发 ack").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckFrontier {
    pub seq: i64,
    /// Base64 of the client's frontier at `seq`, exactly as it arrived on the wire — the server
    /// never decodes or interprets it in v0.5, it only retains the last acknowledged one.
    pub frontier: String,
}

/// Per-session connection metadata (`limits-v1.md`'s `user_connections`/`document_connections`/
/// `workspace_connections` accounting dimensions), keyed by `session_id` so
/// [`SessionRegistry::try_register_authorized`]/[`SessionRegistry::unregister`] can maintain it without a
/// second index.
struct ConnMeta {
    document_id: Uuid,
    object_id: Uuid,
    user_id: Uuid,
    workspace_id: Uuid,
    /// The highest `ack` this session has sent, or `None` until it sends its first one.
    acked: Option<AckFrontier>,
}

#[derive(Default)]
struct ConnectionRegistry {
    by_session: HashMap<Uuid, ConnMeta>,
    /// `object_id -> session_id`. This is deliberately separate from the document index: the two
    /// UUIDs name different rows, and authorization mutations are addressed to the object.
    by_object: HashMap<Uuid, HashSet<Uuid>>,
    /// Highest committed authorization epoch this instance's post-commit revocation path has
    /// observed for each workspace. It closes the open/revoke race: an open checked before a
    /// revoke cannot register after the revoker has snapshotted the active set.
    observed_workspace_epochs: HashMap<Uuid, i64>,
}

/// Immutable identity needed to re-evaluate and, if necessary, remove one active subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSession {
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub object_id: Uuid,
    pub user_id: Uuid,
}

/// Which `limits-v1.md` connection ceiling a [`SessionRegistry::try_register_authorized`] admission refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionLimit {
    PerUser,
    PerDocument,
    PerWorkspace,
}

/// Why an otherwise valid WebSocket open could not reserve a registry slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationError {
    Limit(ConnectionLimit),
    /// A permission check completed before a committed authorization change this process has
    /// already observed. The caller must re-check instead of registering stale authority.
    StaleAuthorization,
}

impl ConnectionLimit {
    /// The exact `limit_kind` wire value (`limits-v1.md`'s table).
    #[must_use]
    pub const fn limit_kind(self) -> &'static str {
        match self {
            Self::PerUser => "user_connections",
            Self::PerDocument => "document_connections",
            Self::PerWorkspace => "workspace_connections",
        }
    }

    #[must_use]
    pub const fn limit(self) -> u64 {
        match self {
            Self::PerUser => CONNECTIONS_PER_USER_MAX,
            Self::PerDocument => CONNECTIONS_PER_DOCUMENT_MAX,
            Self::PerWorkspace => CONNECTIONS_PER_WORKSPACE_MAX,
        }
    }
}

/// What [`SessionRegistry::try_register_authorized`] hands back on success: the channel the session's
/// outbound writer task should drain, plus the shared slow-consumer queue counters so the session
/// loop can release each frame's charge once it is actually dequeued.
pub struct RegisteredSession {
    pub receiver: mpsc::UnboundedReceiver<OutboundEvent>,
    queue: Arc<QueueState>,
}

impl RegisteredSession {
    /// Releases one previously-[`SessionHandle::deliver`]-charged frame's contribution to the
    /// slow-consumer queue ceiling. The session loop must call this exactly once for every
    /// `OutboundEvent::Frame` it takes off [`Self::receiver`] — whether the frame is forwarded
    /// immediately, buffered for `update`/`accepted` pairing, or dropped as a stale duplicate —
    /// because the ceiling bounds the *channel* backlog, not any further in-process buffering.
    pub fn record_dequeued(&self, encoded_len: usize) {
        self.queue.frames.fetch_sub(1, Ordering::AcqRel);
        self.queue.bytes.fetch_sub(encoded_len, Ordering::AcqRel);
    }
}

#[derive(Default)]
pub struct SessionRegistry {
    // document_id -> session_id -> handle
    sessions: Mutex<HashMap<Uuid, HashMap<Uuid, SessionHandle>>>,
    // Connection accounting, authorization reverse index, and observed epochs share one lock so
    // registration/unregistration cannot expose a half-updated object index.
    connections: Mutex<ConnectionRegistry>,
    // (document_id, session_id) -> presence
    presence: Mutex<HashMap<(Uuid, Uuid), PresenceEntry>>,
}

/// Why a `presence` upsert was refused (`limits-v1.md`'s two independent presence ceilings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceLimit {
    PerConnection,
    PerDocument,
}

impl PresenceLimit {
    /// The exact `limit_kind` wire value (`limits-v1.md`'s table). Named here, next to the
    /// enforcement, for the same reason [`ConnectionLimit::limit_kind`] is: the string a caller
    /// branches on must not be re-spelled at the transport call site.
    #[must_use]
    pub const fn limit_kind(self) -> &'static str {
        match self {
            Self::PerConnection => "presence_entries_per_connection",
            Self::PerDocument => "presence_entries_per_document",
        }
    }

    #[must_use]
    pub const fn limit(self) -> u64 {
        match self {
            Self::PerConnection => PRESENCE_ENTRIES_PER_CONNECTION_MAX as u64,
            Self::PerDocument => PRESENCE_ENTRIES_PER_DOCUMENT_MAX as u64,
        }
    }
}

impl SessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically checks `connections_per_user_max`/`connections_per_document_max`/
    /// `connections_per_workspace_max` and, if all three have room, reserves the slot and
    /// registers the session's outbound channel in one critical section (`limits-v1.md`: "计数与
    /// 连接 slot 在 v0.4 单实例 session registry 原子保留/释放") — so two connections racing to open
    /// against an at-capacity ceiling cannot both observe "room" and both be admitted.
    ///
    /// # Errors
    /// The specific ceiling that was hit, or `StaleAuthorization` when a revocation committed
    /// after this open's permission check.
    #[allow(clippy::significant_drop_tightening)] // guard held across check-then-insert by design
    pub fn try_register_authorized(
        &self,
        document_id: Uuid,
        object_id: Uuid,
        user_id: Uuid,
        workspace_id: Uuid,
        session_id: Uuid,
        checked_epoch: i64,
    ) -> Result<RegisteredSession, RegistrationError> {
        let mut connections = self.connections.lock();
        if connections
            .observed_workspace_epochs
            .get(&workspace_id)
            .is_some_and(|observed| checked_epoch < *observed)
        {
            return Err(RegistrationError::StaleAuthorization);
        }
        let per_user = connections.by_session.values().filter(|c| c.user_id == user_id).count();
        if u64::try_from(per_user).unwrap_or(u64::MAX) >= CONNECTIONS_PER_USER_MAX {
            return Err(RegistrationError::Limit(ConnectionLimit::PerUser));
        }
        let per_document = connections
            .by_session
            .values()
            .filter(|c| c.document_id == document_id)
            .count();
        if u64::try_from(per_document).unwrap_or(u64::MAX) >= CONNECTIONS_PER_DOCUMENT_MAX {
            return Err(RegistrationError::Limit(ConnectionLimit::PerDocument));
        }
        let per_workspace = connections
            .by_session
            .values()
            .filter(|c| c.workspace_id == workspace_id)
            .count();
        if u64::try_from(per_workspace).unwrap_or(u64::MAX) >= CONNECTIONS_PER_WORKSPACE_MAX {
            return Err(RegistrationError::Limit(ConnectionLimit::PerWorkspace));
        }

        let (sender, receiver) = mpsc::unbounded_channel();
        let queue = Arc::new(QueueState::default());
        self.sessions.lock().entry(document_id).or_default().insert(
            session_id,
            SessionHandle {
                session_id,
                sender,
                queue: queue.clone(),
            },
        );
        connections.by_session.insert(
            session_id,
            ConnMeta {
                document_id,
                object_id,
                user_id,
                workspace_id,
                acked: None,
            },
        );
        connections.by_object.entry(object_id).or_default().insert(session_id);
        Ok(RegisteredSession { receiver, queue })
    }

    /// Compact test-only adapter for the registry's pre-v0.5 call shape. Production must always
    /// pass the real object id and checked epoch through [`Self::try_register_authorized`].
    #[cfg(test)]
    pub fn try_register(
        &self,
        document_id: Uuid,
        user_id: Uuid,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<RegisteredSession, ConnectionLimit> {
        match self.try_register_authorized(document_id, document_id, user_id, workspace_id, session_id, 0) {
            Ok(registered) => Ok(registered),
            Err(RegistrationError::Limit(limit)) => Err(limit),
            Err(RegistrationError::StaleAuthorization) => {
                panic!("a fresh test registry unexpectedly rejected epoch zero as stale")
            }
        }
    }

    #[allow(clippy::significant_drop_tightening)] // guard used for two related mutations, not idle
    pub fn unregister(&self, document_id: Uuid, session_id: Uuid) {
        let mut connections = self.connections.lock();
        if let Some(meta) = connections.by_session.remove(&session_id)
            && let Some(by_session) = connections.by_object.get_mut(&meta.object_id)
        {
            by_session.remove(&session_id);
            if by_session.is_empty() {
                connections.by_object.remove(&meta.object_id);
            }
        }
        let mut sessions = self.sessions.lock();
        if let Some(by_session) = sessions.get_mut(&document_id) {
            by_session.remove(&session_id);
            if by_session.is_empty() {
                sessions.remove(&document_id);
            }
        }
        drop(sessions);
        drop(connections);
        self.presence.lock().remove(&(document_id, session_id));
    }

    /// Direct test-only view of the authorization reverse index. Tests that assert index cleanup
    /// must not go through an active-session reader: those readers intentionally cross-check
    /// `by_session` and would hide a dangling `by_object` member.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn object_index_session_count(&self, object_id: Uuid) -> usize {
        self.connections
            .lock()
            .by_object
            .get(&object_id)
            .map_or(0, HashSet::len)
    }

    /// Records the committed epoch before snapshotting sessions in `object_ids`. Holding the same
    /// lock registration uses makes the snapshot and the stale-open barrier one atomic action.
    #[must_use]
    pub fn observe_epoch_and_subtree_sessions(
        &self,
        workspace_id: Uuid,
        committed_epoch: i64,
        object_ids: &HashSet<Uuid>,
    ) -> Vec<ActiveSession> {
        let mut connections = self.connections.lock();
        connections
            .observed_workspace_epochs
            .entry(workspace_id)
            .and_modify(|epoch| *epoch = (*epoch).max(committed_epoch))
            .or_insert(committed_epoch);
        let active = object_ids
            .iter()
            .filter_map(|object_id| connections.by_object.get(object_id))
            .flat_map(|session_ids| session_ids.iter())
            .filter_map(|session_id| {
                let meta = connections.by_session.get(session_id)?;
                (meta.workspace_id == workspace_id).then_some(ActiveSession {
                    session_id: *session_id,
                    document_id: meta.document_id,
                    object_id: meta.object_id,
                    user_id: meta.user_id,
                })
            })
            .collect();
        drop(connections);
        active
    }

    /// Workspace-wide counterpart used for membership, baseline, and feature-flag changes.
    #[must_use]
    pub fn observe_epoch_and_workspace_sessions(&self, workspace_id: Uuid, committed_epoch: i64) -> Vec<ActiveSession> {
        let mut connections = self.connections.lock();
        connections
            .observed_workspace_epochs
            .entry(workspace_id)
            .and_modify(|epoch| *epoch = (*epoch).max(committed_epoch))
            .or_insert(committed_epoch);
        let active = connections
            .by_session
            .iter()
            .filter_map(|(session_id, meta)| {
                (meta.workspace_id == workspace_id).then_some(ActiveSession {
                    session_id: *session_id,
                    document_id: meta.document_id,
                    object_id: meta.object_id,
                    user_id: meta.user_id,
                })
            })
            .collect();
        drop(connections);
        active
    }

    /// Deletes revoked sessions' presence independently of close delivery. This is intentionally
    /// a separate operation so a failed/missing disconnect cannot retain presence until TTL.
    pub fn remove_presence_for_sessions(&self, revoked: &[ActiveSession]) -> usize {
        let mut presence = self.presence.lock();
        revoked
            .iter()
            .filter(|session| presence.remove(&(session.document_id, session.session_id)).is_some())
            .count()
    }

    /// Removes revoked subscriptions from every live-session index, then best-effort sends the
    /// fixed close. Once removed, they cannot receive later document/presence broadcasts even if
    /// their socket writer has already disappeared.
    pub fn disconnect_authorization_sessions(&self, revoked: &[ActiveSession], code: u16, reason: &str) -> usize {
        let mut connections = self.connections.lock();
        let mut sessions = self.sessions.lock();
        let mut handles = Vec::with_capacity(revoked.len());
        for revoked_session in revoked {
            let matches = connections
                .by_session
                .get(&revoked_session.session_id)
                .is_some_and(|meta| {
                    meta.document_id == revoked_session.document_id && meta.object_id == revoked_session.object_id
                });
            if !matches {
                continue;
            }
            connections.by_session.remove(&revoked_session.session_id);
            if let Some(by_session) = connections.by_object.get_mut(&revoked_session.object_id) {
                by_session.remove(&revoked_session.session_id);
                if by_session.is_empty() {
                    connections.by_object.remove(&revoked_session.object_id);
                }
            }
            if let Some(by_session) = sessions.get_mut(&revoked_session.document_id) {
                if let Some(handle) = by_session.remove(&revoked_session.session_id) {
                    handles.push(handle);
                }
                if by_session.is_empty() {
                    sessions.remove(&revoked_session.document_id);
                }
            }
        }
        drop(sessions);
        drop(connections);
        for handle in &handles {
            handle.send(OutboundEvent::Close {
                code,
                reason: reason.to_string(),
            });
        }
        handles.len()
    }

    /// Sends `frame` to every session with `document_id` open other than `exclude`, each through
    /// [`SessionHandle::deliver`]'s slow-consumer accounting.
    ///
    /// Order note: this instance's write path only ever calls this once per document, strictly
    /// after that write's transaction commits, and the per-document coordinator (`coordinator.rs`)
    /// serializes writers for the same document within this instance — so commits (and therefore
    /// these broadcasts) happen in commit order with no reordering possible to guard against here.
    /// A multi-instance deployment would need the cross-instance ordered egress channel
    /// `ADR-0010` defers past v0.4; this module's docs already flag that as not implemented.
    #[allow(clippy::significant_drop_tightening)] // the guard must outlive the loop it feeds
    pub fn broadcast(&self, document_id: Uuid, frame: &Frame, exclude: Option<Uuid>) {
        let Ok(encoded) = serde_json::to_string(frame) else {
            // Every `Frame` variant here is plain, already-validated data (base64 strings, UUIDs,
            // safe JSON `Value`s) -- serialization failing at all would be a programming error, not
            // something a peer session can trigger, so there is nothing safe to forward.
            return;
        };
        let encoded_len = encoded.len();
        let sessions = self.sessions.lock();
        let Some(by_session) = sessions.get(&document_id) else {
            return;
        };
        for (session_id, handle) in by_session {
            if Some(*session_id) != exclude {
                handle.deliver(frame, encoded_len);
            }
        }
    }

    /// Force-closes every session with `document_id` open. Authorization revocation uses the
    /// session-selective [`Self::disconnect_authorization_sessions`] path instead, because two
    /// users on the same document can resolve to different effective grades.
    pub fn disconnect_document(&self, document_id: Uuid, code: u16, reason: &str) {
        let mut sessions = self.sessions.lock();
        if let Some(by_session) = sessions.remove(&document_id) {
            for handle in by_session.values() {
                handle.send(OutboundEvent::Close {
                    code,
                    reason: reason.to_string(),
                });
            }
        }
        drop(sessions);
        self.presence.lock().retain(|(doc_id, _), _| *doc_id != document_id);
    }

    /// Force-closes every session this instance's registry knows about (every document, every
    /// workspace) with `server_draining{reason:"drain"}` at [`DRAIN_CLOSE_CODE`]
    /// (`collab-protocol-v1.md`: "实例 ... 显式排空"). Returns how many sessions were closed.
    ///
    /// Reached in production through [`super::runtime::CollabRuntime::begin_process_drain`], which
    /// `apps/api/src/main.rs`'s graceful-shutdown hook calls on SIGTERM/Ctrl-C — so this is the
    /// instance half of the drain that a real deploy/restart produces, not only a fixture. (A
    /// workspace-admin drain endpoint is still absent; [`Self::drain_workspace`] remains reachable
    /// only through the shared `begin_workspace_drain` producer.)
    pub fn drain_all(&self, retry_after_ms: u64) -> usize {
        let signal = DrainSignal::new(retry_after_ms);
        let reason = signal.close_reason();
        let sessions = self.sessions.lock();
        let mut closed = 0usize;
        for (document_id, by_session) in sessions.iter() {
            for handle in by_session.values() {
                let frame = drain_rejected_frame(*document_id, signal);
                handle.send(OutboundEvent::ControlFrame(Box::new(frame)));
                handle.send(OutboundEvent::Close {
                    code: DRAIN_CLOSE_CODE,
                    reason: reason.clone(),
                });
                closed += 1;
            }
        }
        drop(sessions);
        closed
    }

    /// The workspace-scoped half of [`Self::drain_all`] (`collab-protocol-v1.md`: "实例/workspace
    /// 显式排空"), called by [`super::runtime::CollabRuntime::begin_workspace_drain`].
    pub fn drain_workspace(&self, workspace_id: Uuid, retry_after_ms: u64) -> usize {
        let member_sessions: HashMap<Uuid, Uuid> = self
            .connections
            .lock()
            .by_session
            .iter()
            .filter(|(_, meta)| meta.workspace_id == workspace_id)
            .map(|(session_id, meta)| (*session_id, meta.document_id))
            .collect();
        if member_sessions.is_empty() {
            return 0;
        }
        let signal = DrainSignal::new(retry_after_ms);
        let reason = signal.close_reason();
        let sessions = self.sessions.lock();
        let mut closed = 0usize;
        for by_session in sessions.values() {
            for (session_id, handle) in by_session {
                if let Some(document_id) = member_sessions.get(session_id) {
                    let frame = drain_rejected_frame(*document_id, signal);
                    handle.send(OutboundEvent::ControlFrame(Box::new(frame)));
                    handle.send(OutboundEvent::Close {
                        code: DRAIN_CLOSE_CODE,
                        reason: reason.clone(),
                    });
                    closed += 1;
                }
            }
        }
        drop(sessions);
        closed
    }

    /// Recoverably drains only the supplied active sessions. Authorization revalidation uses
    /// this when the database cannot determine whether those sessions remain authorized: 4403
    /// would falsely claim a permanent denial, while the existing structured 4410 drain tells
    /// clients to reconnect and re-run ticket/open authorization.
    pub fn drain_sessions(&self, candidates: &[ActiveSession], retry_after_ms: u64) -> usize {
        if candidates.is_empty() {
            return 0;
        }
        let sessions = self.sessions.lock();
        let handles: Vec<(Uuid, SessionHandle)> = candidates
            .iter()
            .filter_map(|candidate| {
                sessions
                    .get(&candidate.document_id)?
                    .get(&candidate.session_id)
                    .cloned()
                    .map(|handle| (candidate.document_id, handle))
            })
            .collect();
        drop(sessions);

        let signal = DrainSignal::new(retry_after_ms);
        let reason = signal.close_reason();
        for (document_id, handle) in &handles {
            handle.send(OutboundEvent::ControlFrame(Box::new(drain_rejected_frame(
                *document_id,
                signal,
            ))));
            handle.send(OutboundEvent::Close {
                code: DRAIN_CLOSE_CODE,
                reason: reason.clone(),
            });
        }
        handles.len()
    }

    #[must_use]
    pub fn session_count(&self, document_id: Uuid) -> usize {
        self.sessions.lock().get(&document_id).map_or(0, HashMap::len)
    }

    /// Records one `ack` frame's `(seq, frontier)` for `session_id`.
    ///
    /// Monotonic by construction: `collab-protocol-v1.md` states an `ack` at or below what the
    /// client already acknowledged is an idempotent repeat the server ignores ("`seq<=
    /// last_applied_seq` 是幂等重复,忽略但可重发 ack"), so a regression must never move the recorded
    /// frontier backwards — otherwise a duplicate ack replayed after a later one would rewind a
    /// position that v0.8 retention/compaction is meant to read.
    ///
    /// Returns whether the recorded frontier actually advanced (`false` for a repeat, and for an
    /// unknown `session_id` — a session already unregistered).
    #[allow(clippy::significant_drop_tightening)] // guard held across the read-then-update by design
    pub fn record_ack(&self, session_id: Uuid, seq: i64, frontier: String) -> bool {
        let mut connections = self.connections.lock();
        let Some(meta) = connections.by_session.get_mut(&session_id) else {
            return false;
        };
        if meta.acked.as_ref().is_some_and(|current| seq <= current.seq) {
            return false;
        }
        meta.acked = Some(AckFrontier { seq, frontier });
        true
    }

    /// The last position `session_id` acknowledged, or `None` if it has acknowledged nothing (or
    /// is no longer registered).
    #[must_use]
    pub fn acked(&self, session_id: Uuid) -> Option<AckFrontier> {
        self.connections.lock().by_session.get(&session_id)?.acked.clone()
    }

    /// The live presence entries for `document_id`, excluding `exclude` (the session that is about
    /// to receive them), with expired entries swept first so a joining session is never handed a
    /// cursor the contract already required to have disappeared.
    ///
    /// This is the "join" half of presence fan-out: `broadcast` only reaches sessions that are
    /// already connected when a `presence` frame arrives, so without this a session joining a
    /// document sees nobody until each peer's next refresh. Read-only — it never inserts, never
    /// refreshes an expiry, and therefore cannot be used by a joining session to keep another
    /// session's entry alive.
    #[must_use]
    pub fn presence_snapshot(&self, document_id: Uuid, exclude: Option<Uuid>) -> Vec<(Uuid, Value)> {
        let mut presence = self.presence.lock();
        Self::expire_presence(&mut presence);
        presence
            .iter()
            .filter(|((did, sid), _)| *did == document_id && Some(*sid) != exclude)
            .map(|((_, sid), entry)| (*sid, entry.payload.clone()))
            .collect()
    }

    fn expire_presence(presence: &mut HashMap<(Uuid, Uuid), PresenceEntry>) {
        let now = Instant::now();
        presence.retain(|_, entry| entry.expires_at > now);
    }

    /// `limits-v1.md`'s presence upsert rule: same `(document_id, session_id)` key only refreshes
    /// payload/expiry (no count change); a *new* key is checked against both ceilings before being
    /// admitted, and an over-ceiling attempt evicts nothing else (`不驱逐其他 session`).
    ///
    /// # Errors
    /// The specific ceiling that was hit, so the caller can map it to the matching `limit_kind`.
    #[allow(clippy::significant_drop_tightening)] // guard held across the whole check-then-insert
    pub fn upsert_presence(
        &self,
        document_id: Uuid,
        session_id: Uuid,
        payload: Value,
        ttl: Duration,
    ) -> Result<(), PresenceLimit> {
        let mut presence = self.presence.lock();
        Self::expire_presence(&mut presence);

        let key = (document_id, session_id);
        if !presence.contains_key(&key) {
            let per_connection = presence.keys().filter(|(_, sid)| *sid == session_id).count();
            if per_connection >= PRESENCE_ENTRIES_PER_CONNECTION_MAX {
                return Err(PresenceLimit::PerConnection);
            }
            let per_document = presence.keys().filter(|(did, _)| *did == document_id).count();
            if per_document >= PRESENCE_ENTRIES_PER_DOCUMENT_MAX {
                return Err(PresenceLimit::PerDocument);
            }
        }
        presence.insert(
            key,
            PresenceEntry {
                payload,
                expires_at: Instant::now() + ttl,
            },
        );
        Ok(())
    }

    #[must_use]
    pub fn presence_count(&self, document_id: Uuid) -> usize {
        let mut presence = self.presence.lock();
        Self::expire_presence(&mut presence);
        presence.keys().filter(|(did, _)| *did == document_id).count()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::collections::HashSet;

    use super::{
        CONNECTIONS_PER_DOCUMENT_MAX, CONNECTIONS_PER_USER_MAX, CONNECTIONS_PER_WORKSPACE_MAX, ConnectionLimit,
        OutboundEvent, PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX, PresenceLimit,
        RegistrationError, SLOW_CONSUMER_QUEUE_BYTES_MAX, SLOW_CONSUMER_QUEUE_FRAMES_MAX, SessionRegistry,
    };
    use crate::flow::collab::frame::{Frame, PROTOCOL_VERSION, RejectedCode};
    use serde_json::{Value, json};
    use std::time::Duration;
    use uuid::Uuid;

    fn ping(nonce: &str) -> Frame {
        Frame::Ping {
            protocol_version: PROTOCOL_VERSION,
            nonce: nonce.to_string(),
        }
    }

    /// A `Frame::Ping` whose `serde_json::to_string` length is exactly `target_len` bytes --
    /// achieved by padding `nonce` with plain ASCII (no JSON escaping, so every extra character
    /// grows the encoded frame by exactly one byte with zero framing overhead beyond the base
    /// frame's own fixed shape).
    fn ping_of_exact_encoded_len(target_len: usize) -> Frame {
        let base_len = serde_json::to_string(&ping("")).expect("ping serializes").len();
        assert!(
            target_len >= base_len,
            "target_len ({target_len}) must be at least the frame's fixed overhead ({base_len} bytes)"
        );
        ping(&"a".repeat(target_len - base_len))
    }

    #[test]
    fn presence_same_key_refreshes_without_growing_count() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        for _ in 0..5 {
            registry
                .upsert_presence(document_id, session_id, json!({"x": 1}), Duration::from_secs(30))
                .expect("upserts");
        }
        assert_eq!(registry.presence_count(document_id), 1);
    }

    #[test]
    fn per_connection_presence_ceiling_is_enforced() {
        let registry = SessionRegistry::new();
        let session_id = Uuid::new_v4();
        for _ in 0..PRESENCE_ENTRIES_PER_CONNECTION_MAX {
            registry
                .upsert_presence(Uuid::new_v4(), session_id, json!({}), Duration::from_secs(30))
                .expect("upserts under the ceiling");
        }
        let result = registry.upsert_presence(Uuid::new_v4(), session_id, json!({}), Duration::from_secs(30));
        assert_eq!(result, Err(PresenceLimit::PerConnection));
    }

    #[test]
    fn per_document_presence_ceiling_is_enforced_and_does_not_evict_others() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        for _ in 0..PRESENCE_ENTRIES_PER_DOCUMENT_MAX {
            registry
                .upsert_presence(document_id, Uuid::new_v4(), json!({}), Duration::from_secs(30))
                .expect("upserts under the ceiling");
        }
        let before = registry.presence_count(document_id);
        let result = registry.upsert_presence(document_id, Uuid::new_v4(), json!({}), Duration::from_secs(30));
        assert_eq!(result, Err(PresenceLimit::PerDocument));
        assert_eq!(
            registry.presence_count(document_id),
            before,
            "a rejected new entry evicts nothing"
        );
    }

    #[test]
    fn expired_presence_is_swept_and_frees_room() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        registry
            .upsert_presence(document_id, Uuid::new_v4(), json!({}), Duration::from_millis(10))
            .expect("upserts");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(registry.presence_count(document_id), 0, "expired entries are swept");
    }

    #[test]
    fn unregister_removes_session_and_its_presence() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let _registered = registry
            .try_register(document_id, user_id, workspace_id, session_id)
            .expect("registers under every ceiling");
        registry
            .upsert_presence(document_id, session_id, json!({}), Duration::from_secs(30))
            .expect("upserts");
        assert_eq!(registry.session_count(document_id), 1);

        registry.unregister(document_id, session_id);
        assert_eq!(registry.session_count(document_id), 0);
        assert_eq!(registry.presence_count(document_id), 0);
    }

    #[test]
    fn per_user_connection_ceiling_is_enforced_and_freed_on_unregister() {
        let registry = SessionRegistry::new();
        let user_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let mut sessions = Vec::new();
        for _ in 0..CONNECTIONS_PER_USER_MAX {
            let document_id = Uuid::new_v4();
            let session_id = Uuid::new_v4();
            registry
                .try_register(document_id, user_id, workspace_id, session_id)
                .expect("registers under the per-user ceiling");
            sessions.push((document_id, session_id));
        }
        let result = registry.try_register(Uuid::new_v4(), user_id, workspace_id, Uuid::new_v4());
        assert_eq!(result.err().map(ConnectionLimit::limit_kind), Some("user_connections"));

        let (document_id, session_id) = sessions.remove(0);
        registry.unregister(document_id, session_id);
        registry
            .try_register(Uuid::new_v4(), user_id, workspace_id, Uuid::new_v4())
            .expect("freed slot admits a new connection");
    }

    #[test]
    fn per_document_connection_ceiling_is_enforced() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        for _ in 0..CONNECTIONS_PER_DOCUMENT_MAX {
            registry
                .try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
                .expect("registers under the per-document ceiling");
        }
        let result = registry.try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            result.err().map(ConnectionLimit::limit_kind),
            Some("document_connections")
        );
    }

    #[test]
    fn per_workspace_connection_ceiling_is_enforced() {
        let registry = SessionRegistry::new();
        let workspace_id = Uuid::new_v4();
        for _ in 0..CONNECTIONS_PER_WORKSPACE_MAX {
            registry
                .try_register(Uuid::new_v4(), Uuid::new_v4(), workspace_id, Uuid::new_v4())
                .expect("registers under the per-workspace ceiling");
        }
        let result = registry.try_register(Uuid::new_v4(), Uuid::new_v4(), workspace_id, Uuid::new_v4());
        assert_eq!(
            result.err().map(ConnectionLimit::limit_kind),
            Some("workspace_connections")
        );
    }

    #[test]
    fn a_slow_consumer_is_force_closed_once_the_queue_frame_ceiling_is_exceeded() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let mut registered = registry
            .try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), session_id)
            .expect("registers");

        // Never drain `registered.receiver` -- simulates a consumer that has stopped reading.
        for i in 0..SLOW_CONSUMER_QUEUE_FRAMES_MAX {
            registry.broadcast(document_id, &ping(&i.to_string()), None);
        }
        // One more frame past the ceiling must flip this session to force-close instead of queuing
        // another frame.
        registry.broadcast(document_id, &ping("over"), None);

        let mut saw_close = false;
        let mut frame_count = 0u64;
        while let Ok(event) = registered.receiver.try_recv() {
            match event {
                OutboundEvent::Frame(..) => frame_count += 1,
                OutboundEvent::ControlFrame(_) => panic!("unexpected control frame"),
                OutboundEvent::Close { code, reason } => {
                    saw_close = true;
                    assert_eq!(code, 4408, "slow consumer must close at the frozen limit_exceeded code");
                    let details: Value = serde_json::from_str(&reason).expect("close reason is structured JSON");
                    assert_eq!(details["code"], "limit_exceeded");
                    assert_eq!(details["limit_kind"], "slow_consumer_queue_frames");
                    assert_eq!(details["limit"], SLOW_CONSUMER_QUEUE_FRAMES_MAX);
                }
            }
        }
        assert!(
            saw_close,
            "exceeding the queue frame ceiling must force-close the session"
        );
        assert_eq!(
            frame_count, SLOW_CONSUMER_QUEUE_FRAMES_MAX,
            "the frame that pushed the queue over the ceiling must never itself be queued"
        );

        // Further broadcasts to the now-closing session must not queue anything else (and must not
        // send a second Close).
        registry.broadcast(document_id, &ping("after-close"), None);
        assert!(
            registered.receiver.try_recv().is_err(),
            "a closing session receives nothing further"
        );
    }

    /// The frame-count ceiling test above never drives `bytes` anywhere near
    /// `SLOW_CONSUMER_QUEUE_BYTES_MAX` (256 small `Ping` frames total a few KB) -- this test
    /// drives the *byte* ceiling to its own exact boundary independently, using few enough frames
    /// (8, each exactly 1 MiB) that `SLOW_CONSUMER_QUEUE_FRAMES_MAX` (256) is nowhere close to
    /// tripping first.
    #[test]
    fn a_slow_consumer_is_force_closed_once_the_queue_byte_ceiling_is_exceeded_independent_of_frame_count() {
        const CHUNK_BYTES: usize = 1_048_576;

        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let mut registered = registry
            .try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), session_id)
            .expect("registers");

        assert_eq!(
            u64::try_from(CHUNK_BYTES).expect("fits u64") * 8,
            SLOW_CONSUMER_QUEUE_BYTES_MAX,
            "this fixture assumes SLOW_CONSUMER_QUEUE_BYTES_MAX is exactly 8 * 1 MiB"
        );
        let large_frame = ping_of_exact_encoded_len(CHUNK_BYTES);
        assert_eq!(
            serde_json::to_string(&large_frame).expect("frame serializes").len(),
            CHUNK_BYTES
        );

        // Never drain `registered.receiver` -- simulates a consumer that has stopped reading.
        // Eight exact-1-MiB frames sum to exactly the byte ceiling and must all be queued.
        for _ in 0..8 {
            registry.broadcast(document_id, &large_frame, None);
        }
        // One more frame of any size pushes the queue's byte total one byte past
        // SLOW_CONSUMER_QUEUE_BYTES_MAX -- the frame count (9) stays far under
        // SLOW_CONSUMER_QUEUE_FRAMES_MAX (256), proving this is the byte ceiling firing, not the
        // frame ceiling.
        registry.broadcast(document_id, &ping("over-byte-ceiling"), None);

        let mut saw_close = false;
        let mut frame_count = 0u64;
        while let Ok(event) = registered.receiver.try_recv() {
            match event {
                OutboundEvent::Frame(..) => frame_count += 1,
                OutboundEvent::ControlFrame(_) => panic!("unexpected control frame"),
                OutboundEvent::Close { code, reason } => {
                    saw_close = true;
                    assert_eq!(code, 4408, "slow consumer must close at the frozen limit_exceeded code");
                    let details: Value = serde_json::from_str(&reason).expect("close reason is structured JSON");
                    assert_eq!(details["code"], "limit_exceeded");
                    assert_eq!(details["limit_kind"], "slow_consumer_queue_bytes");
                    assert_eq!(details["limit"], SLOW_CONSUMER_QUEUE_BYTES_MAX);
                }
            }
        }
        assert!(
            saw_close,
            "exceeding the queue byte ceiling must force-close the session"
        );
        assert_eq!(
            frame_count, 8,
            "the frame that pushed the byte queue over the ceiling must never itself be queued"
        );

        // Further broadcasts to the now-closing session must not queue anything else (and must not
        // send a second Close).
        registry.broadcast(document_id, &ping("after-close"), None);
        assert!(
            registered.receiver.try_recv().is_err(),
            "a closing session receives nothing further"
        );
    }

    /// The "不得无界缓冲" half of `limits-v1.md`'s slow-consumer rule, stated as a bound rather than
    /// as an event: however many frames are broadcast at a session that has stopped reading, the
    /// outbound channel must never hold more than `slow_consumer_queue_frames_max` of them. The
    /// force-close assertions above stop at the boundary; this one keeps pushing well past it, so
    /// removing the ceiling shows up as the queue length itself growing without bound rather than
    /// only as a missing `Close`.
    #[test]
    fn a_stalled_sessions_outbound_queue_never_grows_past_the_frame_ceiling_however_much_is_pushed() {
        const PUSHED: u64 = 1_000;

        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let mut registered = registry
            .try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), session_id)
            .expect("registers");

        // Never drained: the session's consumer is stalled for the whole burst.
        for i in 0..PUSHED {
            registry.broadcast(document_id, &ping(&i.to_string()), None);
        }

        let mut queued = 0u64;
        while let Ok(event) = registered.receiver.try_recv() {
            if matches!(event, OutboundEvent::Frame(..)) {
                queued += 1;
            }
        }
        assert!(
            queued <= SLOW_CONSUMER_QUEUE_FRAMES_MAX,
            "a stalled session buffered {queued} frames out of {PUSHED} pushed; the ceiling is {SLOW_CONSUMER_QUEUE_FRAMES_MAX}"
        );
    }

    /// `collab-protocol-v1.md`: "`seq<=last_applied_seq` 是幂等重复,忽略但可重发 ack". A replayed
    /// older ack arriving after a newer one -- the exact shape a client's reconnect outbox
    /// produces -- must not rewind the recorded position.
    #[test]
    fn ack_advances_only_forwards_and_a_replayed_older_ack_never_rewinds_it() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let unknown_session = Uuid::new_v4();
        let _registered = registry
            .try_register(document_id, Uuid::new_v4(), Uuid::new_v4(), session_id)
            .expect("registers");

        assert!(registry.acked(session_id).is_none(), "nothing is acknowledged yet");
        assert!(registry.record_ack(session_id, 4, "frontier-4".to_string()));
        assert!(registry.record_ack(session_id, 9, "frontier-9".to_string()));

        assert!(
            !registry.record_ack(session_id, 4, "frontier-4-replayed".to_string()),
            "a replayed older ack must be reported as a no-op"
        );
        assert!(
            !registry.record_ack(session_id, 9, "frontier-9-again".to_string()),
            "re-acking the current position is an idempotent repeat, not an advance"
        );
        let recorded = registry.acked(session_id).expect("an ack is recorded");
        assert_eq!(recorded.seq, 9);
        assert_eq!(
            recorded.frontier, "frontier-9",
            "the payload of a rewinding ack must not be stored either"
        );

        assert!(
            !registry.record_ack(unknown_session, 1, "orphan".to_string()),
            "an ack for a session that is no longer registered is dropped"
        );
        assert!(registry.acked(unknown_session).is_none());
    }

    /// `presence_snapshot` is the "join" half of presence fan-out. It must exclude the joining
    /// session itself, be scoped to one document, and never hand out an entry whose TTL has
    /// already lapsed -- the contract requires a crashed peer's cursor to be gone at 30 seconds
    /// whether or not anyone happened to call `presence_count` in the meantime.
    #[test]
    fn presence_snapshot_is_document_scoped_excludes_the_joiner_and_omits_expired_entries() {
        let registry = SessionRegistry::new();
        let document_id = Uuid::new_v4();
        let other_document = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let joiner = Uuid::new_v4();
        let elsewhere = Uuid::new_v4();
        let expiring = Uuid::new_v4();

        registry
            .upsert_presence(document_id, peer, json!({"cursor": 1}), Duration::from_secs(30))
            .expect("peer presence is accepted");
        registry
            .upsert_presence(document_id, joiner, json!({"cursor": 2}), Duration::from_secs(30))
            .expect("joiner presence is accepted");
        registry
            .upsert_presence(other_document, elsewhere, json!({"cursor": 3}), Duration::from_secs(30))
            .expect("another document's presence is accepted");
        registry
            .upsert_presence(document_id, expiring, json!({"cursor": 4}), Duration::from_millis(1))
            .expect("expiring presence is accepted");
        std::thread::sleep(Duration::from_millis(20));

        let snapshot = registry.presence_snapshot(document_id, Some(joiner));
        assert_eq!(
            snapshot.len(),
            1,
            "only the live peer on this document may be handed to the joiner, got {snapshot:?}"
        );
        assert_eq!(snapshot[0].0, peer);
        assert_eq!(snapshot[0].1["cursor"], 1);

        // Reading the snapshot must not have refreshed anything: the expired entry is gone for
        // good, and the joiner's own entry is still its own.
        assert_eq!(registry.presence_count(document_id), 2);
        assert_eq!(registry.presence_count(other_document), 1);
    }

    #[test]
    fn drain_all_closes_every_session_with_the_frozen_drain_code_and_reason_shape() {
        let registry = SessionRegistry::new();
        let document_a = Uuid::new_v4();
        let document_b = Uuid::new_v4();
        let mut a = registry
            .try_register(document_a, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
            .expect("registers");
        let mut b = registry
            .try_register(document_b, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
            .expect("registers");

        let closed = registry.drain_all(1500);
        assert_eq!(closed, 2);

        for receiver in [&mut a.receiver, &mut b.receiver] {
            let rejected = receiver.try_recv().expect("a rejected control frame is queued");
            let OutboundEvent::ControlFrame(frame) = rejected else {
                panic!("expected a rejected frame before close");
            };
            let Frame::Rejected { code, details, .. } = *frame else {
                panic!("expected a Rejected frame");
            };
            assert_eq!(code, RejectedCode::ServerDraining);
            let details = details.expect("drain rejection carries details");
            assert_eq!(details["reason"], "drain");
            assert_eq!(details["retry_after_ms"], 1500);

            let event = receiver.try_recv().expect("a close event is queued");
            let OutboundEvent::Close { code, reason } = event else {
                panic!("expected a Close event");
            };
            assert_eq!(code, 4410, "drain must close at the frozen server_draining(drain) code");
            let parsed: serde_json::Value = serde_json::from_str(&reason).expect("reason is valid JSON");
            assert_eq!(parsed["reason"], "drain");
            assert_eq!(parsed["retry_after_ms"], 1500);
            assert_eq!(
                parsed.as_object().expect("reason is a JSON object").len(),
                2,
                "the close reason must carry only the two safe fields"
            );
        }
    }

    #[test]
    fn drain_workspace_closes_only_that_workspaces_sessions() {
        let registry = SessionRegistry::new();
        let target_workspace = Uuid::new_v4();
        let other_workspace = Uuid::new_v4();
        let mut target = registry
            .try_register(Uuid::new_v4(), Uuid::new_v4(), target_workspace, Uuid::new_v4())
            .expect("registers");
        let mut other = registry
            .try_register(Uuid::new_v4(), Uuid::new_v4(), other_workspace, Uuid::new_v4())
            .expect("registers");

        let closed = registry.drain_workspace(target_workspace, 500);
        assert_eq!(closed, 1);

        assert!(matches!(
            target.receiver.try_recv(),
            Ok(OutboundEvent::ControlFrame(..))
        ));
        assert!(matches!(
            target.receiver.try_recv(),
            Ok(OutboundEvent::Close { code: 4410, .. })
        ));
        assert!(
            other.receiver.try_recv().is_err(),
            "a different workspace's session must not be touched"
        );
    }

    #[test]
    fn recoverable_drain_is_limited_to_uncertain_authorization_sessions() {
        let registry = SessionRegistry::new();
        let workspace_id = Uuid::new_v4();
        let uncertain_object = Uuid::new_v4();
        let healthy_object = Uuid::new_v4();
        let uncertain_document = Uuid::new_v4();
        let healthy_document = Uuid::new_v4();
        let uncertain_session = Uuid::new_v4();
        let mut uncertain = registry
            .try_register_authorized(
                uncertain_document,
                uncertain_object,
                Uuid::new_v4(),
                workspace_id,
                uncertain_session,
                1,
            )
            .expect("uncertain fixture registers");
        let mut healthy = registry
            .try_register_authorized(
                healthy_document,
                healthy_object,
                Uuid::new_v4(),
                workspace_id,
                Uuid::new_v4(),
                1,
            )
            .expect("healthy fixture registers");

        let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, 2);
        let uncertain_candidate = candidates
            .into_iter()
            .find(|candidate| candidate.session_id == uncertain_session)
            .expect("uncertain session is observable");
        assert_eq!(registry.drain_sessions(&[uncertain_candidate], 5_000), 1);

        let OutboundEvent::ControlFrame(frame) = uncertain.receiver.try_recv().expect("drain control frame is queued")
        else {
            panic!("expected structured drain control frame")
        };
        let Frame::Rejected {
            code,
            recoverable,
            details,
            ..
        } = *frame
        else {
            panic!("expected server_draining rejection")
        };
        assert_eq!(code, RejectedCode::ServerDraining);
        assert!(recoverable);
        assert_eq!(
            details,
            Some(serde_json::json!({"reason": "drain", "retry_after_ms": 5_000}))
        );
        assert!(matches!(
            uncertain.receiver.try_recv(),
            Ok(OutboundEvent::Close { code: 4410, .. })
        ));
        assert!(
            healthy.receiver.try_recv().is_err(),
            "a known-healthy session must not be drained with the uncertain object"
        );
    }

    #[test]
    fn object_reverse_index_and_stale_open_barrier_follow_the_connection_lifecycle() {
        let registry = SessionRegistry::new();
        let workspace_id = Uuid::new_v4();
        let object_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let _registered = registry
            .try_register_authorized(document_id, object_id, user_id, workspace_id, session_id, 7)
            .expect("current authorization registers");
        assert_eq!(registry.object_index_session_count(object_id), 1);

        let objects = HashSet::from([object_id]);
        let active = registry.observe_epoch_and_subtree_sessions(workspace_id, 8, &objects);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].object_id, object_id);
        assert_eq!(
            active[0].document_id, document_id,
            "object and document ids stay distinct"
        );

        assert!(matches!(
            registry.try_register_authorized(
                Uuid::new_v4(),
                object_id,
                Uuid::new_v4(),
                workspace_id,
                Uuid::new_v4(),
                7,
            ),
            Err(RegistrationError::StaleAuthorization)
        ));

        registry.unregister(document_id, session_id);
        assert_eq!(
            registry.object_index_session_count(object_id),
            0,
            "unregister must remove the object reverse-index member, not leave a dangling entry"
        );
        assert_eq!(registry.session_count(document_id), 0);
    }

    #[test]
    fn authorization_presence_cleanup_is_independent_and_revoked_session_leaves_fanout() {
        let registry = SessionRegistry::new();
        let workspace_id = Uuid::new_v4();
        let object_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let revoked_session_id = Uuid::new_v4();
        let survivor_session_id = Uuid::new_v4();
        let mut revoked = registry
            .try_register_authorized(
                document_id,
                object_id,
                Uuid::new_v4(),
                workspace_id,
                revoked_session_id,
                3,
            )
            .expect("revoked fixture registers");
        let _survivor = registry
            .try_register_authorized(
                document_id,
                object_id,
                Uuid::new_v4(),
                workspace_id,
                survivor_session_id,
                3,
            )
            .expect("survivor fixture registers");
        for session_id in [revoked_session_id, survivor_session_id] {
            registry
                .upsert_presence(
                    document_id,
                    session_id,
                    json!({"cursor": session_id}),
                    Duration::from_secs(30),
                )
                .expect("presence is accepted");
        }

        let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, 4);
        let revoked_candidate = candidates
            .into_iter()
            .find(|candidate| candidate.session_id == revoked_session_id)
            .expect("reverse index finds the revoked session");

        assert_eq!(registry.remove_presence_for_sessions(&[revoked_candidate]), 1);
        assert_eq!(registry.presence_count(document_id), 1);
        assert!(
            revoked.receiver.try_recv().is_err(),
            "presence deletion must happen before and independently of close delivery"
        );

        assert_eq!(
            registry.disconnect_authorization_sessions(&[revoked_candidate], 4403, "authorization revoked"),
            1
        );
        assert_eq!(
            registry.object_index_session_count(object_id),
            1,
            "disconnect must remove exactly the revoked reverse-index member"
        );
        registry.unregister(document_id, revoked_session_id);
        assert_eq!(
            registry.object_index_session_count(object_id),
            1,
            "the session loop's later unregister must keep the already-disconnected path idempotent"
        );
        let close_event = revoked.receiver.try_recv().expect("close is queued");
        let OutboundEvent::Close { code, reason } = close_event else {
            panic!("expected an authorization close")
        };
        assert_eq!(code, 4403);
        assert_eq!(reason, "authorization revoked");
        assert!(!reason.contains(&object_id.to_string()));
        assert!(!reason.contains(&revoked_session_id.to_string()));

        registry.broadcast(
            document_id,
            &Frame::Presence {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                session_id: survivor_session_id,
                payload: json!({"cursor": "later"}),
                ttl_seconds: Some(30),
            },
            Some(survivor_session_id),
        );
        assert!(
            revoked.receiver.try_recv().is_err(),
            "a revoked session removed from the document index must receive no later presence fan-out"
        );
        assert_eq!(registry.session_count(document_id), 1);

        registry.unregister(document_id, survivor_session_id);
        assert_eq!(
            registry.object_index_session_count(object_id),
            0,
            "the last live session must remove the reverse-index bucket"
        );
    }
}
