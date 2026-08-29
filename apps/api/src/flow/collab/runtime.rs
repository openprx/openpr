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
use std::sync::OnceLock;

use super::cache::WarmCache;
use super::coordinator::DocumentCoordinator;
use super::registry::SessionRegistry;

#[derive(Default)]
pub struct CollabRuntime {
    pub cache: WarmCache,
    pub coordinator: DocumentCoordinator,
    pub registry: SessionRegistry,
}

static RUNTIME: OnceLock<CollabRuntime> = OnceLock::new();

/// The process-wide collab runtime, created empty on first use.
pub fn runtime() -> &'static CollabRuntime {
    RUNTIME.get_or_init(CollabRuntime::default)
}
