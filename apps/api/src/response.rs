// The explicit public type contract remains stable without broadening or replacing its API.
#![allow(clippy::trait_duplication_in_bounds)]

use axum::Json;
use serde::Serialize;
use serde_json::Value;

/// Metadata the bot-operation middleware can inspect without buffering a response body.
///
/// The summary is deliberately a fixed server string rather than handler prose, which can
/// contain caller-provided names or values that do not belong in the operation ledger.
#[derive(Debug, Clone, Copy)]
pub struct OperationResponseMeta {
    pub business_code: i32,
    pub error_summary: Option<&'static str>,
}

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// The stable `error-mapping-v1.md` discriminant for an [`crate::error::ApiError::Typed`]
    /// rejection (its `error_code` wire field) -- carried on this one envelope shape instead of
    /// [`crate::error::ApiError`]'s `Typed` arm hand-building a parallel JSON body. Always `None`
    /// on a success response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
    /// Structured, per-`error_code` extra fields (`error-mapping-v1.md`'s `details` column, e.g.
    /// `limit_exceeded`'s `{limit_kind,limit,observed?}` or `stale_frontier`'s
    /// `{current_seq,current_frontier}`). Always `None` on a success response and on any
    /// rejection whose stable code carries no extra structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: Some(data),
            error_code: None,
            details: None,
        })
    }
}

impl ApiResponse<()> {
    pub fn error(code: i32, msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            code,
            message: msg.into(),
            data: None,
            error_code: None,
            details: None,
        })
    }

    pub fn ok() -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: None,
            error_code: None,
            details: None,
        })
    }
}

#[derive(Serialize)]
pub struct PaginatedData<T: Serialize> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

impl<T: Serialize> PaginatedData<T> {
    pub const fn from_items(items: Vec<T>) -> Self {
        // Vec length cannot exceed isize::MAX, which fits in i64 on every supported target.
        #[allow(clippy::cast_possible_wrap)]
        let total = items.len() as i64;
        Self {
            items,
            total,
            page: 1,
            per_page: total,
            total_pages: 1,
        }
    }
}
