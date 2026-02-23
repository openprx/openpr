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
    pub fn success(data: T) -> Json<ApiResponse<T>> {
        Json(Self {
            code: 0,
            message: "success".into(),
            data: Some(data),
        })
    }
}

impl ApiResponse<()> {
    pub fn error(code: i32, msg: impl Into<String>) -> Json<ApiResponse<()>> {
        Json(Self {
            code,
            message: msg.into(),
            data: None,
        })
    }

    pub fn ok() -> Json<ApiResponse<()>> {
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
    pub fn from_items(items: Vec<T>) -> Self {
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
