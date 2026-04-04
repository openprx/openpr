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
        let database_url =
            env::var("DATABASE_URL").map_err(|_| AppError::Config("DATABASE_URL is required".to_string()))?;
        let jwt_secret = env::var("JWT_SECRET").map_err(|_| AppError::Config("JWT_SECRET is required".to_string()))?;
        let jwt_access_ttl_seconds = env::var("JWT_ACCESS_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(1_296_000);
        let jwt_refresh_ttl_seconds = env::var("JWT_REFRESH_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(1_728_000);
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;

    /// 所有必填 + 可选 env var 均设置时，字段值应完整读取
    #[test]
    fn test_from_env_all_required_present() {
        temp_env::with_vars(
            [
                ("APP_NAME", Some("MyApp")),
                ("BIND_ADDR", Some("0.0.0.0:9000")),
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("super-secret")),
                ("JWT_ACCESS_TTL_SECONDS", Some("7200")),
                ("JWT_REFRESH_TTL_SECONDS", Some("86400")),
                ("DEFAULT_AUTHOR_ID", Some("550e8400-e29b-41d4-a716-446655440000")),
            ],
            || {
                let cfg = AppConfig::from_env("default", "127.0.0.1:8080").expect("应成功");
                assert_eq!(cfg.app_name, "MyApp");
                assert_eq!(cfg.bind_addr, "0.0.0.0:9000");
                assert_eq!(cfg.database_url, "postgres://localhost/db");
                assert_eq!(cfg.jwt_secret, "super-secret");
                assert_eq!(cfg.jwt_access_ttl_seconds, 7200);
                assert_eq!(cfg.jwt_refresh_ttl_seconds, 86400);
                assert!(cfg.default_author_id.is_some());
                assert_eq!(
                    cfg.default_author_id.unwrap().to_string(),
                    "550e8400-e29b-41d4-a716-446655440000"
                );
            },
        );
    }

    /// 缺少 DATABASE_URL 时应返回 Config 错误
    #[test]
    fn test_from_env_missing_database_url() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", None::<&str>),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let result = AppConfig::from_env("app", "0.0.0.0:8080");
                assert!(result.is_err(), "缺少 DATABASE_URL 应返回 Err");
                let err_msg = result.unwrap_err().to_string();
                assert!(
                    err_msg.contains("DATABASE_URL"),
                    "错误信息应包含 DATABASE_URL，实际: {err_msg}"
                );
            },
        );
    }

    /// 缺少 JWT_SECRET 时应返回 Config 错误
    #[test]
    fn test_from_env_missing_jwt_secret() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", None::<&str>),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let result = AppConfig::from_env("app", "0.0.0.0:8080");
                assert!(result.is_err(), "缺少 JWT_SECRET 应返回 Err");
                let err_msg = result.unwrap_err().to_string();
                assert!(
                    err_msg.contains("JWT_SECRET"),
                    "错误信息应包含 JWT_SECRET，实际: {err_msg}"
                );
            },
        );
    }

    /// 未设置 APP_NAME 时应使用传入的 default_name 参数
    #[test]
    fn test_from_env_default_app_name() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let cfg = AppConfig::from_env("FallbackName", "0.0.0.0:8080").expect("应成功");
                assert_eq!(cfg.app_name, "FallbackName");
            },
        );
    }

    /// 未设置 BIND_ADDR 时应使用传入的 default_bind 参数
    #[test]
    fn test_from_env_default_bind_addr() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:3000").expect("应成功");
                assert_eq!(cfg.bind_addr, "0.0.0.0:3000");
            },
        );
    }

    /// 未设置 TTL 时应回退到硬编码默认值 1_296_000 / 1_728_000
    #[test]
    fn test_from_env_default_ttl_values() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:8080").expect("应成功");
                assert_eq!(cfg.jwt_access_ttl_seconds, 1_296_000);
                assert_eq!(cfg.jwt_refresh_ttl_seconds, 1_728_000);
            },
        );
    }

    /// 自定义 TTL 值应被正确解析
    #[test]
    fn test_from_env_custom_ttl_values() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", Some("3600")),
                ("JWT_REFRESH_TTL_SECONDS", Some("604800")),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:8080").expect("应成功");
                assert_eq!(cfg.jwt_access_ttl_seconds, 3600);
                assert_eq!(cfg.jwt_refresh_ttl_seconds, 604_800);
            },
        );
    }

    /// TTL 值为非数字字符串时应静默回退到默认值
    #[test]
    fn test_from_env_invalid_ttl_falls_back() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", Some("abc")),
                ("JWT_REFRESH_TTL_SECONDS", Some("xyz")),
                ("DEFAULT_AUTHOR_ID", None::<&str>),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:8080").expect("应成功");
                assert_eq!(
                    cfg.jwt_access_ttl_seconds, 1_296_000,
                    "非法 access TTL 应回退默认值"
                );
                assert_eq!(
                    cfg.jwt_refresh_ttl_seconds, 1_728_000,
                    "非法 refresh TTL 应回退默认值"
                );
            },
        );
    }

    /// 合法 UUID 字符串应被正确解析为 Some(Uuid)
    #[test]
    fn test_from_env_valid_default_author_id() {
        let uuid_str = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", Some(uuid_str)),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:8080").expect("应成功");
                assert!(cfg.default_author_id.is_some(), "合法 UUID 应被解析");
                assert_eq!(
                    cfg.default_author_id.unwrap().to_string(),
                    uuid_str
                );
            },
        );
    }

    /// 无效 UUID 字符串时 default_author_id 应静默为 None（不报错）
    #[test]
    fn test_from_env_invalid_author_id_ignored() {
        temp_env::with_vars(
            [
                ("DATABASE_URL", Some("postgres://localhost/db")),
                ("JWT_SECRET", Some("secret")),
                ("APP_NAME", None::<&str>),
                ("BIND_ADDR", None::<&str>),
                ("JWT_ACCESS_TTL_SECONDS", None::<&str>),
                ("JWT_REFRESH_TTL_SECONDS", None::<&str>),
                ("DEFAULT_AUTHOR_ID", Some("not-a-valid-uuid")),
            ],
            || {
                let cfg = AppConfig::from_env("app", "0.0.0.0:8080").expect("无效 UUID 不应报错");
                assert!(cfg.default_author_id.is_none(), "无效 UUID 应静默为 None");
            },
        );
    }
}
