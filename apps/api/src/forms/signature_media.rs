// Extension matching stays case-sensitive to preserve the established upload and media policy.
#![allow(clippy::case_sensitive_file_extension_comparisons)]

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{error::ApiError, services::object_storage::ObjectStorage};

use super::schema::parse_fields;

const MAX_SIGNATURE_BYTES: usize = 2 * 1024 * 1024;

pub struct SignatureMaterialization {
    pub values: Value,
    pub audit_entries: Vec<Value>,
}

pub struct SignatureRetentionCleanup {
    pub values: Value,
    pub audit_entries: Vec<Value>,
    pub deleted_count: usize,
}

pub struct SignatureAuditRetentionCleanup {
    pub source: Value,
    pub audit_entries: Vec<Value>,
    pub deleted_count: usize,
}

pub struct SignatureAuditVerification {
    pub result: Value,
    pub verified: bool,
}

pub struct SignatureAuditVerificationSummary {
    pub result: Value,
    pub total_count: usize,
    pub verified_count: usize,
    pub failed_count: usize,
}

pub async fn materialize_signature_values(schema: &Value, values: Value) -> Result<Value, ApiError> {
    Ok(materialize_signature_values_with_audit(schema, values).await?.values)
}

pub async fn materialize_signature_values_with_audit(
    schema: &Value,
    values: Value,
) -> Result<SignatureMaterialization, ApiError> {
    materialize_signature_values_with_existing_storage(schema, values, None, None).await
}

pub async fn materialize_signature_values_with_existing_audit(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<SignatureMaterialization, ApiError> {
    materialize_signature_values_with_existing_storage(schema, values, Some(existing_values), None).await
}

#[cfg(test)]
async fn materialize_signature_values_with_storage(
    schema: &Value,
    values: Value,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureMaterialization, ApiError> {
    materialize_signature_values_with_existing_storage(schema, values, None, storage_override).await
}

async fn materialize_signature_values_with_existing_storage(
    schema: &Value,
    values: Value,
    existing_values: Option<&Value>,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureMaterialization, ApiError> {
    let mut object = values
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("values must be a JSON object".to_string()))?;
    let mut audit_entries = Vec::new();
    let signature_fields = parse_fields(schema)
        .map_err(ApiError::BadRequest)?
        .into_iter()
        .filter(|field| field.field_type == "signature")
        .collect::<Vec<_>>();
    if signature_fields.is_empty() {
        return Ok(SignatureMaterialization {
            values: Value::Object(object),
            audit_entries,
        });
    }

    let mut storage = storage_override;
    for field in signature_fields {
        let Some(value) = object.get(&field.key).and_then(Value::as_str) else {
            continue;
        };
        let Some(bytes) = decode_signature_data_url(value)? else {
            continue;
        };
        let object_storage = match &storage {
            Some(object_storage) => object_storage,
            None => storage.insert(ObjectStorage::from_runtime_config()?),
        };
        let file_name = format!("signature-{}-{}.png", field.key, Uuid::new_v4());
        let object_key = format!("signatures/{file_name}");
        let object_ref = object_storage.put(&object_key, &bytes).await?;
        let url = format!("/api/v1/uploads/signatures/{file_name}");
        let mut audit_entry = serde_json::Map::new();
        audit_entry.insert("action".to_string(), json!("materialize"));
        audit_entry.insert("field_key".to_string(), json!(field.key));
        audit_entry.insert("file_name".to_string(), json!(file_name));
        audit_entry.insert("url".to_string(), json!(url));
        audit_entry.insert("object_key".to_string(), json!(object_ref.key));
        audit_entry.insert("storage_backend".to_string(), json!(object_ref.backend));
        audit_entry.insert("content_type".to_string(), json!("image/png"));
        audit_entry.insert("byte_size".to_string(), json!(bytes.len()));
        audit_entry.insert("sha256".to_string(), json!(signature_sha256_hex(&bytes)));
        audit_entry.insert("materialized_at".to_string(), json!(Utc::now()));
        if let Some(previous_url) = existing_values
            .and_then(|values| values.get(&field.key))
            .and_then(Value::as_str)
            && let Some(previous_object_key) = signature_object_key_from_url(previous_url)
        {
            audit_entry.insert("previous_url".to_string(), json!(previous_url));
            audit_entry.insert("previous_object_key".to_string(), json!(previous_object_key));
        }
        if let Some(signature) = &field.signature {
            if let Some(reason_field) = signature.reason_field.as_deref() {
                let reason = object
                    .get(reason_field)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if signature.reason_required && reason.is_none() {
                    return Err(ApiError::BadRequest(format!(
                        "signature reason field is required for {}: {}",
                        field.key, reason_field
                    )));
                }
                audit_entry.insert("reason_field".to_string(), json!(reason_field));
                if let Some(reason) = reason {
                    audit_entry.insert("reason".to_string(), json!(reason));
                }
                audit_entry.insert("reason_required".to_string(), json!(signature.reason_required));
            }
            if let Some(consent_statement) = signature.consent_statement.as_deref() {
                audit_entry.insert("consent_statement".to_string(), json!(consent_statement));
            }
        }
        audit_entries.push(Value::Object(audit_entry));
        object.insert(field.key, json!(url));
    }

    Ok(SignatureMaterialization {
        values: Value::Object(object),
        audit_entries,
    })
}

fn decode_signature_data_url(value: &str) -> Result<Option<Vec<u8>>, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("/api/v1/uploads/signatures/")
        || trimmed.starts_with("/uploads/signatures/")
    {
        return Ok(None);
    }
    let Some(encoded) = trimmed.strip_prefix("data:image/png;base64,") else {
        if trimmed.starts_with("data:image/") {
            return Err(ApiError::BadRequest(
                "signature data URL must be image/png base64".to_string(),
            ));
        }
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::BadRequest("signature data URL is not valid base64".to_string()))?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("signature image cannot be empty".to_string()));
    }
    if bytes.len() > MAX_SIGNATURE_BYTES {
        return Err(ApiError::BadRequest(
            "signature image must be less than or equal to 2MB".to_string(),
        ));
    }
    Ok(Some(bytes))
}

pub fn append_signature_audit_source(source: Value, audit_entries: Vec<Value>) -> Result<Value, ApiError> {
    if audit_entries.is_empty() {
        return Ok(source);
    }
    let mut object = source
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("source must be a JSON object".to_string()))?;
    let mut signature_media = object
        .remove("signature_media")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    signature_media.insert("version".to_string(), json!("v1"));
    let mut entries = signature_media
        .remove("entries")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    entries.extend(audit_entries);
    signature_media.insert("entries".to_string(), Value::Array(entries));
    object.insert("signature_media".to_string(), Value::Object(signature_media));
    Ok(Value::Object(object))
}

pub fn signature_lifecycle_summary(schema: &Value, values: &Value, source: &Value) -> Result<Value, ApiError> {
    let fields = parse_fields(schema)
        .map_err(ApiError::BadRequest)?
        .into_iter()
        .filter(|field| field.field_type == "signature")
        .collect::<Vec<_>>();
    let entries = source
        .get("signature_media")
        .and_then(|value| value.get("entries"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut field_summaries = Vec::new();
    let mut event_summaries = Vec::new();
    let mut materialize_count = 0usize;
    let mut retention_delete_count = 0usize;
    let mut replacement_retention_delete_count = 0usize;
    let mut verifiable_count = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let action = entry.get("action").and_then(Value::as_str).unwrap_or("unknown");
        match action {
            "materialize" => materialize_count += 1,
            "retention_delete" => retention_delete_count += 1,
            "replacement_retention_delete" => replacement_retention_delete_count += 1,
            _ => {}
        }
        if is_verifiable_signature_audit_entry(entry) {
            verifiable_count += 1;
        }
        event_summaries.push(json!({
            "index": index,
            "action": action,
            "field_key": entry.get("field_key").and_then(Value::as_str),
            "object_key": entry.get("object_key").and_then(Value::as_str),
            "previous_object_key": entry.get("previous_object_key").and_then(Value::as_str),
            "retention_days": entry.get("retention_days").and_then(Value::as_i64)
                .or_else(|| entry.get("previous_retention_days").and_then(Value::as_i64)),
            "happened_at": signature_lifecycle_event_timestamp(entry),
            "actor_type": entry.get("actor_type").and_then(Value::as_str),
            "actor_id": entry.get("actor_id").and_then(Value::as_str),
            "operation": entry.get("operation").and_then(Value::as_str),
            "schema_version": entry.get("schema_version").and_then(Value::as_i64),
            "reason_present": entry.get("reason").and_then(Value::as_str).is_some_and(|value| !value.trim().is_empty()),
            "consent_statement_present": entry.get("consent_statement").and_then(Value::as_str).is_some_and(|value| !value.trim().is_empty()),
            "verifiable": is_verifiable_signature_audit_entry(entry),
        }));
    }

    for field in fields {
        let current_url = values
            .get(&field.key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let current_object_key = current_url.as_deref().and_then(signature_object_key_from_url);
        let field_entries = entries
            .iter()
            .filter(|entry| entry.get("field_key").and_then(Value::as_str) == Some(field.key.as_str()))
            .collect::<Vec<_>>();
        let field_materialize_count = field_entries
            .iter()
            .filter(|entry| entry.get("action").and_then(Value::as_str) == Some("materialize"))
            .count();
        let field_retention_delete_count = field_entries
            .iter()
            .filter(|entry| entry.get("action").and_then(Value::as_str) == Some("retention_delete"))
            .count();
        let field_replacement_delete_count = field_entries
            .iter()
            .filter(|entry| entry.get("action").and_then(Value::as_str) == Some("replacement_retention_delete"))
            .count();
        let current_audit_entry_count = field_entries
            .iter()
            .filter(|entry| {
                entry.get("action").and_then(Value::as_str) == Some("materialize")
                    && entry.get("object_key").and_then(Value::as_str) == current_object_key.as_deref()
            })
            .count();
        let replacement_count = field_entries
            .iter()
            .filter(|entry| entry.get("previous_object_key").and_then(Value::as_str).is_some())
            .count();
        let previous_objects = field_entries
            .iter()
            .filter_map(|entry| {
                let previous_object_key = entry.get("previous_object_key").and_then(Value::as_str)?;
                Some(json!({
                    "object_key": previous_object_key,
                    "url": entry.get("previous_url").and_then(Value::as_str),
                    "deleted_at": entry.get("previous_deleted_at").and_then(Value::as_str),
                    "retention_days": entry.get("previous_retention_days").and_then(Value::as_i64),
                }))
            })
            .collect::<Vec<_>>();
        let deleted_objects = field_entries
            .iter()
            .filter_map(|entry| match entry.get("action").and_then(Value::as_str) {
                Some("retention_delete" | "replacement_retention_delete") => Some(json!({
                    "action": entry.get("action").and_then(Value::as_str),
                    "object_key": entry.get("object_key").and_then(Value::as_str),
                    "url": entry.get("url").and_then(Value::as_str),
                    "deleted_at": entry.get("deleted_at").and_then(Value::as_str),
                    "retention_days": entry.get("retention_days").and_then(Value::as_i64),
                })),
                _ => None,
            })
            .collect::<Vec<_>>();
        let latest_event_at = field_entries
            .iter()
            .filter_map(|entry| {
                signature_lifecycle_event_timestamp(entry).and_then(|value| parse_signature_audit_timestamp(&value))
            })
            .max()
            .map(|timestamp| timestamp.to_rfc3339());
        let status = if current_object_key.is_some() && current_audit_entry_count > 0 {
            "active_with_audit"
        } else if current_object_key.is_some() {
            "active_without_audit"
        } else if field_retention_delete_count > 0 {
            "retention_deleted"
        } else if field_materialize_count > 0 {
            "inactive_with_audit"
        } else {
            "empty"
        };

        field_summaries.push(json!({
            "field_key": field.key,
            "retention_days": field.signature.and_then(|signature| signature.retention_days),
            "current_url": current_url,
            "current_object_key": current_object_key,
            "status": status,
            "materialize_count": field_materialize_count,
            "current_audit_entry_count": current_audit_entry_count,
            "replacement_count": replacement_count,
            "retention_delete_count": field_retention_delete_count,
            "replacement_retention_delete_count": field_replacement_delete_count,
            "previous_objects": previous_objects,
            "deleted_objects": deleted_objects,
            "latest_event_at": latest_event_at,
        }));
    }

    Ok(json!({
        "version": "v1",
        "signature_field_count": field_summaries.len(),
        "event_count": entries.len(),
        "materialize_count": materialize_count,
        "retention_delete_count": retention_delete_count,
        "replacement_retention_delete_count": replacement_retention_delete_count,
        "verifiable_event_count": verifiable_count,
        "fields": field_summaries,
        "events": event_summaries,
    }))
}

pub fn signature_workflow_verification_summary(audit: &SignatureAuditVerificationSummary, lifecycle: &Value) -> Value {
    let fields = lifecycle
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let events = lifecycle
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let signature_field_count = lifecycle
        .get("signature_field_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::try_from(fields.len()).unwrap_or(u64::MAX));
    let materialize_count = lifecycle
        .get("materialize_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let verifiable_event_count = lifecycle
        .get("verifiable_event_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let retention_delete_count = lifecycle
        .get("retention_delete_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let replacement_retention_delete_count = lifecycle
        .get("replacement_retention_delete_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    let mut active_signature_field_count = 0u64;
    let mut active_with_audit_count = 0u64;
    let mut active_without_audit_count = 0u64;
    let mut current_audit_entry_count = 0u64;
    for field in &fields {
        let status = field.get("status").and_then(Value::as_str).unwrap_or_default();
        let field_current_audit_count = field
            .get("current_audit_entry_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        current_audit_entry_count = current_audit_entry_count.saturating_add(field_current_audit_count);
        if matches!(status, "active_with_audit" | "active_without_audit") {
            active_signature_field_count = active_signature_field_count.saturating_add(1);
        }
        if status == "active_with_audit" {
            active_with_audit_count = active_with_audit_count.saturating_add(1);
        }
        if status == "active_without_audit" {
            active_without_audit_count = active_without_audit_count.saturating_add(1);
        }
    }

    let reasoned_event_count = events
        .iter()
        .filter(|event| event.get("reason_present").and_then(Value::as_bool) == Some(true))
        .count();
    let consented_event_count = events
        .iter()
        .filter(|event| event.get("consent_statement_present").and_then(Value::as_bool) == Some(true))
        .count();
    let actor_attributed_event_count = events
        .iter()
        .filter(|event| event.get("actor_id").and_then(Value::as_str).is_some())
        .count();
    let operation_attributed_event_count = events
        .iter()
        .filter(|event| event.get("operation").and_then(Value::as_str).is_some())
        .count();

    let verified_audit_count = u64::try_from(audit.verified_count).unwrap_or(u64::MAX);
    let failed_audit_count = u64::try_from(audit.failed_count).unwrap_or(u64::MAX);
    let total_audit_count = u64::try_from(audit.total_count).unwrap_or(u64::MAX);
    let status = if signature_field_count == 0 || (active_signature_field_count == 0 && materialize_count == 0) {
        "unsigned"
    } else if failed_audit_count > 0 {
        "failed"
    } else if active_without_audit_count > 0
        || active_with_audit_count < active_signature_field_count
        || current_audit_entry_count == 0
        || verified_audit_count < current_audit_entry_count
    {
        "incomplete"
    } else {
        "verified"
    };

    json!({
        "version": "v1",
        "status": status,
        "verified": status == "verified",
        "signature_field_count": signature_field_count,
        "active_signature_field_count": active_signature_field_count,
        "active_with_audit_count": active_with_audit_count,
        "active_without_audit_count": active_without_audit_count,
        "current_audit_entry_count": current_audit_entry_count,
        "total_audit_count": total_audit_count,
        "verified_audit_count": verified_audit_count,
        "failed_audit_count": failed_audit_count,
        "materialize_count": materialize_count,
        "verifiable_event_count": verifiable_event_count,
        "reasoned_event_count": reasoned_event_count,
        "consented_event_count": consented_event_count,
        "actor_attributed_event_count": actor_attributed_event_count,
        "operation_attributed_event_count": operation_attributed_event_count,
        "retention_delete_count": retention_delete_count,
        "replacement_retention_delete_count": replacement_retention_delete_count,
    })
}

fn signature_lifecycle_event_timestamp(entry: &Value) -> Option<String> {
    entry
        .get("materialized_at")
        .or_else(|| entry.get("deleted_at"))
        .or_else(|| entry.get("previous_deleted_at"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn annotate_signature_audit_entries(
    audit_entries: Vec<Value>,
    actor_type: &str,
    actor_id: Uuid,
    operation: &str,
    form_id: Uuid,
    record_id: Uuid,
    schema_version: i32,
) -> Vec<Value> {
    audit_entries
        .into_iter()
        .map(|entry| {
            let mut object = entry.as_object().cloned().unwrap_or_default();
            object.insert("actor_type".to_string(), json!(actor_type));
            object.insert("actor_id".to_string(), json!(actor_id));
            object.insert("operation".to_string(), json!(operation));
            object.insert("form_id".to_string(), json!(form_id));
            object.insert("record_id".to_string(), json!(record_id));
            object.insert("schema_version".to_string(), json!(schema_version));
            Value::Object(object)
        })
        .collect()
}

pub async fn verify_signature_audit_entry(entry: &Value) -> Result<SignatureAuditVerification, ApiError> {
    verify_signature_audit_entry_with_storage(entry, None).await
}

pub async fn verify_signature_audit_entry_with_storage(
    entry: &Value,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureAuditVerification, ApiError> {
    let object = entry
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("signature audit entry must be an object".to_string()))?;
    let object_key = object
        .get("object_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("signature audit entry object_key is missing".to_string()))?;
    let expected_sha256 = object
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("signature audit entry sha256 is missing".to_string()))?;
    let expected_byte_size = object
        .get("byte_size")
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::BadRequest("signature audit entry byte_size is missing".to_string()))?;
    let configured_storage;
    let object_storage = if let Some(object_storage) = &storage_override {
        object_storage
    } else {
        configured_storage = ObjectStorage::from_runtime_config()?;
        &configured_storage
    };
    let bytes = object_storage.get(object_key).await?;
    let actual_sha256 = signature_sha256_hex(&bytes);
    let actual_byte_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let verified = actual_sha256 == expected_sha256 && actual_byte_size == expected_byte_size;
    Ok(SignatureAuditVerification {
        verified,
        result: json!({
            "object_key": object_key,
            "expected_sha256": expected_sha256,
            "actual_sha256": actual_sha256,
            "expected_byte_size": expected_byte_size,
            "actual_byte_size": actual_byte_size,
            "verified": verified,
            "verified_at": Utc::now()
        }),
    })
}

pub async fn verify_signature_audit_source(source: &Value) -> Result<SignatureAuditVerificationSummary, ApiError> {
    verify_signature_audit_source_with_storage(source, None).await
}

pub async fn verify_signature_audit_source_with_storage(
    source: &Value,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureAuditVerificationSummary, ApiError> {
    let entries = source
        .get("signature_media")
        .and_then(|value| value.get("entries"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut results = Vec::new();
    let mut total_count = 0;
    let mut verified_count = 0;
    let mut failed_count = 0;
    for entry in entries {
        if !is_verifiable_signature_audit_entry(&entry) {
            continue;
        }
        total_count += 1;
        match verify_signature_audit_entry_with_storage(&entry, storage_override.clone()).await {
            Ok(verification) => {
                if verification.verified {
                    verified_count += 1;
                } else {
                    failed_count += 1;
                }
                results.push(verification.result);
            }
            Err(error) => {
                failed_count += 1;
                results.push(json!({
                    "object_key": entry.get("object_key").and_then(Value::as_str),
                    "verified": false,
                    "error": error.to_string(),
                    "verified_at": Utc::now()
                }));
            }
        }
    }
    Ok(SignatureAuditVerificationSummary {
        result: json!({
            "total_count": total_count,
            "verified_count": verified_count,
            "failed_count": failed_count,
            "results": results
        }),
        total_count,
        verified_count,
        failed_count,
    })
}

fn is_verifiable_signature_audit_entry(entry: &Value) -> bool {
    entry.get("object_key").and_then(Value::as_str).is_some()
        && entry.get("sha256").and_then(Value::as_str).is_some()
        && entry.get("byte_size").and_then(Value::as_u64).is_some()
}

fn signature_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn signature_retention_days(schema: &Value, field_key: &str) -> Result<Option<i64>, ApiError> {
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    Ok(fields
        .into_iter()
        .find(|field| field.field_type == "signature" && field.key == field_key)
        .and_then(|field| field.signature.and_then(|signature| signature.retention_days)))
}

pub fn signature_object_key_from_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let path = trimmed
        .strip_prefix("/api/v1/uploads/signatures/")
        .or_else(|| trimmed.strip_prefix("/uploads/signatures/"))?;
    let file_name = path.split_once('?').map_or(path, |(file_name, _query)| file_name);
    if !is_safe_signature_file_name(file_name) {
        return None;
    }
    Some(format!("signatures/{file_name}"))
}

pub fn expired_signature_object_key(
    schema: &Value,
    field_key: &str,
    value: &str,
    retained_since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<String>, ApiError> {
    let Some(retention_days) = signature_retention_days(schema, field_key)? else {
        return Ok(None);
    };
    let Some(object_key) = signature_object_key_from_url(value) else {
        return Ok(None);
    };
    if now >= retained_since + Duration::days(retention_days) {
        Ok(Some(object_key))
    } else {
        Ok(None)
    }
}

pub async fn cleanup_expired_signature_values(
    schema: &Value,
    values: Value,
    retained_since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<SignatureRetentionCleanup, ApiError> {
    cleanup_expired_signature_values_with_storage(schema, values, retained_since, now, None).await
}

pub async fn cleanup_expired_signature_values_with_storage(
    schema: &Value,
    values: Value,
    retained_since: DateTime<Utc>,
    now: DateTime<Utc>,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureRetentionCleanup, ApiError> {
    let mut object = values
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("values must be a JSON object".to_string()))?;
    let signature_fields = parse_fields(schema)
        .map_err(ApiError::BadRequest)?
        .into_iter()
        .filter(|field| field.field_type == "signature")
        .collect::<Vec<_>>();
    if signature_fields.is_empty() {
        return Ok(SignatureRetentionCleanup {
            values: Value::Object(object),
            audit_entries: Vec::new(),
            deleted_count: 0,
        });
    }

    let mut storage = storage_override;
    let mut audit_entries = Vec::new();
    let mut deleted_count = 0;
    for field in signature_fields {
        let Some(retention_days) = field.signature.and_then(|signature| signature.retention_days) else {
            continue;
        };
        let Some(value) = object.get(&field.key).and_then(Value::as_str) else {
            continue;
        };
        let Some(object_key) = signature_object_key_from_url(value) else {
            continue;
        };
        if now < retained_since + Duration::days(retention_days) {
            continue;
        }
        let object_storage = match &storage {
            Some(object_storage) => object_storage,
            None => storage.insert(ObjectStorage::from_runtime_config()?),
        };
        object_storage.delete(&object_key).await?;
        let storage_ref = object_storage.reference(object_key.clone());
        audit_entries.push(json!({
            "action": "retention_delete",
            "field_key": field.key,
            "url": value,
            "object_key": storage_ref.key,
            "storage_backend": storage_ref.backend,
            "retention_days": retention_days,
            "retained_since": retained_since,
            "deleted_at": now
        }));
        object.insert(field.key, Value::Null);
        deleted_count += 1;
    }

    Ok(SignatureRetentionCleanup {
        values: Value::Object(object),
        audit_entries,
        deleted_count,
    })
}

pub async fn cleanup_expired_replaced_signature_audit_entries(
    schema: &Value,
    values: &Value,
    source: Value,
    now: DateTime<Utc>,
) -> Result<SignatureAuditRetentionCleanup, ApiError> {
    cleanup_expired_replaced_signature_audit_entries_with_storage(schema, values, source, now, None).await
}

pub async fn cleanup_expired_replaced_signature_audit_entries_with_storage(
    schema: &Value,
    values: &Value,
    source: Value,
    now: DateTime<Utc>,
    storage_override: Option<ObjectStorage>,
) -> Result<SignatureAuditRetentionCleanup, ApiError> {
    let mut source_object = source
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("source must be a JSON object".to_string()))?;
    let Some(mut signature_media) = source_object
        .remove("signature_media")
        .and_then(|value| value.as_object().cloned())
    else {
        return Ok(SignatureAuditRetentionCleanup {
            source: Value::Object(source_object),
            audit_entries: Vec::new(),
            deleted_count: 0,
        });
    };
    let mut entries = signature_media
        .remove("entries")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut storage = storage_override;
    let mut audit_entries = Vec::new();
    let mut deleted_count = 0;

    for entry in &mut entries {
        let Some(entry_object) = entry.as_object_mut() else {
            continue;
        };
        if entry_object.contains_key("previous_deleted_at") {
            continue;
        }
        let Some(field_key) = entry_object
            .get("field_key")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some(previous_object_key) = entry_object
            .get("previous_object_key")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        if current_signature_object_key(values, &field_key).as_deref() == Some(previous_object_key.as_str()) {
            continue;
        }
        let Some(retention_days) = signature_retention_days(schema, &field_key)? else {
            continue;
        };
        let Some(retained_since) = entry_object
            .get("materialized_at")
            .and_then(Value::as_str)
            .and_then(parse_signature_audit_timestamp)
        else {
            continue;
        };
        if now < retained_since + Duration::days(retention_days) {
            continue;
        }
        let object_storage = match &storage {
            Some(object_storage) => object_storage,
            None => storage.insert(ObjectStorage::from_runtime_config()?),
        };
        object_storage.delete(&previous_object_key).await?;
        let storage_ref = object_storage.reference(previous_object_key);
        let previous_url = entry_object
            .get("previous_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        entry_object.insert("previous_deleted_at".to_string(), json!(now));
        entry_object.insert("previous_retention_days".to_string(), json!(retention_days));
        audit_entries.push(json!({
            "action": "replacement_retention_delete",
            "field_key": field_key,
            "url": previous_url,
            "object_key": storage_ref.key,
            "storage_backend": storage_ref.backend,
            "retention_days": retention_days,
            "retained_since": retained_since,
            "deleted_at": now
        }));
        deleted_count += 1;
    }

    signature_media.insert("version".to_string(), json!("v1"));
    signature_media.insert("entries".to_string(), Value::Array(entries));
    source_object.insert("signature_media".to_string(), Value::Object(signature_media));
    Ok(SignatureAuditRetentionCleanup {
        source: Value::Object(source_object),
        audit_entries,
        deleted_count,
    })
}

fn current_signature_object_key(values: &Value, field_key: &str) -> Option<String> {
    values
        .get(field_key)
        .and_then(Value::as_str)
        .and_then(signature_object_key_from_url)
}

fn parse_signature_audit_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn is_safe_signature_file_name(file_name: &str) -> bool {
    !file_name.is_empty()
        && file_name.ends_with(".png")
        && !file_name.contains('/')
        && !file_name.contains('\\')
        && !file_name.contains("..")
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        SignatureAuditVerificationSummary, annotate_signature_audit_entries, append_signature_audit_source,
        cleanup_expired_replaced_signature_audit_entries_with_storage, cleanup_expired_signature_values_with_storage,
        decode_signature_data_url, expired_signature_object_key, materialize_signature_values_with_existing_storage,
        materialize_signature_values_with_storage, signature_lifecycle_summary, signature_object_key_from_url,
        signature_retention_days, signature_sha256_hex, signature_workflow_verification_summary,
        verify_signature_audit_entry_with_storage, verify_signature_audit_source_with_storage,
    };
    use crate::{error::ApiError, services::object_storage::ObjectStorage};
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;
    use std::{fs, path::PathBuf};
    use uuid::Uuid;

    #[test]
    fn decodes_png_signature_data_url() {
        let bytes = decode_signature_data_url("data:image/png;base64,c2lnbmF0dXJl")
            .expect("data URL should decode")
            .expect("data URL should materialize");

        assert_eq!(bytes, b"signature");
    }

    #[test]
    fn leaves_existing_signature_urls_unchanged() {
        assert!(
            decode_signature_data_url("/api/v1/uploads/signatures/signature-1.png")
                .expect("signature url should be accepted")
                .is_none()
        );
    }

    #[test]
    fn rejects_non_png_signature_data_urls() {
        assert!(matches!(
            decode_signature_data_url("data:image/jpeg;base64,c2ln"),
            Err(ApiError::BadRequest(message)) if message.contains("image/png")
        ));
    }

    #[test]
    fn reads_signature_retention_policy() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 30}},
                {"field_id": "fld_note", "key": "note", "type": "text"}
            ]
        });

        assert_eq!(
            signature_retention_days(&schema, "guest_signature").expect("policy should parse"),
            Some(30)
        );
        assert_eq!(
            signature_retention_days(&schema, "note").expect("missing signature policy should parse"),
            None
        );
    }

    #[test]
    fn signature_object_key_is_extracted_from_server_urls() {
        assert_eq!(
            signature_object_key_from_url("/api/v1/uploads/signatures/signature-guest-123.png"),
            Some("signatures/signature-guest-123.png".to_string())
        );
        assert_eq!(
            signature_object_key_from_url("/uploads/signatures/signature-guest-123.png?download=1"),
            Some("signatures/signature-guest-123.png".to_string())
        );
        assert_eq!(signature_object_key_from_url("/uploads/signatures/../secret.png"), None);
        assert_eq!(
            signature_object_key_from_url("/uploads/signatures/signature-guest-123.jpg"),
            None
        );
    }

    #[test]
    fn expired_signature_object_key_honors_retention_days() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 7}}
            ]
        });
        let retained_since = Utc
            .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
            .single()
            .expect("timestamp should be valid");
        let value = "/api/v1/uploads/signatures/signature-guest-123.png";

        assert_eq!(
            expired_signature_object_key(
                &schema,
                "guest_signature",
                value,
                retained_since,
                retained_since + Duration::days(6)
            )
            .expect("retention should evaluate"),
            None
        );
        assert_eq!(
            expired_signature_object_key(
                &schema,
                "guest_signature",
                value,
                retained_since,
                retained_since + Duration::days(7)
            )
            .expect("retention should evaluate"),
            Some("signatures/signature-guest-123.png".to_string())
        );
    }

    #[tokio::test]
    async fn cleanup_expired_signature_values_deletes_objects_and_records_audit() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-expired.png", b"expired")
            .await
            .expect("expired signature should be stored");
        storage
            .put("signatures/signature-guest-fresh.png", b"fresh")
            .await
            .expect("fresh signature should be stored");
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 7}},
                {"field_id": "fld_fresh", "key": "fresh_signature", "type": "signature", "signature": {"retention_days": 30}},
                {"field_id": "fld_note", "key": "note", "type": "text"}
            ]
        });
        let retained_since = Utc
            .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
            .single()
            .expect("timestamp should be valid");
        let now = retained_since + Duration::days(8);

        let cleanup = cleanup_expired_signature_values_with_storage(
            &schema,
            json!({
                "guest_signature": "/api/v1/uploads/signatures/signature-guest-expired.png",
                "fresh_signature": "/api/v1/uploads/signatures/signature-guest-fresh.png",
                "note": "keep me"
            }),
            retained_since,
            now,
            Some(storage.clone()),
        )
        .await
        .expect("expired signature should clean up");

        assert_eq!(cleanup.deleted_count, 1);
        assert!(cleanup.values["guest_signature"].is_null());
        assert_eq!(
            cleanup.values["fresh_signature"],
            "/api/v1/uploads/signatures/signature-guest-fresh.png"
        );
        assert_eq!(cleanup.values["note"], "keep me");
        assert!(storage.get("signatures/signature-guest-expired.png").await.is_err());
        assert_eq!(
            storage
                .get("signatures/signature-guest-fresh.png")
                .await
                .expect("fresh signature should remain"),
            b"fresh"
        );
        assert_eq!(cleanup.audit_entries.len(), 1);
        let entry = &cleanup.audit_entries[0];
        assert_eq!(entry["action"], "retention_delete");
        assert_eq!(entry["field_key"], "guest_signature");
        assert_eq!(entry["object_key"], "signatures/signature-guest-expired.png");
        assert_eq!(entry["storage_backend"], "local");
        assert_eq!(entry["retention_days"], 7);
        assert_eq!(entry["retained_since"].as_str(), Some("2026-06-01T00:00:00Z"));
        assert_eq!(entry["deleted_at"].as_str(), Some("2026-06-09T00:00:00Z"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cleanup_expired_replaced_signature_objects_from_audit_source() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-old.png", b"old-signature")
            .await
            .expect("old signature should be stored");
        storage
            .put("signatures/signature-guest-current.png", b"current-signature")
            .await
            .expect("current signature should be stored");
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 7}}
            ]
        });
        let retained_since = Utc
            .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
            .single()
            .expect("timestamp should be valid");
        let now = retained_since + Duration::days(8);
        let source = json!({
            "type": "user",
            "signature_media": {
                "version": "v1",
                "entries": [
                    {
                        "action": "materialize",
                        "field_key": "guest_signature",
                        "object_key": "signatures/signature-guest-current.png",
                        "previous_url": "/api/v1/uploads/signatures/signature-guest-old.png",
                        "previous_object_key": "signatures/signature-guest-old.png",
                        "materialized_at": retained_since
                    }
                ]
            }
        });

        let cleanup = cleanup_expired_replaced_signature_audit_entries_with_storage(
            &schema,
            &json!({
                "guest_signature": "/api/v1/uploads/signatures/signature-guest-current.png"
            }),
            source,
            now,
            Some(storage.clone()),
        )
        .await
        .expect("replaced signature should clean up");

        assert_eq!(cleanup.deleted_count, 1);
        assert!(storage.get("signatures/signature-guest-old.png").await.is_err());
        assert_eq!(
            storage
                .get("signatures/signature-guest-current.png")
                .await
                .expect("current signature should remain"),
            b"current-signature"
        );
        assert_eq!(cleanup.audit_entries.len(), 1);
        assert_eq!(cleanup.audit_entries[0]["action"], "replacement_retention_delete");
        assert_eq!(
            cleanup.audit_entries[0]["object_key"],
            "signatures/signature-guest-old.png"
        );
        assert_eq!(
            cleanup.source["signature_media"]["entries"][0]["previous_deleted_at"].as_str(),
            Some("2026-06-09T00:00:00Z")
        );
        assert_eq!(
            cleanup.source["signature_media"]["entries"][0]["previous_retention_days"],
            7
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn materializes_signature_data_urls_to_object_storage() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "label": "Signature", "type": "signature"},
                {"field_id": "fld_note", "key": "note", "label": "Note", "type": "text"}
            ]
        });

        let materialized = materialize_signature_values_with_storage(
            &schema,
            json!({
                "guest_signature": "data:image/png;base64,c2lnbmF0dXJl",
                "note": "keep me"
            }),
            Some(storage.clone()),
        )
        .await
        .expect("signature should materialize");
        let values = materialized.values;

        let url = values["guest_signature"]
            .as_str()
            .expect("signature value should become a URL");
        assert!(url.starts_with("/api/v1/uploads/signatures/signature-guest_signature-"));
        assert_eq!(values["note"], "keep me");
        let file_name = url
            .strip_prefix("/api/v1/uploads/signatures/")
            .expect("signature URL should use upload signature route");
        let stored = storage
            .get(&format!("signatures/{file_name}"))
            .await
            .expect("signature object should be stored");
        assert_eq!(stored, b"signature");
        assert_eq!(materialized.audit_entries.len(), 1);
        let entry = &materialized.audit_entries[0];
        assert_eq!(entry["field_key"], "guest_signature");
        assert_eq!(entry["url"], url);
        assert_eq!(entry["object_key"], format!("signatures/{file_name}"));
        assert_eq!(entry["content_type"], "image/png");
        assert_eq!(entry["byte_size"], b"signature".len());
        assert_eq!(
            entry["sha256"],
            "1a2fc26dc7ea5a2a4748b7cb2b1ef193d96ab2c99f93092f69e63075b28d1278"
        );
        assert!(entry["materialized_at"].as_str().is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn materialized_signature_records_reason_and_consent_policy() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        let schema = json!({
            "fields": [
                {"field_id": "fld_reason", "key": "signature_reason", "label": "Reason", "type": "text"},
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "label": "Signature",
                    "type": "signature",
                    "signature": {
                        "reason_field": "signature_reason",
                        "reason_required": true,
                        "consent_statement": "I confirm this signature is intentional."
                    }
                }
            ]
        });

        let materialized = materialize_signature_values_with_storage(
            &schema,
            json!({
                "guest_signature": "data:image/png;base64,c2lnbmF0dXJl",
                "signature_reason": "Delivery accepted"
            }),
            Some(storage.clone()),
        )
        .await
        .expect("signature reason metadata should materialize");

        assert_eq!(materialized.audit_entries.len(), 1);
        let entry = &materialized.audit_entries[0];
        assert_eq!(entry["reason_field"], "signature_reason");
        assert_eq!(entry["reason"], "Delivery accepted");
        assert_eq!(entry["reason_required"], true);
        assert_eq!(entry["consent_statement"], "I confirm this signature is intentional.");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn required_signature_reason_is_enforced_before_materialization() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        let schema = json!({
            "fields": [
                {"field_id": "fld_reason", "key": "signature_reason", "label": "Reason", "type": "text"},
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "label": "Signature",
                    "type": "signature",
                    "signature": {
                        "reason_field": "signature_reason",
                        "reason_required": true
                    }
                }
            ]
        });

        let result = materialize_signature_values_with_storage(
            &schema,
            json!({
                "guest_signature": "data:image/png;base64,c2lnbmF0dXJl",
                "signature_reason": "   "
            }),
            Some(storage.clone()),
        )
        .await;

        assert!(matches!(
            result,
            Err(ApiError::BadRequest(message))
                if message.contains("signature reason field is required")
                    && message.contains("signature_reason")
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn materialized_signature_records_previous_object_metadata_when_replaced() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-old.png", b"old-signature")
            .await
            .expect("old signature should be stored");
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "label": "Signature", "type": "signature"}
            ]
        });
        let previous_url = "/api/v1/uploads/signatures/signature-guest-old.png";

        let materialized = materialize_signature_values_with_existing_storage(
            &schema,
            json!({
                "guest_signature": "data:image/png;base64,bmV3LXNpZ25hdHVyZQ=="
            }),
            Some(&json!({
                "guest_signature": previous_url
            })),
            Some(storage.clone()),
        )
        .await
        .expect("replacement signature should materialize");

        assert_eq!(materialized.audit_entries.len(), 1);
        let entry = &materialized.audit_entries[0];
        assert_eq!(entry["action"], "materialize");
        assert_eq!(entry["previous_url"], previous_url);
        assert_eq!(entry["previous_object_key"], "signatures/signature-guest-old.png");
        assert_eq!(
            storage
                .get("signatures/signature-guest-old.png")
                .await
                .expect("old signature should remain for retention/audit policy"),
            b"old-signature"
        );
        let new_url = materialized.values["guest_signature"]
            .as_str()
            .expect("new signature value should be URL");
        assert_ne!(new_url, previous_url);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn verifies_signature_audit_entry_digest_and_size() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-verified.png", b"verified-signature")
            .await
            .expect("signature should be stored");

        let verification = verify_signature_audit_entry_with_storage(
            &json!({
                "object_key": "signatures/signature-guest-verified.png",
                "sha256": "acc6629c9e93fdd11bbc49090ea945594ea8ac3dde6438a58481fc3a762ef243",
                "byte_size": 18
            }),
            Some(storage.clone()),
        )
        .await
        .expect("audit entry should verify");

        assert!(verification.verified);
        assert_eq!(verification.result["verified"], true);
        assert_eq!(
            verification.result["actual_sha256"],
            "acc6629c9e93fdd11bbc49090ea945594ea8ac3dde6438a58481fc3a762ef243"
        );
        assert_eq!(verification.result["actual_byte_size"], 18);
        assert!(verification.result["verified_at"].as_str().is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn detects_signature_audit_entry_digest_mismatch() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-mismatch.png", b"actual-signature")
            .await
            .expect("signature should be stored");

        let verification = verify_signature_audit_entry_with_storage(
            &json!({
                "object_key": "signatures/signature-guest-mismatch.png",
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "byte_size": 16
            }),
            Some(storage.clone()),
        )
        .await
        .expect("audit entry mismatch should return verification result");

        assert!(!verification.verified);
        assert_eq!(verification.result["verified"], false);
        assert_ne!(
            verification.result["actual_sha256"],
            verification.result["expected_sha256"]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn verifies_signature_audit_source_summary() {
        let root = test_upload_root();
        let storage = ObjectStorage::local(&root, "local").expect("local storage should construct");
        storage
            .put("signatures/signature-guest-ok.png", b"source-ok")
            .await
            .expect("matching signature should be stored");
        storage
            .put("signatures/signature-guest-bad.png", b"source-bad")
            .await
            .expect("mismatched signature should be stored");

        let summary = verify_signature_audit_source_with_storage(
            &json!({
                "signature_media": {
                    "version": "v1",
                    "entries": [
                        {
                            "action": "materialize",
                            "object_key": "signatures/signature-guest-ok.png",
                            "sha256": signature_sha256_hex(b"source-ok"),
                            "byte_size": 9
                        },
                        {
                            "action": "materialize",
                            "object_key": "signatures/signature-guest-bad.png",
                            "sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                            "byte_size": 10
                        },
                        {
                            "action": "replacement_retention_delete",
                            "object_key": "signatures/signature-guest-deleted.png"
                        }
                    ]
                }
            }),
            Some(storage.clone()),
        )
        .await
        .expect("source audit verification should summarize entries");

        assert_eq!(summary.total_count, 2);
        assert_eq!(summary.verified_count, 1);
        assert_eq!(summary.failed_count, 1);
        assert_eq!(summary.result["total_count"], 2);
        assert_eq!(summary.result["verified_count"], 1);
        assert_eq!(summary.result["failed_count"], 1);
        assert_eq!(
            summary.result["results"]
                .as_array()
                .expect("results should be an array")
                .len(),
            2
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn appends_signature_audit_entries_to_source() {
        let source = append_signature_audit_source(
            json!({"type": "user"}),
            vec![json!({"field_key": "guest_signature", "object_key": "signatures/sig.png"})],
        )
        .expect("source should accept audit entries");

        assert_eq!(source["type"], "user");
        assert_eq!(source["signature_media"]["version"], "v1");
        assert_eq!(
            source["signature_media"]["entries"][0]["object_key"],
            "signatures/sig.png"
        );
    }

    #[test]
    fn annotates_signature_audit_entries_with_actor_context() {
        let actor_id = Uuid::new_v4();
        let form_id = Uuid::new_v4();
        let record_id = Uuid::new_v4();
        let entries = annotate_signature_audit_entries(
            vec![json!({"field_key": "guest_signature", "object_key": "signatures/sig.png"})],
            "user",
            actor_id,
            "record.create",
            form_id,
            record_id,
            3,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["field_key"], "guest_signature");
        assert_eq!(entries[0]["actor_type"], "user");
        assert_eq!(entries[0]["actor_id"], actor_id.to_string());
        assert_eq!(entries[0]["operation"], "record.create");
        assert_eq!(entries[0]["form_id"], form_id.to_string());
        assert_eq!(entries[0]["record_id"], record_id.to_string());
        assert_eq!(entries[0]["schema_version"], 3);
    }

    #[test]
    fn summarizes_signature_lifecycle_status_and_events() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 7}},
                {"field_id": "fld_retired", "key": "retired_signature", "type": "signature", "signature": {"retention_days": 1}},
                {"field_id": "fld_note", "key": "note", "type": "text"}
            ]
        });
        let values = json!({
            "guest_signature": "/api/v1/uploads/signatures/signature-guest-new.png",
            "retired_signature": null,
            "note": "keep"
        });
        let source = json!({
            "signature_media": {
                "version": "v1",
                "entries": [
                    {
                        "action": "materialize",
                        "field_key": "guest_signature",
                        "object_key": "signatures/signature-guest-new.png",
                        "previous_url": "/api/v1/uploads/signatures/signature-guest-old.png",
                        "previous_object_key": "signatures/signature-guest-old.png",
                        "previous_deleted_at": "2026-06-09T00:00:00Z",
                        "previous_retention_days": 7,
                        "sha256": "acc6629c9e93fdd11bbc49090ea945594ea8ac3dde6438a58481fc3a762ef243",
                        "byte_size": 18,
                        "materialized_at": "2026-06-02T00:00:00Z",
                        "actor_type": "user",
                        "actor_id": Uuid::new_v4(),
                        "operation": "record.update",
                        "schema_version": 5,
                        "reason": "Accepted delivery",
                        "consent_statement": "I confirm this signature is intentional."
                    },
                    {
                        "action": "replacement_retention_delete",
                        "field_key": "guest_signature",
                        "object_key": "signatures/signature-guest-old.png",
                        "retention_days": 7,
                        "deleted_at": "2026-06-09T00:00:00Z"
                    },
                    {
                        "action": "retention_delete",
                        "field_key": "retired_signature",
                        "object_key": "signatures/signature-retired.png",
                        "retention_days": 1,
                        "deleted_at": "2026-06-03T00:00:00Z"
                    }
                ]
            }
        });

        let summary =
            signature_lifecycle_summary(&schema, &values, &source).expect("signature lifecycle should summarize");

        assert_eq!(summary["version"], "v1");
        assert_eq!(summary["signature_field_count"], 2);
        assert_eq!(summary["event_count"], 3);
        assert_eq!(summary["materialize_count"], 1);
        assert_eq!(summary["retention_delete_count"], 1);
        assert_eq!(summary["replacement_retention_delete_count"], 1);
        assert_eq!(summary["verifiable_event_count"], 1);

        let fields = summary["fields"].as_array().expect("fields should be an array");
        let guest = fields
            .iter()
            .find(|field| field["field_key"] == "guest_signature")
            .expect("guest signature summary should exist");
        assert_eq!(guest["status"], "active_with_audit");
        assert_eq!(guest["current_object_key"], "signatures/signature-guest-new.png");
        assert_eq!(guest["retention_days"], 7);
        assert_eq!(guest["current_audit_entry_count"], 1);
        assert_eq!(guest["replacement_count"], 1);
        assert_eq!(guest["replacement_retention_delete_count"], 1);
        assert_eq!(
            guest["previous_objects"][0]["object_key"],
            "signatures/signature-guest-old.png"
        );
        assert_eq!(guest["previous_objects"][0]["deleted_at"], "2026-06-09T00:00:00Z");

        let retired = fields
            .iter()
            .find(|field| field["field_key"] == "retired_signature")
            .expect("retired signature summary should exist");
        assert_eq!(retired["status"], "retention_deleted");
        assert_eq!(retired["retention_delete_count"], 1);
        assert_eq!(
            retired["deleted_objects"][0]["object_key"],
            "signatures/signature-retired.png"
        );

        let events = summary["events"].as_array().expect("events should be an array");
        assert_eq!(events[0]["action"], "materialize");
        assert_eq!(events[0]["reason_present"], true);
        assert_eq!(events[0]["consent_statement_present"], true);
        assert_eq!(events[0]["verifiable"], true);
        assert_eq!(events[1]["happened_at"], "2026-06-09T00:00:00Z");
    }

    #[test]
    fn summarizes_signature_workflow_verification_decision() {
        let verified_audit = SignatureAuditVerificationSummary {
            result: json!({}),
            total_count: 1,
            verified_count: 1,
            failed_count: 0,
        };
        let verified_lifecycle = json!({
            "version": "v1",
            "signature_field_count": 1,
            "materialize_count": 1,
            "verifiable_event_count": 1,
            "retention_delete_count": 0,
            "replacement_retention_delete_count": 0,
            "fields": [
                {
                    "field_key": "guest_signature",
                    "status": "active_with_audit",
                    "current_audit_entry_count": 1
                }
            ],
            "events": [
                {
                    "action": "materialize",
                    "reason_present": true,
                    "consent_statement_present": true,
                    "actor_id": Uuid::new_v4().to_string(),
                    "operation": "record.update"
                }
            ]
        });

        let workflow = signature_workflow_verification_summary(&verified_audit, &verified_lifecycle);
        assert_eq!(workflow["version"], "v1");
        assert_eq!(workflow["status"], "verified");
        assert_eq!(workflow["verified"], true);
        assert_eq!(workflow["active_signature_field_count"], 1);
        assert_eq!(workflow["current_audit_entry_count"], 1);
        assert_eq!(workflow["verified_audit_count"], 1);
        assert_eq!(workflow["reasoned_event_count"], 1);
        assert_eq!(workflow["consented_event_count"], 1);
        assert_eq!(workflow["actor_attributed_event_count"], 1);
        assert_eq!(workflow["operation_attributed_event_count"], 1);

        let failed_audit = SignatureAuditVerificationSummary {
            result: json!({}),
            total_count: 1,
            verified_count: 0,
            failed_count: 1,
        };
        let failed_workflow = signature_workflow_verification_summary(&failed_audit, &verified_lifecycle);
        assert_eq!(failed_workflow["status"], "failed");
        assert_eq!(failed_workflow["verified"], false);

        let incomplete_lifecycle = json!({
            "version": "v1",
            "signature_field_count": 1,
            "materialize_count": 0,
            "verifiable_event_count": 0,
            "fields": [
                {
                    "field_key": "guest_signature",
                    "status": "active_without_audit",
                    "current_audit_entry_count": 0
                }
            ],
            "events": []
        });
        let incomplete_audit = SignatureAuditVerificationSummary {
            result: json!({}),
            total_count: 0,
            verified_count: 0,
            failed_count: 0,
        };
        let incomplete_workflow = signature_workflow_verification_summary(&incomplete_audit, &incomplete_lifecycle);
        assert_eq!(incomplete_workflow["status"], "incomplete");
        assert_eq!(incomplete_workflow["active_without_audit_count"], 1);

        let unsigned_lifecycle = json!({
            "version": "v1",
            "signature_field_count": 1,
            "materialize_count": 0,
            "verifiable_event_count": 0,
            "fields": [
                {
                    "field_key": "guest_signature",
                    "status": "empty",
                    "current_audit_entry_count": 0
                }
            ],
            "events": []
        });
        let unsigned_workflow = signature_workflow_verification_summary(&incomplete_audit, &unsigned_lifecycle);
        assert_eq!(unsigned_workflow["status"], "unsigned");
        assert_eq!(unsigned_workflow["verified"], false);
    }

    fn test_upload_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("openpr-signature-media-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test upload root should be created");
        root
    }
}
