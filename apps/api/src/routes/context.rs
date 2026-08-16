use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::{error::ApiError, middleware::bot_auth::BotAuthContext, response::ApiResponse};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextProject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub type_key: String,
    pub type_settings: Value,
    pub workflow_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextType {
    pub key: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub enabled_capabilities: Value,
    pub field_schema: Value,
    pub artifact_schema: Value,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextResource {
    pub id: Uuid,
    pub project_id: Uuid,
    pub kind: String,
    pub name: String,
    pub locator: Value,
    pub permission_policy: Value,
    pub sync_status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextGovernance {
    pub project_id: Uuid,
    pub review_required: bool,
    pub auto_review_days: i32,
    pub review_reminder_days: i32,
    pub audit_report_cron: String,
    pub trust_update_mode: String,
    pub config: Value,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextWorkflow {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub is_system_default: bool,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextWorkflowState {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub key: String,
    pub display_name: String,
    pub category: String,
    pub position: i32,
    pub color: Option<String>,
    pub is_initial: bool,
    pub is_terminal: bool,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectContextDecision {
    pub id: String,
    pub proposal_id: String,
    pub result: String,
    pub approval_rate: Option<f64>,
    pub total_votes: i32,
    pub decided_at: chrono::DateTime<chrono::Utc>,
}

pub async fn get_project_context(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let project = load_project(&state, project_id).await?;
    ensure_project_context_access(&state, &claims, bot.as_ref().map(|b| &b.0), &project).await?;
    let project_type = load_project_type(&state, &project.type_key).await?;
    let resources = load_resources(&state, project_id).await?;
    let governance = load_governance(&state, project_id).await?;
    let workflow = load_workflow(&state, project_id).await?;
    let decisions = load_recent_decisions(&state).await?;
    let agent_policy = build_agent_policy(&project, project_type.as_ref(), governance.as_ref());

    Ok(ApiResponse::success(json!({
        "project": project,
        "project_type": project_type,
        "resources": resources,
        "governance": governance,
        "workflow": workflow,
        "recent_decisions": decisions,
        "agent_policy": agent_policy,
    })))
}

pub async fn get_project_governance_context(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let project = load_project(&state, project_id).await?;
    ensure_project_context_access(&state, &claims, bot.as_ref().map(|b| &b.0), &project).await?;
    let governance = load_governance(&state, project_id).await?;
    let workflow = load_workflow(&state, project_id).await?;
    let decisions = load_recent_decisions(&state).await?;

    Ok(ApiResponse::success(json!({
        "project_id": project_id,
        "governance": governance,
        "workflow": workflow,
        "recent_decisions": decisions,
    })))
}

pub async fn get_project_agent_policy(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let project = load_project(&state, project_id).await?;
    ensure_project_context_access(&state, &claims, bot.as_ref().map(|b| &b.0), &project).await?;
    let project_type = load_project_type(&state, &project.type_key).await?;
    let governance = load_governance(&state, project_id).await?;
    let policy = build_agent_policy(&project, project_type.as_ref(), governance.as_ref());

    Ok(ApiResponse::success(policy))
}

async fn load_project(state: &AppState, project_id: Uuid) -> Result<ProjectContextProject, ApiError> {
    ProjectContextProject::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, key, name, COALESCE(description, '') AS description,
                   COALESCE(type_key, 'code_project') AS type_key,
                   COALESCE(type_settings, '{}'::jsonb) AS type_settings,
                   workflow_id, created_at, updated_at
            FROM projects
            WHERE id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

async fn load_project_type(state: &AppState, type_key: &str) -> Result<Option<ProjectContextType>, ApiError> {
    ProjectContextType::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT key, name, description, domain, enabled_capabilities,
                   field_schema, artifact_schema
            FROM project_types
            WHERE key = $1
        ",
        vec![type_key.to_string().into()],
    ))
    .one(&state.db)
    .await
    .map_err(ApiError::from)
}

async fn load_resources(state: &AppState, project_id: Uuid) -> Result<Vec<ProjectContextResource>, ApiError> {
    ProjectContextResource::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, project_id, kind, name, locator, permission_policy,
                   sync_status, created_at, updated_at
            FROM project_resources
            WHERE project_id = $1
            ORDER BY created_at DESC
        ",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

async fn load_governance(state: &AppState, project_id: Uuid) -> Result<Option<ProjectContextGovernance>, ApiError> {
    ProjectContextGovernance::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT project_id, review_required, auto_review_days, review_reminder_days,
                   audit_report_cron, trust_update_mode, config, updated_at
            FROM governance_configs
            WHERE project_id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await
    .map_err(ApiError::from)
}

async fn load_workflow(state: &AppState, project_id: Uuid) -> Result<Value, ApiError> {
    let workflow = ProjectContextWorkflow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT w.id, w.workspace_id, w.name, COALESCE(w.description, '') AS description,
                   w.is_system_default
            FROM projects p
            INNER JOIN workflows w
                ON w.id = COALESCE(p.workflow_id, (SELECT id FROM workflows WHERE is_system_default = true ORDER BY created_at LIMIT 1))
            WHERE p.id = $1
            LIMIT 1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?;

    let Some(workflow) = workflow else {
        return Ok(json!(null));
    };

    let states = ProjectContextWorkflowState::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workflow_id, key, display_name, category, position,
                   color, is_initial, is_terminal
            FROM workflow_states
            WHERE workflow_id = $1
            ORDER BY position ASC
        ",
        vec![workflow.id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(json!({
        "workflow": workflow,
        "states": states,
    }))
}

async fn load_recent_decisions(state: &AppState) -> Result<Vec<ProjectContextDecision>, ApiError> {
    ProjectContextDecision::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, proposal_id, result::text AS result, approval_rate, total_votes, decided_at
            FROM decisions
            ORDER BY decided_at DESC
            LIMIT 10
        ",
        vec![],
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

async fn ensure_project_context_access(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    project: &ProjectContextProject,
) -> Result<(), ApiError> {
    if let Some(bot) = bot {
        if bot.workspace_id != project.workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this project".to_string()));
        }
        return Ok(());
    }

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let has_access = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![project.workspace_id.into(), user_id.into()],
        ))
        .await?
        .is_some();
    if !has_access {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}

fn build_agent_policy(
    project: &ProjectContextProject,
    project_type: Option<&ProjectContextType>,
    governance: Option<&ProjectContextGovernance>,
) -> Value {
    let capabilities = project_type.map_or_else(|| json!([]), |item| item.enabled_capabilities.clone());
    let high_risk_requires_review = governance.is_none_or(|item| item.review_required);

    json!({
        "project_id": project.id,
        "project_type": project.type_key,
        "capabilities": capabilities,
        "action_classes": {
            "read_only": { "mode": "direct" },
            "comment_result": { "mode": "direct" },
            "low_risk_mutation": { "mode": "direct_with_audit" },
            "high_risk_mutation": {
                "mode": if high_risk_requires_review { "proposal_or_check_result" } else { "direct_with_audit" }
            },
            "external_side_effect": { "mode": "explicit_api_policy_required" },
            "financial_legal_compliance": { "mode": "proposal_vote_required" }
        },
        "mcp": {
            "workspace_scope_required": true,
            "project_context_required": true,
            "tool_registry": build_mcp_tool_registry(&capabilities)
        }
    })
}

fn build_mcp_tool_registry(capabilities: &Value) -> Value {
    let capability_set = capability_set(capabilities);
    let mut groups: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

    groups.insert(
        "core",
        tool_set(&[
            "projects.list",
            "projects.get",
            "project_types.list",
            "project_types.get",
            "context.get_project",
            "context.get_governance",
            "context.get_agent_policy",
            "release.readiness.get",
            "project_resources.list",
            "members.list",
            "search.all",
        ]),
    );

    if capability_set.contains("issues") || capability_set.contains("board") {
        groups.insert(
            "work_items",
            tool_set(&[
                "work_items.list",
                "work_items.get",
                "work_items.get_by_identifier",
                "work_items.create",
                "work_items.update",
                "work_items.delete",
                "work_items.search",
                "work_items.add_label",
                "work_items.add_labels",
                "work_items.remove_label",
                "work_items.list_labels",
                "comments.list",
                "comments.create",
                "comments.delete",
                "labels.list",
                "labels.list_by_project",
                "labels.create",
                "labels.update",
                "labels.delete",
                "files.upload",
            ]),
        );
    }

    if capability_set.contains("sprints") || capability_set.contains("milestones") {
        groups.insert(
            "planning",
            tool_set(&["sprints.list", "sprints.create", "sprints.update", "sprints.delete"]),
        );
    }

    if capability_set.contains("governance")
        || capability_set.contains("approval")
        || capability_set.contains("corrective_action")
        || capability_set.contains("inspection")
    {
        groups.insert(
            "governance",
            tool_set(&[
                "proposals.list",
                "proposals.get",
                "proposals.create",
                "proposals.create_from_result",
                "check_results.create",
                "release.readiness.get",
                "context.get_governance",
                "context.get_agent_policy",
            ]),
        );
    }

    if capability_set.contains("code_context") {
        groups.insert(
            "code",
            tool_set(&[
                "code.resources.list",
                "code.directory.get",
                "code.task_context.get",
                "code.change_proposal.create",
            ]),
        );
    }

    if capability_set.contains("documents") || capability_set.contains("approval") {
        groups.insert(
            "documents",
            tool_set(&["documents.extract_summary", "documents.review_risk", "approval.request"]),
        );
    }

    if capability_set.contains("inspection") || capability_set.contains("corrective_action") {
        groups.insert(
            "operations",
            tool_set(&["inspection.report", "corrective_action.propose"]),
        );
    }

    if capability_set.contains("mcp") {
        groups.insert(
            "mcp_execution",
            tool_set(&[
                "context.get_project",
                "context.get_governance",
                "context.get_agent_policy",
                "check_results.create",
                "release.readiness.get",
            ]),
        );
    }

    if capability_set.contains("forms") {
        groups.insert(
            "forms",
            tool_set(&[
                "forms.list",
                "forms.get",
                "forms.create",
                "forms.create_from_template",
                "scenario_templates.install",
                "forms.update_schema",
                "forms.duplicate",
                "forms.schema_summary",
                "forms.field_usage",
                "forms.field_dependencies",
                "form_schema_versions.list",
                "form_schema_versions.get",
                "form_permissions.get",
                "form_permissions.update",
                "form_views.list",
                "form_attachments.list",
                "form_attachments.create",
                "form_attachments.archive",
                "form_attachments.restore",
                "form_records.list",
                "form_records.export",
                "form_records.import_preview",
                "form_records.import_commit",
                "form_records.get",
                "form_records.create",
                "form_records.update",
                "form_records.link",
                "form_records.relation_targets",
                "form_records.children",
                "form_records.child_create",
                "form_records.child_update",
                "form_records.child_archive",
                "form_records.child_restore",
                "form_records.aggregate",
                "events.tail",
            ]),
        );
    }

    if capability_set.contains("plugins") {
        groups.insert(
            "plugins",
            tool_set(&[
                "plugins.list",
                "plugins.get",
                "plugins.install",
                "plugins.invoke",
                "plugin_invocations.list",
            ]),
        );
    }

    if capability_set.contains("code_context")
        || capability_set.contains("documents")
        || capability_set.contains("attachments")
        || capability_set.contains("inspection")
        || capability_set.contains("corrective_action")
    {
        groups.insert(
            "resources",
            tool_set(&[
                "project_resources.list",
                "project_resources.create",
                "project_resources.update",
                "project_resources.delete",
                "files.upload",
            ]),
        );
    }

    let enabled_tools = groups
        .values()
        .flat_map(|tools| tools.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let groups_json = groups
        .into_iter()
        .map(|(group, tools)| (group, tools.into_iter().map(str::to_string).collect::<Vec<_>>()))
        .collect::<BTreeMap<_, _>>();
    json!({
        "source": "project_type_capabilities",
        "groups": groups_json,
        "enabled_tools": enabled_tools,
    })
}

fn capability_set(capabilities: &Value) -> BTreeSet<&str> {
    capabilities
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn tool_set(tools: &[&'static str]) -> BTreeSet<&'static str> {
    tools.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::build_mcp_tool_registry;
    use serde_json::json;

    fn enabled_tools(registry: &serde_json::Value) -> Vec<String> {
        registry["enabled_tools"]
            .as_array()
            .expect("enabled_tools should be an array")
            .iter()
            .map(|value| value.as_str().expect("tool name should be a string").to_string())
            .collect()
    }

    #[test]
    fn mcp_tool_registry_enables_tools_from_project_capabilities() {
        let registry = build_mcp_tool_registry(&json!([
            "issues",
            "sprints",
            "mcp",
            "governance",
            "code_context",
            "forms",
            "plugins"
        ]));
        let tools = enabled_tools(&registry);

        assert!(tools.contains(&"context.get_project".to_string()));
        assert!(tools.contains(&"release.readiness.get".to_string()));
        assert!(tools.contains(&"work_items.create".to_string()));
        assert!(tools.contains(&"sprints.create".to_string()));
        assert!(tools.contains(&"proposals.create".to_string()));
        assert!(tools.contains(&"proposals.create_from_result".to_string()));
        assert!(tools.contains(&"check_results.create".to_string()));
        assert!(tools.contains(&"project_resources.create".to_string()));
        assert!(tools.contains(&"forms.create_from_template".to_string()));
        assert!(tools.contains(&"scenario_templates.install".to_string()));
        assert!(tools.contains(&"forms.duplicate".to_string()));
        assert!(tools.contains(&"forms.schema_summary".to_string()));
        assert!(tools.contains(&"form_schema_versions.list".to_string()));
        assert!(tools.contains(&"form_schema_versions.get".to_string()));
        assert!(tools.contains(&"form_permissions.get".to_string()));
        assert!(tools.contains(&"form_attachments.create".to_string()));
        assert!(tools.contains(&"form_records.export".to_string()));
        assert!(tools.contains(&"form_records.import_preview".to_string()));
        assert!(tools.contains(&"form_records.import_commit".to_string()));
        assert!(tools.contains(&"form_records.relation_targets".to_string()));
        assert!(tools.contains(&"form_records.children".to_string()));
        assert!(tools.contains(&"form_records.child_create".to_string()));
        assert!(tools.contains(&"form_records.child_update".to_string()));
        assert!(tools.contains(&"form_records.child_archive".to_string()));
        assert!(tools.contains(&"form_records.child_restore".to_string()));
        assert!(tools.contains(&"plugins.install".to_string()));
        assert!(tools.contains(&"plugins.invoke".to_string()));
    }

    #[test]
    fn mcp_tool_registry_hides_disabled_capability_tools() {
        let registry = build_mcp_tool_registry(&json!(["issues", "mcp"]));
        let tools = enabled_tools(&registry);

        assert!(tools.contains(&"work_items.list".to_string()));
        assert!(!tools.iter().any(|tool| tool.starts_with("invocations.")));
        assert!(!tools.contains(&"sprints.create".to_string()));
        assert!(!tools.contains(&"proposals.create".to_string()));
    }
}
