use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::{AppConfig, DatabaseRuntime};

#[derive(Clone)]
pub struct AppState {
    pub cfg: AppConfig,
    pub db: DatabaseConnection,
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
