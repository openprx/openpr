use serde_json::{Map, Value, json};

use super::{
    decimal::normalize_amount_decimal,
    schema::{FormField, parse_fields},
};

pub fn normalize_record_values(schema: &Value, values: Value) -> Result<Value, String> {
    normalize_record_values_with_existing(schema, values, None)
}

pub fn normalize_record_values_with_existing(
    schema: &Value,
    values: Value,
    existing_values: Option<&Value>,
) -> Result<Value, String> {
    if !values.is_object() {
        return Err("values must be a JSON object".to_string());
    }
    let fields = parse_fields(schema)?;
    let input = values
        .as_object()
        .ok_or_else(|| "values must be a JSON object".to_string())?;
    let mut normalized = Map::new();

    for field in fields {
        let raw = input.get(&field.key);
        if field_hidden_by_condition(&field, input, existing_values) {
            continue;
        }
        if is_missing(raw) {
            if field.required && field.field_type != "autonumber" {
                return Err(format!("field '{}' is required", field.key));
            }
            continue;
        }
        let Some(raw_value) = raw else {
            continue;
        };
        let value = normalize_field_value(&field, raw_value, existing_values.and_then(|item| item.get(&field.key)))?;
        if field_read_only_by_condition(&field, input, existing_values) {
            if let Some(existing) = existing_values.and_then(|item| item.get(&field.key)) {
                if existing != &value {
                    return Err(format!("field '{}' is read-only by condition", field.key));
                }
            }
        }
        normalized.insert(field.key, value);
    }

    Ok(Value::Object(normalized))
}

fn normalize_field_value(field: &FormField, raw: &Value, existing: Option<&Value>) -> Result<Value, String> {
    match field.field_type.as_str() {
        "text" | "textarea" | "rich_text" | "date" | "datetime" | "ai_summary" | "phone" | "address" | "scan"
        | "signature" | "autonumber" => raw
            .as_str()
            .map(|value| json!(value))
            .ok_or_else(|| format!("field '{}' must be a string", field.key)),
        "email" => normalize_email(field, raw),
        "location" => normalize_location(field, raw),
        "single_select" => {
            let Some(value) = raw.as_str() else {
                return Err(format!("field '{}' must be a string", field.key));
            };
            if !field.options.is_empty() && !field.options.iter().any(|option| option == value) {
                return Err(format!("field '{}' has an unsupported option", field.key));
            }
            ensure_option_write_allowed(field, value, existing)?;
            Ok(json!(value))
        }
        "multi_select" => {
            let Some(items) = raw.as_array() else {
                return Err(format!("field '{}' must be an array", field.key));
            };
            let mut values = Vec::new();
            for item in items {
                let Some(value) = item.as_str() else {
                    return Err(format!("field '{}' options must be strings", field.key));
                };
                if !field.options.is_empty() && !field.options.iter().any(|option| option == value) {
                    return Err(format!("field '{}' has an unsupported option", field.key));
                }
                ensure_option_write_allowed(field, value, existing)?;
                values.push(json!(value));
            }
            Ok(Value::Array(values))
        }
        "boolean" => raw
            .as_bool()
            .map(|value| json!(value))
            .ok_or_else(|| format!("field '{}' must be a boolean", field.key)),
        "integer" => normalize_integer(field, raw),
        "number" => normalize_decimal_like(field, raw, "decimal"),
        "amount" => normalize_amount(field, raw),
        "rating" => normalize_bounded_integer(field, raw, 1, 5, "rating"),
        "progress" => normalize_bounded_integer(field, raw, 0, 100, "progress"),
        "member" => normalize_member(field, raw),
        "attachment" | "image" | "relation" | "child_table" | "formula" => Ok(raw.clone()),
        _ => Err(format!("unsupported field type: {}", field.field_type)),
    }
}

fn ensure_option_write_allowed(field: &FormField, value: &str, existing: Option<&Value>) -> Result<(), String> {
    if !field.option_disabled.iter().any(|option| option == value)
        && !field.option_archived.iter().any(|option| option == value)
    {
        return Ok(());
    }
    let existed = match existing {
        Some(Value::String(existing)) => existing == value,
        Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(value)),
        _ => false,
    };
    if existed {
        return Ok(());
    }
    if field.option_archived.iter().any(|option| option == value) {
        return Err(format!("field '{}' option '{}' is archived", field.key, value));
    }
    Err(format!("field '{}' option '{}' is disabled", field.key, value))
}

fn normalize_member(field: &FormField, raw: &Value) -> Result<Value, String> {
    if let Some(user_id) = raw.as_str().map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(json!({ "user_id": user_id }));
    }
    let Some(object) = raw.as_object() else {
        return Err(format!("field '{}' must be a member object", field.key));
    };
    let user_id = object
        .get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("field '{}' member.user_id is required", field.key))?;
    let mut value = Map::new();
    value.insert("user_id".to_string(), json!(user_id));
    for key in ["name", "email"] {
        if let Some(item) = object.get(key).and_then(Value::as_str).map(str::trim) {
            if !item.is_empty() {
                value.insert(key.to_string(), json!(item));
            }
        }
    }
    Ok(Value::Object(value))
}

fn normalize_email(field: &FormField, raw: &Value) -> Result<Value, String> {
    let value = raw
        .as_str()
        .ok_or_else(|| format!("field '{}' must be a string", field.key))?
        .trim();
    if !value.contains('@') || value.starts_with('@') || value.ends_with('@') {
        return Err(format!("field '{}' must be an email address", field.key));
    }
    Ok(json!(value))
}

fn normalize_location(field: &FormField, raw: &Value) -> Result<Value, String> {
    if let Some(value) = raw.as_str() {
        return Ok(json!(value));
    }
    let Some(object) = raw.as_object() else {
        return Err(format!("field '{}' must be a string or location object", field.key));
    };
    let lat = object
        .get("lat")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("field '{}' location.lat must be a number", field.key))?;
    let lng = object
        .get("lng")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("field '{}' location.lng must be a number", field.key))?;
    if !(-90.0..=90.0).contains(&lat) {
        return Err(format!("field '{}' location.lat is out of range", field.key));
    }
    if !(-180.0..=180.0).contains(&lng) {
        return Err(format!("field '{}' location.lng is out of range", field.key));
    }
    let mut value = Map::new();
    value.insert("lat".to_string(), json!(lat));
    value.insert("lng".to_string(), json!(lng));
    if let Some(label) = object.get("label").and_then(Value::as_str).map(str::trim) {
        if !label.is_empty() {
            value.insert("label".to_string(), json!(label));
        }
    }
    Ok(Value::Object(value))
}

fn normalize_integer(field: &FormField, raw: &Value) -> Result<Value, String> {
    if let Some(value) = raw.get("value").and_then(Value::as_i64) {
        return Ok(json!({ "type": "integer", "value": value }));
    }
    if let Some(value) = raw.as_i64() {
        return Ok(json!({ "type": "integer", "value": value }));
    }
    if let Some(value) = raw.as_str() {
        let parsed = value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("field '{}' must be an integer", field.key))?;
        return Ok(json!({ "type": "integer", "value": parsed }));
    }
    Err(format!("field '{}' must be an integer", field.key))
}

fn normalize_bounded_integer(
    field: &FormField,
    raw: &Value,
    min: i64,
    max: i64,
    value_type: &str,
) -> Result<Value, String> {
    let value = if let Some(value) = raw.get("value").and_then(Value::as_i64) {
        value
    } else if let Some(value) = raw.as_i64() {
        value
    } else if let Some(value) = raw.as_str() {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("field '{}' must be an integer", field.key))?
    } else {
        return Err(format!("field '{}' must be an integer", field.key));
    };
    if value < min || value > max {
        return Err(format!("field '{}' must be between {} and {}", field.key, min, max));
    }
    Ok(json!({ "type": value_type, "value": value }))
}

fn normalize_decimal_like(field: &FormField, raw: &Value, value_type: &str) -> Result<Value, String> {
    if raw.is_number() {
        return Err(format!(
            "field '{}' must be a decimal string, not a JSON number",
            field.key
        ));
    }
    let decimal = if let Some(decimal) = raw.as_str() {
        decimal
    } else if let Some(decimal) = raw.get("decimal").and_then(Value::as_str) {
        decimal
    } else {
        return Err(format!("field '{}' must be a decimal string", field.key));
    };
    let parsed = decimal
        .trim()
        .parse::<rust_decimal::Decimal>()
        .map_err(|_| format!("field '{}' decimal is invalid", field.key))?;
    Ok(json!({
        "type": value_type,
        "decimal": parsed.to_string()
    }))
}

fn normalize_amount(field: &FormField, raw: &Value) -> Result<Value, String> {
    if raw.is_number() {
        return Err(format!(
            "field '{}' must be a decimal string, not a JSON number",
            field.key
        ));
    }
    let config = field
        .amount
        .as_ref()
        .ok_or_else(|| format!("field '{}' amount config is missing", field.key))?;
    let (decimal, currency) = if let Some(value) = raw.as_str() {
        (value, config.currency.as_str())
    } else if let Some(object) = raw.as_object() {
        let decimal = object
            .get("decimal")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("field '{}' amount.decimal is required", field.key))?;
        let currency = object
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or(&config.currency);
        (decimal, currency)
    } else {
        return Err(format!(
            "field '{}' must be an amount object or decimal string",
            field.key
        ));
    };
    if currency.to_ascii_uppercase() != config.currency {
        return Err(format!("field '{}' currency mismatch", field.key));
    }
    let normalized = normalize_amount_decimal(decimal, config)?;
    Ok(json!({
        "type": "amount",
        "decimal": normalized,
        "currency": config.currency,
        "scale": config.scale
    }))
}

fn is_missing(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value.trim().is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        _ => false,
    }
}

fn field_hidden_by_condition(field: &FormField, input: &Map<String, Value>, existing_values: Option<&Value>) -> bool {
    let Some(condition) = field.conditional.as_ref().and_then(|item| item.get("hidden_when")) else {
        return false;
    };
    condition_matches(condition, input, existing_values)
}

fn field_read_only_by_condition(
    field: &FormField,
    input: &Map<String, Value>,
    existing_values: Option<&Value>,
) -> bool {
    let Some(condition) = field.conditional.as_ref().and_then(|item| item.get("read_only_when")) else {
        return false;
    };
    condition_matches(condition, input, existing_values)
}

fn condition_matches(condition: &Value, input: &Map<String, Value>, existing_values: Option<&Value>) -> bool {
    let Some(object) = condition.as_object() else {
        return false;
    };
    if let Some(conditions) = object.get("conditions").and_then(Value::as_array) {
        let checks = conditions
            .iter()
            .filter(|item| item.as_object().is_some())
            .map(|item| condition_matches(item, input, existing_values))
            .collect::<Vec<_>>();
        if checks.is_empty() {
            return false;
        }
        return if object.get("logic").and_then(Value::as_str) == Some("any") {
            checks.iter().any(|item| *item)
        } else {
            checks.iter().all(|item| *item)
        };
    }

    let Some(field_key) = object.get("field").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if field_key.is_empty() {
        return false;
    }
    let operator = normalize_condition_operator(object.get("operator").and_then(Value::as_str));
    let condition_value = object
        .get("value")
        .or_else(|| object.get("equals"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if condition_operator_requires_value(operator) && condition_value.is_none() {
        return false;
    }
    let actual = condition_lookup_value(field_key, input, existing_values);
    match operator {
        "equals" => actual == condition_value.unwrap_or_default(),
        "not_equals" => actual != condition_value.unwrap_or_default(),
        "contains" => actual.contains(condition_value.unwrap_or_default()),
        "not_contains" => !actual.contains(condition_value.unwrap_or_default()),
        "is_empty" => actual.trim().is_empty(),
        "not_empty" => !actual.trim().is_empty(),
        "gt" | "gte" | "lt" | "lte" => {
            let Ok(actual) = actual.parse::<f64>() else {
                return false;
            };
            let Ok(expected) = condition_value.unwrap_or_default().parse::<f64>() else {
                return false;
            };
            match operator {
                "gt" => actual > expected,
                "gte" => actual >= expected,
                "lt" => actual < expected,
                "lte" => actual <= expected,
                _ => false,
            }
        }
        _ => false,
    }
}

fn normalize_condition_operator(operator: Option<&str>) -> &'static str {
    match operator.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("not_equals") | Some("neq") | Some("!=") | Some("<>") => "not_equals",
        Some("contains") => "contains",
        Some("not_contains") | Some("not-contains") => "not_contains",
        Some("is_empty") | Some("empty") => "is_empty",
        Some("not_empty") | Some("exists") => "not_empty",
        Some("gt") | Some(">") => "gt",
        Some("gte") | Some(">=") => "gte",
        Some("lt") | Some("<") => "lt",
        Some("lte") | Some("<=") => "lte",
        _ => "equals",
    }
}

fn condition_operator_requires_value(operator: &str) -> bool {
    !matches!(operator, "is_empty" | "not_empty")
}

fn condition_lookup_value(field_key: &str, input: &Map<String, Value>, existing_values: Option<&Value>) -> String {
    input
        .get(field_key)
        .or_else(|| existing_values.and_then(|item| item.get(field_key)))
        .map(condition_value_string)
        .unwrap_or_default()
}

fn condition_value_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(object) => {
            for key in ["record_id", "user_id", "value", "decimal"] {
                if let Some(value) = object.get(key) {
                    return condition_value_string(value);
                }
            }
            value.to_string()
        }
        Value::Null => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_record_values, normalize_record_values_with_existing};
    use serde_json::json;

    #[test]
    fn normalizes_amount_values_without_json_numbers() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_amount",
                "key": "amount",
                "type": "amount",
                "required": true,
                "amount": {"currency": "CNY", "scale": 2}
            }]
        });
        let normalized = normalize_record_values(&schema, json!({"amount": "0.1"})).unwrap();
        assert_eq!(
            normalized
                .get("amount")
                .and_then(|value| value.get("decimal"))
                .and_then(serde_json::Value::as_str),
            Some("0.10")
        );
        assert!(normalize_record_values(&schema, json!({"amount": 0.1})).is_err());
    }

    #[test]
    fn validates_required_fields_and_select_options() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_status",
                "key": "status",
                "type": "single_select",
                "required": true,
                "options": ["open", "closed"]
            }]
        });
        assert!(normalize_record_values(&schema, json!({})).is_err());
        assert!(normalize_record_values(&schema, json!({"status": "bad"})).is_err());
        assert_eq!(
            normalize_record_values(&schema, json!({"status": "open"}))
                .unwrap()
                .get("status")
                .and_then(serde_json::Value::as_str),
            Some("open")
        );
    }

    #[test]
    fn rejects_disabled_options_unless_value_already_exists() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_status",
                    "key": "status",
                    "type": "single_select",
                    "options": ["open", "closed", "archived"],
                    "option_disabled": ["archived"]
                },
                {
                    "field_id": "fld_tags",
                    "key": "tags",
                    "type": "multi_select",
                    "options": ["fresh", "legacy"],
                    "option_disabled": ["legacy"]
                }
            ]
        });
        assert!(normalize_record_values(&schema, json!({"status": "archived"})).is_err());
        assert!(normalize_record_values(&schema, json!({"tags": ["fresh", "legacy"]})).is_err());
        let normalized = normalize_record_values_with_existing(
            &schema,
            json!({"status": "archived", "tags": ["legacy"]}),
            Some(&json!({"status": "archived", "tags": ["legacy"]})),
        )
        .expect("existing disabled options should stay valid");
        assert_eq!(
            normalized.get("status").and_then(serde_json::Value::as_str),
            Some("archived")
        );
        assert!(
            normalize_record_values_with_existing(
                &schema,
                json!({"status": "open", "tags": ["fresh", "legacy"]}),
                Some(&json!({"status": "open", "tags": ["fresh"]})),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_archived_options_unless_value_already_exists() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_status",
                    "key": "status",
                    "type": "single_select",
                    "options": ["open", "closed", "retired"],
                    "option_archived": ["retired"]
                },
                {
                    "field_id": "fld_tags",
                    "key": "tags",
                    "type": "multi_select",
                    "options": ["fresh", "retired"],
                    "option_archived": ["retired"]
                }
            ]
        });
        assert!(normalize_record_values(&schema, json!({"status": "retired"})).is_err());
        assert!(normalize_record_values(&schema, json!({"tags": ["fresh", "retired"]})).is_err());
        let normalized = normalize_record_values_with_existing(
            &schema,
            json!({"status": "retired", "tags": ["retired"]}),
            Some(&json!({"status": "retired", "tags": ["retired"]})),
        )
        .expect("existing archived options should stay valid");
        assert_eq!(
            normalized.get("status").and_then(serde_json::Value::as_str),
            Some("retired")
        );
        assert!(
            normalize_record_values_with_existing(
                &schema,
                json!({"status": "open", "tags": ["fresh", "retired"]}),
                Some(&json!({"status": "open", "tags": ["fresh"]})),
            )
            .is_err()
        );
    }

    #[test]
    fn enforces_conditional_hidden_fields() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_status",
                    "key": "status",
                    "type": "single_select",
                    "options": ["normal", "priority"]
                },
                {
                    "field_id": "fld_vip_note",
                    "key": "vip_note",
                    "type": "text",
                    "required": true,
                    "conditional": {
                        "hidden_when": {
                            "logic": "any",
                            "conditions": [
                                {"field": "status", "equals": "normal"},
                                {"field": "status", "operator": "not_equals", "value": "priority"}
                            ]
                        }
                    }
                }
            ]
        });
        let normalized = normalize_record_values(&schema, json!({"status": "normal", "vip_note": "server bypass"}))
            .expect("hidden required field should be omitted");
        assert_eq!(
            normalized.get("status").and_then(serde_json::Value::as_str),
            Some("normal")
        );
        assert!(normalized.get("vip_note").is_none());
        assert!(normalize_record_values(&schema, json!({"status": "priority"})).is_err());
    }

    #[test]
    fn enforces_conditional_read_only_updates() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_status",
                    "key": "status",
                    "type": "single_select",
                    "options": ["open", "closed"]
                },
                {
                    "field_id": "fld_approval",
                    "key": "approval_note",
                    "type": "text",
                    "conditional": {
                        "read_only_when": {
                            "logic": "all",
                            "conditions": [
                                {"field": "status", "equals": "closed"}
                            ]
                        }
                    }
                }
            ]
        });
        let existing = json!({"status": "closed", "approval_note": "Approved"});
        let unchanged = normalize_record_values_with_existing(
            &schema,
            json!({"status": "closed", "approval_note": "Approved"}),
            Some(&existing),
        )
        .expect("unchanged read-only value should pass");
        assert_eq!(
            unchanged.get("approval_note").and_then(serde_json::Value::as_str),
            Some("Approved")
        );
        assert!(
            normalize_record_values_with_existing(
                &schema,
                json!({"status": "closed", "approval_note": "Changed"}),
                Some(&existing),
            )
            .is_err()
        );
        assert!(
            normalize_record_values_with_existing(
                &schema,
                json!({"status": "open", "approval_note": "Changed"}),
                Some(&existing),
            )
            .is_ok()
        );
    }

    #[test]
    fn enforces_conditional_operator_expressions() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_note",
                    "key": "handoff_note",
                    "type": "text"
                },
                {
                    "field_id": "fld_rating",
                    "key": "service_rating",
                    "type": "rating",
                    "validation": {"min": 1, "max": 5}
                },
                {
                    "field_id": "fld_signature",
                    "key": "guest_signature",
                    "type": "signature"
                },
                {
                    "field_id": "fld_manager_note",
                    "key": "manager_note",
                    "type": "text",
                    "required": true,
                    "conditional": {
                        "hidden_when": {
                            "logic": "all",
                            "conditions": [
                                {"field": "handoff_note", "operator": "contains", "value": "VIP"},
                                {"field": "service_rating", "operator": "gte", "value": "4"}
                            ]
                        }
                    }
                },
                {
                    "field_id": "fld_audit_note",
                    "key": "audit_note",
                    "type": "text",
                    "conditional": {
                        "read_only_when": {
                            "logic": "any",
                            "conditions": [
                                {"field": "guest_signature", "operator": "not_empty"},
                                {"field": "handoff_note", "operator": "is_empty"}
                            ]
                        }
                    }
                }
            ]
        });
        let normalized = normalize_record_values(
            &schema,
            json!({
                "handoff_note": "VIP handoff",
                "service_rating": 4,
                "guest_signature": "",
                "manager_note": "hidden",
                "audit_note": "draft"
            }),
        )
        .expect("hidden required field should be omitted when contains/gte match");
        assert!(normalized.get("manager_note").is_none());

        assert!(
            normalize_record_values(
                &schema,
                json!({
                    "handoff_note": "standard handoff",
                    "service_rating": 4,
                    "guest_signature": "",
                    "audit_note": "draft"
                }),
            )
            .is_err()
        );

        let existing = json!({"guest_signature": "data:image/png;base64,abc", "audit_note": "Signed"});
        assert!(
            normalize_record_values_with_existing(
                &schema,
                json!({"guest_signature": "data:image/png;base64,abc", "audit_note": "Changed"}),
                Some(&existing),
            )
            .is_err()
        );
    }

    #[test]
    fn normalizes_phone_and_email_values() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_phone",
                    "key": "guest_phone",
                    "type": "phone"
                },
                {
                    "field_id": "fld_email",
                    "key": "guest_email",
                    "type": "email"
                }
            ]
        });
        let normalized = normalize_record_values(
            &schema,
            json!({"guest_phone": "+1 555 0100", "guest_email": "guest@example.test"}),
        )
        .unwrap();
        assert_eq!(
            normalized.get("guest_phone").and_then(serde_json::Value::as_str),
            Some("+1 555 0100")
        );
        assert_eq!(
            normalized.get("guest_email").and_then(serde_json::Value::as_str),
            Some("guest@example.test")
        );
        assert!(normalize_record_values(&schema, json!({"guest_email": "not-email"})).is_err());
    }

    #[test]
    fn normalizes_address_and_location_values() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_address",
                    "key": "delivery_address",
                    "type": "address"
                },
                {
                    "field_id": "fld_location",
                    "key": "delivery_location",
                    "type": "location"
                }
            ]
        });
        let normalized = normalize_record_values(
            &schema,
            json!({
                "delivery_address": "123 Test Ave",
                "delivery_location": {"lat": 40.7128, "lng": -74.006, "label": "Store"}
            }),
        )
        .expect("address/location values should normalize");
        assert_eq!(
            normalized.get("delivery_address").and_then(serde_json::Value::as_str),
            Some("123 Test Ave")
        );
        assert_eq!(
            normalized.get("delivery_location"),
            Some(&json!({"lat": 40.7128, "lng": -74.006, "label": "Store"}))
        );
        let normalized_string = normalize_record_values(
            &schema,
            json!({"delivery_address": "123 Test Ave", "delivery_location": "40.7128,-74.0060"}),
        )
        .expect("string location should normalize");
        assert_eq!(
            normalized_string
                .get("delivery_location")
                .and_then(serde_json::Value::as_str),
            Some("40.7128,-74.0060")
        );
        assert!(
            normalize_record_values(
                &schema,
                json!({"delivery_address": "123 Test Ave", "delivery_location": {"lat": 91.0, "lng": 0.0}})
            )
            .is_err()
        );
    }

    #[test]
    fn normalizes_rating_and_progress_values() {
        let schema = json!({
            "fields": [
                {
                    "field_id": "fld_rating",
                    "key": "service_rating",
                    "type": "rating"
                },
                {
                    "field_id": "fld_progress",
                    "key": "prep_progress",
                    "type": "progress"
                }
            ]
        });
        let normalized = normalize_record_values(&schema, json!({"service_rating": "4", "prep_progress": 75}))
            .expect("rating/progress values should normalize");
        assert_eq!(
            normalized.get("service_rating"),
            Some(&json!({"type": "rating", "value": 4}))
        );
        assert_eq!(
            normalized.get("prep_progress"),
            Some(&json!({"type": "progress", "value": 75}))
        );
        assert!(normalize_record_values(&schema, json!({"service_rating": 6})).is_err());
        assert!(normalize_record_values(&schema, json!({"prep_progress": -1})).is_err());
    }

    #[test]
    fn normalizes_scan_values() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_scan",
                "key": "scan_code",
                "type": "scan"
            }]
        });
        let normalized = normalize_record_values(&schema, json!({"scan_code": "SKU-2026-0001"}))
            .expect("scan values should normalize");
        assert_eq!(
            normalized.get("scan_code").and_then(serde_json::Value::as_str),
            Some("SKU-2026-0001")
        );
        assert!(normalize_record_values(&schema, json!({"scan_code": 20260001})).is_err());
    }

    #[test]
    fn normalizes_signature_values() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_signature",
                "key": "guest_signature",
                "type": "signature"
            }]
        });
        let normalized = normalize_record_values(&schema, json!({"guest_signature": "Ada Lovelace"}))
            .expect("signature values should normalize");
        assert_eq!(
            normalized.get("guest_signature").and_then(serde_json::Value::as_str),
            Some("Ada Lovelace")
        );
        assert!(normalize_record_values(&schema, json!({"guest_signature": {"name": "Ada"}})).is_err());
    }

    #[test]
    fn normalizes_autonumber_values() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_autonumber",
                "key": "ticket_no",
                "type": "autonumber",
                "required": true
            }]
        });
        let missing =
            normalize_record_values(&schema, json!({})).expect("missing autonumber should be generated later");
        assert!(missing.get("ticket_no").is_none());
        let normalized = normalize_record_values(&schema, json!({"ticket_no": "AUTO-000001"}))
            .expect("autonumber values should normalize");
        assert_eq!(
            normalized.get("ticket_no").and_then(serde_json::Value::as_str),
            Some("AUTO-000001")
        );
        assert!(normalize_record_values(&schema, json!({"ticket_no": 1})).is_err());
    }

    #[test]
    fn normalizes_member_values() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_member",
                "key": "assignee",
                "type": "member"
            }]
        });
        let normalized = normalize_record_values(
            &schema,
            json!({"assignee": {"user_id": "user-smoke", "name": "Smoke Admin", "email": "smoke@example.com"}}),
        )
        .expect("member values should normalize");
        assert_eq!(
            normalized
                .get("assignee")
                .and_then(serde_json::Value::as_object)
                .and_then(|value| value.get("user_id"))
                .and_then(serde_json::Value::as_str),
            Some("user-smoke")
        );
        let normalized_string = normalize_record_values(&schema, json!({"assignee": "user-smoke"}))
            .expect("string member values should normalize");
        assert_eq!(
            normalized_string
                .get("assignee")
                .and_then(serde_json::Value::as_object)
                .and_then(|value| value.get("user_id"))
                .and_then(serde_json::Value::as_str),
            Some("user-smoke")
        );
        assert!(normalize_record_values(&schema, json!({"assignee": {"name": "Smoke Admin"}})).is_err());
    }
}
