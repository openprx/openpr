use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
};
use std::env;
use platform::{app::AppState, auth::JwtClaims};
use serde::Serialize;
use tokio::fs;
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

const MAX_FILE_SIZE: usize = 200 * 1024 * 1024;
const DEFAULT_UPLOAD_DIR: &str = "./uploads";

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
    pub filename: String,
}

/// POST /api/v1/upload - Upload image/video files for markdown content
pub async fn upload_file(
    State(_state): State<AppState>,
    Extension(_claims): Extension<JwtClaims>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let upload_dir = upload_dir();
    fs::create_dir_all(&upload_dir)
        .await
        .map_err(|_| ApiError::Internal)?;

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::BadRequest("missing content-type header".to_string()))?;

    let boundary = parse_boundary(content_type)?;
    let part = extract_file_part(&body, &boundary)?;

    if part.data.is_empty() {
        return Err(ApiError::BadRequest("file is empty".to_string()));
    }
    if part.data.len() > MAX_FILE_SIZE {
        return Err(ApiError::BadRequest(
            "file size must be less than or equal to 200MB".to_string(),
        ));
    }

    let ext = match part.content_type.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-msvideo" => "avi",
        _ => {
            return Err(ApiError::BadRequest(
                "only png/jpg/gif/webp/mp4/webm/mov/avi are supported".to_string(),
            ));
        }
    };

    let file_name = format!("{}.{}", Uuid::new_v4(), ext);
    let file_path = format!("{}/{}", upload_dir, file_name);
    fs::write(&file_path, &part.data)
        .await
        .map_err(|_| ApiError::Internal)?;

    Ok(ApiResponse::success(UploadResponse {
        url: format!("/api/v1/uploads/{}", file_name),
        filename: file_name,
    }))
}

/// GET /uploads/:file_name - Serve uploaded file
pub async fn get_uploaded_file(
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
        return Err(ApiError::NotFound("file not found".to_string()));
    }

    let file_path = format!("{}/{}", upload_dir(), file_name);
    let data = fs::read(&file_path)
        .await
        .map_err(|_| ApiError::NotFound("file not found".to_string()))?;

    let content_type = if file_name.ends_with(".png") {
        HeaderValue::from_static("image/png")
    } else if file_name.ends_with(".jpg") || file_name.ends_with(".jpeg") {
        HeaderValue::from_static("image/jpeg")
    } else if file_name.ends_with(".gif") {
        HeaderValue::from_static("image/gif")
    } else if file_name.ends_with(".webp") {
        HeaderValue::from_static("image/webp")
    } else if file_name.ends_with(".mp4") {
        HeaderValue::from_static("video/mp4")
    } else if file_name.ends_with(".webm") {
        HeaderValue::from_static("video/webm")
    } else if file_name.ends_with(".mov") {
        HeaderValue::from_static("video/quicktime")
    } else if file_name.ends_with(".avi") {
        HeaderValue::from_static("video/x-msvideo")
    } else {
        HeaderValue::from_static("application/octet-stream")
    };

    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], data))
}

fn upload_dir() -> String {
    env::var("UPLOAD_DIR").unwrap_or_else(|_| DEFAULT_UPLOAD_DIR.to_string())
}

struct ParsedFilePart {
    content_type: String,
    data: Vec<u8>,
}

fn parse_boundary(content_type: &str) -> Result<String, ApiError> {
    if !content_type.starts_with("multipart/form-data") {
        return Err(ApiError::BadRequest(
            "content-type must be multipart/form-data".to_string(),
        ));
    }

    for segment in content_type.split(';') {
        let trimmed = segment.trim();
        if let Some(boundary) = trimmed.strip_prefix("boundary=") {
            if boundary.is_empty() {
                return Err(ApiError::BadRequest("invalid multipart boundary".to_string()));
            }
            return Ok(boundary.trim_matches('"').to_string());
        }
    }

    Err(ApiError::BadRequest(
        "missing multipart boundary".to_string(),
    ))
}

fn extract_file_part(body: &[u8], boundary: &str) -> Result<ParsedFilePart, ApiError> {
    let boundary_marker = format!("--{boundary}");
    let boundary_bytes = boundary_marker.as_bytes();

    let first_boundary = find_subslice(body, boundary_bytes)
        .ok_or_else(|| ApiError::BadRequest("invalid multipart payload".to_string()))?;
    let first_line_end = find_subslice(&body[first_boundary..], b"\r\n")
        .ok_or_else(|| ApiError::BadRequest("invalid multipart payload".to_string()))?
        + first_boundary;

    let headers_end = find_subslice(&body[first_line_end + 2..], b"\r\n\r\n")
        .ok_or_else(|| ApiError::BadRequest("invalid multipart payload".to_string()))?
        + first_line_end
        + 2;
    let header_bytes = &body[first_line_end + 2..headers_end];
    let header_text = String::from_utf8_lossy(header_bytes).to_string();

    if !header_text.contains("name=\"file\"") {
        return Err(ApiError::BadRequest(
            "missing file field in multipart form data".to_string(),
        ));
    }

    let content_type = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-type") {
                Some(value.trim().to_ascii_lowercase())
            } else {
                None
            }
        })
        .ok_or_else(|| ApiError::BadRequest("missing file content-type".to_string()))?;

    let data_start = headers_end + 4;
    let next_boundary_marker = format!("\r\n--{boundary}");
    let data_end_rel = find_subslice(&body[data_start..], next_boundary_marker.as_bytes())
        .ok_or_else(|| ApiError::BadRequest("invalid multipart payload".to_string()))?;
    let data_end = data_start + data_end_rel;

    Ok(ParsedFilePart {
        content_type,
        data: body[data_start..data_end].to_vec(),
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }

    haystack.windows(needle.len()).position(|window| window == needle)
}
