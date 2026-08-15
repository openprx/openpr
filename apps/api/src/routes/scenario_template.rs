use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, FromQueryResult)]
struct ScenarioTemplateRow {
    pub id: Uuid,
    pub key: String,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub industry: String,
    pub project_type_key: String,
    pub audience_label: String,
    pub workflow_template: JsonValue,
    pub field_schema: JsonValue,
    pub resource_schema: JsonValue,
    pub ai_roles: JsonValue,
    pub governance_policy: JsonValue,
    pub connector_suggestions: JsonValue,
    pub sample_data: JsonValue,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ScenarioTemplateResponse {
    pub id: Uuid,
    pub key: String,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub industry: String,
    pub project_type_key: String,
    pub audience_label: String,
    pub workflow_template: JsonValue,
    pub field_schema: JsonValue,
    pub resource_schema: JsonValue,
    pub ai_roles: JsonValue,
    pub governance_policy: JsonValue,
    pub connector_suggestions: JsonValue,
    pub sample_data: JsonValue,
    pub usage_guide: JsonValue,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ScenarioTemplateRow> for ScenarioTemplateResponse {
    fn from(row: ScenarioTemplateRow) -> Self {
        let usage_guide = scenario_template_usage_guide(&row.key, &row.name);
        Self {
            id: row.id,
            key: row.key,
            workspace_id: row.workspace_id,
            name: row.name,
            description: row.description,
            industry: row.industry,
            project_type_key: row.project_type_key,
            audience_label: row.audience_label,
            workflow_template: row.workflow_template,
            field_schema: row.field_schema,
            resource_schema: row.resource_schema,
            ai_roles: row.ai_roles,
            governance_policy: row.governance_policy,
            connector_suggestions: row.connector_suggestions,
            sample_data: row.sample_data,
            usage_guide,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ScenarioTemplateQuery {
    pub project_type_key: Option<String>,
    pub industry: Option<String>,
}

fn normalize_key(raw: &str, field: &str) -> Result<String, ApiError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err(ApiError::BadRequest(format!("{field} is required")));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ApiError::BadRequest(format!(
            "{field} must contain lowercase letters, digits, or underscores"
        )));
    }
    Ok(key)
}

fn optional_normalized_filter(value: Option<String>, field: &str) -> Result<Option<String>, ApiError> {
    value.as_deref().map(|raw| normalize_key(raw, field)).transpose()
}

fn scenario_operator_entrypoints() -> JsonValue {
    json!([
        {
            "key": "frontend_project_wizard",
            "label": "Frontend project wizard",
            "consumer": "human_operator",
            "command_hint": "Open the workspace project list and pick a scenario template card"
        },
        {
            "key": "rest_projects_create",
            "label": "REST project creation API",
            "consumer": "integration_service",
            "command_hint": "POST /api/v1/workspaces/{workspace_id}/projects with scenario_template_key"
        },
        {
            "key": "mcp_projects_create",
            "label": "MCP projects.create",
            "consumer": "ai_or_automation_client",
            "command_hint": "projects.create with scenario_template_key, then forms.list"
        }
    ])
}

fn scenario_template_usage_guide(key: &str, name: &str) -> JsonValue {
    let common_entrypoints = scenario_operator_entrypoints();
    match key {
        "code_delivery_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the code delivery template",
                "Register repository and working-directory resources",
                "Create code tasks and change records",
                "Record CI and release evidence before handoff"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "events.tail"],
            "connector_kinds": ["mcp", "webhook"],
            "plugin_keys": [],
            "acceptance_focus": ["Repository resource is visible", "Release check records hold verification evidence"]
        }),
        "contract_review_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the contract review template",
                "Upload or link the contract document resource",
                "Create contract and risk-clause records",
                "Route approval records through MCP or webhook consumers"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "events.tail"],
            "connector_kinds": ["mcp", "webhook"],
            "plugin_keys": [],
            "acceptance_focus": ["Contract amount stays decimal-safe", "Risk clauses are linked to the contract"]
        }),
        "equipment_maintenance_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the equipment maintenance template",
                "Register equipment and site resources",
                "Create repair orders from fault reports",
                "Close the workflow with inspection records"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.update", "events.tail"],
            "connector_kinds": ["webhook", "mcp"],
            "plugin_keys": [],
            "acceptance_focus": ["Repair orders reference equipment", "Inspection acceptance is captured as a record"]
        }),
        "quality_corrective_action_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the quality corrective action template",
                "Create defect records by batch or order",
                "Link root-cause analysis to corrective actions",
                "Record recheck results and audit evidence"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "events.tail"],
            "connector_kinds": ["mcp", "webhook"],
            "plugin_keys": [],
            "acceptance_focus": ["Defect, root-cause, and corrective-action records stay linked", "Recheck status is visible to audit consumers"]
        }),
        "customer_delivery_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the customer delivery template",
                "Register customer and acceptance-material resources",
                "Create milestone and change-request records",
                "Send updates through MCP, webhook, or REST connectors"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.update", "events.tail"],
            "connector_kinds": ["mcp", "webhook", "rest"],
            "plugin_keys": [],
            "acceptance_focus": ["Milestones expose acceptance status", "Change requests keep commercial risk evidence"]
        }),
        "restaurant_ordering_default" => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                "Create the project from the restaurant ordering template",
                "Create menu categories, SKU, and tables",
                "Create orders with order-line child records",
                "Generate print jobs, receipts, and business reports"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "form_records.link", "form_records.aggregate", "events.tail", "plugins.invoke"],
            "connector_kinds": ["mcp", "print", "webhook"],
            "plugin_keys": ["restaurant_calc"],
            "acceptance_focus": ["Order-line totals are calculated by the plugin", "Print connector receipts update delivery state", "Revenue aggregate returns decimal strings"]
        }),
        _ => json!({
            "schema_version": "openpr.scenario_template.usage_guide.v1",
            "operator_entrypoints": common_entrypoints,
            "operator_steps": [
                format!("Create the project from the {name} template"),
                "Review generated forms and connector suggestions",
                "Create records through the frontend, REST API, or MCP tools"
            ],
            "primary_mcp_tools": ["scenario_templates.get", "projects.create", "forms.list", "form_records.create", "events.tail"],
            "connector_kinds": ["mcp"],
            "plugin_keys": [],
            "acceptance_focus": ["Generated forms are visible", "Records can be created through the shared API/MCP surface"]
        }),
    }
}

pub async fn list_scenario_templates(
    State(state): State<AppState>,
    Extension(_claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(query): Query<ScenarioTemplateQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = bot.map(|Extension(ctx)| ctx.workspace_id);
    let project_type_key = optional_normalized_filter(query.project_type_key, "project_type_key")?;
    let industry = optional_normalized_filter(query.industry, "industry")?;

    let mut clauses = Vec::new();
    let mut values = Vec::new();
    let mut param_idx = 1;

    if let Some(workspace_id) = workspace_id {
        clauses.push(format!("(workspace_id IS NULL OR workspace_id = ${param_idx})"));
        values.push(workspace_id.into());
        param_idx += 1;
    } else {
        clauses.push("workspace_id IS NULL".to_string());
    }

    if let Some(project_type_key) = project_type_key {
        clauses.push(format!("project_type_key = ${param_idx}"));
        values.push(project_type_key.into());
        param_idx += 1;
    }

    if let Some(industry) = industry {
        clauses.push(format!("industry = ${param_idx}"));
        values.push(industry.into());
    }

    let sql = format!(
        r"
            SELECT id, key, workspace_id, name, description, industry, project_type_key,
                   audience_label, workflow_template, field_schema, resource_schema,
                   ai_roles, governance_policy, connector_suggestions, sample_data,
                   created_at, updated_at
            FROM scenario_templates
            WHERE {}
            ORDER BY workspace_id NULLS FIRST, industry, name
        ",
        clauses.join(" AND ")
    );

    let items =
        ScenarioTemplateRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
            .all(&state.db)
            .await?;
    let items = items.into_iter().map(ScenarioTemplateResponse::from).collect();

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn get_scenario_template(
    State(state): State<AppState>,
    Extension(_claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let key = normalize_key(&key, "scenario template key")?;

    let (workspace_clause, values) = if let Some(Extension(ctx)) = bot {
        (
            "key = $1 AND (workspace_id IS NULL OR workspace_id = $2)",
            vec![key.into(), ctx.workspace_id.into()],
        )
    } else {
        ("key = $1 AND workspace_id IS NULL", vec![key.into()])
    };

    let sql = format!(
        r"
            SELECT id, key, workspace_id, name, description, industry, project_type_key,
                   audience_label, workflow_template, field_schema, resource_schema,
                   ai_roles, governance_policy, connector_suggestions, sample_data,
                   created_at, updated_at
            FROM scenario_templates
            WHERE {workspace_clause}
            ORDER BY workspace_id NULLS LAST
            LIMIT 1
        "
    );

    let template =
        ScenarioTemplateRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
            .one(&state.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("scenario template not found".to_string()))?;

    Ok(ApiResponse::success(ScenarioTemplateResponse::from(template)))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{normalize_key, optional_normalized_filter, scenario_template_usage_guide};

    #[test]
    fn normalizes_template_filters() {
        assert_eq!(normalize_key(" Contract_Review ", "key").unwrap(), "contract_review");
        assert_eq!(
            optional_normalized_filter(Some(" Quality ".to_string()), "industry")
                .unwrap()
                .as_deref(),
            Some("quality")
        );
    }

    #[test]
    fn rejects_invalid_template_keys() {
        assert!(normalize_key("", "key").is_err());
        assert!(normalize_key("contract-review", "key").is_err());
        assert!(normalize_key("legal/risk", "key").is_err());
    }

    #[test]
    fn restaurant_usage_guide_exposes_runtime_integration_contract() {
        let usage = scenario_template_usage_guide("restaurant_ordering_default", "Restaurant Ordering");
        assert_eq!(usage["schema_version"], "openpr.scenario_template.usage_guide.v1");
        assert!(
            usage["operator_entrypoints"]
                .as_array()
                .expect("entrypoints")
                .iter()
                .any(|entry| entry["key"] == "mcp_projects_create")
        );
        assert!(
            usage["primary_mcp_tools"]
                .as_array()
                .expect("tools")
                .iter()
                .any(|tool| tool == "form_records.aggregate")
        );
        assert!(
            usage["connector_kinds"]
                .as_array()
                .expect("connector kinds")
                .iter()
                .any(|kind| kind == "print")
        );
        assert_eq!(usage["plugin_keys"][0], "restaurant_calc");
    }

    #[test]
    fn custom_template_usage_guide_has_safe_generic_contract() {
        let usage = scenario_template_usage_guide("custom_ops", "Custom Ops");
        assert_eq!(usage["schema_version"], "openpr.scenario_template.usage_guide.v1");
        assert!(
            usage["primary_mcp_tools"]
                .as_array()
                .expect("tools")
                .iter()
                .any(|tool| tool == "projects.create")
        );
        assert_eq!(usage["connector_kinds"][0], "mcp");
    }
}
