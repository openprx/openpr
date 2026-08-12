use serde_json::Value;

use super::values::{
    NormalizedValues, normalize_record_values, normalize_record_values_report, normalize_record_values_with_existing,
};

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

/// Same as [`validate_and_normalize_values_with_existing`], but also reports the required fields
/// that were already unmet on the stored record and that this write did not address.
pub fn validate_and_normalize_values_with_existing_report(
    schema: &Value,
    values: Value,
    existing_values: &Value,
) -> Result<NormalizedValues, String> {
    normalize_record_values_report(schema, values, Some(existing_values))
}
