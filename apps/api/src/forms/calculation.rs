use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value, json};
use std::str::FromStr;

use super::schema::{FormField, parse_fields};

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
    use super::evaluate_calculated_values;
    use serde_json::json;

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
