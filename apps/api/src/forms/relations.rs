use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{error::ApiError, forms::schema::normalize_key};

#[derive(Debug, Deserialize)]
pub struct RelationTargetsQuery {
    pub field_key: Option<String>,
    pub form_key: Option<String>,
    pub q: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListChildrenQuery {
    pub relation_key: Option<String>,
    pub child_form_key: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateChildRecordRequest {
    pub child_form_id: Option<Uuid>,
    pub child_form_key: Option<String>,
    pub relation_key: String,
    pub values: Value,
    pub title: Option<String>,
    pub source: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChildRecordRequest {
    pub relation_key: Option<String>,
    pub values: Option<Value>,
    pub title: Option<String>,
    pub source: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChildRecordRelationQuery {
    pub relation_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLinkRequest {
    pub target_type: String,
    pub target_id: String,
    pub relation_key: String,
    pub relation_type: String,
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct LinkResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub source_record_id: Uuid,
    pub target_type: String,
    pub target_id: String,
    pub relation_key: String,
    pub relation_type: String,
    pub metadata: Value,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct RelationTargetResponse {
    pub record_id: Uuid,
    pub form_id: Uuid,
    pub form_key: String,
    pub form_name: String,
    pub title: String,
    pub display: String,
    pub values: Value,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
pub struct ChildRecordRow {
    pub link_id: Uuid,
    pub relation_key: String,
    pub relation_type: String,
    pub record_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub title: String,
    pub values: Value,
    pub source: Value,
    pub schema_version: Option<i32>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationTargetConfig {
    pub form_key: String,
    pub display_field: Option<String>,
}

pub fn normalize_target_type(raw: &str) -> Result<String, ApiError> {
    match raw.trim() {
        "form_record" | "work_item" | "project_resource" | "external_object" => Ok(raw.trim().to_string()),
        _ => Err(ApiError::BadRequest("unsupported target_type".to_string())),
    }
}

pub fn normalize_relation_type(raw: &str) -> Result<String, ApiError> {
    match raw.trim() {
        "parent_child" | "relation" | "external_relation" => Ok(raw.trim().to_string()),
        _ => Err(ApiError::BadRequest("unsupported relation_type".to_string())),
    }
}

pub fn normalize_optional_relation_key(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_key(value).map_err(ApiError::BadRequest))
        .transpose()
}

pub fn normalize_optional_child_form_key(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_key(value).map_err(ApiError::BadRequest))
        .transpose()
}

pub fn relation_target_config(
    schema: &Value,
    field_key: Option<&str>,
    explicit_form_key: Option<&str>,
) -> Result<RelationTargetConfig, ApiError> {
    let field_relation = field_key
        .and_then(|raw_key| {
            schema.get("fields").and_then(Value::as_array).and_then(|fields| {
                fields.iter().find(|field| {
                    field
                        .get("key")
                        .and_then(Value::as_str)
                        .is_some_and(|key| key == raw_key)
                })
            })
        })
        .and_then(|field| field.get("relation"))
        .and_then(Value::as_object);

    let form_key = explicit_form_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            field_relation
                .and_then(|relation| relation.get("form_key"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| ApiError::BadRequest("relation target form_key is required".to_string()))
        .and_then(|value| normalize_key(&value).map_err(ApiError::BadRequest))?;

    let display_field = field_relation
        .and_then(|relation| relation.get("display_field"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(RelationTargetConfig {
        form_key,
        display_field,
    })
}

/// Returns the parent schema field declared under `relation_key`, if the schema declares one.
fn parent_schema_field<'a>(parent_schema: &'a Value, relation_key: &str) -> Option<&'a Value> {
    parent_schema
        .get("fields")
        .and_then(Value::as_array)
        .and_then(|fields| {
            fields
                .iter()
                .find(|field| field.get("key").and_then(Value::as_str) == Some(relation_key))
        })
}

/// Checks that a declared relation field really is a `parent_child` field pointing at `child_form_key`.
fn ensure_field_declares_parent_child(field: &Value, child_form_key: &str) -> Result<(), ApiError> {
    let relation = field
        .get("relation")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::BadRequest("parent_child relation metadata is required".to_string()))?;
    let relation_type = relation
        .get("relation_type")
        .and_then(Value::as_str)
        .unwrap_or("reference");
    if relation_type != "parent_child" {
        return Err(ApiError::BadRequest(
            "relation field must use relation_type parent_child".to_string(),
        ));
    }
    let target_form_key = relation
        .get("form_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("parent_child relation form_key is required".to_string()))
        .and_then(|value| normalize_key(value).map_err(ApiError::BadRequest))?;
    if target_form_key != child_form_key {
        return Err(ApiError::BadRequest(
            "child form does not match parent_child relation target".to_string(),
        ));
    }
    Ok(())
}

/// Resolves the child table field a parent form declares for `relation_key`.
///
/// Used by the endpoints that create or address a child record *through* a parent form field, where
/// the declaration is what names the child form, so a missing declaration is a client error.
pub fn validate_parent_child_relation(
    parent_schema: &Value,
    child_form_key: &str,
    relation_key: &str,
) -> Result<String, ApiError> {
    let relation_key = normalize_key(relation_key).map_err(ApiError::BadRequest)?;
    parent_schema
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("parent form schema fields are required".to_string()))?;
    let field = parent_schema_field(parent_schema, &relation_key)
        .ok_or_else(|| ApiError::BadRequest("parent_child relation field not found".to_string()))?;
    ensure_field_declares_parent_child(field, child_form_key)?;
    Ok(relation_key)
}

/// Validates a `parent_child` link key only when the parent schema declares a field for it.
///
/// The generic link endpoint also serves the modelling where the child form holds the parent id and
/// the parent form declares no child table field at all, so an undeclared key is a legitimate link
/// rather than an error. A declared key is still validated so a link can never contradict the field
/// that the parent form publishes under the same name.
pub fn validate_optional_parent_child_relation(
    parent_schema: &Value,
    child_form_key: &str,
    relation_key: &str,
) -> Result<String, ApiError> {
    let relation_key = normalize_key(relation_key).map_err(ApiError::BadRequest)?;
    if let Some(field) = parent_schema_field(parent_schema, &relation_key) {
        ensure_field_declares_parent_child(field, child_form_key)?;
    }
    Ok(relation_key)
}

pub async fn find_link(state: &AppState, link_id: Uuid) -> Result<LinkResponse, ApiError> {
    LinkResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, source_record_id, target_type,
                   target_id, relation_key, relation_type, metadata, created_by, created_at
            FROM form_record_links
            WHERE id = $1
        ",
        vec![link_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form record link not found".to_string()))
}

pub async fn find_parent_child_link(
    state: &AppState,
    parent_record_id: Uuid,
    child_record_id: Uuid,
    relation_key: Option<&str>,
) -> Result<LinkResponse, ApiError> {
    let mut values: Vec<sea_orm::Value> = vec![parent_record_id.into(), child_record_id.to_string().into()];
    let mut relation_filter = String::new();
    if let Some(relation_key) = relation_key {
        values.push(relation_key.to_string().into());
        relation_filter = format!(" AND relation_key = ${}", values.len());
    }
    let sql = format!(
        r"
            SELECT id, workspace_id, project_id, source_record_id, target_type,
                   target_id, relation_key, relation_type, metadata, created_by, created_at
            FROM form_record_links
            WHERE source_record_id = $1
              AND target_id = $2
              AND target_type = 'form_record'
              AND relation_type = 'parent_child'
              {relation_filter}
            ORDER BY created_at DESC
            LIMIT 1
        "
    );
    LinkResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::NotFound("parent_child link not found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_optional_child_form_key, normalize_optional_relation_key, normalize_relation_type,
        normalize_target_type, relation_target_config, validate_optional_parent_child_relation,
        validate_parent_child_relation,
    };
    use serde_json::json;

    #[test]
    fn normalizes_relation_and_target_types() {
        assert_eq!(normalize_target_type(" form_record ").unwrap(), "form_record");
        assert_eq!(normalize_relation_type("parent_child").unwrap(), "parent_child");
        assert!(normalize_target_type("form").is_err());
        assert!(normalize_relation_type("lookup").is_err());
    }

    #[test]
    fn reads_relation_target_config_from_field_or_explicit_key() {
        let schema = json!({
            "fields": [{
                "key": "customer",
                "type": "relation",
                "relation": {"form_key": "crm_accounts", "display_field": "name"}
            }]
        });

        let from_field = relation_target_config(&schema, Some("customer"), None).unwrap();
        assert_eq!(from_field.form_key, "crm_accounts");
        assert_eq!(from_field.display_field.as_deref(), Some("name"));

        let explicit = relation_target_config(&schema, Some("customer"), Some("contacts")).unwrap();
        assert_eq!(explicit.form_key, "contacts");
        assert_eq!(explicit.display_field.as_deref(), Some("name"));
    }

    #[test]
    fn validates_parent_child_relation_target() {
        let schema = json!({
            "fields": [{
                "key": "lines",
                "type": "child_table",
                "relation": {"relation_type": "parent_child", "form_key": "order_lines"}
            }]
        });

        assert_eq!(
            validate_parent_child_relation(&schema, "order_lines", "lines").unwrap(),
            "lines"
        );
        assert!(validate_parent_child_relation(&schema, "contacts", "lines").is_err());
    }

    /// The generic link endpoint must serve the modelling where the child form holds the parent id,
    /// which leaves the parent form without a child table field to declare the relation key.
    #[test]
    fn accepts_a_parent_child_link_key_the_parent_schema_does_not_declare() {
        let schema = json!({
            "fields": [
                {"key": "order_no", "type": "text"},
                {"key": "total_amount", "type": "amount"}
            ]
        });

        assert_eq!(
            validate_optional_parent_child_relation(&schema, "order_line", "order_lines").unwrap(),
            "order_lines"
        );
        assert!(validate_parent_child_relation(&schema, "order_line", "order_lines").is_err());
    }

    /// A schema without a usable `fields` array declares nothing, so it cannot contradict the link.
    #[test]
    fn accepts_a_parent_child_link_key_when_the_schema_declares_no_fields() {
        assert_eq!(
            validate_optional_parent_child_relation(&json!({}), "order_line", "order_lines").unwrap(),
            "order_lines"
        );
    }

    /// A declared key still has to agree with the link, otherwise a link could contradict the field
    /// the parent form publishes under the same name.
    #[test]
    fn rejects_a_parent_child_link_key_that_contradicts_the_declared_field() {
        let schema = json!({
            "fields": [{
                "key": "lines",
                "type": "child_table",
                "relation": {"relation_type": "parent_child", "form_key": "order_lines"}
            }, {
                "key": "customer",
                "type": "relation",
                "relation": {"relation_type": "reference", "form_key": "crm_accounts"}
            }]
        });

        assert_eq!(
            validate_optional_parent_child_relation(&schema, "order_lines", "lines").unwrap(),
            "lines"
        );
        assert!(validate_optional_parent_child_relation(&schema, "contacts", "lines").is_err());
        assert!(validate_optional_parent_child_relation(&schema, "crm_accounts", "customer").is_err());
    }

    /// An invalid key is a client error on both paths; relaxing the declaration check must not turn
    /// the relaxed path into one that accepts arbitrary keys.
    #[test]
    fn rejects_malformed_parent_child_link_keys() {
        let schema = json!({"fields": []});

        assert!(validate_optional_parent_child_relation(&schema, "order_line", "Order Lines").is_err());
        assert!(validate_optional_parent_child_relation(&schema, "order_line", "").is_err());
    }

    #[test]
    fn normalizes_optional_child_filters() {
        assert_eq!(
            normalize_optional_relation_key(Some(" line_items "))
                .unwrap()
                .as_deref(),
            Some("line_items")
        );
        assert_eq!(
            normalize_optional_child_form_key(Some(" order_lines "))
                .unwrap()
                .as_deref(),
            Some("order_lines")
        );
        assert_eq!(normalize_optional_relation_key(Some("")).unwrap(), None);
        assert!(normalize_optional_relation_key(Some("Line Items")).is_err());
    }
}
