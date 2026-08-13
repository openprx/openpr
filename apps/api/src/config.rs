//! The configuration sections the API reads outside of `AppState`.
//!
//! [`AppState`](platform::app::AppState) carries the database handle and the handful of values
//! every request handler needs. Three sections do not fit there: object storage, the outbound
//! allowlist and the connector credentials are read from code paths that are several calls away
//! from an `AppState`, and from free functions the worker also links against.
//!
//! They used to be read from the process environment at the point of use, which is precisely what
//! made them reachable from anywhere. This module keeps that reach without keeping the
//! environment: `main` installs the sections once, before the listener is bound, and
//! [`runtime`] hands out a shared borrow of them afterwards.
//!
//! Connector credentials are deliberately *not* read through [`runtime`] by the delivery code.
//! They are passed as an explicit `&ConnectorSecrets` argument together with the workspace id that
//! selects the tenant partition, so the tenant boundary is visible in every signature that crosses
//! it rather than hidden behind a global.

use std::sync::OnceLock;

use platform::config::{
    ConnectorsConfig, DEFAULT_STORAGE_DIR, OpenPrConfig, OutboundConfig, StorageBackend, StorageConfig,
};

/// The sections of the configuration file reached from outside `AppState`.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub storage: StorageConfig,
    pub outbound: OutboundConfig,
    pub connectors: ConnectorsConfig,
}

impl RuntimeConfig {
    /// Projects a validated configuration file onto the sections this module publishes.
    pub fn from_config(config: &OpenPrConfig) -> Self {
        Self {
            storage: config.storage.clone(),
            outbound: config.outbound.clone(),
            connectors: config.connectors.clone(),
        }
    }

    /// The settings a process that never called [`install`] runs with.
    ///
    /// Only unit tests reach this: `main` installs the file before it binds a socket, and a
    /// failure to do so aborts startup. The values are the safe end of each setting — local
    /// storage under [`DEFAULT_STORAGE_DIR`], an empty outbound allowlist with the private
    /// address checks on, and no connector credentials at all.
    fn fallback() -> Self {
        Self {
            storage: StorageConfig {
                backend: StorageBackend::Local,
                dir: DEFAULT_STORAGE_DIR.into(),
                s3: None,
            },
            outbound: OutboundConfig::default(),
            connectors: ConnectorsConfig::default(),
        }
    }
}

static RUNTIME: OnceLock<RuntimeConfig> = OnceLock::new();

/// Publishes the configuration file for the rest of the process.
///
/// Called once by `main`, before anything can serve a request. A second call is an error rather
/// than a silent no-op: two different configurations in one process would mean one half of the
/// service runs on settings the operator cannot see.
pub fn install(config: &OpenPrConfig) -> Result<(), String> {
    RUNTIME
        .set(RuntimeConfig::from_config(config))
        .map_err(|_| "configuration has already been installed for this process".to_string())
}

/// The installed configuration, or the fallback described on [`RuntimeConfig::fallback`].
pub fn runtime() -> &'static RuntimeConfig {
    RUNTIME.get_or_init(RuntimeConfig::fallback)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{RuntimeConfig, runtime};
    use platform::config::StorageBackend;

    #[test]
    fn the_fallback_is_the_safe_end_of_every_setting() {
        let fallback = RuntimeConfig::fallback();
        assert_eq!(fallback.storage.backend, StorageBackend::Local);
        assert!(fallback.storage.s3.is_none());
        assert!(!fallback.outbound.allow_private);
        assert!(fallback.outbound.allowlist_csv().is_empty());
        assert_eq!(fallback.connectors.secrets.entry_count(), 0);
    }

    #[test]
    fn runtime_is_readable_without_an_installed_file() {
        // The unit test process never calls `install`, so this exercises the fallback path the
        // rest of the test suite depends on.
        assert_eq!(runtime().storage.backend, StorageBackend::Local);
    }
}
