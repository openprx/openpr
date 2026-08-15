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

/// Whether `role` is exempt from field-level policies and record scoping.
///
/// Bots used to be exempt too, on the strength of being bots: `is_bot ||` short-circuited every
/// check in this module, so a token could read fields marked `read:false`, write fields marked
/// `write:false`, and see every record of a form scoped to `owned`. A bot now resolves to a
/// workspace role like anybody else — `admin` when its token carries the `admin` permission,
/// `member` otherwise — and is judged by that role alone.
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
    let record_scope = effective_record_scope_from_policies(&role, &policies);
    let fields = effective_field_permissions_from_policies(&role, &policies);
    FormPermissionsResponse {
        form_id,
        policies,
        effective: FormEffectivePermissionResponse {
            role,
            is_bot,
            actions,
            record_scope,
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
        .is_none_or(|policy| permission_policy_allows(&policy.policy, action))
}

fn effective_field_permissions_from_policies(
    role: &str,
    policies: &[FormPermissionPolicyResponse],
) -> BTreeMap<String, BTreeMap<String, bool>> {
    if role_is_form_admin(role) {
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
    values: &Value,
) -> Result<(), ApiError> {
    if role_is_form_admin(role) {
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
    field_key: &str,
) -> Result<(), ApiError> {
    if role_is_form_admin(role) {
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

pub async fn denied_read_field_keys(state: &AppState, form_id: Uuid, role: &str) -> Result<BTreeSet<String>, ApiError> {
    if role_is_form_admin(role) {
        return Ok(BTreeSet::new());
    }
    let Some(row) = permission_policy_for_role(state, form_id, role).await? else {
        return Ok(BTreeSet::new());
    };
    Ok(denied_read_fields_from_policy(&row.policy))
}

/// Field keys the policy marks `read:false`.
pub fn denied_read_fields_from_policy(policy: &Value) -> BTreeSet<String> {
    permission_policy_fields(policy)
        .into_iter()
        .filter_map(|(field_key, actions)| match actions.get("read") {
            Some(false) => Some(field_key),
            _ => None,
        })
        .collect()
}

pub async fn form_record_scope(state: &AppState, form_id: Uuid, role: &str) -> Result<String, ApiError> {
    if role_is_form_admin(role) {
        return Ok("all".to_string());
    }
    Ok(permission_policy_for_role(state, form_id, role).await?.map_or_else(
        || "all".to_string(),
        |policy| permission_policy_record_scope(&policy.policy),
    ))
}

pub async fn append_record_scope_sql(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    actor_id: Uuid,
    where_parts: &mut Vec<String>,
    values: &mut Vec<sea_orm::Value>,
    next_idx: &mut usize,
) -> Result<(), ApiError> {
    if form_record_scope(state, form_id, role).await? == "owned" {
        where_parts.push(format!("form_records.created_by = ${next_idx}"));
        values.push(actor_id.into());
        *next_idx += 1;
    }
    Ok(())
}

/// Builds a row-level ownership predicate for joined records.
///
/// Used by queries whose per-row form is only known at runtime (child records, relation targets).
/// The predicate keeps a row when the actor created it, or when the row's form does not restrict the
/// actor's role to `record_scope = owned`.
///
/// `record_alias` is a caller-controlled SQL alias and is validated before interpolation; every
/// value that can originate from a request is bound as a placeholder.
pub fn role_record_scope_predicate_sql(
    record_alias: &str,
    actor_idx: usize,
    role_idx: usize,
) -> Result<String, ApiError> {
    validate_sql_identifier(record_alias)?;
    Ok(format!(
        "({record_alias}.created_by = ${actor_idx} OR NOT EXISTS (\
             SELECT 1 FROM form_permissions scope_policy \
             WHERE scope_policy.form_id = {record_alias}.form_id \
               AND scope_policy.subject_type = 'role' \
               AND scope_policy.subject_id = ${role_idx} \
               AND scope_policy.policy->>'record_scope' IN ('owned', 'own', 'created_by_me')))"
    ))
}

fn validate_sql_identifier(value: &str) -> Result<(), ApiError> {
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_');
    let valid_rest = chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid_start && valid_rest && value.len() <= 63 {
        Ok(())
    } else {
        Err(ApiError::BadRequest("invalid SQL identifier".to_string()))
    }
}

pub async fn ensure_record_scope_allows_created_by(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    actor_id: Uuid,
    created_by: Option<Uuid>,
) -> Result<(), ApiError> {
    if form_record_scope(state, form_id, role).await? == "owned" && created_by != Some(actor_id) {
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

fn effective_record_scope_from_policies(role: &str, policies: &[FormPermissionPolicyResponse]) -> String {
    if role_is_form_admin(role) {
        return "all".to_string();
    }
    let Ok(subject_id) = normalize_permission_subject_id(role) else {
        return "owned".to_string();
    };
    policies
        .iter()
        .find(|policy| policy.subject_type == "role" && policy.subject_id == subject_id)
        .map_or_else(
            || "all".to_string(),
            |policy| permission_policy_record_scope(&policy.policy),
        )
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
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        FormPermissionPolicyResponse, denied_read_fields_from_policy, filter_values_for_read_policy,
        form_permissions_response, permission_policy_field_allows, permission_policy_fields,
        permission_policy_record_scope, role_record_scope_predicate_sql, validate_permission_policy,
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
        assert!(!fields["unit_price"]["write"]);
        assert!(fields["unit_price"]["read"]);
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
    fn builds_row_level_scope_predicate_from_bound_parameters() {
        let predicate = role_record_scope_predicate_sql("child", 3, 4).expect("alias is a valid identifier");

        assert!(predicate.contains("child.created_by = $3"));
        assert!(predicate.contains("scope_policy.form_id = child.form_id"));
        assert!(predicate.contains("scope_policy.subject_id = $4"));
        assert!(predicate.contains("record_scope"));
        assert!(!predicate.contains("$5"));
    }

    #[test]
    fn rejects_row_level_scope_predicate_for_untrusted_aliases() {
        assert!(role_record_scope_predicate_sql("child; DROP TABLE form_records --", 1, 2).is_err());
        assert!(role_record_scope_predicate_sql("", 1, 2).is_err());
        assert!(role_record_scope_predicate_sql("Child", 1, 2).is_err());
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
        assert!(!response.effective.actions["record.delete"]);
        assert!(response.effective.actions["record.update"]);
        assert!(!response.effective.fields["unit_price"]["read"]);
        assert!(!response.effective.fields["unit_price"]["write"]);
    }

    fn member_policy(form_id: Uuid) -> FormPermissionPolicyResponse {
        FormPermissionPolicyResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            form_id,
            subject_type: "role".to_string(),
            subject_id: "member".to_string(),
            policy: json!({
                "record_scope": "owned",
                "fields": {"salary": {"read": false, "write": false}}
            }),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn bot_member_is_bound_by_the_same_field_and_record_policy_as_a_human_member() {
        let form_id = Uuid::new_v4();
        let policies = vec![member_policy(form_id)];

        let bot = form_permissions_response(form_id, "member".to_string(), true, policies.clone());
        let human = form_permissions_response(form_id, "member".to_string(), false, policies);

        assert!(bot.effective.is_bot);
        assert_eq!(bot.effective.record_scope, "owned");
        assert!(!bot.effective.fields["salary"]["read"]);
        assert!(!bot.effective.fields["salary"]["write"]);
        assert_eq!(bot.effective.record_scope, human.effective.record_scope);
        assert_eq!(bot.effective.fields, human.effective.fields);
    }

    #[test]
    fn bot_admin_keeps_the_workspace_admin_exemption_and_nothing_more() {
        let form_id = Uuid::new_v4();
        let policies = vec![member_policy(form_id)];

        let bot_admin = form_permissions_response(form_id, "admin".to_string(), true, policies);

        assert_eq!(bot_admin.effective.record_scope, "all");
        assert!(bot_admin.effective.fields.is_empty());
    }

    #[test]
    fn bot_reads_are_stripped_of_fields_the_policy_denies() {
        let policy = member_policy(Uuid::new_v4()).policy;
        let denied = denied_read_fields_from_policy(&policy);

        assert!(denied.contains("salary"));

        let values = filter_values_for_read_policy(json!({"name": "Ada", "salary": "9000"}), &denied);

        assert_eq!(values.get("name").and_then(serde_json::Value::as_str), Some("Ada"));
        assert!(values.get("salary").is_none());
    }
}
