//! The single-instance session registry: which sessions have which document open, for presence
//! fan-out and for pushing `accepted`/`resync`/forced-close frames to every session watching a
//! document.
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

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::frame::Frame;
use super::limits::{PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX};

/// What a session's outbound writer task actually receives; the writer decides how to encode each
/// variant onto the socket.
pub enum OutboundEvent {
    Frame(Box<Frame>),
    Close { code: u16, reason: String },
}

#[derive(Clone)]
pub struct SessionHandle {
    pub session_id: Uuid,
    sender: mpsc::UnboundedSender<OutboundEvent>,
}

impl SessionHandle {
    /// Best-effort send: a session whose outbound channel is gone (already disconnecting) is
    /// silently skipped rather than treated as an error — the registry does not own the socket's
    /// lifecycle, the session loop does.
    fn send(&self, event: OutboundEvent) {
        let _ = self.sender.send(event);
    }
}

pub struct PresenceEntry {
    pub payload: Value,
    expires_at: Instant,
}

#[derive(Default)]
pub struct SessionRegistry {
    // document_id -> session_id -> handle
    sessions: Mutex<HashMap<Uuid, HashMap<Uuid, SessionHandle>>>,
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

    /// Registers a session as having `document_id` open, returning the channel its outbound
    /// writer task should drain.
    #[must_use]
    pub fn register(&self, document_id: Uuid, session_id: Uuid) -> mpsc::UnboundedReceiver<OutboundEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.sessions
            .lock()
            .entry(document_id)
            .or_default()
            .insert(session_id, SessionHandle { session_id, sender });
        receiver
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
        self.presence.lock().remove(&(document_id, session_id));
    }

    /// Sends `frame` to every session with `document_id` open other than `exclude`.
    ///
    /// Order note: this instance's write path only ever calls this once per document, strictly
    /// after that write's transaction commits, and the per-document coordinator (`coordinator.rs`)
    /// serializes writers for the same document within this instance — so commits (and therefore
    /// these broadcasts) happen in commit order with no reordering possible to guard against here.
    /// A multi-instance deployment would need the cross-instance ordered egress channel
    /// `ADR-0010` defers past v0.4; this module's docs already flag that as not implemented.
    #[allow(clippy::significant_drop_tightening)] // the guard must outlive the loop it feeds
    pub fn broadcast(&self, document_id: Uuid, frame: &Frame, exclude: Option<Uuid>) {
        let sessions = self.sessions.lock();
        let Some(by_session) = sessions.get(&document_id) else {
            return;
        };
        for (session_id, handle) in by_session {
            if Some(*session_id) != exclude {
                handle.send(OutboundEvent::Frame(Box::new(frame.clone())));
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
        PRESENCE_ENTRIES_PER_CONNECTION_MAX, PRESENCE_ENTRIES_PER_DOCUMENT_MAX, PresenceLimit, SessionRegistry,
    };
    use serde_json::json;
    use std::time::Duration;
    use uuid::Uuid;

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
        let session_id = Uuid::new_v4();
        let _receiver = registry.register(document_id, session_id);
        registry
            .upsert_presence(document_id, session_id, json!({}), Duration::from_secs(30))
            .expect("upserts");
        assert_eq!(registry.session_count(document_id), 1);

        registry.unregister(document_id, session_id);
        assert_eq!(registry.session_count(document_id), 0);
        assert_eq!(registry.presence_count(document_id), 0);
    }
}
