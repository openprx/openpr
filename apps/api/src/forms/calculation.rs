use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::str::FromStr;

use super::schema::{FormField, parse_fields};

/// Builds the value map formula evaluation runs against.
///
/// A formula argument that is absent resolves to `formula field 'x' is missing`, so evaluating a
/// partial payload rejects the whole write. Callers routinely submit partial payloads: a field with
/// `read:false` is stripped from every API response and can therefore never be echoed back, a field
/// hidden by `hidden_when` is not rendered, and a plain PATCH only carries what changed. Seeding
/// from the stored record — restricted to keys the schema still declares, so removed fields are not
/// resurrected — lets the formula see the record as it will be after the write. The caller's payload
/// always wins, including an explicit `null` that means "clear this field".
///
/// On the create path there is nothing stored yet, so the payload is returned unchanged and a
/// formula over a genuinely absent field still fails.
pub fn merge_values_for_calculation(
    schema: &Value,
    values: &Value,
    existing_values: Option<&Value>,
) -> Result<Value, String> {
    let input = values
        .as_object()
        .ok_or_else(|| "values must be a JSON object".to_string())?;
    let Some(existing) = existing_values.and_then(Value::as_object) else {
        return Ok(Value::Object(input.clone()));
    };
    let fields = parse_fields(schema)?;
    let declared_keys: BTreeSet<&str> = fields.iter().map(|field| field.key.as_str()).collect();
    let mut merged = Map::new();
    for (key, value) in existing {
        if declared_keys.contains(key.as_str()) {
            merged.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in input {
        merged.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(merged))
}

/// Folds what a calculation produced back onto the caller's own payload.
///
/// Only keys the calculation actually changed relative to its seeded input are copied over. Seeding
/// must not smuggle a stored value back in as if the caller had submitted it: downstream steps key
/// off "did the caller mention this field" to decide explicit clears, required-field errors and
/// read-only-by-condition rejections, and the field-level write policy is checked against the raw
/// payload before this runs.
pub fn overlay_calculated_values(values: Value, seeded: &Value, calculated: &Value) -> Result<Value, String> {
    let Value::Object(mut output) = values else {
        return Err("values must be a JSON object".to_string());
    };
    let seeded = seeded
        .as_object()
        .ok_or_else(|| "values must be a JSON object".to_string())?;
    let calculated = calculated
        .as_object()
        .ok_or_else(|| "values must be a JSON object".to_string())?;
    for (key, value) in calculated {
        if seeded.get(key) != Some(value) {
            output.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(output))
}

pub fn evaluate_calculated_values(schema: &Value, values: Value) -> Result<Value, String> {
    let Some(input) = values.as_object() else {
        return Err("values must be a JSON object".to_string());
    };
    let mut output = input.clone();
    let fields = parse_fields(schema)?;
    let schema_fields = schema
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for field in fields {
        let Some(field_schema) = schema_fields
            .iter()
            .find(|item| item.get("key").and_then(Value::as_str) == Some(field.key.as_str()))
        else {
            continue;
        };
        let Some(formula) = field_schema.get("formula").and_then(Value::as_object) else {
            continue;
        };
        if formula
            .get("op")
            .and_then(Value::as_str)
            .is_some_and(|op| op.starts_with("child_"))
        {
            continue;
        }
        let calculated = evaluate_formula(formula, &output)?;
        let field_value = formula_result_to_field_value(&field, formula, calculated)?;
        output.insert(field.key.clone(), field_value);
    }

    Ok(Value::Object(output))
}

fn evaluate_formula(formula: &Map<String, Value>, values: &Map<String, Value>) -> Result<Decimal, String> {
    let op = formula
        .get("op")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("add");
    let args = formula
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| "formula.args must be an array".to_string())?;
    if args.is_empty() {
        return Err("formula.args must not be empty".to_string());
    }
    let operands = args
        .iter()
        .map(|arg| formula_arg_decimal(arg, values))
        .collect::<Result<Vec<_>, _>>()?;

    match op {
        "add" | "sum" => Ok(operands.into_iter().sum()),
        "subtract" => fold_binary(operands, |left, right| Ok(left - right)),
        "multiply" => fold_binary(operands, |left, right| Ok(left * right)),
        "divide" => fold_binary(operands, |left, right| {
            if right.is_zero() {
                Err("formula divide by zero".to_string())
            } else {
                Ok(left / right)
            }
        }),
        "min" => operands
            .into_iter()
            .min()
            .ok_or_else(|| "formula.args must not be empty".to_string()),
        "max" => operands
            .into_iter()
            .max()
            .ok_or_else(|| "formula.args must not be empty".to_string()),
        "count" => Ok(Decimal::from(operands.len())),
        _ => Err(format!("unsupported formula op: {op}")),
    }
}

fn fold_binary<F>(operands: Vec<Decimal>, op: F) -> Result<Decimal, String>
where
    F: Fn(Decimal, Decimal) -> Result<Decimal, String>,
{
    let mut iter = operands.into_iter();
    let Some(mut result) = iter.next() else {
        return Err("formula.args must not be empty".to_string());
    };
    for operand in iter {
        result = op(result, operand)?;
    }
    Ok(result)
}

fn formula_arg_decimal(arg: &Value, values: &Map<String, Value>) -> Result<Decimal, String> {
    if let Some(field_key) = arg.as_str() {
        if let Some(value) = values.get(field_key) {
            return value_to_decimal(value).map_err(|error| format!("formula field '{field_key}': {error}"));
        }
        return Decimal::from_str(field_key.trim()).map_err(|_| format!("formula field '{field_key}' is missing"));
    }
    if let Some(object) = arg.as_object() {
        if let Some(field_key) = object.get("field").and_then(Value::as_str) {
            let value = values
                .get(field_key)
                .ok_or_else(|| format!("formula field '{field_key}' is missing"))?;
            return value_to_decimal(value).map_err(|error| format!("formula field '{field_key}': {error}"));
        }
        if let Some(value) = object.get("value").and_then(Value::as_str) {
            return Decimal::from_str(value.trim()).map_err(|_| "formula constant is invalid".to_string());
        }
    }
    Err("formula args must be field names or decimal string constants".to_string())
}

fn value_to_decimal(value: &Value) -> Result<Decimal, String> {
    if let Some(value) = value.as_str() {
        return Decimal::from_str(value.trim()).map_err(|_| "decimal string is invalid".to_string());
    }
    if let Some(decimal) = value.get("decimal").and_then(Value::as_str) {
        return Decimal::from_str(decimal.trim()).map_err(|_| "decimal string is invalid".to_string());
    }
    if let Some(value) = value.get("value").and_then(Value::as_i64) {
        return Ok(Decimal::from(value));
    }
    if let Some(value) = value.as_i64() {
        return Ok(Decimal::from(value));
    }
    Err("value is not decimal-compatible".to_string())
}

fn formula_result_to_field_value(
    field: &FormField,
    formula: &Map<String, Value>,
    value: Decimal,
) -> Result<Value, String> {
    match field.field_type.as_str() {
        "amount" => {
            let config = field
                .amount
                .as_ref()
                .ok_or_else(|| format!("field '{}' amount config is missing", field.key))?;
            let decimal = rounded_decimal_string(value, config.scale, &config.rounding);
            Ok(json!({
                "decimal": decimal,
                "currency": config.currency
            }))
        }
        "number" | "formula" => {
            let scale = formula.get("scale").and_then(Value::as_u64).unwrap_or(8).min(8) as u32;
            Ok(json!(rounded_decimal_string(value, scale, "half_up")))
        }
        "integer" => {
            if value.fract() != Decimal::ZERO {
                return Err(format!("field '{}' formula result must be an integer", field.key));
            }
            Ok(json!(value.to_i64().ok_or_else(|| {
                format!("field '{}' formula result is outside integer range", field.key)
            })?))
        }
        _ => Ok(json!(value.normalize().to_string())),
    }
}

fn rounded_decimal_string(value: Decimal, scale: u32, rounding: &str) -> String {
    let rounded = value.round_dp_with_strategy(scale, rounding_strategy(rounding));
    format!("{:.*}", scale as usize, rounded)
}

fn rounding_strategy(rounding: &str) -> RoundingStrategy {
    match rounding {
        "half_even" => RoundingStrategy::MidpointNearestEven,
        "down" => RoundingStrategy::ToZero,
        "up" => RoundingStrategy::AwayFromZero,
        _ => RoundingStrategy::MidpointAwayFromZero,
    }
}

trait DecimalToI64 {
    fn to_i64(self) -> Option<i64>;
}

impl DecimalToI64 for Decimal {
    fn to_i64(self) -> Option<i64> {
        self.to_string().parse::<i64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{evaluate_calculated_values, merge_values_for_calculation, overlay_calculated_values};
    use crate::forms::values::normalize_record_values_with_existing;
    use serde_json::{Value, json};

    /// An order form whose total is computed from a field the caller cannot read.
    fn formula_schema() -> Value {
        json!({
            "fields": [
                {"field_id": "fld_quantity", "key": "quantity", "type": "integer"},
                {
                    "field_id": "fld_price",
                    "key": "unit_price",
                    "type": "amount",
                    "amount": {"currency": "CNY", "scale": 2}
                },
                {
                    "field_id": "fld_total",
                    "key": "line_total",
                    "type": "amount",
                    "amount": {"currency": "CNY", "scale": 2},
                    "formula": {"op": "multiply", "args": ["quantity", "unit_price"]}
                }
            ]
        })
    }

    /// The non-database part of the record update pipeline, in the order the route runs it.
    fn update_pipeline(schema: &Value, payload: Value, existing: &Value) -> Result<Value, String> {
        let seeded = merge_values_for_calculation(schema, &payload, Some(existing))?;
        let calculated = evaluate_calculated_values(schema, seeded.clone())?;
        let overlaid = overlay_calculated_values(payload, &seeded, &calculated)?;
        normalize_record_values_with_existing(schema, overlaid, Some(existing))
    }

    #[test]
    fn partial_update_of_a_formula_form_no_longer_fails_on_unsubmitted_arguments() {
        let schema = formula_schema();
        let existing = json!({
            "quantity": 3,
            "unit_price": {"decimal": "0.10", "currency": "CNY"},
            "line_total": {"decimal": "0.30", "currency": "CNY"}
        });
        // `unit_price` is `read:false` for this caller, so it never reached the client and cannot be
        // echoed back. Evaluating the raw payload is what used to answer every such update with 400.
        let payload = json!({"quantity": 4});

        let unseeded = evaluate_calculated_values(&schema, payload.clone());
        assert!(
            unseeded
                .as_ref()
                .err()
                .is_some_and(|error| error.contains("formula field 'unit_price' is missing")),
            "unseeded evaluation should still be the failing case: {unseeded:?}"
        );

        let normalized = update_pipeline(&schema, payload, &existing).expect("partial update should succeed");

        assert_eq!(normalized["line_total"]["decimal"].as_str(), Some("0.40"));
        assert_eq!(normalized["quantity"]["value"].as_i64(), Some(4));
        // The value the caller could not see is still on the record.
        assert_eq!(normalized["unit_price"]["decimal"].as_str(), Some("0.10"));
    }

    #[test]
    fn seeding_does_not_make_stored_values_look_submitted() {
        let schema = formula_schema();
        let existing = json!({
            "quantity": 3,
            "unit_price": {"decimal": "0.10", "currency": "CNY"},
            "line_total": {"decimal": "0.30", "currency": "CNY"}
        });
        let payload = json!({"quantity": 4});

        let seeded = merge_values_for_calculation(&schema, &payload, Some(&existing)).expect("seeding should succeed");
        let calculated = evaluate_calculated_values(&schema, seeded.clone()).expect("evaluation should succeed");
        let overlaid = overlay_calculated_values(payload, &seeded, &calculated).expect("overlay should succeed");

        // Only the caller's own key plus the recalculated formula field survive the overlay.
        assert_eq!(overlaid["quantity"].as_i64(), Some(4));
        assert_eq!(overlaid["line_total"]["decimal"].as_str(), Some("0.40"));
        assert!(overlaid.get("unit_price").is_none());
    }

    #[test]
    fn seeding_never_resurrects_a_field_the_schema_dropped() {
        let schema = formula_schema();
        let existing = json!({"quantity": 3, "unit_price": {"decimal": "0.10", "currency": "CNY"}, "legacy": "x"});

        let seeded =
            merge_values_for_calculation(&schema, &json!({}), Some(&existing)).expect("seeding should succeed");

        assert!(seeded.get("legacy").is_none());
        assert!(seeded.get("quantity").is_some());
    }

    #[test]
    fn an_explicit_clear_still_wins_over_the_stored_value() {
        let schema = formula_schema();
        let existing = json!({"quantity": 3, "unit_price": {"decimal": "0.10", "currency": "CNY"}});

        let seeded = merge_values_for_calculation(&schema, &json!({"quantity": Value::Null}), Some(&existing))
            .expect("seeding should succeed");

        assert_eq!(seeded.get("quantity"), Some(&Value::Null));
    }

    #[test]
    fn creation_still_rejects_a_formula_over_a_genuinely_absent_field() {
        let schema = formula_schema();

        let seeded = merge_values_for_calculation(&schema, &json!({"quantity": 4}), None).expect("seeding is a no-op");
        let calculated = evaluate_calculated_values(&schema, seeded);

        assert!(
            calculated
                .err()
                .is_some_and(|error| error.contains("formula field 'unit_price' is missing"))
        );
    }

    #[test]
    fn multiplies_amount_formula_without_float() {
        let schema = json!({
            "fields": [
                {"field_id": "fld_quantity", "key": "quantity", "type": "integer"},
                {"field_id": "fld_price", "key": "unit_price", "type": "amount", "amount": {"currency": "CNY", "scale": 2}},
                {
                    "field_id": "fld_total",
                    "key": "line_total",
                    "type": "amount",
                    "amount": {"currency": "CNY", "scale": 2},
                    "formula": {"op": "multiply", "args": ["quantity", "unit_price"]}
                }
            ]
        });
        let values = evaluate_calculated_values(&schema, json!({"quantity": 3, "unit_price": "0.10"})).unwrap();
        assert_eq!(values["line_total"]["decimal"].as_str(), Some("0.30"));
    }

    #[test]
    fn rejects_divide_by_zero() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_total",
                "key": "total",
                "type": "number",
                "formula": {"op": "divide", "args": [{"value": "1"}, {"value": "0"}]}
            }]
        });
        assert!(evaluate_calculated_values(&schema, json!({})).is_err());
    }

    #[test]
    fn skips_child_aggregate_formulas_for_child_pipeline() {
        let schema = json!({
            "fields": [{
                "field_id": "fld_total",
                "key": "order_total",
                "type": "amount",
                "amount": {"currency": "CNY", "scale": 2},
                "formula": {"op": "child_sum", "relation_key": "lines", "field": "line_total"}
            }]
        });
        let values = evaluate_calculated_values(&schema, json!({"order_name": "A"})).unwrap();
        assert_eq!(values.get("order_total"), None);
    }
}
