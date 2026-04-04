use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
}

pub type AppResult<T> = Result<T, AppError>;

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

    /// Config 错误的 Display 格式应包含 "configuration error:" 前缀和消息体
    #[test]
    fn test_config_error_display() {
        let err = AppError::Config("DATABASE_URL is required".to_string());
        let display = err.to_string();
        assert!(
            display.contains("configuration error:"),
            "Display 应含 'configuration error:'，实际: {display}"
        );
        assert!(
            display.contains("DATABASE_URL is required"),
            "Display 应含原始消息，实际: {display}"
        );
    }

    /// From<DbErr> 应能将 sea_orm::DbErr 转换为 AppError::Database
    #[test]
    fn test_database_error_from_sea_orm() {
        let db_err = sea_orm::DbErr::Custom("connection refused".to_string());
        let app_err: AppError = AppError::from(db_err);
        assert!(
            matches!(app_err, AppError::Database(_)),
            "From<DbErr> 应产生 AppError::Database，实际: {app_err:?}"
        );
    }

    /// AppResult::Ok 路径应能正常解包
    #[test]
    fn test_app_result_ok() {
        let result: AppResult<i32> = Ok(42);
        assert!(result.is_ok());
        assert_eq!(result.ok(), Some(42));
    }

    /// 错误通过 ? 传播时，调用方应收到同一 AppError 变体
    #[test]
    fn test_app_result_err_propagation() {
        fn produce_error() -> AppResult<()> {
            Err(AppError::Config("propagation test".to_string()))
        }

        fn caller() -> AppResult<String> {
            produce_error()?;
            Ok("reached".to_string())
        }

        let result = caller();
        assert!(result.is_err(), "错误应向上传播");
        assert!(
            matches!(result, Err(AppError::Config(_))),
            "传播后变体应保持 Config"
        );
    }

    /// Config 错误中的消息字符串应被完整保留，不截断也不改写
    #[test]
    fn test_config_error_message_preserved() {
        let long_msg = "a".repeat(512);
        let err = AppError::Config(long_msg.clone());
        let display = err.to_string();
        assert!(
            display.contains(&long_msg),
            "512 字符长消息应完整保留在 Display 输出中"
        );
    }
}
