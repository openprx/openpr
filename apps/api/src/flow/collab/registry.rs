//! The single-instance session registry: which sessions have which document open, for presence
//! fan-out and for pushing `accepted`/`resync`/forced-close frames to every session watching a
//! document; and, since this package's limits-enforcement pass, the accounting for
//! `limits-v1.md`'s connection ceilings (`connections_per_user_max`/`_per_document_max`/
//! `_per_workspace_max`) and slow-consumer outbound queue ceiling
//! (`slow_consumer_queue_frames_max`/`_bytes_max`).
//!
//! Scope note (documented rather than silently short of the full `ADR-0012` §5 mechanism): §5
//! calls for (a) a WS server + session registry, (b) an `object_id → active sessions` index, and
//! (c) subtree membership (`recursive CTE`) with **cross-instance** broadcast, wired to fire on
//! every authorization-affecting write. This module is (a), and — since v0.4 has exactly one
//! `collab_documents` row per `flow_objects` row — `document_id` doubles as (b)'s index with no
//! separate table. It does **not** implement (c): there is no cross-instance broadcast channel
//! (`ADR-0010`'s "ordered egress / invalidation 通道" is itself deferred past v0.4 by that ADR),
//! and no subtree-recompute trigger exists because v0.4 ships no grant/inheritance/membership
//! write path to trigger it from (`ADR-0012` marks that whole surface v0.5). What *is* real here —
//! [`SessionRegistry::disconnect_document`] — is the mechanism a v0.5 subtree-revocation path will
//! call once it exists; today nothing calls it automatically. Correctness never depends on this
//! module: `authz::fence_epoch_for_share` is the actual barrier (`collab-protocol-v1.md`: "撤连是
//! 尽力而为的及时性手段;正确性由 commit-time fencing 保证"). This registry is that timeliness
//! mechanism's single-instance slice, not the correctness guarantee.
//!
//! [`SessionRegistry::drain_all`]/[`SessionRegistry::drain_workspace`] carry the same
//! documented-gap shape: they are the tested, callable `server_draining{reason:"drain"}` primitive
//! (`collab-protocol-v1.md` "服务端主动排空"), but v0.4 has no instance-shutdown hook or
//! workspace-admin drain endpoint anywhere in this codebase to call them from yet.

#![allow(clippy::too_long_first_doc_paragraph, clippy::struct_field_names)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::frame::Frame;
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

/// Builds the drain close reason (`collab-protocol-v1.md`: "UTF-8 close reason 是只含这两个安全字段
/// 的 JSON且 reason=drain"). Deliberately hand-built rather than `serde_json::json!` + `to_string`:
/// this is the one place in the whole package where a value gets serialized directly into a
/// user-visible wire field with no schema type to statically guarantee it stays limited to these
/// two safe fields, so the shape is spelled out here rather than assembled from a `Value` that a
/// future edit could accidentally grow.
fn drain_close_reason(retry_after_ms: u64) -> String {
    format!(r#"{{"reason":"drain","retry_after_ms":{retry_after_ms}}}"#)
}

/// What a session's outbound writer task actually receives; the writer decides how to encode each
/// variant onto the socket. `Frame`'s `usize` is that frame's encoded byte length at send time —
/// charged against [`QueueState`] by [`SessionHandle::deliver`] and released by the session loop's
/// own [`RegisteredSession::record_dequeued`] once it has been taken off this channel.
pub enum OutboundEvent {
    Frame(Box<Frame>, usize),
    Close { code: u16, reason: String },
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
        if u64::try_from(frames).unwrap_or(u64::MAX) > SLOW_CONSUMER_QUEUE_FRAMES_MAX
            || u64::try_from(bytes).unwrap_or(u64::MAX) > SLOW_CONSUMER_QUEUE_BYTES_MAX
        {
            // Roll back this frame's own contribution — it is never actually sent — then close.
            self.queue.frames.fetch_sub(1, Ordering::AcqRel);
            self.queue.bytes.fetch_sub(encoded_len, Ordering::AcqRel);
            if !self.queue.close_sent.swap(true, Ordering::AcqRel) {
                self.send(OutboundEvent::Close {
                    code: SLOW_CONSUMER_CLOSE_CODE,
                    reason: "slow consumer: outbound queue limit exceeded".to_string(),
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

/// Per-session connection metadata (`limits-v1.md`'s `user_connections`/`document_connections`/
/// `workspace_connections` accounting dimensions), keyed by `session_id` so
/// [`SessionRegistry::try_register`]/[`SessionRegistry::unregister`] can maintain it without a
/// second index.
struct ConnMeta {
    document_id: Uuid,
    user_id: Uuid,
    workspace_id: Uuid,
}

/// Which `limits-v1.md` connection ceiling a [`SessionRegistry::try_register`] admission refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionLimit {
    PerUser,
    PerDocument,
    PerWorkspace,
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

/// What [`SessionRegistry::try_register`] hands back on success: the channel the session's
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
    // session_id -> connection metadata (`limits-v1.md` connection-ceiling accounting)
    connections: Mutex<HashMap<Uuid, ConnMeta>>,
    // (document_id, session_id) -> presence
    presence: Mutex<HashMap<(Uuid, Uuid), PresenceEntry>>,
}

/// Why a `presence` upsert was refused (`limits-v1.md`'s two independent presence ceilings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceLimit {
    PerConnection,
    PerDocument,
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
    /// The specific ceiling that was hit, so the caller can map it to the matching `limit_kind`.
    #[allow(clippy::significant_drop_tightening)] // guard held across check-then-insert by design
    pub fn try_register(
        &self,
        document_id: Uuid,
        user_id: Uuid,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<RegisteredSession, ConnectionLimit> {
        let mut connections = self.connections.lock();
        let per_user = connections.values().filter(|c| c.user_id == user_id).count();
        if u64::try_from(per_user).unwrap_or(u64::MAX) >= CONNECTIONS_PER_USER_MAX {
            return Err(ConnectionLimit::PerUser);
        }
        let per_document = connections.values().filter(|c| c.document_id == document_id).count();
        if u64::try_from(per_document).unwrap_or(u64::MAX) >= CONNECTIONS_PER_DOCUMENT_MAX {
            return Err(ConnectionLimit::PerDocument);
        }
        let per_workspace = connections.values().filter(|c| c.workspace_id == workspace_id).count();
        if u64::try_from(per_workspace).unwrap_or(u64::MAX) >= CONNECTIONS_PER_WORKSPACE_MAX {
            return Err(ConnectionLimit::PerWorkspace);
        }
        connections.insert(
            session_id,
            ConnMeta {
                document_id,
                user_id,
                workspace_id,
            },
        );
        drop(connections);

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
        Ok(RegisteredSession { receiver, queue })
    }

    #[allow(clippy::significant_drop_tightening)] // guard used for two related mutations, not idle
    pub fn unregister(&self, document_id: Uuid, session_id: Uuid) {
        let mut sessions = self.sessions.lock();
        if let Some(by_session) = sessions.get_mut(&document_id) {
            by_session.remove(&session_id);
            if by_session.is_empty() {
                sessions.remove(&document_id);
            }
        }
        drop(sessions);
        self.connections.lock().remove(&session_id);
        self.presence.lock().remove(&(document_id, session_id));
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

    /// Force-closes every session with `document_id` open (feature disabled, `flow_enabled`
    /// turned off, or — once a v0.5 caller exists — a subtree revocation). See the module doc
    /// comment for what "subtree" coverage this does and does not provide today.
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
    /// No production caller exists yet: v0.4 has no instance-shutdown hook or workspace-admin
    /// drain endpoint anywhere in this codebase (grepped clean) to call it from — the same
    /// documented-gap shape this module's own [`Self::disconnect_document`] already carries for
    /// its v0.5 subtree-revocation caller. This is the tested, callable primitive such a trigger
    /// wires into; see this module's own doc comment.
    pub fn drain_all(&self, retry_after_ms: u64) -> usize {
        let reason = drain_close_reason(retry_after_ms);
        let sessions = self.sessions.lock();
        let mut closed = 0usize;
        for by_session in sessions.values() {
            for handle in by_session.values() {
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
    /// 显式排空") — same no-production-caller status; see this module's own doc comment.
    pub fn drain_workspace(&self, workspace_id: Uuid, retry_after_ms: u64) -> usize {
        let member_sessions: HashSet<Uuid> = self
            .connections
            .lock()
            .iter()
            .filter(|(_, meta)| meta.workspace_id == workspace_id)
            .map(|(session_id, _)| *session_id)
            .collect();
        if member_sessions.is_empty() {
            return 0;
        }
        let reason = drain_close_reason(retry_after_ms);
        let sessions = self.sessions.lock();
        let mut closed = 0usize;
        for by_session in sessions.values() {
            for (session_id, handle) in by_session {
                if member_sessions.contains(session_id) {
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

    #[must_use]
    pub fn session_count(&self, document_id: Uuid) -> usize {
        self.sessions.lock().get(&document_id).map_or(0, HashMap::len)
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
    use super::{
        CONNECTIONS_PER_DOCUMENT_MAX, CONNECTIONS_PER_USER_MAX, CONNECTIONS_PER_WORKSPACE_MAX, ConnectionLimit,
        OutboundEvent, PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX, PresenceLimit,
        SLOW_CONSUMER_QUEUE_BYTES_MAX, SLOW_CONSUMER_QUEUE_FRAMES_MAX, SessionRegistry,
    };
    use crate::flow::collab::frame::{Frame, PROTOCOL_VERSION};
    use serde_json::json;
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
                OutboundEvent::Close { code, .. } => {
                    saw_close = true;
                    assert_eq!(code, 4408, "slow consumer must close at the frozen limit_exceeded code");
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
                OutboundEvent::Close { code, .. } => {
                    saw_close = true;
                    assert_eq!(code, 4408, "slow consumer must close at the frozen limit_exceeded code");
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

        assert!(
            matches!(target.receiver.try_recv(), Ok(OutboundEvent::Close { code: 4410, .. })),
            "the target workspace's session must be closed"
        );
        assert!(
            other.receiver.try_recv().is_err(),
            "a different workspace's session must not be touched"
        );
    }
}
