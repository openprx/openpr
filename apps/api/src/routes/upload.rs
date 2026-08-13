use axum::{
    Extension,
    body::Bytes,
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use image::{ImageFormat, ImageReader};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use std::{collections::BTreeSet, convert::Infallible, io::Cursor};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, bot_or_user_auth_middleware, require_workspace_access_from_auth},
    response::ApiResponse,
    services::object_storage::ObjectStorage,
};

const MAX_FILE_SIZE: usize = 50 * 1024 * 1024;
pub(crate) const THUMBNAIL_MAX_DIMENSION: u32 = 320;
const SIGNATURE_SIGNED_DOWNLOAD_TTL_MINUTES: i64 = 10;
const SUPPORTED_FILE_TYPES_MESSAGE: &str =
    "only png/jpg/gif/webp/mp4/webm/mov/avi/zip/gz/tar.gz/log/txt/pdf/json/csv/xml are supported";
const UPLOAD_PATH_PREFIXES: [&str; 2] = ["/api/v1/uploads/", "/uploads/"];
const UPLOAD_DERIVATIVE_PREFIXES: [&str; 4] = ["thumbnails/", "previews/", "variants/", "signatures/"];
const UPLOAD_NOT_FOUND_MESSAGE: &str = "file not found";
const SIGNATURE_FILE_NAME_PREFIX: &str = "signature-";
const UUID_TEXT_LENGTH: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageDerivativeFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
    pub filename: String,
    pub storage_backend: String,
    pub object_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_object_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SignatureDownloadQuery {
    pub expires: i64,
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct SignatureSignedDownloadResponse {
    pub url: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Query parameters accepted by every uploaded-object download route.
#[derive(Debug, Default, Deserialize)]
pub struct UploadAccessQuery {
    pub expires: Option<i64>,
    pub signature: Option<String>,
}

/// Marker injected by [`uploads_access_middleware`] once a request proved it carries a
/// valid, unexpired signed download URL for the requested object key.
#[derive(Debug, Clone, Copy)]
pub struct SignedUploadAccess;

/// Authorization context of an uploaded-object download request.
///
/// Exactly one of the two legitimate access paths must be present:
/// a session (JWT cookie/bearer or bot token) or a validated signed URL.
#[derive(Debug)]
pub struct UploadAccess {
    claims: Option<JwtClaims>,
    bot: Option<BotAuthContext>,
    signed: bool,
}

impl<S> FromRequestParts<S> for UploadAccess
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self {
            claims: parts.extensions.get::<JwtClaims>().cloned(),
            bot: parts.extensions.get::<BotAuthContext>().cloned(),
            signed: parts.extensions.get::<SignedUploadAccess>().is_some(),
        })
    }
}

/// The access path a download request qualified for.
#[derive(Debug)]
enum UploadAccessMode<'a> {
    /// Anonymous request carrying a signed URL already validated by the middleware.
    SignedUrl,
    /// Authenticated request; the object still has to belong to a workspace the actor is in.
    Session(&'a JwtClaims),
}

impl UploadAccess {
    /// Classify the request, rejecting anything that carries neither a session nor a signed URL.
    fn mode(&self) -> Result<UploadAccessMode<'_>, ApiError> {
        match (self.claims.as_ref(), self.signed) {
            (Some(claims), _) => Ok(UploadAccessMode::Session(claims)),
            (None, true) => Ok(UploadAccessMode::SignedUrl),
            (None, false) => Err(ApiError::Unauthorized("missing access token".to_string())),
        }
    }

    /// Reject the request unless the caller may read `object_key`.
    ///
    /// Signed URLs are trusted as-is: they are only minted after a workspace check and are bound
    /// to a single object key with an expiry, which is what keeps anonymous `<img src>` rendering
    /// working. Session requests are additionally scoped to the workspace that owns the object
    /// whenever that ownership is recorded in the database.
    async fn authorize(&self, state: &AppState, object_key: &str) -> Result<(), ApiError> {
        let claims = match self.mode()? {
            UploadAccessMode::SignedUrl => return Ok(()),
            UploadAccessMode::Session(claims) => claims,
        };
        let workspace_id = match resolve_upload_object_owner(state, object_key).await? {
            UploadObjectOwner::Unowned => None,
            UploadObjectOwner::Owned(workspace_id) => Some(workspace_id),
            // More than one workspace claims the object, so no membership check can be trusted.
            UploadObjectOwner::Ambiguous => {
                return Err(ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()));
            }
        };
        self.authorize_owner_workspace(state, claims, workspace_id).await
    }

    /// Require workspace membership for the resolved owner of the object.
    ///
    /// Ownership that cannot be resolved (markdown uploads have no ownership row) leaves the
    /// request at "any authenticated actor". Denials are reported as a plain not-found so the
    /// endpoint cannot be used to probe which object keys exist in other workspaces.
    async fn authorize_owner_workspace(
        &self,
        state: &AppState,
        claims: &JwtClaims,
        workspace_id: Option<Uuid>,
    ) -> Result<(), ApiError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(());
        };
        match require_workspace_access_from_auth(state, claims, self.bot.as_ref(), workspace_id).await {
            Ok(_) => Ok(()),
            Err(ApiError::Database(err)) => Err(ApiError::Database(err)),
            Err(ApiError::Internal) => Err(ApiError::Internal),
            Err(_) => Err(ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string())),
        }
    }
}

/// Access gate shared by every `GET /uploads/...` route.
///
/// Two access paths are accepted:
/// 1. a signed download URL (`?expires=...&signature=...`), which stays anonymous on purpose so
///    that expiring links keep rendering in `<img>` tags outside an authenticated session;
/// 2. a normal session, delegated to the shared bot/user auth middleware so the handlers can
///    run per-object workspace checks on the injected claims.
pub async fn uploads_access_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let query = Query::<UploadAccessQuery>::try_from_uri(req.uri())
        .map(|Query(query)| query)
        .unwrap_or_default();
    let (Some(expires), Some(signature)) = (query.expires, query.signature.as_deref()) else {
        return bot_or_user_auth_middleware(State(state), req, next).await;
    };
    let object_key = upload_object_key_from_path(req.uri().path())
        .ok_or_else(|| ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()))?;
    verify_upload_download_signature(
        &state.cfg.jwt_secret,
        &object_key,
        expires,
        signature,
        Utc::now().timestamp(),
    )?;
    req.extensions_mut().insert(SignedUploadAccess);
    Ok(next.run(req).await)
}

/// POST /api/v1/upload - Upload files for markdown content
pub async fn upload_file(
    State(_state): State<AppState>,
    Extension(_claims): Extension<JwtClaims>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(upload_file_from_multipart(headers, body).await?))
}

async fn upload_file_from_multipart(headers: HeaderMap, body: Bytes) -> Result<UploadResponse, ApiError> {
    let object_storage = ObjectStorage::from_env()?;

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
            "file size must be less than or equal to 50MB".to_string(),
        ));
    }

    let ext = resolve_file_extension(part.filename.as_deref(), part.content_type.as_deref())
        .ok_or_else(|| ApiError::BadRequest(SUPPORTED_FILE_TYPES_MESSAGE.to_string()))?;

    let file_name = format!("{}.{}", Uuid::new_v4(), ext);
    let object_ref = object_storage.put(&file_name, &part.data).await?;
    let thumbnail_url = if is_image_extension(&ext) {
        let thumbnail_name = thumbnail_file_name(&file_name);
        let thumbnail_key = format!("thumbnails/{thumbnail_name}");
        let thumbnail = build_thumbnail_derivative(part.data.clone()).await?;
        let thumbnail_ref = object_storage.put(&thumbnail_key, &thumbnail).await?;
        Some((
            format!("/api/v1/uploads/thumbnails/{thumbnail_name}"),
            thumbnail_ref.key,
        ))
    } else {
        None
    };

    let (thumbnail_url, thumbnail_object_key) = thumbnail_url
        .map(|(url, key)| (Some(url), Some(key)))
        .unwrap_or((None, None));

    Ok(UploadResponse {
        url: format!("/api/v1/uploads/{}", file_name),
        filename: file_name,
        storage_backend: object_ref.backend,
        object_key: object_ref.key,
        thumbnail_url,
        thumbnail_object_key,
    })
}

/// GET /uploads/:file_name - Serve uploaded file
pub async fn get_uploaded_file(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_upload_file_name(&file_name)?;
    access.authorize(&state, &file_name).await?;

    let data = ObjectStorage::from_env()?
        .get(&file_name)
        .await
        .map_err(|_| ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()))?;

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
    } else if file_name.ends_with(".tar.gz") {
        HeaderValue::from_static("application/gzip")
    } else if file_name.ends_with(".zip") {
        HeaderValue::from_static("application/zip")
    } else if file_name.ends_with(".gz") {
        HeaderValue::from_static("application/gzip")
    } else if file_name.ends_with(".log") || file_name.ends_with(".txt") {
        HeaderValue::from_static("text/plain; charset=utf-8")
    } else if file_name.ends_with(".pdf") {
        HeaderValue::from_static("application/pdf")
    } else if file_name.ends_with(".json") {
        HeaderValue::from_static("application/json")
    } else if file_name.ends_with(".csv") {
        HeaderValue::from_static("text/csv; charset=utf-8")
    } else if file_name.ends_with(".xml") {
        HeaderValue::from_static("application/xml")
    } else {
        HeaderValue::from_static("application/octet-stream")
    };

    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], data))
}

/// GET /uploads/thumbnails/:file_name - Serve uploaded thumbnail derivative
pub async fn get_uploaded_thumbnail(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let data = read_derivative_object(&state, &access, "thumbnails", &file_name).await?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, derivative_content_type(&file_name))],
        data,
    ))
}

/// GET /uploads/previews/:file_name - Serve uploaded preview derivative
pub async fn get_uploaded_preview(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let data = read_derivative_object(&state, &access, "previews", &file_name).await?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, derivative_content_type(&file_name))],
        data,
    ))
}

/// GET /uploads/variants/:file_name - Serve uploaded configured variant derivative
pub async fn get_uploaded_variant(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let data = read_derivative_object(&state, &access, "variants", &file_name).await?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, derivative_content_type(&file_name))],
        data,
    ))
}

/// GET /uploads/signatures/:file_name - Serve stored signature image
pub async fn get_uploaded_signature(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_signature_file_name(&file_name)?;
    access.authorize(&state, &format!("signatures/{file_name}")).await?;
    let data = read_signature_object(&file_name).await?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("image/png"))],
        data,
    ))
}

/// POST /api/v1/uploads/signatures/:file_name/signed-url - Create signed signature download URL
pub async fn create_uploaded_signature_signed_url(
    State(state): State<AppState>,
    access: UploadAccess,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_signature_file_name(&file_name)?;
    access.authorize(&state, &format!("signatures/{file_name}")).await?;
    let _ = read_signature_object(&file_name).await?;
    let expires_at = Utc::now() + Duration::minutes(SIGNATURE_SIGNED_DOWNLOAD_TTL_MINUTES);
    Ok(ApiResponse::success(signature_signed_download_response(
        &state.cfg.jwt_secret,
        &file_name,
        expires_at,
    )?))
}

/// GET /api/v1/uploads/signatures/:file_name/download - Serve stored signature image after signed URL validation
pub async fn download_signed_uploaded_signature(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
    Query(query): Query<SignatureDownloadQuery>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_signature_file_name(&file_name)?;
    if query.expires < Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("signature download link expired".to_string()));
    }
    let expected = sign_signature_download(&state.cfg.jwt_secret, &file_name, query.expires)?;
    if !constant_time_eq(expected.as_bytes(), query.signature.as_bytes()) {
        return Err(ApiError::Unauthorized(
            "invalid signature download signature".to_string(),
        ));
    }
    let data = read_signature_object(&file_name).await?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("image/png"))],
        data,
    ))
}

pub(crate) fn is_image_extension(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp")
}

pub(crate) fn thumbnail_file_name(file_name: &str) -> String {
    let base_name = file_name.rsplit_once('.').map_or(file_name, |(base, _ext)| base);
    format!("thumb-{base_name}.png")
}

pub(crate) fn thumbnail_file_name_with_dimension_and_format(
    file_name: &str,
    max_dimension: u32,
    format: ImageDerivativeFormat,
) -> String {
    let base_name = file_name.rsplit_once('.').map_or(file_name, |(base, _ext)| base);
    format!("thumb-{base_name}-{max_dimension}.{}", format.extension())
}

pub(crate) fn preview_file_name_with_dimension_and_format(
    file_name: &str,
    max_dimension: u32,
    format: ImageDerivativeFormat,
) -> String {
    let base_name = file_name.rsplit_once('.').map_or(file_name, |(base, _ext)| base);
    format!("preview-{base_name}-{max_dimension}.{}", format.extension())
}

pub(crate) fn variant_file_name_with_dimension_and_format(
    kind: &str,
    file_name: &str,
    max_dimension: u32,
    format: ImageDerivativeFormat,
) -> String {
    let base_name = file_name.rsplit_once('.').map_or(file_name, |(base, _ext)| base);
    format!("variant-{kind}-{base_name}-{max_dimension}.{}", format.extension())
}

async fn build_thumbnail_derivative(data: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    build_thumbnail_derivative_with_max_dimension(data, THUMBNAIL_MAX_DIMENSION).await
}

pub(crate) async fn build_thumbnail_derivative_with_max_dimension(
    data: Vec<u8>,
    max_dimension: u32,
) -> Result<Vec<u8>, ApiError> {
    build_image_derivative_with_max_dimension_and_format(data, max_dimension, ImageDerivativeFormat::Png).await
}

pub(crate) async fn build_image_derivative_with_max_dimension_and_format(
    data: Vec<u8>,
    max_dimension: u32,
    format: ImageDerivativeFormat,
) -> Result<Vec<u8>, ApiError> {
    tokio::task::spawn_blocking(move || build_image_derivative_sync(&data, max_dimension, format))
        .await
        .map_err(|_| ApiError::Internal)?
}

pub(crate) async fn image_dimensions(data: Vec<u8>) -> Result<(u32, u32), ApiError> {
    tokio::task::spawn_blocking(move || image_dimensions_sync(&data))
        .await
        .map_err(|_| ApiError::Internal)?
}

fn build_image_derivative_sync(
    data: &[u8],
    max_dimension: u32,
    format: ImageDerivativeFormat,
) -> Result<Vec<u8>, ApiError> {
    let image = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| ApiError::BadRequest("unable to read image upload".to_string()))?
        .decode()
        .map_err(|_| ApiError::BadRequest("unable to decode image upload".to_string()))?;
    let thumbnail = image.thumbnail(max_dimension, max_dimension);
    let mut output = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut output, format.image_format())
        .map_err(|_| ApiError::Internal)?;
    Ok(output.into_inner())
}

fn image_dimensions_sync(data: &[u8]) -> Result<(u32, u32), ApiError> {
    let image = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| ApiError::BadRequest("unable to read image upload".to_string()))?
        .decode()
        .map_err(|_| ApiError::BadRequest("unable to decode image upload".to_string()))?;
    Ok((image.width(), image.height()))
}

impl ImageDerivativeFormat {
    pub(crate) fn from_schema(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "png" | "image/png" => Some(Self::Png),
            "jpg" | "jpeg" | "image/jpg" | "image/jpeg" => Some(Self::Jpeg),
            "webp" | "image/webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
        }
    }
}

fn derivative_content_type(file_name: &str) -> HeaderValue {
    if file_name.ends_with(".png") {
        HeaderValue::from_static("image/png")
    } else if file_name.ends_with(".jpg") || file_name.ends_with(".jpeg") {
        HeaderValue::from_static("image/jpeg")
    } else if file_name.ends_with(".webp") {
        HeaderValue::from_static("image/webp")
    } else {
        HeaderValue::from_static("application/octet-stream")
    }
}

struct ParsedFilePart {
    filename: Option<String>,
    content_type: Option<String>,
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

    Err(ApiError::BadRequest("missing multipart boundary".to_string()))
}

#[allow(clippy::indexing_slicing)]
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

    let content_type = header_text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-type") {
            Some(value.trim().to_ascii_lowercase())
        } else {
            None
        }
    });

    let filename = header_text
        .lines()
        .find_map(parse_filename_from_content_disposition)
        .filter(|name| !name.trim().is_empty());

    let data_start = headers_end + 4;
    let next_boundary_marker = format!("\r\n--{boundary}");
    let data_end_rel = find_subslice(&body[data_start..], next_boundary_marker.as_bytes())
        .ok_or_else(|| ApiError::BadRequest("invalid multipart payload".to_string()))?;
    let data_end = data_start + data_end_rel;

    Ok(ParsedFilePart {
        filename,
        content_type,
        data: body[data_start..data_end].to_vec(),
    })
}

fn parse_filename_from_content_disposition(line: &str) -> Option<String> {
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-disposition") {
        return None;
    }

    value.split(';').map(str::trim).find_map(|segment| {
        segment
            .strip_prefix("filename=")
            .map(|raw| raw.trim_matches('"').to_string())
    })
}

fn resolve_file_extension(filename: Option<&str>, content_type: Option<&str>) -> Option<&'static str> {
    filename
        .and_then(extension_from_filename)
        .or_else(|| content_type.and_then(extension_from_content_type))
}

fn extension_from_filename(filename: &str) -> Option<&'static str> {
    let lower = filename.trim().to_ascii_lowercase();

    if lower.ends_with(".tar.gz") {
        Some("tar.gz")
    } else if lower.ends_with(".png") {
        Some("png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("jpg")
    } else if lower.ends_with(".gif") {
        Some("gif")
    } else if lower.ends_with(".webp") {
        Some("webp")
    } else if lower.ends_with(".mp4") {
        Some("mp4")
    } else if lower.ends_with(".webm") {
        Some("webm")
    } else if lower.ends_with(".mov") {
        Some("mov")
    } else if lower.ends_with(".avi") {
        Some("avi")
    } else if lower.ends_with(".zip") {
        Some("zip")
    } else if lower.ends_with(".gz") {
        Some("gz")
    } else if lower.ends_with(".log") {
        Some("log")
    } else if lower.ends_with(".txt") {
        Some("txt")
    } else if lower.ends_with(".pdf") {
        Some("pdf")
    } else if lower.ends_with(".json") {
        Some("json")
    } else if lower.ends_with(".csv") {
        Some("csv")
    } else if lower.ends_with(".xml") {
        Some("xml")
    } else {
        None
    }
}

fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    let normalized = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();

    match normalized.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/quicktime" => Some("mov"),
        "video/x-msvideo" | "video/avi" => Some("avi"),
        "application/zip" | "application/x-zip-compressed" => Some("zip"),
        "application/gzip" | "application/x-gzip" => Some("gz"),
        "text/plain" => Some("txt"),
        "text/log" | "text/x-log" => Some("log"),
        "application/pdf" => Some("pdf"),
        "application/json" | "text/json" => Some("json"),
        "text/csv" | "application/csv" => Some("csv"),
        "application/xml" | "text/xml" => Some("xml"),
        _ => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }

    haystack.windows(needle.len()).position(|window| window == needle)
}

async fn read_derivative_object(
    state: &AppState,
    access: &UploadAccess,
    prefix: &str,
    file_name: &str,
) -> Result<Vec<u8>, ApiError> {
    ensure_upload_file_name(file_name)?;
    let object_key = format!("{prefix}/{file_name}");
    access.authorize(state, &object_key).await?;
    ObjectStorage::from_env()?
        .get(&object_key)
        .await
        .map_err(|_| ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()))
}

async fn read_signature_object(file_name: &str) -> Result<Vec<u8>, ApiError> {
    ensure_signature_file_name(file_name)?;
    ObjectStorage::from_env()?
        .get(&format!("signatures/{file_name}"))
        .await
        .map_err(|_| ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()))
}

fn ensure_upload_file_name(file_name: &str) -> Result<(), ApiError> {
    if is_safe_upload_file_name(file_name) {
        Ok(())
    } else {
        Err(ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()))
    }
}

fn ensure_signature_file_name(file_name: &str) -> Result<(), ApiError> {
    if !is_safe_upload_file_name(file_name) || !file_name.ends_with(".png") {
        return Err(ApiError::NotFound(UPLOAD_NOT_FOUND_MESSAGE.to_string()));
    }
    Ok(())
}

fn is_safe_upload_file_name(file_name: &str) -> bool {
    !file_name.is_empty() && !file_name.contains('/') && !file_name.contains('\\') && !file_name.contains("..")
}

/// Map a request path to the object storage key it serves, or `None` when the path is not a
/// download route or the file name is unsafe.
pub(crate) fn upload_object_key_from_path(path: &str) -> Option<String> {
    let remainder = UPLOAD_PATH_PREFIXES
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix))?;
    let (directory, file_name) = match remainder.split_once('/') {
        Some((directory, file_name)) => {
            let directory = format!("{directory}/");
            if !UPLOAD_DERIVATIVE_PREFIXES.contains(&directory.as_str()) {
                return None;
            }
            (Some(directory), file_name)
        }
        None => (None, remainder),
    };
    if !is_safe_upload_file_name(file_name) {
        return None;
    }
    Some(directory.map_or_else(|| file_name.to_string(), |directory| format!("{directory}{file_name}")))
}

/// The workspace an upload object belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadObjectOwner {
    /// No row records ownership of the object (generic markdown uploads).
    Unowned,
    /// Exactly one workspace claims the object.
    Owned(Uuid),
    /// Several workspaces claim the object, so ownership cannot decide access.
    Ambiguous,
}

impl UploadObjectOwner {
    fn from_workspace_ids(workspace_ids: Vec<Uuid>) -> Self {
        let mut distinct = workspace_ids.into_iter().collect::<BTreeSet<_>>().into_iter();
        match (distinct.next(), distinct.next()) {
            (None, _) => Self::Unowned,
            (Some(workspace_id), None) => Self::Owned(workspace_id),
            (Some(_), Some(_)) => Self::Ambiguous,
        }
    }
}

/// Resolve the workspace that owns `object_key`, when that ownership is recorded.
///
/// Objects uploaded through the generic markdown upload endpoint have no ownership row, so
/// `Unowned` is returned for them and the caller falls back to "any authenticated actor". A key
/// claimed by more than one workspace resolves to `Ambiguous`: `LIMIT 1` without `ORDER BY` used to
/// pick an arbitrary row here, which let an attacker register an attachment pointing at somebody
/// else's object and have the access check land on their own workspace.
pub(crate) async fn resolve_upload_object_owner(
    state: &AppState,
    object_key: &str,
) -> Result<UploadObjectOwner, ApiError> {
    if let Some(file_name) = object_key.strip_prefix("signatures/") {
        return resolve_signature_object_owner(state, file_name).await;
    }
    resolve_attachment_object_owner(state, object_key).await
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceIdRow {
    workspace_id: Uuid,
}

/// Look up the owning workspace of a form attachment source file or one of its derivatives.
async fn resolve_attachment_object_owner(state: &AppState, object_key: &str) -> Result<UploadObjectOwner, ApiError> {
    let api_url = format!("/api/v1/uploads/{object_key}");
    let legacy_url = format!("/uploads/{object_key}");
    let thumbnail_match = json!({ "thumbnail": { "object_key": object_key } }).to_string();
    let preview_match = json!({ "preview": { "object_key": object_key } }).to_string();
    let variant_match = json!({ "variants": [{ "policy": { "object_key": object_key } }] }).to_string();
    let rows = WorkspaceIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT DISTINCT workspace_id
            FROM form_attachments
           WHERE storage_key = $1
              OR url IN ($2, $3)
              OR thumbnail_url IN ($2, $3)
              OR media_metadata @> $4::text::jsonb
              OR media_metadata @> $5::text::jsonb
              OR media_metadata @> $6::text::jsonb
           LIMIT 2",
        vec![
            object_key.to_string().into(),
            api_url.into(),
            legacy_url.into(),
            thumbnail_match.into(),
            preview_match.into(),
            variant_match.into(),
        ],
    ))
    .all(&state.db)
    .await?;
    Ok(UploadObjectOwner::from_workspace_ids(
        rows.into_iter().map(|row| row.workspace_id).collect(),
    ))
}

/// Look up the owning workspace of a materialized signature image.
///
/// Signature values are stored on the record itself, so the record that currently references the
/// image identifies the tenant. The lookup rides the `gin(values jsonb_path_ops)` index.
async fn resolve_signature_object_owner(state: &AppState, file_name: &str) -> Result<UploadObjectOwner, ApiError> {
    let Some(field_key) = signature_field_key_from_file_name(file_name) else {
        return Ok(UploadObjectOwner::Unowned);
    };
    let value_match = json!({ field_key: format!("/api/v1/uploads/signatures/{file_name}") }).to_string();
    let rows = WorkspaceIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT DISTINCT workspace_id
             FROM form_records
            WHERE form_records."values" @> $1::text::jsonb
            LIMIT 2"#,
        vec![value_match.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(UploadObjectOwner::from_workspace_ids(
        rows.into_iter().map(|row| row.workspace_id).collect(),
    ))
}

/// Extract the form field key from a materialized signature file name
/// (`signature-{field_key}-{uuid}.png`). Field keys never contain `-`, so the split is exact.
fn signature_field_key_from_file_name(file_name: &str) -> Option<&str> {
    let rest = file_name
        .strip_suffix(".png")?
        .strip_prefix(SIGNATURE_FILE_NAME_PREFIX)?;
    let separator_index = rest.len().checked_sub(UUID_TEXT_LENGTH + 1)?;
    if rest.get(separator_index..=separator_index) != Some("-") {
        return None;
    }
    let field_key = rest.get(..separator_index)?;
    Uuid::parse_str(rest.get(separator_index + 1..)?).ok()?;
    if field_key.is_empty()
        || !field_key
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')
    {
        return None;
    }
    Some(field_key)
}

/// The signature object a record value would claim ownership of, if any.
///
/// `resolve_signature_object_owner` decides who owns a materialized signature by looking for records
/// whose `values` contain `{field_key: "/api/v1/uploads/signatures/{file}"}`, with `field_key`
/// derived from the file name. Storing that exact pair therefore rewrites the object's ownership,
/// which is why record writes have to be screened the same way attachment registrations are. The
/// match is deliberately as narrow as the lookup: a different field key, the legacy `/uploads/`
/// spelling or any surrounding text cannot influence ownership and must not be rejected.
pub(crate) fn signature_object_key_claimed_by_record_value(field_key: &str, value: &str) -> Option<String> {
    let file_name = value.strip_prefix("/api/v1/uploads/signatures/")?;
    if !is_safe_upload_file_name(file_name) {
        return None;
    }
    if signature_field_key_from_file_name(file_name)? != field_key {
        return None;
    }
    Some(format!("signatures/{file_name}"))
}

type HmacSha256 = Hmac<Sha256>;

fn sign_signature_download(secret: &str, file_name: &str, expires: i64) -> Result<String, ApiError> {
    ensure_signature_file_name(file_name)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::Internal)?;
    mac.update(format!("signature:{file_name}:{expires}").as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn sign_upload_download(secret: &str, object_key: &str, expires: i64) -> Result<String, ApiError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::Internal)?;
    mac.update(format!("upload:{object_key}:{expires}").as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Validate a signed download URL bound to a single object key and expiry timestamp.
pub(crate) fn verify_upload_download_signature(
    secret: &str,
    object_key: &str,
    expires: i64,
    signature: &str,
    now: i64,
) -> Result<(), ApiError> {
    if expires < now {
        return Err(ApiError::Unauthorized("upload download link expired".to_string()));
    }
    let expected = sign_upload_download(secret, object_key, expires)?;
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return Err(ApiError::Unauthorized("invalid upload download signature".to_string()));
    }
    Ok(())
}

fn signature_signed_download_response(
    secret: &str,
    file_name: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<SignatureSignedDownloadResponse, ApiError> {
    let expires = expires_at.timestamp();
    let signature = sign_signature_download(secret, file_name, expires)?;
    Ok(SignatureSignedDownloadResponse {
        url: format!("/api/v1/uploads/signatures/{file_name}/download?expires={expires}&signature={signature}"),
        expires_at,
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use chrono::Utc;
    use image::{ImageBuffer, Rgba};
    use platform::{app::AppState, auth::TokenType, config::AppConfig};
    use sea_orm::DatabaseConnection;
    use std::env;

    const TEST_SECRET: &str = "upload-access-test-secret";

    fn test_state() -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "api".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: "postgres://disconnected/openpr".to_string(),
                jwt_secret: TEST_SECRET.to_string(),
                jwt_access_ttl_seconds: 3600,
                jwt_refresh_ttl_seconds: 7200,
                default_author_id: None,
            },
            db: DatabaseConnection::default(),
        }
    }

    fn test_claims(subject: Uuid) -> JwtClaims {
        JwtClaims {
            sub: subject.to_string(),
            email: "member@openpr.local".to_string(),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        }
    }

    fn bot_access(bot_id: Uuid, workspace_id: Uuid) -> UploadAccess {
        UploadAccess {
            claims: Some(test_claims(bot_id)),
            bot: Some(BotAuthContext {
                bot_id,
                workspace_id,
                permissions: vec!["admin".to_string()],
            }),
            signed: false,
        }
    }

    #[test]
    fn object_ownership_stays_undecided_when_two_workspaces_claim_the_same_key() {
        let victim = Uuid::new_v4();
        let attacker = Uuid::new_v4();

        assert_eq!(
            UploadObjectOwner::from_workspace_ids(vec![victim, attacker]),
            UploadObjectOwner::Ambiguous,
            "a claimed object must never resolve to an arbitrary row"
        );
        assert_eq!(
            UploadObjectOwner::from_workspace_ids(vec![victim, victim]),
            UploadObjectOwner::Owned(victim)
        );
        assert_eq!(
            UploadObjectOwner::from_workspace_ids(Vec::new()),
            UploadObjectOwner::Unowned
        );
    }

    #[test]
    fn upload_access_rejects_requests_without_session_or_signed_url() {
        let anonymous = UploadAccess {
            claims: None,
            bot: None,
            signed: false,
        };

        let error = anonymous.mode().expect_err("anonymous download must be rejected");

        assert!(matches!(error, ApiError::Unauthorized(_)));
    }

    #[test]
    fn upload_access_accepts_validated_signed_url_without_session() {
        let signed = UploadAccess {
            claims: None,
            bot: None,
            signed: true,
        };

        assert!(matches!(
            signed.mode().expect("signed download must be accepted"),
            UploadAccessMode::SignedUrl
        ));
    }

    #[tokio::test]
    async fn upload_access_denies_object_owned_by_another_workspace() {
        let state = test_state();
        let bot_id = Uuid::new_v4();
        let access = bot_access(bot_id, Uuid::new_v4());

        let error = access
            .authorize_owner_workspace(&state, &test_claims(bot_id), Some(Uuid::new_v4()))
            .await
            .expect_err("cross-workspace download must be rejected");

        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn upload_access_allows_object_owned_by_the_actor_workspace() {
        let state = test_state();
        let bot_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let access = bot_access(bot_id, workspace_id);

        access
            .authorize_owner_workspace(&state, &test_claims(bot_id), Some(workspace_id))
            .await
            .expect("same-workspace download must be allowed");
    }

    #[tokio::test]
    async fn upload_access_allows_unowned_object_for_authenticated_actor() {
        let state = test_state();
        let bot_id = Uuid::new_v4();
        let access = bot_access(bot_id, Uuid::new_v4());

        access
            .authorize_owner_workspace(&state, &test_claims(bot_id), None)
            .await
            .expect("objects without an ownership row stay readable for authenticated actors");
    }

    #[test]
    fn upload_object_key_maps_download_routes_to_storage_keys() {
        assert_eq!(
            upload_object_key_from_path("/api/v1/uploads/9f1a.png").as_deref(),
            Some("9f1a.png")
        );
        assert_eq!(
            upload_object_key_from_path("/uploads/9f1a.png").as_deref(),
            Some("9f1a.png")
        );
        assert_eq!(
            upload_object_key_from_path("/api/v1/uploads/thumbnails/thumb-9f1a-320.png").as_deref(),
            Some("thumbnails/thumb-9f1a-320.png")
        );
        assert_eq!(
            upload_object_key_from_path("/uploads/signatures/signature-consent-9f1a.png").as_deref(),
            Some("signatures/signature-consent-9f1a.png")
        );
        assert_eq!(upload_object_key_from_path("/api/v1/uploads/"), None);
        assert_eq!(upload_object_key_from_path("/api/v1/uploads/../secrets.env"), None);
        assert_eq!(upload_object_key_from_path("/api/v1/uploads/other/9f1a.png"), None);
        assert_eq!(
            upload_object_key_from_path("/api/v1/uploads/signatures/9f1a.png/download"),
            None
        );
        assert_eq!(upload_object_key_from_path("/api/v1/form-attachments/9f1a"), None);
    }

    #[test]
    fn upload_download_signature_is_bound_to_object_key_and_expiry() {
        let expires = 1_800_000_000;
        let signature =
            sign_upload_download(TEST_SECRET, "thumbnails/thumb-9f1a-320.png", expires).expect("signature should sign");

        verify_upload_download_signature(
            TEST_SECRET,
            "thumbnails/thumb-9f1a-320.png",
            expires,
            &signature,
            expires - 1,
        )
        .expect("fresh signed URL should be accepted");

        let expired = verify_upload_download_signature(
            TEST_SECRET,
            "thumbnails/thumb-9f1a-320.png",
            expires,
            &signature,
            expires + 1,
        )
        .expect_err("expired signed URL must be rejected");
        assert!(matches!(expired, ApiError::Unauthorized(_)));

        let other_object = verify_upload_download_signature(
            TEST_SECRET,
            "thumbnails/thumb-other-320.png",
            expires,
            &signature,
            expires - 1,
        )
        .expect_err("signed URL must not transfer to another object");
        assert!(matches!(other_object, ApiError::Unauthorized(_)));

        let other_secret = verify_upload_download_signature(
            "another-secret",
            "thumbnails/thumb-9f1a-320.png",
            expires,
            &signature,
            expires - 1,
        )
        .expect_err("signed URL must not validate under another secret");
        assert!(matches!(other_secret, ApiError::Unauthorized(_)));

        let tampered = verify_upload_download_signature(
            TEST_SECRET,
            "thumbnails/thumb-9f1a-320.png",
            expires,
            "deadbeef",
            expires - 1,
        )
        .expect_err("truncated signature must be rejected");
        assert!(matches!(tampered, ApiError::Unauthorized(_)));
    }

    #[test]
    fn signature_field_key_is_parsed_from_materialized_file_names() {
        let signature_id = Uuid::new_v4();
        assert_eq!(
            signature_field_key_from_file_name(&format!("signature-approval_sign-{signature_id}.png")),
            Some("approval_sign")
        );
        assert_eq!(
            signature_field_key_from_file_name(&format!("signature-{signature_id}.png")),
            None
        );
        assert_eq!(
            signature_field_key_from_file_name("signature-approval-not-a-uuid.png"),
            None
        );
        assert_eq!(
            signature_field_key_from_file_name(&format!("thumb-approval-{signature_id}.png")),
            None
        );
        assert_eq!(
            signature_field_key_from_file_name(&format!("signature-approval-{signature_id}.jpg")),
            None
        );
    }

    #[test]
    fn upload_file_name_guard_rejects_traversal() {
        assert!(ensure_upload_file_name("9f1a.png").is_ok());
        assert!(ensure_upload_file_name("").is_err());
        assert!(ensure_upload_file_name("../9f1a.png").is_err());
        assert!(ensure_upload_file_name("nested/9f1a.png").is_err());
        assert!(ensure_upload_file_name("nested\\9f1a.png").is_err());
    }

    #[test]
    fn thumbnail_file_name_uses_png_derivative_extension() {
        assert_eq!(thumbnail_file_name("original-image.jpeg"), "thumb-original-image.png");
    }

    #[test]
    fn build_thumbnail_derivative_resizes_to_bounded_png() {
        let source = ImageBuffer::from_pixel(1024, 512, Rgba([32_u8, 96, 160, 255]));
        let mut input = Cursor::new(Vec::new());
        source
            .write_to(&mut input, ImageFormat::Png)
            .expect("test image should encode");

        let thumbnail =
            build_image_derivative_sync(&input.into_inner(), THUMBNAIL_MAX_DIMENSION, ImageDerivativeFormat::Png)
                .expect("thumbnail should be generated");
        let decoded =
            image::load_from_memory_with_format(&thumbnail, ImageFormat::Png).expect("thumbnail should be png");

        assert!(decoded.width() <= THUMBNAIL_MAX_DIMENSION);
        assert!(decoded.height() <= THUMBNAIL_MAX_DIMENSION);
        assert_eq!(decoded.width(), THUMBNAIL_MAX_DIMENSION);
        assert!(thumbnail.len() < 50_000);
    }

    #[test]
    fn thumbnail_file_name_with_dimension_includes_policy_size() {
        assert_eq!(
            thumbnail_file_name_with_dimension_and_format("original-image.jpeg", 640, ImageDerivativeFormat::Png),
            "thumb-original-image-640.png"
        );
    }

    #[test]
    fn preview_file_name_with_dimension_includes_policy_size() {
        assert_eq!(
            preview_file_name_with_dimension_and_format("original-image.jpeg", 1600, ImageDerivativeFormat::Png),
            "preview-original-image-1600.png"
        );
        assert_eq!(
            preview_file_name_with_dimension_and_format("original-image.jpeg", 1600, ImageDerivativeFormat::Webp),
            "preview-original-image-1600.webp"
        );
    }

    #[test]
    fn variant_file_name_with_dimension_includes_kind_and_policy_size() {
        assert_eq!(
            variant_file_name_with_dimension_and_format(
                "gallery",
                "original-image.jpeg",
                1200,
                ImageDerivativeFormat::Png
            ),
            "variant-gallery-original-image-1200.png"
        );
        assert_eq!(
            variant_file_name_with_dimension_and_format(
                "gallery",
                "original-image.jpeg",
                1200,
                ImageDerivativeFormat::Jpeg
            ),
            "variant-gallery-original-image-1200.jpg"
        );
    }

    #[test]
    fn image_derivative_format_parses_schema_values_and_content_type() {
        assert_eq!(
            ImageDerivativeFormat::from_schema("image/jpeg"),
            Some(ImageDerivativeFormat::Jpeg)
        );
        assert_eq!(
            ImageDerivativeFormat::from_schema("WEBP"),
            Some(ImageDerivativeFormat::Webp)
        );
        assert_eq!(ImageDerivativeFormat::from_schema("tiff"), None);
        assert_eq!(derivative_content_type("preview-photo-1600.jpg"), "image/jpeg");
        assert_eq!(derivative_content_type("preview-photo-1600.webp"), "image/webp");
    }

    #[test]
    fn build_image_derivative_resizes_to_requested_format() {
        let source = ImageBuffer::from_pixel(1024, 512, Rgba([32_u8, 96, 160, 255]));
        let mut input = Cursor::new(Vec::new());
        source
            .write_to(&mut input, ImageFormat::Png)
            .expect("test image should encode");
        let input = input.into_inner();

        let jpeg = build_image_derivative_sync(&input, 256, ImageDerivativeFormat::Jpeg)
            .expect("jpeg derivative should be generated");
        let decoded_jpeg =
            image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg).expect("jpeg derivative should decode");
        assert_eq!(decoded_jpeg.width(), 256);
        assert_eq!(decoded_jpeg.height(), 128);

        let webp = build_image_derivative_sync(&input, 128, ImageDerivativeFormat::Webp)
            .expect("webp derivative should be generated");
        let decoded_webp =
            image::load_from_memory_with_format(&webp, ImageFormat::WebP).expect("webp derivative should decode");
        assert_eq!(decoded_webp.width(), 128);
        assert_eq!(decoded_webp.height(), 64);
    }

    #[test]
    fn signature_signed_download_response_uses_hmac_and_expiry() {
        let expires_at = chrono::DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp should be valid");
        let response = signature_signed_download_response("secret", "signature-guest-123.png", expires_at)
            .expect("signed signature URL should be generated");

        assert_eq!(response.expires_at, expires_at);
        assert!(
            response.url.starts_with(
                "/api/v1/uploads/signatures/signature-guest-123.png/download?expires=1800000000&signature="
            )
        );
        let signature = response
            .url
            .split("signature=")
            .nth(1)
            .expect("signature query should be present");
        assert_eq!(
            signature,
            sign_signature_download("secret", "signature-guest-123.png", 1_800_000_000)
                .expect("signature should be reproducible")
        );
        assert!(!constant_time_eq(
            signature.as_bytes(),
            sign_signature_download("other-secret", "signature-guest-123.png", 1_800_000_000)
                .expect("different secret should sign")
                .as_bytes()
        ));
    }

    #[test]
    fn signature_signed_download_rejects_unsafe_file_names() {
        assert!(ensure_signature_file_name("signature-safe.png").is_ok());
        assert!(ensure_signature_file_name("../signature-safe.png").is_err());
        assert!(ensure_signature_file_name("nested/signature-safe.png").is_err());
        assert!(ensure_signature_file_name("signature-safe.jpg").is_err());
        assert!(sign_signature_download("secret", "../signature-safe.png", 1_800_000_000).is_err());
    }

    #[test]
    fn image_dimensions_sync_reads_source_size() {
        let source = ImageBuffer::from_pixel(241, 137, Rgba([64_u8, 128, 192, 255]));
        let mut input = Cursor::new(Vec::new());
        source
            .write_to(&mut input, ImageFormat::Png)
            .expect("test image should encode");

        let dimensions = image_dimensions_sync(&input.into_inner()).expect("dimensions should decode");

        assert_eq!(dimensions, (241, 137));
    }

    #[tokio::test]
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_OBJECT_STORAGE_S3_* env vars"]
    async fn upload_route_round_trips_source_and_thumbnail_against_minio_when_configured() {
        let storage = ObjectStorage::from_env().expect("S3 object storage env should be configured");
        assert_eq!(storage.reference("acceptance/probe.png").backend, "s3");
        if env::var("OPENPR_OBJECT_STORAGE_S3_CREATE_BUCKET_FOR_TEST")
            .ok()
            .as_deref()
            == Some("1")
        {
            storage
                .create_bucket_for_test()
                .await
                .expect("bucket create should succeed");
        }

        let source_bytes = test_png(512, 256);
        let boundary = format!(
            "openpr-minio-upload-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let body = multipart_body(&boundary, "source.png", "image/png", &source_bytes);
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            format!("multipart/form-data; boundary={boundary}")
                .parse()
                .expect("content-type should parse"),
        );

        let uploaded = upload_file_from_multipart(headers, Bytes::from(body))
            .await
            .expect("upload should succeed");
        assert_eq!(uploaded.storage_backend, "s3");
        assert_eq!(uploaded.object_key, uploaded.filename);
        assert_eq!(uploaded.url, format!("/api/v1/uploads/{}", uploaded.filename));
        let thumbnail_key = uploaded
            .thumbnail_object_key
            .as_deref()
            .expect("image upload should create a thumbnail");
        assert!(thumbnail_key.starts_with("thumbnails/"));
        assert_eq!(
            uploaded.thumbnail_url.as_deref(),
            thumbnail_key
                .strip_prefix("thumbnails/")
                .map(|file_name| format!("/api/v1/uploads/thumbnails/{file_name}"))
                .as_deref()
        );

        let stored_source = storage
            .get(&uploaded.object_key)
            .await
            .expect("source object should be in MinIO");
        assert_eq!(stored_source, source_bytes);

        let signed_access = || UploadAccess {
            claims: None,
            bot: None,
            signed: true,
        };
        let source_response = get_uploaded_file(State(test_state()), signed_access(), Path(uploaded.filename.clone()))
            .await
            .expect("source GET should succeed")
            .into_response();
        assert_eq!(source_response.status(), axum::http::StatusCode::OK);
        let source_response_bytes = to_bytes(source_response.into_body(), usize::MAX)
            .await
            .expect("source response body should read");
        assert_eq!(source_response_bytes.as_ref(), source_bytes.as_slice());

        let thumbnail_file_name = thumbnail_key
            .strip_prefix("thumbnails/")
            .expect("thumbnail key should include prefix");
        let thumbnail_response = get_uploaded_thumbnail(
            State(test_state()),
            signed_access(),
            Path(thumbnail_file_name.to_string()),
        )
        .await
        .expect("thumbnail GET should succeed")
        .into_response();
        assert_eq!(thumbnail_response.status(), axum::http::StatusCode::OK);
        let thumbnail_response_bytes = to_bytes(thumbnail_response.into_body(), usize::MAX)
            .await
            .expect("thumbnail response body should read")
            .to_vec();
        assert_eq!(
            image_dimensions(thumbnail_response_bytes)
                .await
                .expect("thumbnail should decode"),
            (THUMBNAIL_MAX_DIMENSION, THUMBNAIL_MAX_DIMENSION / 2)
        );

        storage
            .delete(&uploaded.object_key)
            .await
            .expect("source cleanup should succeed");
        storage
            .delete(thumbnail_key)
            .await
            .expect("thumbnail cleanup should succeed");
    }

    fn test_png(width: u32, height: u32) -> Vec<u8> {
        let source = ImageBuffer::from_pixel(width, height, Rgba([64_u8, 128, 192, 255]));
        let mut input = Cursor::new(Vec::new());
        source
            .write_to(&mut input, ImageFormat::Png)
            .expect("test image should encode");
        input.into_inner()
    }

    fn multipart_body(boundary: &str, file_name: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n").as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    #[test]
    fn a_record_value_holding_a_signature_url_claims_that_object() {
        let file_name = format!("signature-approval-{}.png", Uuid::new_v4());

        assert_eq!(
            signature_object_key_claimed_by_record_value(
                "approval",
                &format!("/api/v1/uploads/signatures/{file_name}")
            ),
            Some(format!("signatures/{file_name}"))
        );
    }

    #[test]
    fn only_values_the_ownership_lookup_matches_count_as_a_claim() {
        let file_name = format!("signature-approval-{}.png", Uuid::new_v4());
        let url = format!("/api/v1/uploads/signatures/{file_name}");

        // A different field key cannot influence the lookup, which keys off the file name.
        assert_eq!(signature_object_key_claimed_by_record_value("notes", &url), None);
        // The legacy spelling is not what the lookup matches on.
        assert_eq!(
            signature_object_key_claimed_by_record_value("approval", &format!("/uploads/signatures/{file_name}")),
            None
        );
        // Free text that merely mentions the URL is not an exact value match either.
        assert_eq!(
            signature_object_key_claimed_by_record_value("approval", &format!("see {url}")),
            None
        );
        assert_eq!(
            signature_object_key_claimed_by_record_value("approval", "/api/v1/uploads/signatures/../../etc/passwd"),
            None
        );
        assert_eq!(signature_object_key_claimed_by_record_value("approval", "Ada"), None);
    }
}
