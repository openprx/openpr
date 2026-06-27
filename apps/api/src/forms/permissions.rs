use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::{error::ApiError, forms::schema::normalize_key};

pub const FORM_PERMISSION_ACTIONS: [&str; 6] = [
    "form.view",
    "form.design",
    "record.create",
    "record.update",
    "record.delete",
    "record.export",
];

const FORM_FIELD_PERMISSION_ACTIONS: [&str; 2] = ["read", "write"];

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct FormPermissionPolicyResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub subject_type: String,
    pub subject_id: String,
    pub policy: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct FormEffectivePermissionResponse {
    pub role: String,
    pub is_bot: bool,
    pub actions: BTreeMap<String, bool>,
    pub record_scope: String,
    pub fields: BTreeMap<String, BTreeMap<String, bool>>,
}

#[derive(Debug, Serialize)]
pub struct FormPermissionsResponse {
    pub form_id: Uuid,
    pub policies: Vec<FormPermissionPolicyResponse>,
    pub effective: FormEffectivePermissionResponse,
}

pub fn role_is_form_admin(role: &str) -> bool {
    matches!(role.trim().to_ascii_lowercase().as_str(), "owner" | "admin")
}

pub async fn form_action_allowed(state: &AppState, form_id: Uuid, role: &str, action: &str) -> Result<bool, ApiError> {
    validate_permission_action(action)?;
    if role_is_form_admin(role) {
        return Ok(true);
    }
    let subject_id = normalize_permission_subject_id(role)?;
    let row = FormPermissionPolicyResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, subject_type, subject_id,
                   policy, created_at, updated_at
            FROM form_permissions
            WHERE form_id = $1 AND subject_type = 'role' AND subject_id = $2
        ",
        vec![form_id.into(), subject_id.into()],
    ))
    .one(&state.db)
    .await?;
    let Some(row) = row else {
        return Ok(true);
    };
    Ok(permission_policy_allows(&row.policy, action))
}

pub async fn list_form_permission_policies(
    state: &AppState,
    form_id: Uuid,
) -> Result<Vec<FormPermissionPolicyResponse>, ApiError> {
    FormPermissionPolicyResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, subject_type, subject_id,
                   policy, created_at, updated_at
            FROM form_permissions
            WHERE form_id = $1
            ORDER BY subject_type ASC, subject_id ASC
        ",
        vec![form_id.into()],
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

pub fn form_permissions_response(
    form_id: Uuid,
    role: String,
    is_bot: bool,
    policies: Vec<FormPermissionPolicyResponse>,
) -> FormPermissionsResponse {
    let mut actions = BTreeMap::new();
    for action in FORM_PERMISSION_ACTIONS {
        actions.insert(
            action.to_string(),
            effective_permission_from_policies(&role, &policies, action),
        );
    }
    let record_scope = effective_record_scope_from_policies(&role, is_bot, &policies);
    let fields = effective_field_permissions_from_policies(&role, is_bot, &policies);
    FormPermissionsResponse {
        form_id,
        policies,
        effective: FormEffectivePermissionResponse {
            record_scope,
            role,
            is_bot,
            actions,
            fields,
        },
    }
}

fn effective_permission_from_policies(role: &str, policies: &[FormPermissionPolicyResponse], action: &str) -> bool {
    if role_is_form_admin(role) {
        return true;
    }
    let Ok(subject_id) = normalize_permission_subject_id(role) else {
        return false;
    };
    policies
        .iter()
        .find(|policy| policy.subject_type == "role" && policy.subject_id == subject_id)
        .map(|policy| permission_policy_allows(&policy.policy, action))
        .unwrap_or(true)
}

fn effective_field_permissions_from_policies(
    role: &str,
    is_bot: bool,
    policies: &[FormPermissionPolicyResponse],
) -> BTreeMap<String, BTreeMap<String, bool>> {
    if is_bot || role_is_form_admin(role) {
        return BTreeMap::new();
    }
    let Ok(subject_id) = normalize_permission_subject_id(role) else {
        return BTreeMap::new();
    };
    let Some(policy) = policies
        .iter()
        .find(|policy| policy.subject_type == "role" && policy.subject_id == subject_id)
    else {
        return BTreeMap::new();
    };
    permission_policy_fields(&policy.policy)
}

pub fn permission_policy_allows(policy: &Value, action: &str) -> bool {
    policy
        .get("actions")
        .and_then(Value::as_object)
        .and_then(|actions| actions.get(action))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

pub fn permission_policy_fields(policy: &Value) -> BTreeMap<String, BTreeMap<String, bool>> {
    let mut fields = BTreeMap::new();
    let Some(field_policies) = policy.get("fields").and_then(Value::as_object) else {
        return fields;
    };
    for (field_key, field_policy) in field_policies {
        let Some(field_policy) = field_policy.as_object() else {
            continue;
        };
        let mut actions = BTreeMap::new();
        for action in FORM_FIELD_PERMISSION_ACTIONS {
            if let Some(allowed) = field_policy.get(action).and_then(Value::as_bool) {
                actions.insert(action.to_string(), allowed);
            }
        }
        if !actions.is_empty() {
            fields.insert(field_key.clone(), actions);
        }
    }
    fields
}

pub fn permission_policy_field_allows(policy: &Value, field_key: &str, action: &str) -> bool {
    policy
        .get("fields")
        .and_then(Value::as_object)
        .and_then(|fields| fields.get(field_key))
        .and_then(Value::as_object)
        .and_then(|field_policy| field_policy.get(action))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

pub fn permission_policy_record_scope(policy: &Value) -> String {
    match policy.get("record_scope").and_then(Value::as_str).unwrap_or("all") {
        "owned" | "own" | "created_by_me" => "owned".to_string(),
        _ => "all".to_string(),
    }
}

pub fn filter_values_for_read_policy(mut values: Value, denied_read_fields: &BTreeSet<String>) -> Value {
    if denied_read_fields.is_empty() {
        return values;
    }
    if let Some(values) = values.as_object_mut() {
        for field_key in denied_read_fields {
            values.remove(field_key);
        }
    }
    values
}

pub async fn ensure_field_write_policy_allows(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    is_bot: bool,
    values: &Value,
) -> Result<(), ApiError> {
    if is_bot || role_is_form_admin(role) {
        return Ok(());
    }
    let Some(value_object) = values.as_object() else {
        return Ok(());
    };
    if value_object.is_empty() {
        return Ok(());
    }
    let Some(row) = permission_policy_for_role(state, form_id, role).await? else {
        return Ok(());
    };
    for field_key in value_object.keys() {
        if !permission_policy_field_allows(&row.policy, field_key, "write") {
            return Err(ApiError::Forbidden(format!(
                "form field write permission denied: {field_key}"
            )));
        }
    }
    Ok(())
}

pub async fn ensure_field_read_policy_allows(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    is_bot: bool,
    field_key: &str,
) -> Result<(), ApiError> {
    if is_bot || role_is_form_admin(role) {
        return Ok(());
    }
    if let Some(row) = permission_policy_for_role(state, form_id, role).await?
        && !permission_policy_field_allows(&row.policy, field_key, "read")
    {
        return Err(ApiError::Forbidden(format!(
            "form field read permission denied: {field_key}"
        )));
    }
    Ok(())
}

pub async fn denied_read_field_keys(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    is_bot: bool,
) -> Result<BTreeSet<String>, ApiError> {
    if is_bot || role_is_form_admin(role) {
        return Ok(BTreeSet::new());
    }
    let Some(row) = permission_policy_for_role(state, form_id, role).await? else {
        return Ok(BTreeSet::new());
    };
    Ok(permission_policy_fields(&row.policy)
        .into_iter()
        .filter_map(|(field_key, actions)| match actions.get("read") {
            Some(false) => Some(field_key),
            _ => None,
        })
        .collect())
}

pub async fn form_record_scope(state: &AppState, form_id: Uuid, role: &str, is_bot: bool) -> Result<String, ApiError> {
    if is_bot || role_is_form_admin(role) {
        return Ok("all".to_string());
    }
    Ok(permission_policy_for_role(state, form_id, role)
        .await?
        .map(|policy| permission_policy_record_scope(&policy.policy))
        .unwrap_or_else(|| "all".to_string()))
}

pub async fn append_record_scope_sql(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    actor_id: Uuid,
    is_bot: bool,
    where_parts: &mut Vec<String>,
    values: &mut Vec<sea_orm::Value>,
    next_idx: &mut usize,
) -> Result<(), ApiError> {
    if form_record_scope(state, form_id, role, is_bot).await? == "owned" {
        where_parts.push(format!("form_records.created_by = ${next_idx}"));
        values.push(actor_id.into());
        *next_idx += 1;
    }
    Ok(())
}

pub async fn ensure_record_scope_allows_created_by(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    actor_id: Uuid,
    is_bot: bool,
    created_by: Option<Uuid>,
) -> Result<(), ApiError> {
    if form_record_scope(state, form_id, role, is_bot).await? == "owned" && created_by != Some(actor_id) {
        return Err(ApiError::Forbidden(
            "form record ownership policy denied access".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_permission_policy(policy: Value) -> Result<Value, ApiError> {
    let object = policy
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("permission policy must be a JSON object".to_string()))?;
    if let Some(actions) = object.get("actions") {
        let actions = actions
            .as_object()
            .ok_or_else(|| ApiError::BadRequest("permission policy actions must be a JSON object".to_string()))?;
        for (action, value) in actions {
            validate_permission_action(action)?;
            if !value.is_boolean() {
                return Err(ApiError::BadRequest(
                    "permission policy action values must be booleans".to_string(),
                ));
            }
        }
    }
    if let Some(record_scope) = object.get("record_scope") {
        match record_scope.as_str() {
            Some("all" | "owned") => {}
            _ => {
                return Err(ApiError::BadRequest(
                    "permission policy record_scope must be all or owned".to_string(),
                ));
            }
        }
    }
    if let Some(fields) = object.get("fields") {
        let fields = fields
            .as_object()
            .ok_or_else(|| ApiError::BadRequest("permission policy fields must be a JSON object".to_string()))?;
        if fields.len() > 200 {
            return Err(ApiError::BadRequest(
                "permission policy fields cannot exceed 200 entries".to_string(),
            ));
        }
        for (field_key, field_policy) in fields {
            normalize_key(field_key).map_err(ApiError::BadRequest)?;
            let field_policy = field_policy.as_object().ok_or_else(|| {
                ApiError::BadRequest("permission policy field entries must be JSON objects".to_string())
            })?;
            for (action, allowed) in field_policy {
                validate_field_permission_action(action)?;
                if !allowed.is_boolean() {
                    return Err(ApiError::BadRequest(
                        "permission policy field action values must be booleans".to_string(),
                    ));
                }
            }
        }
    }
    Ok(policy)
}

fn effective_record_scope_from_policies(role: &str, is_bot: bool, policies: &[FormPermissionPolicyResponse]) -> String {
    if is_bot || role_is_form_admin(role) {
        return "all".to_string();
    }
    let Ok(subject_id) = normalize_permission_subject_id(role) else {
        return "owned".to_string();
    };
    policies
        .iter()
        .find(|policy| policy.subject_type == "role" && policy.subject_id == subject_id)
        .map(|policy| permission_policy_record_scope(&policy.policy))
        .unwrap_or_else(|| "all".to_string())
}

async fn permission_policy_for_role(
    state: &AppState,
    form_id: Uuid,
    role: &str,
) -> Result<Option<FormPermissionPolicyResponse>, ApiError> {
    let subject_id = normalize_permission_subject_id(role)?;
    FormPermissionPolicyResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, form_id, subject_type, subject_id,
                   policy, created_at, updated_at
            FROM form_permissions
            WHERE form_id = $1 AND subject_type = 'role' AND subject_id = $2
        ",
        vec![form_id.into(), subject_id.into()],
    ))
    .one(&state.db)
    .await
    .map_err(ApiError::from)
}

pub fn validate_permission_action(action: &str) -> Result<(), ApiError> {
    if FORM_PERMISSION_ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "unsupported form permission action: {action}"
        )))
    }
}

fn validate_field_permission_action(action: &str) -> Result<(), ApiError> {
    if FORM_FIELD_PERMISSION_ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "unsupported form field permission action: {action}"
        )))
    }
}

pub fn normalize_permission_subject_type(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    if value == "role" {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(
            "permission subject_type must be 'role'".to_string(),
        ))
    }
}

pub fn normalize_permission_subject_id(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "owner" | "admin" | "member" => Ok(value),
        _ => Err(ApiError::BadRequest(
            "permission subject_id must be one of owner, admin, member".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FormPermissionPolicyResponse, filter_values_for_read_policy, form_permissions_response,
        permission_policy_field_allows, permission_policy_fields, permission_policy_record_scope,
        validate_permission_policy,
    };
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn parses_field_level_permission_policy_defaults() {
        let policy = json!({
            "fields": {
                "unit_price": {"write": false, "read": true},
                "internal_note": {"write": true}
            }
        });

        assert!(!permission_policy_field_allows(&policy, "unit_price", "write"));
        assert!(permission_policy_field_allows(&policy, "unit_price", "read"));
        assert!(permission_policy_field_allows(&policy, "missing_field", "write"));
        assert!(permission_policy_field_allows(&policy, "missing_field", "read"));

        let fields = permission_policy_fields(&policy);
        assert_eq!(fields["unit_price"]["write"], false);
        assert_eq!(fields["unit_price"]["read"], true);
    }

    #[test]
    fn filters_unread_field_level_permission_values() {
        let mut denied = std::collections::BTreeSet::new();
        denied.insert("unit_price".to_string());

        let values = filter_values_for_read_policy(
            json!({
                "sku_name": "Beef Noodles",
                "unit_price": {"type": "amount", "decimal": "9.99", "currency": "CNY"}
            }),
            &denied,
        );

        assert_eq!(
            values.get("sku_name").and_then(serde_json::Value::as_str),
            Some("Beef Noodles")
        );
        assert!(values.get("unit_price").is_none());
    }

    #[test]
    fn validates_field_level_permission_policy_shape() {
        assert!(
            validate_permission_policy(json!({
                "fields": {"unit_price": {"read": false, "write": false}}
            }))
            .is_ok()
        );
        assert!(
            validate_permission_policy(json!({
                "fields": {"Unit Price": {"write": false}}
            }))
            .is_err()
        );
        assert!(
            validate_permission_policy(json!({
                "fields": {"unit_price": {"delete": false}}
            }))
            .is_err()
        );
        assert!(
            validate_permission_policy(json!({
                "fields": {"unit_price": {"write": "no"}}
            }))
            .is_err()
        );
    }

    #[test]
    fn parses_record_scope_aliases_for_runtime_policy() {
        assert_eq!(
            permission_policy_record_scope(&json!({"record_scope": "owned"})),
            "owned"
        );
        assert_eq!(permission_policy_record_scope(&json!({"record_scope": "own"})), "owned");
        assert_eq!(
            permission_policy_record_scope(&json!({"record_scope": "created_by_me"})),
            "owned"
        );
        assert_eq!(permission_policy_record_scope(&json!({"record_scope": "all"})), "all");
    }

    #[test]
    fn builds_effective_permissions_for_member_policy() {
        let form_id = Uuid::new_v4();
        let policy = FormPermissionPolicyResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            form_id,
            subject_type: "role".to_string(),
            subject_id: "member".to_string(),
            policy: json!({
                "actions": {"record.delete": false},
                "record_scope": "owned",
                "fields": {"unit_price": {"read": false, "write": false}}
            }),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let response = form_permissions_response(form_id, "member".to_string(), false, vec![policy]);

        assert_eq!(response.form_id, form_id);
        assert_eq!(response.effective.record_scope, "owned");
        assert_eq!(response.effective.actions["record.delete"], false);
        assert_eq!(response.effective.actions["record.update"], true);
        assert_eq!(response.effective.fields["unit_price"]["read"], false);
        assert_eq!(response.effective.fields["unit_price"]["write"], false);
    }
}
