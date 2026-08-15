use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::{error::ApiError, routes::upload, services::object_storage::ObjectStorage};

const DEFAULT_ATTACHMENT_SIGNED_URL_TTL_MINUTES: i64 = 10;
const MIN_ATTACHMENT_SIGNED_URL_TTL_MINUTES: i64 = 1;
const MAX_ATTACHMENT_SIGNED_URL_TTL_MINUTES: i64 = 24 * 60;
const BYTES_PER_MEGABYTE: i64 = 1024 * 1024;
const MIN_ATTACHMENT_THUMBNAIL_DIMENSION: u32 = 64;
const MAX_ATTACHMENT_THUMBNAIL_DIMENSION: u32 = 2048;
const DEFAULT_ATTACHMENT_PREVIEW_DIMENSION: u32 = 1024;
const MIN_ATTACHMENT_PREVIEW_DIMENSION: u32 = 128;
const MAX_ATTACHMENT_PREVIEW_DIMENSION: u32 = 4096;
const MAX_ATTACHMENT_VARIANTS: usize = 6;

#[derive(Debug, PartialEq, Eq)]
struct AttachmentFieldPolicy {
    accept: BTreeSet<String>,
    max_size_bytes: Option<i64>,
    storage_policy: AttachmentStoragePolicy,
    preview_enabled: bool,
    preview_max_dimension: Option<u32>,
    preview_format: upload::ImageDerivativeFormat,
    thumbnail_enabled: bool,
    thumbnail_max_dimension: Option<u32>,
    thumbnail_format: upload::ImageDerivativeFormat,
    variants: Vec<AttachmentVariantPolicy>,
}

impl Default for AttachmentFieldPolicy {
    fn default() -> Self {
        Self {
            accept: BTreeSet::new(),
            max_size_bytes: None,
            storage_policy: AttachmentStoragePolicy::Local,
            preview_enabled: true,
            preview_max_dimension: None,
            preview_format: upload::ImageDerivativeFormat::Png,
            thumbnail_enabled: true,
            thumbnail_max_dimension: None,
            thumbnail_format: upload::ImageDerivativeFormat::Png,
            variants: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AttachmentVariantPolicy {
    kind: String,
    max_dimension: u32,
    format: upload::ImageDerivativeFormat,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum AttachmentStoragePolicy {
    #[default]
    Local,
    Private,
    External,
}

pub struct AttachmentMediaResult {
    pub thumbnail_url: Option<String>,
    pub media_metadata: Value,
}

pub fn attachment_signed_url_ttl_minutes(schema: &Value, field_key: &str) -> i64 {
    schema
        .get("fields")
        .and_then(Value::as_array)
        .and_then(|fields| {
            fields.iter().find(|field| {
                field
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| key == field_key)
            })
        })
        .and_then(|field| field.get("attachment"))
        .and_then(|attachment| attachment.get("signed_url_ttl_minutes"))
        .and_then(attachment_signed_url_ttl_value)
        .unwrap_or(DEFAULT_ATTACHMENT_SIGNED_URL_TTL_MINUTES)
}

fn attachment_signed_url_ttl_value(value: &Value) -> Option<i64> {
    let minutes = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse::<i64>().ok()))?;
    if minutes <= 0 {
        return None;
    }
    Some(minutes.clamp(
        MIN_ATTACHMENT_SIGNED_URL_TTL_MINUTES,
        MAX_ATTACHMENT_SIGNED_URL_TTL_MINUTES,
    ))
}

pub fn validate_attachment_create_policy(
    schema: &Value,
    field_key: &str,
    file_name: &str,
    content_type: &str,
    byte_size: i64,
    url: &str,
) -> Result<(), ApiError> {
    let Some(policy) = attachment_field_policy(schema, field_key) else {
        return Ok(());
    };
    if !policy.accept.is_empty() && !attachment_accepts_file(&policy.accept, file_name, content_type) {
        return Err(ApiError::BadRequest(
            "attachment content_type is not allowed by field policy".to_string(),
        ));
    }
    if let Some(max_size_bytes) = policy.max_size_bytes
        && byte_size > max_size_bytes
    {
        return Err(ApiError::BadRequest(
            "attachment byte_size exceeds field max_size_mb policy".to_string(),
        ));
    }
    validate_attachment_storage_policy(&policy, url)?;
    Ok(())
}

fn attachment_field_policy(schema: &Value, field_key: &str) -> Option<AttachmentFieldPolicy> {
    let attachment = schema
        .get("fields")
        .and_then(Value::as_array)?
        .iter()
        .find(|field| {
            field
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|key| key == field_key)
        })?
        .get("attachment")?;
    let accept = attachment
        .get("accept")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let max_size_bytes = attachment
        .get("max_size_mb")
        .and_then(attachment_max_size_mb)
        .map(|max_size_mb| max_size_mb * BYTES_PER_MEGABYTE);
    let storage_policy = attachment
        .get("storage_policy")
        .and_then(Value::as_str)
        .map(AttachmentStoragePolicy::from_schema)
        .unwrap_or_default();
    let preview_enabled = attachment.get("preview").and_then(Value::as_bool).unwrap_or(true);
    let preview_max_dimension = attachment
        .get("preview_max_dimension")
        .and_then(attachment_preview_max_dimension);
    let preview_format = attachment_derivative_format(attachment.get("preview_format"));
    let thumbnail_enabled = attachment.get("thumbnail").and_then(Value::as_bool).unwrap_or(true);
    let thumbnail_max_dimension = attachment
        .get("thumbnail_max_dimension")
        .and_then(attachment_thumbnail_max_dimension);
    let thumbnail_format = attachment_derivative_format(attachment.get("thumbnail_format"));
    let variants = attachment_variant_policies(attachment);
    if accept.is_empty()
        && max_size_bytes.is_none()
        && storage_policy == AttachmentStoragePolicy::Local
        && preview_enabled
        && preview_max_dimension.is_none()
        && preview_format == upload::ImageDerivativeFormat::Png
        && thumbnail_enabled
        && thumbnail_max_dimension.is_none()
        && thumbnail_format == upload::ImageDerivativeFormat::Png
        && variants.is_empty()
    {
        return None;
    }
    Some(AttachmentFieldPolicy {
        accept,
        max_size_bytes,
        storage_policy,
        preview_enabled,
        preview_max_dimension,
        preview_format,
        thumbnail_enabled,
        thumbnail_max_dimension,
        thumbnail_format,
        variants,
    })
}

impl AttachmentStoragePolicy {
    fn from_schema(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "private" => Self::Private,
            "external" => Self::External,
            _ => Self::Local,
        }
    }
}

fn attachment_max_size_mb(value: &Value) -> Option<i64> {
    let max_size_mb = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse::<i64>().ok()))?;
    (max_size_mb > 0).then_some(max_size_mb)
}

fn attachment_thumbnail_max_dimension(value: &Value) -> Option<u32> {
    let dimension = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse::<u64>().ok()))?;
    if dimension == 0 {
        return None;
    }
    // The clamp caps this value at MAX_ATTACHMENT_THUMBNAIL_DIMENSION, which is a u32 constant.
    #[allow(clippy::cast_possible_truncation)]
    Some(dimension.clamp(
        u64::from(MIN_ATTACHMENT_THUMBNAIL_DIMENSION),
        u64::from(MAX_ATTACHMENT_THUMBNAIL_DIMENSION),
    ) as u32)
}

fn attachment_preview_max_dimension(value: &Value) -> Option<u32> {
    let dimension = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse::<u64>().ok()))?;
    if dimension == 0 {
        return None;
    }
    // The clamp caps this value at MAX_ATTACHMENT_PREVIEW_DIMENSION, which is a u32 constant.
    #[allow(clippy::cast_possible_truncation)]
    Some(dimension.clamp(
        u64::from(MIN_ATTACHMENT_PREVIEW_DIMENSION),
        u64::from(MAX_ATTACHMENT_PREVIEW_DIMENSION),
    ) as u32)
}

fn attachment_derivative_format(value: Option<&Value>) -> upload::ImageDerivativeFormat {
    value
        .and_then(Value::as_str)
        .and_then(upload::ImageDerivativeFormat::from_schema)
        .unwrap_or(upload::ImageDerivativeFormat::Png)
}

fn attachment_variant_policies(attachment: &Value) -> Vec<AttachmentVariantPolicy> {
    let mut seen = BTreeSet::new();
    attachment
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|variant| {
            if variant.get("enabled").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let kind = attachment_variant_kind(variant.get("kind")?.as_str()?)?;
            if !seen.insert(kind.clone()) {
                return None;
            }
            let max_dimension = variant
                .get("max_dimension")
                .and_then(attachment_preview_max_dimension)
                .unwrap_or(DEFAULT_ATTACHMENT_PREVIEW_DIMENSION);
            let format = attachment_derivative_format(variant.get("format"));
            Some(AttachmentVariantPolicy {
                kind,
                max_dimension,
                format,
            })
        })
        .take(MAX_ATTACHMENT_VARIANTS)
        .collect()
}

fn attachment_variant_kind(raw: &str) -> Option<String> {
    let kind = raw.trim().to_ascii_lowercase().replace([' ', '.'], "-");
    if kind.is_empty()
        || kind == "thumbnail"
        || kind == "preview"
        || kind.len() > 32
        || !kind
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some(kind)
}

pub async fn ensure_attachment_media(
    schema: &Value,
    field_key: &str,
    file_name: &str,
    content_type: &str,
    url: &str,
    requested_thumbnail_url: Option<String>,
) -> Result<AttachmentMediaResult, ApiError> {
    let policy = attachment_field_policy(schema, field_key).unwrap_or_default();
    let requested_thumbnail_url = requested_thumbnail_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let upload_file_name = server_owned_upload_file_name(url);

    if let Some(thumbnail_url) = requested_thumbnail_url.filter(|_| policy.thumbnail_enabled) {
        let media_metadata = attachment_image_media_metadata(
            None,
            None,
            json!({
                "enabled": true,
                "source": "client",
                "url": thumbnail_url,
            }),
            None,
            Vec::new(),
        );
        return Ok(AttachmentMediaResult {
            thumbnail_url: Some(thumbnail_url),
            media_metadata,
        });
    }

    let Some(upload_file_name) = upload_file_name else {
        return Ok(AttachmentMediaResult {
            thumbnail_url: None,
            media_metadata: json!({}),
        });
    };

    if !attachment_is_image(file_name, content_type, upload_file_name) {
        return Ok(AttachmentMediaResult {
            thumbnail_url: None,
            media_metadata: json!({}),
        });
    }

    if !policy.thumbnail_enabled && !policy.preview_enabled {
        let media_metadata = attachment_image_media_metadata(
            None,
            None,
            json!({ "enabled": false }),
            Some(json!({ "enabled": false })),
            Vec::new(),
        );
        return Ok(AttachmentMediaResult {
            thumbnail_url: None,
            media_metadata,
        });
    }

    let object_storage = ObjectStorage::from_runtime_config()?;
    let data = object_storage
        .get(upload_file_name)
        .await
        .map_err(|_| ApiError::NotFound("attachment source file not found".to_string()))?;
    let (source_width, source_height) = upload::image_dimensions(data.clone()).await?;
    let (thumbnail_url, thumbnail_metadata) = if policy.thumbnail_enabled {
        let max_dimension = policy
            .thumbnail_max_dimension
            .unwrap_or(upload::THUMBNAIL_MAX_DIMENSION);
        let thumbnail = upload::build_image_derivative_with_max_dimension_and_format(
            data.clone(),
            max_dimension,
            policy.thumbnail_format,
        )
        .await?;
        let thumbnail_name = upload::thumbnail_file_name_with_dimension_and_format(
            upload_file_name,
            max_dimension,
            policy.thumbnail_format,
        );
        let thumbnail_key = format!("thumbnails/{thumbnail_name}");
        object_storage.put(&thumbnail_key, &thumbnail).await?;
        let thumbnail_url = format!("/api/v1/uploads/thumbnails/{thumbnail_name}");
        (
            Some(thumbnail_url.clone()),
            json!({
                "enabled": true,
                "source": "server",
                "max_dimension": max_dimension,
                "format": policy.thumbnail_format.as_str(),
                "url": thumbnail_url,
                "object_key": thumbnail_key,
            }),
        )
    } else {
        (None, json!({ "enabled": false }))
    };
    let preview_metadata = if policy.preview_enabled {
        let max_dimension = policy
            .preview_max_dimension
            .unwrap_or(DEFAULT_ATTACHMENT_PREVIEW_DIMENSION);
        let preview = upload::build_image_derivative_with_max_dimension_and_format(
            data.clone(),
            max_dimension,
            policy.preview_format,
        )
        .await?;
        let preview_name =
            upload::preview_file_name_with_dimension_and_format(upload_file_name, max_dimension, policy.preview_format);
        let preview_key = format!("previews/{preview_name}");
        object_storage.put(&preview_key, &preview).await?;
        let preview_url = format!("/api/v1/uploads/previews/{preview_name}");
        json!({
            "enabled": true,
            "source": "server",
            "max_dimension": max_dimension,
            "format": policy.preview_format.as_str(),
            "url": preview_url,
            "object_key": preview_key,
        })
    } else {
        json!({ "enabled": false })
    };
    let mut variants = Vec::with_capacity(policy.variants.len());
    for variant in policy.variants {
        let bytes = upload::build_image_derivative_with_max_dimension_and_format(
            data.clone(),
            variant.max_dimension,
            variant.format,
        )
        .await?;
        let variant_name = upload::variant_file_name_with_dimension_and_format(
            &variant.kind,
            upload_file_name,
            variant.max_dimension,
            variant.format,
        );
        let variant_key = format!("variants/{variant_name}");
        object_storage.put(&variant_key, &bytes).await?;
        let variant_url = format!("/api/v1/uploads/variants/{variant_name}");
        variants.push(json!({
            "kind": variant.kind,
            "policy": {
                "enabled": true,
                "source": "server",
                "max_dimension": variant.max_dimension,
                "format": variant.format.as_str(),
                "url": variant_url,
                "object_key": variant_key,
            }
        }));
    }
    let media_metadata = attachment_image_media_metadata(
        Some(source_width),
        Some(source_height),
        thumbnail_metadata,
        Some(preview_metadata),
        variants,
    );
    Ok(AttachmentMediaResult {
        thumbnail_url,
        media_metadata,
    })
}

fn attachment_image_media_metadata(
    source_width: Option<u32>,
    source_height: Option<u32>,
    thumbnail: Value,
    preview: Option<Value>,
    mut variants: Vec<Value>,
) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("media_type".to_string(), json!("image"));
    if let Some(source_width) = source_width {
        metadata.insert("source_width".to_string(), json!(source_width));
    }
    if let Some(source_height) = source_height {
        metadata.insert("source_height".to_string(), json!(source_height));
    }
    metadata.insert("thumbnail".to_string(), thumbnail);
    if let Some(preview) = preview {
        metadata.insert("preview".to_string(), preview.clone());
        variants.insert(0, json!({ "kind": "preview", "policy": preview }));
    }
    if !variants.is_empty() {
        metadata.insert("variants".to_string(), Value::Array(variants));
    }
    Value::Object(metadata)
}

fn attachment_is_image(file_name: &str, content_type: &str, upload_file_name: &str) -> bool {
    if content_type.trim().to_ascii_lowercase().starts_with("image/") {
        return true;
    }
    file_extension(file_name).is_some_and(|ext| upload::is_image_extension(&ext))
        || file_extension(upload_file_name).is_some_and(|ext| upload::is_image_extension(&ext))
}

fn file_extension(file_name: &str) -> Option<String> {
    file_name
        .rsplit_once('.')
        .map(|(_base, ext)| ext.trim().to_ascii_lowercase())
        .filter(|ext| !ext.is_empty())
}

fn attachment_accepts_file(accept: &BTreeSet<String>, file_name: &str, content_type: &str) -> bool {
    let normalized_content_type = content_type.trim().to_ascii_lowercase();
    let extension = file_name
        .rsplit_once('.')
        .map(|(_base, ext)| ext.trim().to_ascii_lowercase())
        .filter(|ext| !ext.is_empty());
    accept.iter().any(|rule| {
        if !normalized_content_type.is_empty() {
            if rule == &normalized_content_type {
                return true;
            }
            if let Some(prefix) = rule.strip_suffix("/*") {
                return normalized_content_type.starts_with(&format!("{prefix}/"));
            }
        }
        extension
            .as_ref()
            .is_some_and(|ext| rule == &format!(".{ext}") || rule == ext || rule == &format!("application/{ext}"))
    })
}

fn validate_attachment_storage_policy(policy: &AttachmentFieldPolicy, url: &str) -> Result<(), ApiError> {
    match policy.storage_policy {
        AttachmentStoragePolicy::Local => Ok(()),
        AttachmentStoragePolicy::Private => {
            if is_server_owned_attachment_url(url) {
                Ok(())
            } else {
                Err(ApiError::BadRequest(
                    "private attachment storage_policy requires a server upload URL".to_string(),
                ))
            }
        }
        AttachmentStoragePolicy::External => {
            if is_external_attachment_url(url) {
                Ok(())
            } else {
                Err(ApiError::BadRequest(
                    "external attachment storage_policy requires an absolute http(s) URL".to_string(),
                ))
            }
        }
    }
}

fn is_server_owned_attachment_url(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("/api/v1/uploads/") || trimmed.starts_with("/uploads/")
}

pub fn server_owned_upload_file_name(url: &str) -> Option<&str> {
    let path = url.split('?').next().unwrap_or(url).trim();
    path.strip_prefix("/api/v1/uploads/")
        .or_else(|| path.strip_prefix("/uploads/"))
        .filter(|file_name| {
            !file_name.is_empty()
                && !file_name.starts_with("thumbnails/")
                && !file_name.starts_with("previews/")
                && !file_name.starts_with("variants/")
                && !file_name.contains('/')
                && !file_name.contains('\\')
                && !file_name.contains("..")
        })
}

fn is_external_attachment_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        attachment_derivative_format, attachment_field_policy, attachment_image_media_metadata, attachment_is_image,
        attachment_preview_max_dimension, attachment_signed_url_ttl_minutes, attachment_thumbnail_max_dimension,
        attachment_variant_kind, attachment_variant_policies, ensure_attachment_media, server_owned_upload_file_name,
        validate_attachment_create_policy,
    };
    use crate::{
        routes::upload,
        services::object_storage::{test_object_storage, test_should_create_bucket},
    };
    use chrono::Utc;
    use image::{ImageBuffer, ImageFormat, Rgb};
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn attachment_signed_url_ttl_defaults_without_policy() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_attachment", "key": "receipt", "type": "attachment"}
            ]
        });

        assert_eq!(attachment_signed_url_ttl_minutes(&schema, "receipt"), 10);
    }

    #[test]
    fn attachment_signed_url_ttl_reads_attachment_policy() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"signed_url_ttl_minutes": 15}
                }
            ]
        });

        assert_eq!(attachment_signed_url_ttl_minutes(&schema, "receipt"), 15);
    }

    #[test]
    fn attachment_signed_url_ttl_clamps_policy_range() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"signed_url_ttl_minutes": 999_999}
                },
                {
                    "field_id": "fld_image",
                    "key": "photo",
                    "type": "image",
                    "attachment": {"signed_url_ttl_minutes": "0"}
                }
            ]
        });

        assert_eq!(attachment_signed_url_ttl_minutes(&schema, "receipt"), 24 * 60);
        assert_eq!(attachment_signed_url_ttl_minutes(&schema, "photo"), 10);
    }

    #[test]
    fn attachment_create_policy_accepts_exact_wildcard_and_extension_rules() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"accept": ["application/pdf", "image/*", ".csv"]}
                }
            ]
        });

        assert!(
            validate_attachment_create_policy(&schema, "receipt", "ticket.pdf", "application/pdf", 512, "").is_ok()
        );
        assert!(validate_attachment_create_policy(&schema, "receipt", "photo.png", "image/png", 512, "").is_ok());
        assert!(validate_attachment_create_policy(&schema, "receipt", "rows.csv", "", 512, "").is_ok());
        assert!(
            validate_attachment_create_policy(&schema, "receipt", "payload.json", "application/json", 512, "").is_err()
        );
    }

    #[test]
    fn attachment_create_policy_enforces_max_size_mb() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"max_size_mb": 2}
                }
            ]
        });

        assert!(
            validate_attachment_create_policy(&schema, "receipt", "ticket.pdf", "application/pdf", 2 * 1024 * 1024, "")
                .is_ok()
        );
        assert!(
            validate_attachment_create_policy(
                &schema,
                "receipt",
                "ticket.pdf",
                "application/pdf",
                2 * 1024 * 1024 + 1,
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn attachment_media_policy_parses_thumbnail_controls() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_image",
                    "key": "photo",
                    "type": "image",
                    "attachment": {
                        "preview": false,
                        "preview_format": "webp",
                        "preview_max_dimension": 1600,
                        "thumbnail": false,
                        "thumbnail_format": "jpeg",
                        "thumbnail_max_dimension": 640,
                        "variants": [
                            {"kind": "Gallery", "max_dimension": 1200, "format": "webp"},
                            {"kind": "gallery", "max_dimension": 2000},
                            {"kind": "disabled", "enabled": false}
                        ]
                    }
                }
            ]
        });

        let policy = attachment_field_policy(&schema, "photo").expect("thumbnail policy should be parsed");
        assert!(!policy.preview_enabled);
        assert_eq!(policy.preview_max_dimension, Some(1600));
        assert_eq!(policy.preview_format, upload::ImageDerivativeFormat::Webp);
        assert!(!policy.thumbnail_enabled);
        assert_eq!(policy.thumbnail_max_dimension, Some(640));
        assert_eq!(policy.thumbnail_format, upload::ImageDerivativeFormat::Jpeg);
        assert_eq!(policy.variants.len(), 1);
        assert_eq!(policy.variants[0].kind, "gallery");
        assert_eq!(policy.variants[0].max_dimension, 1200);
        assert_eq!(policy.variants[0].format, upload::ImageDerivativeFormat::Webp);
    }

    #[test]
    fn attachment_thumbnail_dimension_clamps_policy_range() {
        assert_eq!(attachment_thumbnail_max_dimension(&json!(1)), Some(64));
        assert_eq!(attachment_thumbnail_max_dimension(&json!("640")), Some(640));
        assert_eq!(attachment_thumbnail_max_dimension(&json!(5000)), Some(2048));
        assert_eq!(attachment_thumbnail_max_dimension(&json!(0)), None);
        assert_eq!(attachment_thumbnail_max_dimension(&json!("bad")), None);
    }

    #[test]
    fn attachment_preview_dimension_clamps_policy_range() {
        assert_eq!(attachment_preview_max_dimension(&json!(1)), Some(128));
        assert_eq!(attachment_preview_max_dimension(&json!("1600")), Some(1600));
        assert_eq!(attachment_preview_max_dimension(&json!(5000)), Some(4096));
        assert_eq!(attachment_preview_max_dimension(&json!(0)), None);
        assert_eq!(attachment_preview_max_dimension(&json!("bad")), None);
    }

    #[test]
    fn attachment_derivative_format_parses_supported_formats() {
        assert_eq!(
            attachment_derivative_format(Some(&json!("image/jpeg"))),
            upload::ImageDerivativeFormat::Jpeg
        );
        assert_eq!(
            attachment_derivative_format(Some(&json!("webp"))),
            upload::ImageDerivativeFormat::Webp
        );
        assert_eq!(
            attachment_derivative_format(Some(&json!("tiff"))),
            upload::ImageDerivativeFormat::Png
        );
        assert_eq!(attachment_derivative_format(None), upload::ImageDerivativeFormat::Png);
    }

    #[test]
    fn attachment_variant_policy_sanitizes_and_limits_variants() {
        assert_eq!(attachment_variant_kind("Gallery Card").as_deref(), Some("gallery-card"));
        assert_eq!(attachment_variant_kind("preview"), None);
        assert_eq!(attachment_variant_kind("../secret"), None);

        let attachment = json!({
            "variants": [
                {"kind": "card", "max_dimension": 96, "format": "jpeg"},
                {"kind": "full", "max_dimension": 5000, "format": "webp"},
                {"kind": "disabled", "enabled": false},
                {"kind": "card", "max_dimension": 1024}
            ]
        });
        let variants = attachment_variant_policies(&attachment);
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].kind, "card");
        assert_eq!(variants[0].max_dimension, 128);
        assert_eq!(variants[0].format, upload::ImageDerivativeFormat::Jpeg);
        assert_eq!(variants[1].kind, "full");
        assert_eq!(variants[1].max_dimension, 4096);
        assert_eq!(variants[1].format, upload::ImageDerivativeFormat::Webp);
    }

    #[test]
    fn attachment_is_image_uses_content_type_and_extension() {
        assert!(attachment_is_image("ticket.bin", "image/webp", "server-file.bin"));
        assert!(attachment_is_image("ticket.png", "", "server-file.bin"));
        assert!(attachment_is_image("ticket.bin", "", "server-file.jpg"));
        assert!(!attachment_is_image("ticket.pdf", "application/pdf", "server-file.pdf"));
    }

    #[test]
    fn attachment_image_media_metadata_records_dimensions_and_thumbnail_policy() {
        let metadata = attachment_image_media_metadata(
            Some(1200),
            Some(800),
            json!({
                "enabled": true,
                "source": "server",
                "max_dimension": 640,
                "format": "png",
                "url": "/api/v1/uploads/thumbnails/thumb-photo-640.png",
                "object_key": "thumbnails/thumb-photo-640.png"
            }),
            Some(json!({
                "enabled": true,
                "source": "server",
                "max_dimension": 1600,
                "format": "png",
                "url": "/api/v1/uploads/previews/preview-photo-1600.png",
                "object_key": "previews/preview-photo-1600.png"
            })),
            vec![json!({
                "kind": "gallery",
                "policy": {
                    "enabled": true,
                    "source": "server",
                    "max_dimension": 1200,
                    "format": "png",
                    "url": "/api/v1/uploads/variants/variant-gallery-photo-1200.png",
                    "object_key": "variants/variant-gallery-photo-1200.png"
                }
            })],
        );

        assert_eq!(metadata["media_type"], "image");
        assert_eq!(metadata["source_width"], 1200);
        assert_eq!(metadata["source_height"], 800);
        assert_eq!(metadata["thumbnail"]["enabled"], true);
        assert_eq!(metadata["thumbnail"]["max_dimension"], 640);
        assert_eq!(metadata["thumbnail"]["object_key"], "thumbnails/thumb-photo-640.png");
        assert_eq!(metadata["preview"]["enabled"], true);
        assert_eq!(metadata["preview"]["max_dimension"], 1600);
        assert_eq!(metadata["variants"][0]["kind"], "preview");
        assert_eq!(
            metadata["variants"][0]["policy"]["object_key"],
            "previews/preview-photo-1600.png"
        );
        assert_eq!(metadata["variants"][1]["kind"], "gallery");
        assert_eq!(
            metadata["variants"][1]["policy"]["object_key"],
            "variants/variant-gallery-photo-1200.png"
        );
    }

    #[test]
    fn attachment_create_policy_enforces_explicit_private_storage_policy() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"storage_policy": "private"}
                }
            ]
        });

        assert!(
            validate_attachment_create_policy(
                &schema,
                "receipt",
                "ticket.pdf",
                "application/pdf",
                512,
                "/api/v1/uploads/ticket.pdf"
            )
            .is_ok()
        );
        assert!(
            validate_attachment_create_policy(
                &schema,
                "receipt",
                "ticket.pdf",
                "application/pdf",
                512,
                "https://cdn.example.test/ticket.pdf"
            )
            .is_err()
        );
    }

    #[test]
    fn attachment_create_policy_enforces_explicit_external_storage_policy() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_attachment",
                    "key": "receipt",
                    "type": "attachment",
                    "attachment": {"storage_policy": "external"}
                }
            ]
        });

        assert!(
            validate_attachment_create_policy(
                &schema,
                "receipt",
                "ticket.pdf",
                "application/pdf",
                512,
                "https://cdn.example.test/ticket.pdf"
            )
            .is_ok()
        );
        assert!(
            validate_attachment_create_policy(
                &schema,
                "receipt",
                "ticket.pdf",
                "application/pdf",
                512,
                "/api/v1/uploads/ticket.pdf"
            )
            .is_err()
        );
    }

    #[test]
    fn detects_server_owned_upload_file_names() {
        assert_eq!(
            server_owned_upload_file_name("/api/v1/uploads/photo.png"),
            Some("photo.png")
        );
        assert_eq!(
            server_owned_upload_file_name("/uploads/image.png?download=1"),
            Some("image.png")
        );
        assert_eq!(server_owned_upload_file_name("/uploads/photo.png"), Some("photo.png"));
        assert_eq!(
            server_owned_upload_file_name("https://cdn.example.test/photo.png"),
            None
        );
        assert_eq!(
            server_owned_upload_file_name("/api/v1/uploads/thumbnails/thumb-image.png"),
            None
        );
        assert_eq!(
            server_owned_upload_file_name("/api/v1/uploads/previews/preview-image.png"),
            None
        );
        assert_eq!(
            server_owned_upload_file_name("/api/v1/uploads/variants/variant-gallery-image.png"),
            None
        );
        assert_eq!(server_owned_upload_file_name("/api/v1/uploads/nested/photo.png"), None);
        assert_eq!(server_owned_upload_file_name("/api/v1/uploads/../secret.txt"), None);
    }

    #[tokio::test]
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_TEST_CONFIG pointing at an s3 configuration file"]
    async fn attachment_media_derivatives_round_trip_against_minio_when_configured() {
        let storage = test_object_storage().expect("OPENPR_TEST_CONFIG should describe the s3 backend");
        assert_eq!(storage.reference("acceptance/probe.png").backend, "s3");
        if test_should_create_bucket() {
            storage
                .create_bucket_for_test()
                .await
                .expect("bucket create should succeed");
        }

        let source_name = format!(
            "minio-derivative-source-{}.png",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let source_bytes = test_png(256, 128);
        storage
            .put(&source_name, &source_bytes)
            .await
            .expect("source upload should succeed");

        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_image",
                    "key": "photo",
                    "type": "image",
                    "attachment": {
                        "thumbnail": true,
                        "thumbnail_max_dimension": 64,
                        "thumbnail_format": "jpeg",
                        "preview": true,
                        "preview_max_dimension": 128,
                        "preview_format": "webp",
                        "variants": [
                            {"kind": "gallery", "max_dimension": 192, "format": "jpeg"}
                        ]
                    }
                }
            ]
        });
        let result = ensure_attachment_media(
            &schema,
            "photo",
            &source_name,
            "image/png",
            &format!("/api/v1/uploads/{source_name}"),
            None,
        )
        .await
        .expect("attachment media should be generated");

        let expected_thumbnail_url = format!(
            "/api/v1/uploads/thumbnails/{}",
            upload::thumbnail_file_name_with_dimension_and_format(
                &source_name,
                64,
                upload::ImageDerivativeFormat::Jpeg
            )
        );
        assert_eq!(result.thumbnail_url.as_deref(), Some(expected_thumbnail_url.as_str()));
        assert_eq!(result.media_metadata["media_type"], "image");
        assert_eq!(result.media_metadata["source_width"], 256);
        assert_eq!(result.media_metadata["source_height"], 128);
        assert_eq!(result.media_metadata["thumbnail"]["source"], "server");
        assert_eq!(result.media_metadata["thumbnail"]["max_dimension"], 64);
        assert_eq!(result.media_metadata["thumbnail"]["format"], "jpeg");
        assert_eq!(result.media_metadata["preview"]["source"], "server");
        assert_eq!(result.media_metadata["preview"]["max_dimension"], 128);
        assert_eq!(result.media_metadata["preview"]["format"], "webp");
        assert_eq!(result.media_metadata["variants"][1]["kind"], "gallery");
        assert_eq!(result.media_metadata["variants"][1]["policy"]["max_dimension"], 192);
        assert_eq!(result.media_metadata["variants"][1]["policy"]["format"], "jpeg");

        let thumbnail_key = result.media_metadata["thumbnail"]["object_key"]
            .as_str()
            .expect("thumbnail object key should be recorded");
        let preview_key = result.media_metadata["preview"]["object_key"]
            .as_str()
            .expect("preview object key should be recorded");
        let variant_key = result.media_metadata["variants"][1]["policy"]["object_key"]
            .as_str()
            .expect("variant object key should be recorded");
        let thumbnail_bytes = storage
            .get(thumbnail_key)
            .await
            .expect("thumbnail derivative should be stored in MinIO");
        let preview_bytes = storage
            .get(preview_key)
            .await
            .expect("preview derivative should be stored in MinIO");
        let variant_bytes = storage
            .get(variant_key)
            .await
            .expect("variant derivative should be stored in MinIO");
        assert_eq!(
            upload::image_dimensions(thumbnail_bytes)
                .await
                .expect("thumbnail derivative should decode"),
            (64, 32)
        );
        assert_eq!(
            upload::image_dimensions(preview_bytes)
                .await
                .expect("preview derivative should decode"),
            (128, 64)
        );
        assert_eq!(
            upload::image_dimensions(variant_bytes)
                .await
                .expect("variant derivative should decode"),
            (192, 96)
        );

        storage
            .delete(thumbnail_key)
            .await
            .expect("thumbnail cleanup should succeed");
        storage
            .delete(preview_key)
            .await
            .expect("preview cleanup should succeed");
        storage
            .delete(variant_key)
            .await
            .expect("variant cleanup should succeed");
        storage
            .delete(&source_name)
            .await
            .expect("source cleanup should succeed");
    }

    fn test_png(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
        });
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut output, ImageFormat::Png)
            .expect("test image should encode");
        output.into_inner()
    }
}
