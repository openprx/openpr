use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
}

pub type AppResult<T> = Result<T, AppError>;
