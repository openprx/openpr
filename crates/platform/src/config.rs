use std::env;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub app_name: String,
    pub bind_addr: String,
    pub database_url: String,
}

impl AppConfig {
    pub fn from_env(default_name: &str, default_bind: &str) -> AppResult<Self> {
        let app_name = env::var("APP_NAME").unwrap_or_else(|_| default_name.to_string());
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| default_bind.to_string());
        let database_url = env::var("DATABASE_URL")
            .map_err(|_| AppError::Config("DATABASE_URL is required".to_string()))?;

        Ok(Self {
            app_name,
            bind_addr,
            database_url,
        })
    }
}
