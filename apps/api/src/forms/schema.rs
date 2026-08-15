// Public and framework-facing signatures remain stable during this behavior-neutral cleanup.
#![allow(clippy::needless_pass_by_value)]

use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub field_id: String,
    pub key: String,
    pub label: String,
    pub field_type: String,
    pub required: bool,
    pub options: Vec<String>,
    pub option_disabled: Vec<String>,
    pub option_archived: Vec<String>,
    pub conditional: Option<Value>,
    pub amount: Option<AmountFieldConfig>,
    pub signature: Option<SignatureFieldConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmountFieldConfig {
    pub currency: String,
    pub scale: u32,
    pub allow_negative: bool,
    pub rounding: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureFieldConfig {
    pub retention_days: Option<i64>,
    pub reason_field: Option<String>,
    pub reason_required: bool,
    pub consent_statement: Option<String>,
}

pub fn parse_fields(schema: &Value) -> Result<Vec<FormField>, String> {
    let Some(fields) = schema.get("fields").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    fields.iter().map(parse_field).collect::<Result<Vec<_>, _>>()
}

pub fn validate_schema(schema: &Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("schema must be a JSON object".to_string());
    }
    let fields = parse_fields(schema)?;
    validate_signature_reason_fields(&fields)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut seen_field_ids = std::collections::BTreeSet::new();
    for field in fields {
        if !seen_field_ids.insert(field.field_id.clone()) {
            return Err(format!("duplicate field_id: {}", field.field_id));
        }
        if !seen.insert(field.key.clone()) {
            return Err(format!("duplicate field key: {}", field.key));
        }
    }
    Ok(())
}

pub fn ensure_schema_field_ids(schema: Value) -> Result<Value, String> {
    let mut object = schema
        .as_object()
        .cloned()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;
    let fields = object.remove("fields").unwrap_or_else(|| Value::Array(Vec::new()));
    let fields = fields
        .as_array()
        .ok_or_else(|| "schema.fields must be an array".to_string())?;
    let mut next_fields = Vec::with_capacity(fields.len());

    for field in fields {
        let mut field_object = field
            .as_object()
            .cloned()
            .ok_or_else(|| "field definition must be an object".to_string())?;
        let field_id = field_object
            .get("field_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map_or_else(new_field_id, str::to_string);
        validate_field_id(&field_id)?;
        field_object.insert("field_id".to_string(), Value::String(field_id));
        next_fields.push(Value::Object(field_object));
    }

    object.insert("fields".to_string(), Value::Array(next_fields));
    Ok(Value::Object(object))
}

fn parse_field(value: &Value) -> Result<FormField, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "field definition must be an object".to_string())?;
    let field_id = object
        .get("field_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "field_id is required".to_string())?
        .to_string();
    validate_field_id(&field_id)?;
    let key = object
        .get("key")
        .and_then(Value::as_str)
        .map(normalize_key)
        .transpose()?
        .ok_or_else(|| "field key is required".to_string())?;
    let label = object
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&key)
        .to_string();
    let field_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("text")
        .to_string();
    validate_field_type(&field_type)?;
    let options = object
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let option_disabled = object
        .get("option_disabled")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let option_archived = object
        .get("option_archived")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let amount = if field_type == "amount" {
        Some(parse_amount_config(object.get("amount"))?)
    } else {
        None
    };
    let signature = if field_type == "signature" {
        Some(parse_signature_config(object.get("signature"))?)
    } else {
        None
    };

    Ok(FormField {
        field_id,
        key,
        label,
        field_type,
        required: object.get("required").and_then(Value::as_bool).unwrap_or(false),
        options,
        option_disabled,
        option_archived,
        conditional: object.get("conditional").cloned(),
        amount,
        signature,
    })
}

fn new_field_id() -> String {
    format!("fld_{}", Uuid::new_v4().simple())
}

fn validate_field_id(field_id: &str) -> Result<(), String> {
    if field_id.len() < 8 || field_id.len() > 80 {
        return Err("field_id length must be between 8 and 80".to_string());
    }
    if !field_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("field_id must contain lowercase letters, digits, or underscores".to_string());
    }
    Ok(())
}

pub fn normalize_key(raw: &str) -> Result<String, String> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err("key is required".to_string());
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("key must contain lowercase letters, digits, or underscores".to_string());
    }
    let Some(first) = key.chars().next() else {
        return Err("key is required".to_string());
    };
    if !first.is_ascii_lowercase() {
        return Err("key must start with a lowercase letter".to_string());
    }
    Ok(key)
}

fn validate_field_type(field_type: &str) -> Result<(), String> {
    match field_type {
        "text" | "phone" | "email" | "address" | "location" | "scan" | "signature" | "autonumber" | "member"
        | "textarea" | "rich_text" | "number" | "integer" | "amount" | "rating" | "progress" | "date" | "datetime"
        | "single_select" | "multi_select" | "boolean" | "attachment" | "image" | "relation" | "child_table"
        | "formula" | "ai_summary" => Ok(()),
        _ => Err(format!("unsupported field type: {field_type}")),
    }
}

fn parse_amount_config(value: Option<&Value>) -> Result<AmountFieldConfig, String> {
    let object = value.and_then(Value::as_object);
    let currency = object
        .and_then(|item| item.get("currency"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("CNY")
        .to_ascii_uppercase();
    if !currency.chars().all(|c| c.is_ascii_uppercase()) || currency.len() != 3 {
        return Err("amount.currency must be an ISO-like 3 letter code".to_string());
    }
    let scale = object
        .and_then(|item| item.get("scale"))
        .and_then(Value::as_u64)
        .unwrap_or(2);
    if scale > 8 {
        return Err("amount.scale must be between 0 and 8".to_string());
    }
    let rounding = object
        .and_then(|item| item.get("rounding"))
        .and_then(Value::as_str)
        .unwrap_or("half_up")
        .to_string();
    match rounding.as_str() {
        "half_up" | "half_even" | "down" | "up" => {}
        _ => return Err("amount.rounding is unsupported".to_string()),
    }

    Ok(AmountFieldConfig {
        currency,
        scale: u32::try_from(scale).map_err(|_| "amount.scale cannot be represented as u32".to_string())?,
        allow_negative: object
            .and_then(|item| item.get("allow_negative"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        rounding,
    })
}

fn parse_signature_config(value: Option<&Value>) -> Result<SignatureFieldConfig, String> {
    let object = value.and_then(Value::as_object);
    let retention_days = object
        .and_then(|item| item.get("retention_days"))
        .and_then(signature_retention_days_value);
    if let Some(value) = object.and_then(|item| item.get("retention_days"))
        && retention_days.is_none()
        && !value.is_null()
    {
        return Err("signature.retention_days must be between 1 and 3650".to_string());
    }
    let reason_field = object
        .and_then(|item| item.get("reason_field"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_key)
        .transpose()?;
    let reason_required = object
        .and_then(|item| item.get("reason_required"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| reason_field.is_some());
    if reason_required && reason_field.is_none() {
        return Err("signature.reason_field is required when signature.reason_required is true".to_string());
    }
    let consent_statement = object
        .and_then(|item| item.get("consent_statement"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if consent_statement
        .as_ref()
        .is_some_and(|statement| statement.chars().count() > 1000)
    {
        return Err("signature.consent_statement must be 1000 characters or less".to_string());
    }
    Ok(SignatureFieldConfig {
        retention_days,
        reason_field,
        reason_required,
        consent_statement,
    })
}

fn signature_retention_days_value(value: &Value) -> Option<i64> {
    let days = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|item| item.trim().parse::<i64>().ok()))?;
    if (1..=3650).contains(&days) { Some(days) } else { None }
}

fn validate_signature_reason_fields(fields: &[FormField]) -> Result<(), String> {
    let keys = fields
        .iter()
        .map(|field| field.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for field in fields.iter().filter(|field| field.field_type == "signature") {
        let Some(signature) = &field.signature else {
            continue;
        };
        let Some(reason_field) = signature.reason_field.as_deref() else {
            continue;
        };
        if reason_field == field.key {
            return Err("signature.reason_field cannot point to the signature field itself".to_string());
        }
        if !keys.contains(reason_field) {
            return Err(format!(
                "signature.reason_field must reference an existing field key: {reason_field}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{ensure_schema_field_ids, normalize_key, parse_fields, validate_schema};
    use serde_json::json;

    #[test]
    fn validates_form_schema_fields() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_counterparty", "key": "counterparty", "label": "Counterparty", "type": "text", "required": true},
                {"field_id": "fld_amount", "key": "amount", "label": "Amount", "type": "amount", "amount": {"currency": "CNY", "scale": 2}}
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(
            fields
                .get(1)
                .and_then(|field| field.amount.as_ref())
                .map(|amount| amount.currency.as_str()),
            Some("CNY")
        );
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn validates_signature_retention_policy() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 90}}
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        assert_eq!(
            fields
                .first()
                .and_then(|field| field.signature.as_ref())
                .and_then(|signature| signature.retention_days),
            Some(90)
        );
        assert!(validate_schema(&schema).is_ok());

        let invalid = json!({
            "fields": [
                {"field_id": "fld_signature", "key": "guest_signature", "type": "signature", "signature": {"retention_days": 0}}
            ]
        });
        assert!(validate_schema(&invalid).is_err());
    }

    #[test]
    fn validates_signature_reason_and_consent_policy() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_reason", "key": "signature_reason", "type": "text"},
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {
                        "reason_field": "signature_reason",
                        "reason_required": true,
                        "consent_statement": "I confirm this signature is intentional."
                    }
                }
            ]
        });
        let fields = parse_fields(&schema).unwrap();
        let signature = fields
            .iter()
            .find(|field| field.key == "guest_signature")
            .and_then(|field| field.signature.as_ref())
            .expect("signature config should parse");
        assert_eq!(signature.reason_field.as_deref(), Some("signature_reason"));
        assert!(signature.reason_required);
        assert_eq!(
            signature.consent_statement.as_deref(),
            Some("I confirm this signature is intentional.")
        );
        assert!(validate_schema(&schema).is_ok());

        let missing_reason_field = json!({
            "fields": [
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {"reason_field": "signature_reason"}
                }
            ]
        });
        assert!(validate_schema(&missing_reason_field).is_err());

        let impossible_required_reason = json!({
            "fields": [
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature",
                    "signature": {"reason_required": true}
                }
            ]
        });
        assert!(validate_schema(&impossible_required_reason).is_err());
    }

    #[test]
    fn rejects_duplicate_keys() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_amount_1", "key": "amount", "type": "amount"},
                {"field_id": "fld_amount_2", "key": "amount", "type": "text"}
            ]
        });
        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn fills_missing_field_ids() {
        let schema = ensure_schema_field_ids(json!({
            "fields": [
                {"key": "amount", "type": "amount"}
            ]
        }))
        .unwrap();
        let field_id = schema["fields"][0]["field_id"].as_str().unwrap();
        assert!(field_id.starts_with("fld_"));
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn normalizes_safe_keys() {
        assert_eq!(normalize_key("Order_Line").unwrap(), "order_line");
        assert!(normalize_key("order-line").is_err());
        assert!(normalize_key("1order").is_err());
    }
}
