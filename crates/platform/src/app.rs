use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::{AppConfig, DatabaseRuntime};

#[derive(Clone)]
pub struct AppState {
    pub cfg: AppConfig,
    pub db: DatabaseConnection,
    /// Per-process Flow effective-permission cache, type-erased here to keep the generic
    /// `platform` crate independent of the API crate that owns the authorization types.
    pub flow_permission_cache: FlowPermissionCacheSlot,
}

/// A cloneable, per-[`AppState`] slot for the API crate's effective-permission cache.
///
/// The slot is deliberately not a global registry. The concrete cache remains in
/// `apps/api/src/flow/collab/permission_cache.rs`; type erasure avoids a dependency from this
/// lower-level crate back to the API crate.
#[derive(Clone, Default)]
pub struct FlowPermissionCacheSlot {
    inner: Arc<Mutex<Option<Arc<dyn Any + Send + Sync>>>>,
}

impl FlowPermissionCacheSlot {
    /// Returns the slot's concrete service, initializing it exactly once. A type mismatch is
    /// reported as `None` so callers can fail closed instead of panicking on an internal wiring
    /// error.
    pub fn get_or_init<T, F>(&self, init: F) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> T,
    {
        let mut stored = self.inner.lock();
        if let Some(service) = stored.as_ref() {
            return Arc::clone(service).downcast::<T>().ok();
        }
        let service = Arc::new(init());
        *stored = Some(service.clone());
        drop(stored);
        Some(service)
    }
}

/// Opens the connection pool described by the `[database]` section.
///
/// The pool shape used to be hardcoded here; it now comes from the configuration file so a
/// deployment can size it without a rebuild. Takes a [`DatabaseRuntime`] rather than the raw
/// section so that "the file names a database" is settled by
/// [`crate::config::OpenPrConfig::database_runtime`] before anything tries to connect.
pub async fn connect_db(database: &DatabaseRuntime) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut opts = ConnectOptions::new(database.url.expose().to_string());
    opts.max_connections(database.max_connections)
        .min_connections(database.min_connections)
        .connect_timeout(Duration::from_secs(database.connect_timeout_seconds))
        .idle_timeout(Duration::from_secs(database.idle_timeout_seconds))
        .acquire_timeout(Duration::from_secs(database.acquire_timeout_seconds));

    Database::connect(opts).await
}
