//! The process-wide collab runtime: the warm cache, the document coordinator, and the session
//! registry.
//!
//! These three are genuinely per-*process* state (`ADR-0010`: the warm cache and coordinator are
//! instance-local by design; the session registry is this instance's slice of "who has what
//! open"), unlike everything else this crate threads through [`platform::app::AppState`] — so
//! they live behind a `OnceLock`, the same shape `crate::config::runtime` already uses for the
//! configuration file. A handler reaches them via [`runtime`] rather than through `AppState`,
//! which keeps `AppState`'s many existing hand-built test literals (`apps/api/src/routes/*.rs`)
//! unchanged: adding a field there would mean touching every one of them for state that has
//! nothing to do with a database connection or a config file.
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use parking_lot::Mutex;
use uuid::Uuid;

use crate::error::{ApiError, ServerDrainingReason};

use super::cache::WarmCache;
use super::coordinator::DocumentCoordinator;
use super::frame::DrainSignal;
use super::registry::SessionRegistry;
use super::snapshot::SnapshotAdvancer;

#[derive(Default)]
pub struct CollabRuntime {
    pub cache: WarmCache,
    pub coordinator: DocumentCoordinator,
    pub registry: SessionRegistry,
    /// Gate 7 `minimal_snapshot_advancement_bounds_tail` bookkeeping (in-flight background
    /// advancements, last measured rebuild wall time per document). See [`super::snapshot`].
    pub snapshot: SnapshotAdvancer,
    drains: WorkspaceDrains,
}

#[derive(Debug, Default)]
struct DrainEntry {
    retry_holders: BTreeMap<u64, usize>,
}

#[derive(Default)]
struct WorkspaceDrains {
    entries: Mutex<HashMap<Uuid, DrainEntry>>,
}

/// A controlled, workspace-scoped drain producer.
///
/// It is shared by REST, MCP/CLI (through the same REST application services), and WebSocket/UI.
/// Keeping this guard alive makes every Flow surface
/// return `server_draining{reason:"drain"}` for this workspace; dropping the final overlapping
/// guard admits new work again. Existing WebSocket sessions are closed when the guard is created.
///
/// The guard is intentionally process-local, matching the process-local session registry.
pub struct WorkspaceDrainGuard<'a> {
    drains: &'a WorkspaceDrains,
    workspace_id: Uuid,
    retry_after_ms: u64,
}

impl Drop for WorkspaceDrainGuard<'_> {
    fn drop(&mut self) {
        let mut entries = self.drains.entries.lock();
        let should_remove = entries.get_mut(&self.workspace_id).is_some_and(|entry| {
            let remove_retry = entry
                .retry_holders
                .get_mut(&self.retry_after_ms)
                .is_some_and(|holders| {
                    *holders = holders.saturating_sub(1);
                    *holders == 0
                });
            if remove_retry {
                entry.retry_holders.remove(&self.retry_after_ms);
            }
            entry.retry_holders.is_empty()
        });
        if should_remove {
            entries.remove(&self.workspace_id);
        }
    }
}

impl CollabRuntime {
    /// Starts the shared producer fixture for one workspace and immediately drains its live WS
    /// sessions. The returned guard is the fixture lifetime; callers must retain it until the
    /// maintenance/drain condition has ended.
    #[must_use]
    pub fn begin_workspace_drain(&self, workspace_id: Uuid, retry_after_ms: u64) -> WorkspaceDrainGuard<'_> {
        let mut entries = self.drains.entries.lock();
        let entry = entries.entry(workspace_id).or_default();
        let holders = entry.retry_holders.entry(retry_after_ms).or_default();
        *holders = holders.saturating_add(1);
        drop(entries);
        self.registry.drain_workspace(workspace_id, retry_after_ms);
        WorkspaceDrainGuard {
            drains: &self.drains,
            workspace_id,
            retry_after_ms,
        }
    }

    #[must_use]
    pub fn workspace_drain_retry_after_ms(&self, workspace_id: Uuid) -> Option<u64> {
        self.drains
            .entries
            .lock()
            .get(&workspace_id)
            .and_then(|entry| entry.retry_holders.last_key_value().map(|(retry, _)| *retry))
    }

    #[must_use]
    pub fn workspace_drain_signal(&self, workspace_id: Uuid) -> Option<DrainSignal> {
        self.workspace_drain_retry_after_ms(workspace_id).map(DrainSignal::new)
    }

    /// Shared admission check used by every non-WS Flow application service. Since MCP HTTP,
    /// SSE, stdio, and the native CLI all call these REST services, one producer yields the same
    /// structured reason on every surface rather than manufacturing transport-specific errors.
    pub fn ensure_workspace_accepting(&self, workspace_id: Uuid) -> Result<(), ApiError> {
        let Some(signal) = self.workspace_drain_signal(workspace_id) else {
            return Ok(());
        };
        Err(ApiError::server_draining(
            ServerDrainingReason::Drain,
            signal.retry_after_ms,
            "server_draining",
        ))
    }
}

static RUNTIME: OnceLock<CollabRuntime> = OnceLock::new();

/// The process-wide collab runtime, created empty on first use.
pub fn runtime() -> &'static CollabRuntime {
    RUNTIME.get_or_init(CollabRuntime::default)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use serde_json::Value;
    use uuid::Uuid;

    use super::CollabRuntime;
    use crate::flow::collab::registry::OutboundEvent;

    #[tokio::test]
    async fn shared_workspace_drain_fixture_produces_rest_and_ws_drain_wire_shapes() {
        let runtime = CollabRuntime::default();
        let workspace_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let mut registered = runtime
            .registry
            .try_register(document_id, Uuid::new_v4(), workspace_id, Uuid::new_v4())
            .expect("session registers");

        let guard = runtime.begin_workspace_drain(workspace_id, 1_500);

        let rejected = registered.receiver.recv().await.expect("active session gets rejection");
        let OutboundEvent::ControlFrame(frame) = rejected else {
            panic!("expected drain rejection before close");
        };
        let crate::flow::collab::frame::Frame::Rejected { details, .. } = *frame else {
            panic!("expected rejected frame");
        };
        assert_eq!(details.expect("details")["reason"], "drain");

        let event = registered.receiver.recv().await.expect("active session gets a close");
        let OutboundEvent::Close { code, reason } = event else {
            panic!("expected close");
        };
        assert_eq!(code, 4410);
        let ws_details: Value = serde_json::from_str(&reason).expect("close reason JSON");
        assert_eq!(ws_details["reason"], "drain");
        assert_eq!(ws_details["retry_after_ms"], 1_500);

        let response = runtime
            .ensure_workspace_accepting(workspace_id)
            .expect_err("draining workspace rejects REST service")
            .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), 32_768).await.expect("body reads");
        let rest: Value = serde_json::from_slice(&body).expect("REST body JSON");
        assert_eq!(rest["error_code"], "server_draining");
        assert_eq!(rest["details"]["reason"], "drain");
        assert_eq!(rest["details"]["retry_after_ms"], 1_500);

        drop(guard);
        assert!(runtime.ensure_workspace_accepting(workspace_id).is_ok());
    }

    #[test]
    fn overlapping_drain_guards_publish_the_largest_live_retry_and_release_independently() {
        let runtime = CollabRuntime::default();
        let workspace_id = Uuid::new_v4();
        let short = runtime.begin_workspace_drain(workspace_id, 250);
        let long = runtime.begin_workspace_drain(workspace_id, 2_000);
        assert_eq!(runtime.workspace_drain_retry_after_ms(workspace_id), Some(2_000));

        drop(long);
        assert_eq!(runtime.workspace_drain_retry_after_ms(workspace_id), Some(250));
        drop(short);
        assert_eq!(runtime.workspace_drain_retry_after_ms(workspace_id), None);
    }
}
