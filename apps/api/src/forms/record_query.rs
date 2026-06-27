use serde_json::Value;

use crate::{
    error::ApiError,
    forms::schema::{FormField, normalize_key},
};

#[derive(Debug, Default)]
pub struct RecordQueryConfig {
    pub filter_expression: Option<RecordFilterExpressionConfig>,
    pub sort: Option<String>,
}

#[derive(Debug)]
pub struct RecordFilterConfig {
    field: String,
    op: String,
    value: Option<String>,
}

#[derive(Debug)]
struct RecordFilterGroupConfig {
    logic: RecordFilterLogic,
    filters: Vec<RecordFilterConfig>,
}

#[derive(Debug)]
pub enum RecordFilterExpressionConfig {
    Condition(RecordFilterConfig),
    Group {
        logic: RecordFilterLogic,
        children: Vec<RecordFilterExpressionConfig>,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum RecordFilterLogic {
    All,
    Any,
}

#[derive(Debug, Clone, Copy)]
pub enum QueryValueKind {
    Text,
    Decimal,
    Bool,
    Datetime,
}

pub fn query_filter_expression(
    filter_field: Option<&str>,
    filter_op: Option<String>,
    filter_value: Option<String>,
) -> Option<RecordFilterExpressionConfig> {
    filter_field.map(|field| {
        RecordFilterExpressionConfig::Condition(RecordFilterConfig {
            field: field.to_string(),
            op: filter_op.unwrap_or_else(|| "eq".to_string()),
            value: filter_value,
        })
    })
}

pub fn record_filter_expression_from_config(config: &Value) -> Option<RecordFilterExpressionConfig> {
    if let Some(value) = config.get("filter_expression") {
        let mut leaf_count = 0;
        if let Some(expression) = record_filter_expression_config_from_json(value, 0, &mut leaf_count) {
            return Some(expression);
        }
    }
    let groups = record_filter_groups_from_config(config);
    if !groups.is_empty() {
        return Some(record_filter_expression_from_groups(groups));
    }
    None
}

fn record_filter_groups_from_config(config: &Value) -> Vec<RecordFilterGroupConfig> {
    let groups = config.get("filter_groups").and_then(Value::as_array).map(|groups| {
        groups
            .iter()
            .take(5)
            .filter_map(Value::as_object)
            .filter_map(record_filter_group_config_from_json)
            .collect::<Vec<_>>()
    });
    if let Some(groups) = groups.filter(|groups| !groups.is_empty()) {
        return groups;
    }
    let filters = config
        .get("filters")
        .and_then(Value::as_array)
        .map(|filters| {
            filters
                .iter()
                .take(5)
                .filter_map(Value::as_object)
                .filter_map(record_filter_config_from_json)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !filters.is_empty() {
        return vec![RecordFilterGroupConfig {
            logic: RecordFilterLogic::All,
            filters,
        }];
    }
    if let Some(filter) = config
        .get("filter")
        .and_then(Value::as_object)
        .and_then(record_filter_config_from_json)
    {
        return vec![RecordFilterGroupConfig {
            logic: RecordFilterLogic::All,
            filters: vec![filter],
        }];
    }
    Vec::new()
}

fn record_filter_expression_from_groups(groups: Vec<RecordFilterGroupConfig>) -> RecordFilterExpressionConfig {
    let children = groups
        .into_iter()
        .map(|group| RecordFilterExpressionConfig::Group {
            logic: group.logic,
            children: group
                .filters
                .into_iter()
                .map(RecordFilterExpressionConfig::Condition)
                .collect(),
        })
        .collect::<Vec<_>>();
    if children.len() == 1 {
        children.into_iter().next().unwrap()
    } else {
        RecordFilterExpressionConfig::Group {
            logic: RecordFilterLogic::All,
            children,
        }
    }
}

fn record_filter_expression_config_from_json(
    value: &Value,
    depth: usize,
    leaf_count: &mut usize,
) -> Option<RecordFilterExpressionConfig> {
    if depth > 4 || *leaf_count >= 20 {
        return None;
    }
    let object = value.as_object()?;
    if record_filter_json_disabled(object) {
        return None;
    }
    if let Some(filter) = object.get("filter").and_then(Value::as_object) {
        let filter = record_filter_config_from_json(filter)?;
        *leaf_count += 1;
        return Some(RecordFilterExpressionConfig::Condition(filter));
    }
    if object.contains_key("field") {
        let filter = record_filter_config_from_json(object)?;
        *leaf_count += 1;
        return Some(RecordFilterExpressionConfig::Condition(filter));
    }
    let children = object
        .get("children")
        .or_else(|| object.get("expressions"))
        .and_then(Value::as_array)?;
    let children = children
        .iter()
        .take(10)
        .filter_map(|child| record_filter_expression_config_from_json(child, depth + 1, leaf_count))
        .collect::<Vec<_>>();
    if children.is_empty() {
        return None;
    }
    Some(RecordFilterExpressionConfig::Group {
        logic: record_filter_logic_from_value(object.get("logic")),
        children,
    })
}

fn record_filter_group_config_from_json(group: &serde_json::Map<String, Value>) -> Option<RecordFilterGroupConfig> {
    if record_filter_json_disabled(group) {
        return None;
    }
    let logic = record_filter_logic_from_value(group.get("logic"));
    let filters = group.get("filters").and_then(Value::as_array)?;
    let filters = filters
        .iter()
        .take(5)
        .filter_map(Value::as_object)
        .filter_map(record_filter_config_from_json)
        .collect::<Vec<_>>();
    if filters.is_empty() {
        return None;
    }
    Some(RecordFilterGroupConfig { logic, filters })
}

fn record_filter_json_disabled(filter: &serde_json::Map<String, Value>) -> bool {
    filter.get("disabled").and_then(Value::as_bool) == Some(true)
        || filter.get("enabled").and_then(Value::as_bool) == Some(false)
}

fn record_filter_logic_from_value(value: Option<&Value>) -> RecordFilterLogic {
    match value.and_then(Value::as_str).unwrap_or("all") {
        "any" | "or" => RecordFilterLogic::Any,
        _ => RecordFilterLogic::All,
    }
}

fn record_filter_config_from_json(filter: &serde_json::Map<String, Value>) -> Option<RecordFilterConfig> {
    if record_filter_json_disabled(filter) {
        return None;
    }
    let field = filter.get("field").and_then(Value::as_str).map(str::trim)?;
    if field.is_empty() {
        return None;
    }
    let op = match filter.get("operator").and_then(Value::as_str).unwrap_or("contains") {
        "equals" | "eq" => "eq",
        "not_equals" | "neq" => "neq",
        "not_empty" => "not_empty",
        "contains" => "contains",
        value => value,
    };
    let value = filter
        .get("value")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if op != "not_empty" && value.is_none() {
        return None;
    }
    Some(RecordFilterConfig {
        field: field.to_string(),
        op: op.to_string(),
        value,
    })
}

pub fn indexed_value_kind(field_type: &str) -> Option<QueryValueKind> {
    match field_type {
        "amount" | "number" | "integer" => Some(QueryValueKind::Decimal),
        "boolean" => Some(QueryValueKind::Bool),
        "date" | "datetime" => Some(QueryValueKind::Datetime),
        "text" | "textarea" | "rich_text" | "single_select" | "ai_summary" => Some(QueryValueKind::Text),
        _ => None,
    }
}

fn normalize_filter_op(raw: &str) -> Result<String, ApiError> {
    let op = raw.trim().to_ascii_lowercase();
    match op.as_str() {
        "eq" | "neq" | "not_equals" | "contains" | "gt" | "gte" | "lt" | "lte" | "not_empty" => {
            Ok(if op == "not_equals" { "neq".to_string() } else { op })
        }
        _ => Err(ApiError::BadRequest("unsupported filter_op".to_string())),
    }
}

pub fn append_record_filter_expression_sql(
    fields: &[FormField],
    filter_expression: Option<&RecordFilterExpressionConfig>,
    where_parts: &mut Vec<String>,
    values: &mut Vec<sea_orm::Value>,
    next_idx: &mut usize,
) -> Result<(), ApiError> {
    if let Some(expression) = filter_expression
        && let Some(condition) = record_filter_expression_sql(fields, expression, values, next_idx, &mut 0)?
    {
        where_parts.push(condition);
    }
    Ok(())
}

fn record_filter_expression_sql(
    fields: &[FormField],
    expression: &RecordFilterExpressionConfig,
    values: &mut Vec<sea_orm::Value>,
    next_idx: &mut usize,
    alias_idx: &mut usize,
) -> Result<Option<String>, ApiError> {
    match expression {
        RecordFilterExpressionConfig::Condition(filter) => {
            let field = find_schema_field(fields, &filter.field)?;
            let op = normalize_filter_op(&filter.op)?;
            let filter_value = filter
                .value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if op != "not_empty" && filter_value.is_none() {
                return Err(ApiError::BadRequest(
                    "filter_value is required when filter_field is set".to_string(),
                ));
            }
            let kind = indexed_value_kind(&field.field_type)
                .ok_or_else(|| ApiError::BadRequest("filter_field is not queryable".to_string()))?;
            let alias = format!("filter_idx_{}", *alias_idx);
            *alias_idx += 1;
            let field_key_idx = *next_idx;
            values.push(field.key.clone().into());
            *next_idx += 1;
            let value_idx = *next_idx;
            let condition = filter_condition_sql(kind, &op, &alias, value_idx)?;
            if let Some(filter_value) = filter_value {
                values.push(filter_value.into());
                *next_idx += 1;
            }
            Ok(Some(format!(
                "EXISTS (SELECT 1 FROM form_record_field_index {alias} WHERE {alias}.record_id = form_records.id AND {alias}.form_id = form_records.form_id AND {alias}.field_key = ${field_key_idx} AND {condition})"
            )))
        }
        RecordFilterExpressionConfig::Group { logic, children } => {
            let child_conditions = children
                .iter()
                .map(|child| record_filter_expression_sql(fields, child, values, next_idx, alias_idx))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if child_conditions.is_empty() {
                return Ok(None);
            }
            let separator = match logic {
                RecordFilterLogic::All => " AND ",
                RecordFilterLogic::Any => " OR ",
            };
            Ok(Some(format!("({})", child_conditions.join(separator))))
        }
    }
}

fn filter_condition_sql(kind: QueryValueKind, op: &str, alias: &str, value_idx: usize) -> Result<String, ApiError> {
    match (kind, op) {
        (QueryValueKind::Text, "eq") => Ok(format!("{alias}.value_text = ${value_idx}")),
        (QueryValueKind::Text, "neq") => Ok(format!("{alias}.value_text <> ${value_idx}")),
        (QueryValueKind::Text, "contains") => Ok(format!("{alias}.value_text ILIKE '%' || ${value_idx} || '%'")),
        (QueryValueKind::Text, "not_empty") => Ok(format!("COALESCE({alias}.value_text, '') <> ''")),
        (QueryValueKind::Decimal, "eq") => Ok(format!("{alias}.value_decimal = ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "neq") => Ok(format!("{alias}.value_decimal <> ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "gt") => Ok(format!("{alias}.value_decimal > ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "gte") => Ok(format!("{alias}.value_decimal >= ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "lt") => Ok(format!("{alias}.value_decimal < ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "lte") => Ok(format!("{alias}.value_decimal <= ${value_idx}::numeric")),
        (QueryValueKind::Decimal, "not_empty") => Ok(format!("{alias}.value_decimal IS NOT NULL")),
        (QueryValueKind::Bool, "eq") => Ok(format!("{alias}.value_bool = ${value_idx}::boolean")),
        (QueryValueKind::Bool, "neq") => Ok(format!("{alias}.value_bool <> ${value_idx}::boolean")),
        (QueryValueKind::Bool, "not_empty") => Ok(format!("{alias}.value_bool IS NOT NULL")),
        (QueryValueKind::Datetime, "eq") => Ok(format!("{alias}.value_datetime = ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "neq") => Ok(format!("{alias}.value_datetime <> ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "gt") => Ok(format!("{alias}.value_datetime > ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "gte") => Ok(format!("{alias}.value_datetime >= ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "lt") => Ok(format!("{alias}.value_datetime < ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "lte") => Ok(format!("{alias}.value_datetime <= ${value_idx}::timestamptz")),
        (QueryValueKind::Datetime, "not_empty") => Ok(format!("{alias}.value_datetime IS NOT NULL")),
        _ => Err(ApiError::BadRequest(
            "filter_op is not supported for field type".to_string(),
        )),
    }
}

pub fn record_sort_sql(
    raw_sort: Option<&str>,
    fields: &[FormField],
    key_idx: usize,
) -> Result<(Option<String>, String, Option<String>), ApiError> {
    let Some(raw_sort) = raw_sort.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, "form_records.updated_at DESC".to_string(), None));
    };
    let mut parts = raw_sort.split(':');
    let raw_field = parts.next().unwrap_or_default().trim();
    let direction = parts.next().unwrap_or("asc").trim().to_ascii_lowercase();
    let direction = match direction.as_str() {
        "asc" => "ASC",
        "desc" => "DESC",
        _ => return Err(ApiError::BadRequest("unsupported sort direction".to_string())),
    };
    match raw_field {
        "title" => return Ok((None, format!("form_records.title {direction}"), None)),
        "created_at" => return Ok((None, format!("form_records.created_at {direction}"), None)),
        "updated_at" => return Ok((None, format!("form_records.updated_at {direction}"), None)),
        _ => {}
    }

    let field = find_schema_field(fields, raw_field)?;
    let kind = indexed_value_kind(&field.field_type)
        .ok_or_else(|| ApiError::BadRequest("sort field is not queryable".to_string()))?;
    let join = format!(
        "LEFT JOIN form_record_field_index sort_idx ON sort_idx.record_id = form_records.id AND sort_idx.form_id = form_records.form_id AND sort_idx.field_key = ${key_idx}"
    );
    let value_expr = match kind {
        QueryValueKind::Text => "LOWER(sort_idx.value_text)",
        QueryValueKind::Decimal => "sort_idx.value_decimal",
        QueryValueKind::Bool => "sort_idx.value_bool",
        QueryValueKind::Datetime => "sort_idx.value_datetime",
    };
    Ok((
        Some(join),
        format!("{value_expr} {direction} NULLS LAST, form_records.updated_at DESC"),
        Some(field.key.clone()),
    ))
}

fn find_schema_field<'a>(fields: &'a [FormField], raw_key: &str) -> Result<&'a FormField, ApiError> {
    let key = normalize_key(raw_key).map_err(ApiError::BadRequest)?;
    fields
        .iter()
        .find(|field| field.key == key)
        .ok_or_else(|| ApiError::BadRequest("field does not exist in form schema".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        append_record_filter_expression_sql, indexed_value_kind, query_filter_expression,
        record_filter_expression_from_config, record_sort_sql,
    };
    use crate::forms::schema::FormField;
    use serde_json::json;

    #[test]
    fn parses_nested_filter_expression_and_appends_sql() {
        let expression = record_filter_expression_from_config(&json!({
            "filter_expression": {
                "logic": "any",
                "children": [
                    {"field": "status", "operator": "equals", "value": "open"},
                    {"field": "quantity", "operator": "gt", "value": "2"},
                    {"field": "hidden", "operator": "equals", "value": "ignored", "disabled": true}
                ]
            }
        }));
        let fields = vec![
            test_field("status", "Status", "text"),
            test_field("quantity", "Quantity", "integer"),
            test_field("hidden", "Hidden", "text"),
        ];
        let mut where_parts = Vec::new();
        let mut values = Vec::new();
        let mut next_idx = 1;

        append_record_filter_expression_sql(
            &fields,
            expression.as_ref(),
            &mut where_parts,
            &mut values,
            &mut next_idx,
        )
        .unwrap();

        assert_eq!(where_parts.len(), 1);
        assert!(where_parts[0].contains(" OR "));
        assert!(where_parts[0].contains("filter_idx_0.value_text = $2"));
        assert!(where_parts[0].contains("filter_idx_1.value_decimal > $4::numeric"));
        assert_eq!(values.len(), 4);
        assert_eq!(next_idx, 5);
    }

    #[test]
    fn builds_query_filter_expression_from_request_params() {
        let expression = query_filter_expression(
            Some("status"),
            Some("not_equals".to_string()),
            Some("closed".to_string()),
        );
        let fields = vec![test_field("status", "Status", "single_select")];
        let mut where_parts = Vec::new();
        let mut values = Vec::new();
        let mut next_idx = 1;

        append_record_filter_expression_sql(
            &fields,
            expression.as_ref(),
            &mut where_parts,
            &mut values,
            &mut next_idx,
        )
        .unwrap();

        assert!(where_parts[0].contains("filter_idx_0.value_text <> $2"));
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn builds_indexed_field_sort_sql() {
        let fields = vec![test_field("quantity", "Quantity", "integer")];
        let (join, order_by, field_key) = record_sort_sql(Some("quantity:desc"), &fields, 7).unwrap();

        assert!(join.unwrap().contains("sort_idx.field_key = $7"));
        assert_eq!(
            order_by,
            "sort_idx.value_decimal DESC NULLS LAST, form_records.updated_at DESC"
        );
        assert_eq!(field_key.as_deref(), Some("quantity"));
    }

    #[test]
    fn detects_indexed_field_kinds() {
        assert!(indexed_value_kind("text").is_some());
        assert!(indexed_value_kind("integer").is_some());
        assert!(indexed_value_kind("boolean").is_some());
        assert!(indexed_value_kind("datetime").is_some());
        assert!(indexed_value_kind("attachment").is_none());
    }

    fn test_field(key: &str, label: &str, field_type: &str) -> FormField {
        FormField {
            field_id: format!("fld_{key}"),
            key: key.to_string(),
            label: label.to_string(),
            field_type: field_type.to_string(),
            required: false,
            options: Vec::new(),
            option_disabled: Vec::new(),
            option_archived: Vec::new(),
            conditional: None,
            amount: None,
            signature: None,
        }
    }
}
