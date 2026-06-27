use serde_json::Value;

use super::values::{normalize_record_values, normalize_record_values_with_existing};

pub fn validate_and_normalize_values(schema: &Value, values: Value) -> Result<Value, String> {
    normalize_record_values(schema, values)
}

pub fn validate_and_normalize_values_with_existing(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<Value, String> {
    normalize_record_values_with_existing(schema, values, Some(existing_values))
}
