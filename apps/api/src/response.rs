// The explicit public type contract remains stable without broadening or replacing its API.
// Collection length retains the established signed pagination representation.
#![allow(clippy::cast_possible_wrap, clippy::trait_duplication_in_bounds)]

use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: Some(data),
        })
    }
}

impl ApiResponse<()> {
    pub fn error(code: i32, msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            code,
            message: msg.into(),
            data: None,
        })
    }

    pub fn ok() -> Json<Self> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: None,
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
