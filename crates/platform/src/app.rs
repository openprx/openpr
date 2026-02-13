use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub cfg: AppConfig,
    pub db: DatabaseConnection,
}

pub async fn connect_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut opts = ConnectOptions::new(database_url.to_string());
    opts.max_connections(20)
        .min_connections(2)
        .connect_timeout(std::time::Duration::from_secs(5))
        .idle_timeout(std::time::Duration::from_secs(30))
        .acquire_timeout(std::time::Duration::from_secs(5));

    Database::connect(opts).await
}
