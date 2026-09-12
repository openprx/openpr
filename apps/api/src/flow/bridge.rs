//! Flow <-> Forms bridge domain rules frozen by ADR-0019.
//!
//! This module deliberately does not call the Forms direct-access helpers: those helpers retain
//! Forms' historical "no policy row means allow" default, while BR-4 requires the bridge to treat
//! that same state as read-only and visibly unconfigured. The bridge computes one intersection
//! decision and every reference, embed and conversion path consumes it.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::Extensions;
use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::collab::authz::PermissionLevel;
use super::event_origin::CommandOrigin;
use super::{collab::authz, policy, repository};
use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};
use crate::forms::permissions::{permission_policy_field_allows, permission_policy_record_scope};

pub const BRIDGE_ACTIONS: [&str; 6] = [
    "form.view",
    "record.create",
    "record.update",
    "record.delete",
    "record.export",
    "form.design",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgePrincipal {
    WorkspaceRole,
    Guest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyConfiguration {
    Explicit,
    Unconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAccess {
    ReadOnly,
    Controlled,
}

/// Public permission shape. It says what the caller can do, never why another action was denied.
/// In particular it contains no target-existence bit, policy JSON, denied field names, record
/// owner identity or record-scope expression (ADR-0019 BR-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgePermissionState {
    pub access: BridgeAccess,
    pub configuration: PolicyConfiguration,
    pub actions: Vec<String>,
    pub field_read_limited: bool,
    pub field_write_limited: bool,
    pub record_limited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePermissionDecision {
    pub state: BridgePermissionState,
    denied_read_fields: BTreeSet<String>,
    denied_write_fields: BTreeSet<String>,
    record_scope: String,
}

impl BridgePermissionDecision {
    #[must_use]
    pub fn allows(&self, action: &str) -> bool {
        self.state.actions.iter().any(|allowed| allowed == action)
    }

    #[must_use]
    pub fn field_allows(&self, field_key: &str, action: &str) -> bool {
        match action {
            "read" => !self.denied_read_fields.contains(field_key),
            "write" => !self.denied_write_fields.contains(field_key),
            _ => false,
        }
    }

    #[must_use]
    pub fn record_in_scope(&self, actor_is_owner: bool) -> bool {
        self.record_scope != "owned" || actor_is_owner
    }
}

fn flow_action_ceiling(level: PermissionLevel) -> BTreeMap<&'static str, bool> {
    let mut actions = BTreeMap::new();
    for action in BRIDGE_ACTIONS {
        let allowed = match action {
            "form.view" => level >= PermissionLevel::View,
            "record.create" | "record.update" => level >= PermissionLevel::Edit,
            "record.delete" | "record.export" => level >= PermissionLevel::FullAccess,
            // BR-2: schema authority never crosses the bridge.
            "form.design" => false,
            _ => false,
        };
        actions.insert(action, allowed);
    }
    actions
}

/// Computes ADR-0019's Flow-grade x explicit-Forms-policy intersection.
///
/// `None` is the non-enumerating answer: callers must omit the reference/embed entirely. It is
/// returned for Flow `Denied`, guests, an explicit Forms denial of `form.view`, or an out-of-scope
/// record. A missing policy row is intentionally *not* passed to Forms' permissive direct-access
/// default; BR-4 narrows it to `form.view` and marks it `unconfigured`.
#[must_use]
pub fn bridge_permission(
    flow_level: PermissionLevel,
    principal: BridgePrincipal,
    forms_policy: Option<&Value>,
    record_owner_matches: Option<bool>,
) -> Option<BridgePermissionDecision> {
    if flow_level == PermissionLevel::Denied || principal == BridgePrincipal::Guest {
        return None;
    }

    let ceiling = flow_action_ceiling(flow_level);
    let (configuration, policy_actions, denied_read_fields, denied_write_fields, record_scope) = forms_policy
        .map_or_else(
            || {
                let actions = BTreeMap::from([
                    ("form.view", true),
                    ("record.create", false),
                    ("record.update", false),
                    ("record.delete", false),
                    ("record.export", false),
                    ("form.design", false),
                ]);
                (
                    PolicyConfiguration::Unconfigured,
                    actions,
                    BTreeSet::new(),
                    BTreeSet::new(),
                    "all".to_string(),
                )
            },
            |policy| {
                let actions = BRIDGE_ACTIONS
                    .into_iter()
                    .map(|action| {
                        let allowed = action != "form.design"
                            && policy
                                .get("actions")
                                .and_then(Value::as_object)
                                .and_then(|actions| actions.get(action))
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                        (action, allowed)
                    })
                    .collect::<BTreeMap<_, _>>();
                let field_keys = policy
                    .get("fields")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|fields| fields.keys())
                    .cloned()
                    .collect::<Vec<_>>();
                let denied_read = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "read"))
                    .cloned()
                    .collect();
                let denied_write = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "write"))
                    .cloned()
                    .collect();
                (
                    PolicyConfiguration::Explicit,
                    actions,
                    denied_read,
                    denied_write,
                    permission_policy_record_scope(policy),
                )
            },
        );

    if record_scope == "owned" && record_owner_matches == Some(false) {
        return None;
    }

    let actions = BRIDGE_ACTIONS
        .into_iter()
        .filter(|action| ceiling.get(action).copied().unwrap_or(false))
        .filter(|action| policy_actions.get(action).copied().unwrap_or(false))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !actions.iter().any(|action| action == "form.view") {
        return None;
    }
    let controlled = actions.iter().any(|action| action != "form.view");
    Some(BridgePermissionDecision {
        state: BridgePermissionState {
            access: if controlled {
                BridgeAccess::Controlled
            } else {
                BridgeAccess::ReadOnly
            },
            configuration,
            actions,
            field_read_limited: !denied_read_fields.is_empty(),
            field_write_limited: !denied_write_fields.is_empty(),
            record_limited: record_scope == "owned",
        },
        denied_read_fields,
        denied_write_fields,
        record_scope,
    })
}

#[derive(Debug, Deserialize)]
pub struct BridgeDisplay {
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateReferenceInput {
    pub target_type: String,
    pub target_id: Uuid,
    #[serde(default)]
    pub display: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
pub struct ReferenceReceipt {
    pub reference_id: Uuid,
    pub source_object_id: Uuid,
    pub target_type: String,
    pub target_id: Uuid,
    pub lineage_id: Option<Uuid>,
    pub permission_state: BridgePermissionState,
}

#[derive(Debug, Serialize)]
pub struct RemoveReferenceReceipt {
    pub removed: bool,
    pub event_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(tag = "visibility", rename_all = "snake_case")]
pub enum BridgeReferenceView {
    Available {
        reference_id: Uuid,
        source_object_id: Uuid,
        target_type: String,
        target_id: Uuid,
        title: String,
        status: String,
        summary: Value,
        display: Value,
        permission_state: BridgePermissionState,
    },
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ReferenceList {
    pub items: Vec<BridgeReferenceView>,
}

#[derive(Debug, FromQueryResult)]
struct ReferenceRow {
    id: Uuid,
    workspace_id: Uuid,
    source_object_id: Uuid,
    target_type: String,
    target_id: Uuid,
    display: Value,
    removed_at: Option<DateTime<Utc>>,
}

struct BridgeActor {
    id: Uuid,
    role: String,
    is_bot: bool,
    flow_level: PermissionLevel,
}

struct TargetView {
    form_id: Uuid,
    title: String,
    status: String,
    summary: Value,
    created_by: Option<Uuid>,
}

async fn bridge_enabled<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<bool, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        bridge_enabled: bool,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT bridge_enabled FROM flow_workspace_settings WHERE workspace_id = $1 AND flow_enabled = true",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?
    .is_some_and(|row| row.bridge_enabled))
}

async fn source_actor(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    minimum: PermissionLevel,
) -> Result<(repository::ObjectViewRow, BridgeActor), ApiError> {
    let source = repository::fetch_object_view(&state.db, source_object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let (actor_id, role, is_bot) = policy::resolve_flow_principal(state, extensions, source.workspace_id)
        .await
        .map_err(|_| ApiError::NotFound("flow object not found".to_string()))?;
    let principal_kind = if is_bot { "bot" } else { "user" };
    let flow_level = authz::effective_permission(
        &state.db,
        source.workspace_id,
        source.id,
        principal_kind,
        actor_id,
        &role,
    )
    .await
    .map_err(|_| ApiError::NotFound("flow object not found".to_string()))?;
    if flow_level < minimum {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }
    Ok((
        source,
        BridgeActor {
            id: actor_id,
            role,
            is_bot,
            flow_level,
        },
    ))
}

async fn target_view<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    target_type: &str,
    target_id: Uuid,
) -> Result<Option<TargetView>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        form_id: Uuid,
        title: String,
        summary: Value,
        created_by: Option<Uuid>,
    }
    let row = match target_type {
        "form" => {
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id AS form_id, name AS title, \
             jsonb_build_object('description', description) AS summary, created_by \
             FROM project_forms WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL",
                vec![target_id.into(), workspace_id.into()],
            ))
            .one(conn)
            .await?
        }
        "form_record" => {
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT r.form_id, r.title, r.values AS summary, r.created_by \
             FROM form_records r INNER JOIN project_forms f ON f.id = r.form_id \
             WHERE r.id = $1 AND r.workspace_id = $2 AND r.archived_at IS NULL AND f.archived_at IS NULL",
                vec![target_id.into(), workspace_id.into()],
            ))
            .one(conn)
            .await?
        }
        _ => {
            return Err(ApiError::BadRequest(
                "target_type must be form or form_record".to_string(),
            ));
        }
    };
    Ok(row.map(|row| TargetView {
        form_id: row.form_id,
        title: row.title,
        status: "active".to_string(),
        summary: row.summary,
        created_by: row.created_by,
    }))
}

async fn target_permission<C: ConnectionTrait>(
    conn: &C,
    actor: &BridgeActor,
    target: &TargetView,
) -> Result<Option<BridgePermissionDecision>, ApiError> {
    if actor.role == "__flow_guest" {
        return Ok(None);
    }
    #[derive(FromQueryResult)]
    struct Row {
        policy: Value,
    }
    let policy = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT policy FROM form_permissions \
         WHERE form_id = $1 AND subject_type = 'role' AND subject_id = $2",
        vec![target.form_id.into(), actor.role.clone().into()],
    ))
    .one(conn)
    .await?;
    Ok(bridge_permission(
        actor.flow_level,
        BridgePrincipal::WorkspaceRole,
        policy.as_ref().map(|row| &row.policy),
        Some(target.created_by == Some(actor.id)),
    ))
}

async fn lock_and_reauthorize_reference(
    tx: &sea_orm::DatabaseTransaction,
    source: &repository::ObjectViewRow,
    actor: &mut BridgeActor,
    target_type: &str,
    target_id: Uuid,
) -> Result<(TargetView, BridgePermissionDecision), ApiError> {
    #[derive(FromQueryResult)]
    struct SettingsRow {
        bridge_enabled: bool,
    }
    let settings = SettingsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT bridge_enabled FROM flow_workspace_settings \
         WHERE workspace_id = $1 AND flow_enabled = true FOR SHARE",
        vec![source.workspace_id.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ApiError::policy_rejected("forms bridge is disabled"))?;
    if !settings.bridge_enabled {
        return Err(ApiError::policy_rejected("forms bridge is disabled"));
    }

    if !actor.is_bot {
        #[derive(FromQueryResult)]
        struct RoleRow {
            role: String,
        }
        actor.role = RoleRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![source.workspace_id.into(), actor.id.into()],
        ))
        .one(tx)
        .await?
        .map_or_else(|| "__flow_guest".to_string(), |row| row.role);
    }
    actor.flow_level = authz::effective_permission(
        tx,
        source.workspace_id,
        source.id,
        if actor.is_bot { "bot" } else { "user" },
        actor.id,
        &actor.role,
    )
    .await?;
    if actor.flow_level < PermissionLevel::Edit {
        return Err(ApiError::policy_rejected("bridge permission changed before commit"));
    }

    let form_id = match target_type {
        "form" => target_id,
        "form_record" => {
            #[derive(FromQueryResult)]
            struct FormRow {
                form_id: Uuid,
            }
            FormRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT form_id FROM form_records WHERE id = $1 AND workspace_id = $2",
                vec![target_id.into(), source.workspace_id.into()],
            ))
            .one(tx)
            .await?
            .ok_or_else(|| ApiError::NotFound("bridge target not found".to_string()))?
            .form_id
        }
        _ => {
            return Err(ApiError::BadRequest(
                "target_type must be form or form_record".to_string(),
            ));
        }
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 19019))",
        vec![form_id.to_string().into()],
    ))
    .await?;
    let target = target_view(tx, source.workspace_id, target_type, target_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("bridge target not found".to_string()))?;
    let permission = target_permission(tx, actor, &target)
        .await?
        .ok_or_else(|| ApiError::policy_rejected("bridge permission changed before commit"))?;
    Ok((target, permission))
}

fn read_only_state(mut state: BridgePermissionState) -> BridgePermissionState {
    state.actions.retain(|action| action == "form.view");
    state.access = BridgeAccess::ReadOnly;
    state
}

pub async fn create_reference(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    input: CreateReferenceInput,
    origin: CommandOrigin,
) -> Result<ReferenceReceipt, ApiError> {
    if input.idempotency_key.trim().is_empty() {
        return Err(ApiError::BadRequest("idempotency_key is required".to_string()));
    }
    if !input.display.is_object() {
        return Err(ApiError::BadRequest("display must be an object".to_string()));
    }
    let (source, mut actor) = source_actor(state, extensions, source_object_id, PermissionLevel::Edit).await?;
    if !bridge_enabled(&state.db, source.workspace_id).await? {
        return Err(ApiError::policy_rejected("forms bridge is disabled"));
    }
    let tx = state.db.begin().await?;
    let (_target, permission) =
        lock_and_reauthorize_reference(&tx, &source, &mut actor, &input.target_type, input.target_id).await?;
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let proposed_id = Uuid::new_v4();
    let inserted = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_bridge_references \
         (id, workspace_id, source_object_id, target_type, target_id, display, idempotency_key, created_by, created_by_kind) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
         ON CONFLICT (workspace_id, idempotency_key) DO NOTHING RETURNING id",
        vec![
            proposed_id.into(),
            source.workspace_id.into(),
            source.id.into(),
            input.target_type.clone().into(),
            input.target_id.into(),
            input.display.clone().into(),
            input.idempotency_key.clone().into(),
            actor.id.into(),
            (if actor.is_bot { "bot" } else { "user" }).into(),
        ],
    ))
    .one(&tx)
    .await?;
    let (reference_id, was_new) = if let Some(row) = inserted {
        (row.id, true)
    } else {
        let existing = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
             FROM flow_bridge_references WHERE workspace_id = $1 AND idempotency_key = $2",
            vec![source.workspace_id.into(), input.idempotency_key.clone().into()],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;
        if existing.source_object_id != source.id
            || existing.target_type != input.target_type
            || existing.target_id != input.target_id
            || existing.display != input.display
            || existing.removed_at.is_some()
        {
            return Err(ApiError::policy_rejected(
                "idempotency key was used for another reference",
            ));
        }
        (existing.id, false)
    };

    if was_new {
        insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: source.workspace_id,
                project_id: source.project_id,
                event_type: "flow.reference.created".to_string(),
                aggregate_type: "flow_reference".to_string(),
                aggregate_id: reference_id.to_string(),
                actor_id: (!actor.is_bot).then_some(actor.id),
                source: origin.source_json(),
                payload: json!({
                    "reference_id": reference_id,
                    "source_object_id": source.id,
                    "target_type": input.target_type,
                    "target_id": input.target_id,
                    "lineage_id": Value::Null,
                }),
                metadata: json!({}),
                correlation_id: Some(origin.correlation_id),
                causation_id: origin.causation_id,
                idempotency_key: Some(format!("bridge-reference:{}", input.idempotency_key)),
            },
            Some(FlowDispatchSpec {
                max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?;
    }
    tx.commit().await?;

    Ok(ReferenceReceipt {
        reference_id,
        source_object_id: source.id,
        target_type: input.target_type,
        target_id: input.target_id,
        lineage_id: None,
        permission_state: permission.state,
    })
}

pub async fn list_references(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
) -> Result<ReferenceList, ApiError> {
    let (source, actor) = source_actor(state, extensions, source_object_id, PermissionLevel::View).await?;
    let enabled = bridge_enabled(&state.db, source.workspace_id).await?;
    let rows = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
         FROM flow_bridge_references WHERE workspace_id = $1 AND source_object_id = $2 AND removed_at IS NULL \
         ORDER BY created_at, id",
        vec![source.workspace_id.into(), source.id.into()],
    ))
    .all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(target) = target_view(&state.db, row.workspace_id, &row.target_type, row.target_id).await? else {
            items.push(BridgeReferenceView::Unavailable);
            continue;
        };
        let Some(permission) = target_permission(&state.db, &actor, &target).await? else {
            items.push(BridgeReferenceView::Unavailable);
            continue;
        };
        let summary = if row.target_type == "form_record" {
            permission
                .denied_read_fields
                .iter()
                .fold(target.summary, |mut values, key| {
                    if let Some(object) = values.as_object_mut() {
                        object.remove(key);
                    }
                    values
                })
        } else {
            target.summary
        };
        let permission_state = if enabled {
            permission.state
        } else {
            read_only_state(permission.state)
        };
        items.push(BridgeReferenceView::Available {
            reference_id: row.id,
            source_object_id: row.source_object_id,
            target_type: row.target_type,
            target_id: row.target_id,
            title: target.title,
            status: target.status,
            summary,
            display: row.display,
            permission_state,
        });
    }
    Ok(ReferenceList { items })
}

pub async fn remove_reference(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    reference_id: Uuid,
    idempotency_key: String,
    origin: CommandOrigin,
) -> Result<RemoveReferenceReceipt, ApiError> {
    if idempotency_key.trim().is_empty() {
        return Err(ApiError::BadRequest("Idempotency-Key is required".to_string()));
    }
    let (source, actor) = source_actor(state, extensions, source_object_id, PermissionLevel::Edit).await?;
    let tx = state.db.begin().await?;
    let reference = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
         FROM flow_bridge_references WHERE id = $1 AND workspace_id = $2 AND source_object_id = $3 FOR UPDATE",
        vec![reference_id.into(), source.workspace_id.into(), source.id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("reference not found".to_string()))?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_bridge_references SET removed_at = COALESCE(removed_at, now()) WHERE id = $1",
        vec![reference_id.into()],
    ))
    .await?;
    let outcome = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            event_type: "flow.reference.removed".to_string(),
            aggregate_type: "flow_reference".to_string(),
            aggregate_id: reference_id.to_string(),
            actor_id: (!actor.is_bot).then_some(actor.id),
            source: origin.source_json(),
            payload: json!({
                "reference_id": reference_id,
                "source_object_id": source.id,
                "target_type": reference.target_type,
            }),
            metadata: json!({}),
            correlation_id: Some(origin.correlation_id),
            causation_id: origin.causation_id,
            idempotency_key: Some(format!("bridge-unreference:{idempotency_key}")),
        },
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(RemoveReferenceReceipt {
        removed: true,
        event_id: outcome.event_id,
    })
}

#[cfg(test)]
mod tests {
    use super::{BridgeAccess, BridgePrincipal, PolicyConfiguration, bridge_permission};
    use crate::flow::collab::authz::PermissionLevel;
    use serde_json::json;

    #[test]
    fn bridge_permission_is_the_intersection_in_both_directions() {
        let forms_denies_update = json!({"actions": {"form.view": true, "record.update": false}});
        let full = bridge_permission(
            PermissionLevel::FullAccess,
            BridgePrincipal::WorkspaceRole,
            Some(&forms_denies_update),
            None,
        )
        .expect("view remains visible");
        assert!(!full.allows("record.update"), "Forms denial must beat Flow FullAccess");

        let forms_allows_delete = json!({"actions": {"form.view": true, "record.delete": true}});
        let view = bridge_permission(
            PermissionLevel::View,
            BridgePrincipal::WorkspaceRole,
            Some(&forms_allows_delete),
            None,
        )
        .expect("view remains visible");
        assert!(
            !view.allows("record.delete"),
            "Flow View must beat a Forms delete allowance"
        );
    }

    #[test]
    fn bridge_never_grants_form_design() {
        let policy = json!({"actions": {"form.view": true, "form.design": true}});
        let decision = bridge_permission(
            PermissionLevel::FullAccess,
            BridgePrincipal::WorkspaceRole,
            Some(&policy),
            None,
        )
        .expect("view remains visible");
        assert!(!decision.allows("form.design"));
    }

    #[test]
    fn unconfigured_forms_are_read_only_and_honestly_labelled() {
        let decision = bridge_permission(PermissionLevel::FullAccess, BridgePrincipal::WorkspaceRole, None, None)
            .expect("the BR-4 read-only floor remains visible");
        assert_eq!(decision.state.configuration, PolicyConfiguration::Unconfigured);
        assert_eq!(decision.state.access, BridgeAccess::ReadOnly);
        assert_eq!(decision.state.actions, ["form.view"]);
        assert!(!decision.allows("record.update"));
    }

    #[test]
    fn guests_and_flow_denied_principals_get_no_reference_shape() {
        let permissive = json!({"actions": {"form.view": true, "record.update": true}});
        assert!(
            bridge_permission(
                PermissionLevel::FullAccess,
                BridgePrincipal::Guest,
                Some(&permissive),
                None
            )
            .is_none()
        );
        assert!(
            bridge_permission(
                PermissionLevel::Denied,
                BridgePrincipal::WorkspaceRole,
                Some(&permissive),
                None
            )
            .is_none()
        );
    }

    #[test]
    fn explicit_field_and_record_restrictions_survive_the_intersection() {
        let policy = json!({
            "actions": {"form.view": true, "record.update": true},
            "fields": {"private": {"read": false, "write": false}},
            "record_scope": "owned"
        });
        assert!(
            bridge_permission(
                PermissionLevel::Edit,
                BridgePrincipal::WorkspaceRole,
                Some(&policy),
                Some(false)
            )
            .is_none(),
            "an out-of-scope record must not produce a placeholder"
        );
        let decision = bridge_permission(
            PermissionLevel::Edit,
            BridgePrincipal::WorkspaceRole,
            Some(&policy),
            Some(true),
        )
        .expect("the owned record is visible");
        assert!(!decision.field_allows("private", "read"));
        assert!(!decision.field_allows("private", "write"));
        assert!(decision.state.field_read_limited);
        assert!(decision.state.field_write_limited);
        assert!(decision.state.record_limited);
    }
}
