use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::{
    error::ApiError,
    forms::{
        record_query::indexed_value_kind,
        schema::{FormField, normalize_key, parse_fields},
    },
};

#[derive(Debug, Serialize)]
pub struct FormSchemaSummaryResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub field_count: usize,
    pub view_count: i64,
    pub record_count: i64,
    pub archived_record_count: i64,
    pub fields: Vec<FormFieldSummary>,
}

#[derive(Debug, Serialize)]
pub struct FormFieldSummary {
    pub field_id: String,
    pub key: String,
    pub label: String,
    pub field_type: String,
    pub required: bool,
    pub indexed: bool,
    pub list_visible: bool,
    pub detail_visible: bool,
}

#[derive(Debug, Serialize)]
pub struct FormFieldUsageResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub fields: Vec<FormFieldUsage>,
}

#[derive(Debug, Serialize)]
pub struct FormFieldUsage {
    pub field_id: String,
    pub field_key: String,
    pub field_type: String,
    pub value_count: i64,
    pub indexed_count: i64,
    pub dependency_count: usize,
}

#[derive(Debug, Serialize)]
pub struct FormFieldDependenciesResponse {
    pub form_id: Uuid,
    pub schema_version: i32,
    pub dependencies: Vec<FormFieldDependency>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct FormFieldDependency {
    pub field_id: String,
    pub field_key: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub source_key: Option<String>,
    pub reason: String,
    pub blocking: bool,
}

#[derive(Debug, FromQueryResult)]
struct ViewDependencyRow {
    id: Uuid,
    key: String,
    config: Value,
}

pub async fn form_schema_summary(
    state: &AppState,
    form_id: Uuid,
    schema_version: i32,
    schema: &Value,
) -> Result<FormSchemaSummaryResponse, ApiError> {
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let view_count = count_query(
        state,
        "SELECT COUNT(*)::bigint AS count FROM form_views WHERE form_id = $1 AND archived_at IS NULL",
        vec![form_id.into()],
    )
    .await?;
    let record_count = count_query(
        state,
        "SELECT COUNT(*)::bigint AS count FROM form_records WHERE form_id = $1 AND archived_at IS NULL",
        vec![form_id.into()],
    )
    .await?;
    let archived_record_count = count_query(
        state,
        "SELECT COUNT(*)::bigint AS count FROM form_records WHERE form_id = $1 AND archived_at IS NOT NULL",
        vec![form_id.into()],
    )
    .await?;
    let fields = fields
        .into_iter()
        .map(|field| FormFieldSummary {
            field_id: field.field_id,
            key: field.key,
            label: field.label,
            indexed: indexed_value_kind(&field.field_type).is_some(),
            field_type: field.field_type,
            required: field.required,
            list_visible: true,
            detail_visible: true,
        })
        .collect::<Vec<_>>();

    Ok(FormSchemaSummaryResponse {
        form_id,
        schema_version,
        field_count: fields.len(),
        view_count,
        record_count,
        archived_record_count,
        fields,
    })
}

pub async fn form_field_usage(
    state: &AppState,
    input: &SchemaInsightInput<'_>,
) -> Result<FormFieldUsageResponse, ApiError> {
    let fields = parse_fields(input.schema).map_err(ApiError::BadRequest)?;
    let dependencies = collect_field_dependencies(state, input).await?;
    let mut dependency_counts: BTreeMap<String, usize> = BTreeMap::new();
    for dependency in dependencies {
        *dependency_counts.entry(dependency.field_key).or_default() += 1;
    }

    let mut usage = Vec::with_capacity(fields.len());
    for field in fields {
        let value_count = count_query(
            state,
            "SELECT COUNT(*)::bigint AS count FROM form_records WHERE form_id = $1 AND archived_at IS NULL AND values ? $2",
            vec![input.form_id.into(), field.key.clone().into()],
        )
        .await?;
        let indexed_count = count_query(
            state,
            r"
                SELECT COUNT(*)::bigint AS count
                FROM form_record_field_index
                JOIN form_records ON form_records.id = form_record_field_index.record_id
                WHERE form_record_field_index.form_id = $1
                  AND form_record_field_index.field_key = $2
                  AND form_records.archived_at IS NULL
            ",
            vec![input.form_id.into(), field.key.clone().into()],
        )
        .await?;
        usage.push(FormFieldUsage {
            dependency_count: dependency_counts.get(&field.key).copied().unwrap_or_default(),
            field_id: field.field_id,
            field_key: field.key,
            field_type: field.field_type,
            value_count,
            indexed_count,
        });
    }

    Ok(FormFieldUsageResponse {
        form_id: input.form_id,
        schema_version: input.schema_version,
        fields: usage,
    })
}

pub async fn form_field_dependencies(
    state: &AppState,
    input: &SchemaInsightInput<'_>,
) -> Result<FormFieldDependenciesResponse, ApiError> {
    Ok(FormFieldDependenciesResponse {
        form_id: input.form_id,
        schema_version: input.schema_version,
        dependencies: collect_field_dependencies(state, input).await?,
    })
}

pub struct SchemaInsightInput<'a> {
    pub form_id: Uuid,
    pub form_key: &'a str,
    pub schema_version: i32,
    pub schema: &'a Value,
    pub detail_layout: &'a Value,
    pub title_template: &'a str,
}

pub async fn collect_field_dependencies(
    state: &AppState,
    input: &SchemaInsightInput<'_>,
) -> Result<Vec<FormFieldDependency>, ApiError> {
    let fields = parse_fields(input.schema).map_err(ApiError::BadRequest)?;
    let mut dependencies = Vec::new();
    let views = ViewDependencyRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, key, config
            FROM form_views
            WHERE form_id = $1 AND archived_at IS NULL
        ",
        vec![input.form_id.into()],
    ))
    .all(&state.db)
    .await?;

    for field in &fields {
        if input.title_template.contains(&format!("{{{}}}", field.key)) {
            dependencies.push(FormFieldDependency {
                field_id: field.field_id.clone(),
                field_key: field.key.clone(),
                source_type: "title_template".to_string(),
                source_id: Some(input.form_id.to_string()),
                source_key: Some(input.form_key.to_string()),
                reason: "form title template references field".to_string(),
                blocking: true,
            });
        }
        if json_contains_string(input.detail_layout, &field.key)
            || json_contains_string(input.detail_layout, &field.field_id)
        {
            dependencies.push(FormFieldDependency {
                field_id: field.field_id.clone(),
                field_key: field.key.clone(),
                source_type: "detail_layout".to_string(),
                source_id: Some(input.form_id.to_string()),
                source_key: Some(input.form_key.to_string()),
                reason: "detail layout references field".to_string(),
                blocking: true,
            });
        }
        for view in &views {
            if json_contains_string(&view.config, &field.key) || json_contains_string(&view.config, &field.field_id) {
                dependencies.push(FormFieldDependency {
                    field_id: field.field_id.clone(),
                    field_key: field.key.clone(),
                    source_type: "view".to_string(),
                    source_id: Some(view.id.to_string()),
                    source_key: Some(view.key.clone()),
                    reason: "view config references field".to_string(),
                    blocking: true,
                });
            }
        }
    }

    if let Some(schema_fields) = input.schema.get("fields").and_then(Value::as_array) {
        for source_field in schema_fields {
            let source_key = source_field.get("key").and_then(Value::as_str).map(str::to_string);
            let source_field_id = source_field.get("field_id").and_then(Value::as_str).map(str::to_string);
            for field in &fields {
                if source_key.as_deref() == Some(field.key.as_str())
                    || source_field_id.as_deref() == Some(field.field_id.as_str())
                {
                    continue;
                }
                if json_contains_string(source_field, &field.key) || json_contains_string(source_field, &field.field_id)
                {
                    dependencies.push(FormFieldDependency {
                        field_id: field.field_id.clone(),
                        field_key: field.key.clone(),
                        source_type: "schema_field".to_string(),
                        source_id: source_field_id.clone(),
                        source_key: source_key.clone(),
                        reason: "another field configuration references field".to_string(),
                        blocking: true,
                    });
                }
            }
        }
    }

    Ok(deduplicate_dependencies(dependencies))
}

pub fn find_schema_field<'a>(fields: &'a [FormField], raw_key: &str) -> Result<&'a FormField, ApiError> {
    let key = normalize_key(raw_key).map_err(ApiError::BadRequest)?;
    fields
        .iter()
        .find(|field| field.key == key)
        .ok_or_else(|| ApiError::BadRequest("field does not exist in form schema".to_string()))
}

fn deduplicate_dependencies(dependencies: Vec<FormFieldDependency>) -> Vec<FormFieldDependency> {
    let mut seen = BTreeSet::new();
    dependencies
        .into_iter()
        .filter(|dependency| {
            seen.insert(format!(
                "{}:{}:{}:{}:{}",
                dependency.field_id,
                dependency.field_key,
                dependency.source_type,
                dependency.source_id.as_deref().unwrap_or_default(),
                dependency.source_key.as_deref().unwrap_or_default()
            ))
        })
        .collect()
}

fn json_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value == needle || value.contains(needle),
        Value::Array(items) => items.iter().any(|item| json_contains_string(item, needle)),
        Value::Object(object) => object.values().any(|item| json_contains_string(item, needle)),
        _ => false,
    }
}

async fn count_query(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    state
        .db
        .query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")
        .map_err(ApiError::from)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{FormFieldDependency, SchemaInsightInput, find_schema_field, json_contains_string};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn detects_nested_json_string_references() {
        let value = json!({"sections": [{"fields": ["customer_name"]}]});
        assert!(json_contains_string(&value, "customer_name"));
        assert!(!json_contains_string(&value, "missing_field"));
    }

    #[test]
    fn deduplicates_dependency_identity() {
        let dependencies = super::deduplicate_dependencies(vec![
            FormFieldDependency {
                field_id: "fld_name".to_string(),
                field_key: "name".to_string(),
                source_type: "view".to_string(),
                source_id: Some("view-1".to_string()),
                source_key: Some("grid".to_string()),
                reason: "view config references field".to_string(),
                blocking: true,
            },
            FormFieldDependency {
                field_id: "fld_name".to_string(),
                field_key: "name".to_string(),
                source_type: "view".to_string(),
                source_id: Some("view-1".to_string()),
                source_key: Some("grid".to_string()),
                reason: "duplicate reason is ignored".to_string(),
                blocking: false,
            },
        ]);

        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].reason, "view config references field");
    }

    #[test]
    fn finds_schema_field_by_normalized_key() {
        let fields = crate::forms::schema::parse_fields(&json!({
            "fields": [{"field_id": "fld_name", "key": "name", "type": "text"}]
        }))
        .unwrap();

        assert_eq!(find_schema_field(&fields, "name").unwrap().field_id, "fld_name");
        assert!(find_schema_field(&fields, "missing").is_err());
    }

    #[test]
    fn dependency_input_shape_is_stable() {
        let form_id = Uuid::nil();
        let schema = json!({"fields": []});
        let detail_layout = json!({});
        let input = SchemaInsightInput {
            form_id,
            form_key: "empty",
            schema_version: 1,
            schema: &schema,
            detail_layout: &detail_layout,
            title_template: "{title}",
        };

        assert_eq!(input.form_id, form_id);
        assert_eq!(input.schema_version, 1);
    }
}
