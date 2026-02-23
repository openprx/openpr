use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use thiserror::Error;

use crate::response::ApiResponse;

pub fn request_lang(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("zh")
}

pub fn localize_error(msg: &str, lang: &str) -> String {
    if lang.starts_with("en") {
        match msg {
            "content is required" => "Content is required".to_string(),
            "content cannot be empty" => "Content cannot be empty".to_string(),
            "issue not found or access denied" => "Issue not found or access denied".to_string(),
            _ => msg.to_string(),
        }
    } else {
        match msg {
            "content is required" => "内容不能为空".to_string(),
            "content cannot be empty" => "内容不能为空".to_string(),
            "issue not found or access denied" => "Issue 未找到或无权限".to_string(),
            _ => msg.to_string(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("internal server error")]
    Internal,
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (code, message) = match self {
            Self::BadRequest(msg) => (400, msg),
            Self::Unauthorized(msg) => (401, msg),
            Self::Forbidden(msg) => (403, msg),
            Self::NotFound(msg) => (404, msg),
            Self::Conflict(msg) => (409, msg),
            Self::Internal => (500, "internal server error".to_string()),
            Self::Database(_) => (500, "database error".to_string()),
        };

        (StatusCode::OK, ApiResponse::error(code, &message)).into_response()
    }
}
