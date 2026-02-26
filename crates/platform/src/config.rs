use std::env;

use crate::error::{AppError, AppResult};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub app_name: String,
    pub bind_addr: String,
    pub database_url: String,
    pub jwt_secret: String,
    pub jwt_access_ttl_seconds: i64,
    pub jwt_refresh_ttl_seconds: i64,
    pub default_author_id: Option<Uuid>,
}

impl AppConfig {
    pub fn from_env(default_name: &str, default_bind: &str) -> AppResult<Self> {
        let app_name = env::var("APP_NAME").unwrap_or_else(|_| default_name.to_string());
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| default_bind.to_string());
        let database_url = env::var("DATABASE_URL")
            .map_err(|_| AppError::Config("DATABASE_URL is required".to_string()))?;
        let jwt_secret = env::var("JWT_SECRET")
            .map_err(|_| AppError::Config("JWT_SECRET is required".to_string()))?;
        let jwt_access_ttl_seconds = env::var("JWT_ACCESS_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(1296000);
        let jwt_refresh_ttl_seconds = env::var("JWT_REFRESH_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(1728000);
        let default_author_id = env::var("DEFAULT_AUTHOR_ID")
            .ok()
            .and_then(|v| Uuid::parse_str(&v).ok());

        Ok(Self {
            app_name,
            bind_addr,
            database_url,
            jwt_secret,
            jwt_access_ttl_seconds,
            jwt_refresh_ttl_seconds,
            default_author_id,
        })
    }
}
