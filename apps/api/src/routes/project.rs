// Public and framework-facing signatures remain stable during this behavior-neutral cleanup.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::similar_names
)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    forms::schema::{ensure_schema_field_ids, validate_schema},
    middleware::bot_auth::{BotAuthContext, require_workspace_access, require_workspace_access_from_auth},
    plugins::{manifest::parse_manifest, runtime::validate_wasm_module},
    response::{ApiResponse, PaginatedData},
    routes::decision_domain,
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub type_key: String,
    pub type_settings: JsonValue,
    pub created_at: String,
    pub updated_at: String,
    pub issue_counts: Option<IssueCounts>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct IssueCounts {
    pub by_state: HashMap<String, i64>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub type_key: Option<String>,
    pub type_settings: Option<JsonValue>,
    pub scenario_template_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub key: Option<String>,
    pub description: Option<String>,
    pub type_key: Option<String>,
    pub type_settings: Option<JsonValue>,
}

#[derive(Debug, FromQueryResult)]
struct ScenarioTemplateRow {
    key: String,
    name: String,
    industry: String,
    project_type_key: String,
    audience_label: String,
    workflow_template: JsonValue,
    field_schema: JsonValue,
    resource_schema: JsonValue,
    ai_roles: JsonValue,
    governance_policy: JsonValue,
    connector_suggestions: JsonValue,
    sample_data: JsonValue,
}

#[derive(Debug, FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct ScenarioProjectRow {
    id: Uuid,
    workspace_id: Uuid,
    key: String,
    name: String,
    description: String,
    type_key: Option<String>,
    type_settings: JsonValue,
    workflow_id: Option<Uuid>,
    created_by: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn ensure_project_type_exists(state: &AppState, type_key: &str) -> Result<(), ApiError> {
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM project_types WHERE key = $1",
            vec![type_key.to_string().into()],
        ))
        .await?
        .is_some();

    if exists {
        Ok(())
    } else {
        Err(ApiError::BadRequest("project type does not exist".to_string()))
    }
}

fn normalize_scenario_template_key(raw: &str) -> Result<String, ApiError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err(ApiError::BadRequest("scenario_template_key is required".to_string()));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ApiError::BadRequest(
            "scenario_template_key must contain lowercase letters, digits, or underscores".to_string(),
        ));
    }
    Ok(key)
}

async fn load_scenario_template(
    state: &AppState,
    workspace_id: Uuid,
    key: &str,
) -> Result<ScenarioTemplateRow, ApiError> {
    ScenarioTemplateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT key, name, industry, project_type_key, audience_label, workflow_template,
                   field_schema, resource_schema, ai_roles, governance_policy,
                   connector_suggestions, sample_data
            FROM scenario_templates
            WHERE key = $1 AND (workspace_id IS NULL OR workspace_id = $2)
            ORDER BY workspace_id NULLS LAST
            LIMIT 1
        ",
        vec![key.to_string().into(), workspace_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("scenario template not found".to_string()))
}

fn merge_template_type_settings(
    settings: Option<JsonValue>,
    template: Option<&ScenarioTemplateRow>,
) -> Result<JsonValue, ApiError> {
    let mut settings = settings.unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        return Err(ApiError::BadRequest("type_settings must be a JSON object".to_string()));
    }

    if let Some(template) = template {
        let object = settings
            .as_object_mut()
            .ok_or_else(|| ApiError::BadRequest("type_settings must be a JSON object".to_string()))?;
        object.insert("scenario_template_key".to_string(), json!(template.key));
        object.insert(
            "scenario_template".to_string(),
            json!({
                "key": template.key,
                "name": template.name,
                "industry": template.industry,
                "project_type_key": template.project_type_key,
                "audience_label": template.audience_label,
                "field_schema": template.field_schema,
                "resource_schema": template.resource_schema,
                "ai_roles": template.ai_roles,
                "governance_policy": template.governance_policy,
                "connector_suggestions": template.connector_suggestions,
                "sample_data": template.sample_data
            }),
        );
    }

    Ok(settings)
}

async fn create_template_workflow(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_name: &str,
    template: &ScenarioTemplateRow,
) -> Result<Option<Uuid>, ApiError> {
    let Some(states) = template
        .workflow_template
        .get("states")
        .and_then(JsonValue::as_array)
        .filter(|states| !states.is_empty())
    else {
        return Ok(None);
    };

    let workflow_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO workflows (id, workspace_id, name, description, is_system_default, created_at, updated_at)
            VALUES ($1, $2, $3, $4, false, now(), now())
        ",
        vec![
            workflow_id.into(),
            workspace_id.into(),
            format!("{project_name} workflow").into(),
            format!("Created from scenario template {}", template.key).into(),
        ],
    ))
    .await?;

    for (idx, state) in states.iter().enumerate() {
        let key = state
            .get("key")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::BadRequest("template workflow state key is required".to_string()))?;
        let display_name = state
            .get("name")
            .or_else(|| state.get("display_name"))
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(key);
        let category = state
            .get("category")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("active");
        let color = state
            .get("color")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let is_initial = state
            .get("initial")
            .or_else(|| state.get("is_initial"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(idx == 0);
        let is_terminal = state
            .get("terminal")
            .or_else(|| state.get("is_terminal"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let position = i32::try_from(idx + 1)
            .map_err(|_| ApiError::BadRequest("template defines too many workflow states".to_string()))?;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO workflow_states (
                    id, workflow_id, key, display_name, category, position, color,
                    is_initial, is_terminal, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())
            ",
            vec![
                Uuid::new_v4().into(),
                workflow_id.into(),
                key.to_string().into(),
                display_name.to_string().into(),
                category.to_string().into(),
                position.into(),
                color.into(),
                is_initial.into(),
                is_terminal.into(),
            ],
        ))
        .await?;
    }

    Ok(Some(workflow_id))
}

async fn apply_scenario_template(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_id: Uuid,
    project_key: &str,
    actor_id: Option<Uuid>,
    registered_by: Uuid,
    template: &ScenarioTemplateRow,
    causation_id: Option<Uuid>,
) -> Result<(), ApiError> {
    create_template_labels(tx, workspace_id, template).await?;
    create_template_governance_config(tx, project_id, actor_id, template).await?;
    create_template_ai_placeholders(tx, project_id, registered_by, template).await?;
    create_template_connector_suggestions(
        tx,
        workspace_id,
        project_id,
        actor_id,
        project_key,
        template,
        causation_id,
    )
    .await?;
    create_template_forms(tx, workspace_id, project_id, actor_id, template, causation_id).await?;
    create_template_plugins(tx, workspace_id, project_id, actor_id, template, causation_id).await?;
    Ok(())
}

struct TemplatePlugin {
    manifest: JsonValue,
    wasm_bytes: Vec<u8>,
    status: String,
}

struct ScenarioTemplateEventContext<'a> {
    workspace_id: Uuid,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    template: &'a ScenarioTemplateRow,
    causation_id: Option<Uuid>,
}

struct ScenarioBusinessEvent<'a> {
    event_type: &'a str,
    aggregate_type: &'a str,
    aggregate_id: String,
    payload: JsonValue,
    metadata: JsonValue,
}

async fn create_template_plugins(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    template: &ScenarioTemplateRow,
    causation_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let event_ctx = ScenarioTemplateEventContext {
        workspace_id,
        project_id,
        actor_id,
        template,
        causation_id,
    };
    for plugin in scenario_plugin_templates(template)? {
        validate_wasm_module(&plugin.wasm_bytes).map_err(ApiError::BadRequest)?;
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let manifest_value = serde_json::to_value(&manifest).map_err(|_| ApiError::Internal)?;
        let wasm_sha256 = hex::encode(Sha256::digest(&plugin.wasm_bytes));
        let plugin_key = manifest.key.clone();
        let plugin_name = manifest.name.clone();
        let plugin_version = manifest.version.clone();
        let plugin_description = manifest.description.clone();
        let plugin_status = plugin.status.clone();

        let inserted = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO plugins (
                    workspace_id, project_id, key, name, version, description, manifest,
                    wasm_bytes, wasm_sha256, status, installed_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now(), now())
                ON CONFLICT (project_id, key, version) DO NOTHING
                RETURNING id
            ",
            vec![
                workspace_id.into(),
                project_id.into(),
                plugin_key.clone().into(),
                plugin_name.into(),
                plugin_version.clone().into(),
                plugin_description.into(),
                manifest_value.into(),
                plugin.wasm_bytes.into(),
                wasm_sha256.clone().into(),
                plugin_status.clone().into(),
                actor_id.into(),
            ],
        ))
        .one(tx)
        .await?;
        if let Some(inserted) = inserted {
            insert_scenario_business_event(
                tx,
                &event_ctx,
                ScenarioBusinessEvent {
                    event_type: "plugin.installed",
                    aggregate_type: "plugin",
                    aggregate_id: inserted.id.to_string(),
                    payload: json!({
                        "plugin_id": inserted.id,
                        "plugin_key": plugin_key,
                        "version": plugin_version,
                        "status": plugin_status,
                        "wasm_sha256": wasm_sha256,
                        "scenario_template_key": template.key
                    }),
                    metadata: json!({
                        "plugin_id": inserted.id,
                        "plugin_key": plugin_key,
                        "version": plugin_version,
                        "scenario_template_initialized": true
                    }),
                },
            )
            .await?;
        }
    }

    Ok(())
}

async fn create_template_forms(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    template: &ScenarioTemplateRow,
    causation_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let event_ctx = ScenarioTemplateEventContext {
        workspace_id,
        project_id,
        actor_id,
        template,
        causation_id,
    };
    let forms = scenario_form_templates(template);
    for form in forms {
        let key = template_string(&form, "key", "form")?;
        let name = template_string(&form, "name", &key)?;
        let description = template_optional_string(&form, "description").unwrap_or_default();
        let icon = template_optional_string(&form, "icon");
        let color = template_optional_string(&form, "color");
        let title_template = template_optional_string(&form, "title_template").unwrap_or_else(|| "{id}".to_string());
        let schema =
            normalize_form_template_schema(form.get("schema").cloned().unwrap_or_else(|| json!({"fields": []})))?;
        let detail_layout = ensure_template_object(
            form.get("detail_layout").cloned().unwrap_or_else(|| json!({})),
            "detail_layout",
        )?;
        let requested_form_id = Uuid::new_v4();
        let inserted_form = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO project_forms (
                    id, workspace_id, project_id, key, name, description, icon, color,
                    title_template, schema, detail_layout, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())
                ON CONFLICT (project_id, key) DO NOTHING
                RETURNING id
            ",
            vec![
                requested_form_id.into(),
                workspace_id.into(),
                project_id.into(),
                key.clone().into(),
                name.clone().into(),
                description.clone().into(),
                icon.clone().into(),
                color.clone().into(),
                title_template.clone().into(),
                schema.clone().into(),
                detail_layout.clone().into(),
                actor_id.into(),
            ],
        ))
        .one(tx)
        .await?;
        let (form_id, form_created) = if let Some(inserted_form) = inserted_form {
            (inserted_form.id, true)
        } else {
            let existing = IdRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM project_forms WHERE project_id = $1 AND key = $2",
                vec![project_id.into(), key.clone().into()],
            ))
            .one(tx)
            .await?
            .ok_or_else(|| ApiError::Internal)?;
            (existing.id, false)
        };
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO form_schema_versions (
                    form_id, version, schema, detail_layout, changed_by, change_summary, created_at
                )
                SELECT id, schema_version, schema, detail_layout, $3, $4, now()
                FROM project_forms
                WHERE project_id = $1 AND key = $2
                ON CONFLICT (form_id, version) DO NOTHING
            ",
            vec![
                project_id.into(),
                key.clone().into(),
                actor_id.into(),
                "scenario template baseline schema".to_string().into(),
            ],
        ))
        .await?;
        if form_created {
            insert_scenario_form_event(
                tx,
                &event_ctx,
                form_id,
                None,
                "form.created",
                json!({
                    "form_id": form_id,
                    "key": key,
                    "name": name,
                    "scenario_template_key": template.key
                }),
                json!({
                    "form_id": form_id,
                    "form_key": key,
                    "scenario_template_initialized": true
                }),
            )
            .await?;
        }

        let views = template_views(&form, &schema);
        for view in views {
            let view_key = template_string(&view, "key", "grid")?;
            let view_name = template_string(&view, "name", &view_key)?;
            let view_type = template_string(&view, "view_type", "grid")?;
            let config =
                ensure_template_object(view.get("config").cloned().unwrap_or_else(|| json!({})), "view.config")?;
            let requested_view_id = Uuid::new_v4();
            let inserted_view = IdRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO form_views (
                        id, workspace_id, project_id, form_id, key, name, view_type,
                        config, created_by, created_at, updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())
                    ON CONFLICT (form_id, key) DO NOTHING
                    RETURNING id
                ",
                vec![
                    requested_view_id.into(),
                    workspace_id.into(),
                    project_id.into(),
                    form_id.into(),
                    view_key.clone().into(),
                    view_name.clone().into(),
                    view_type.clone().into(),
                    config.into(),
                    actor_id.into(),
                ],
            ))
            .one(tx)
            .await?;
            if let Some(inserted_view) = inserted_view {
                insert_scenario_form_event(
                    tx,
                    &event_ctx,
                    form_id,
                    None,
                    "form.view.created",
                    json!({
                        "form_id": form_id,
                        "view_id": inserted_view.id,
                        "key": view_key,
                        "view_type": view_type,
                        "scenario_template_key": template.key
                    }),
                    json!({
                        "form_id": form_id,
                        "form_key": key,
                        "view_id": inserted_view.id,
                        "view_key": view_key,
                        "scenario_template_initialized": true
                    }),
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn create_template_labels(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    template: &ScenarioTemplateRow,
) -> Result<(), ApiError> {
    let palette = ["#2563eb", "#7c3aed", "#dc2626", "#0891b2", "#16a34a", "#ca8a04"];
    let Some(labels) = template
        .workflow_template
        .get("default_labels")
        .and_then(JsonValue::as_array)
    else {
        return Ok(());
    };

    for (idx, label) in labels.iter().filter_map(JsonValue::as_str).enumerate() {
        let name = label.trim();
        if name.is_empty() {
            continue;
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO labels (id, workspace_id, name, color, description, created_at)
                VALUES ($1, $2, $3, $4, $5, now())
                ON CONFLICT (workspace_id, name) DO NOTHING
            ",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                name.to_string().into(),
                palette
                    .get(idx % palette.len())
                    .copied()
                    .unwrap_or("#64748b")
                    .to_string()
                    .into(),
                format!("Created from scenario template {}", template.key).into(),
            ],
        ))
        .await?;
    }

    Ok(())
}

async fn create_template_governance_config(
    tx: &DatabaseTransaction,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    template: &ScenarioTemplateRow,
) -> Result<(), ApiError> {
    let config = json!({
        "source": "scenario_template",
        "scenario_template_key": template.key,
        "industry": template.industry,
        "policy": template.governance_policy
    });

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO governance_configs (
                project_id, review_required, auto_review_days, review_reminder_days,
                audit_report_cron, trust_update_mode, config, updated_by, created_at, updated_at
            )
            VALUES ($1, true, 30, 7, '0 0 1 * *', 'review_based', $2, $3, now(), now())
            ON CONFLICT (project_id)
            DO UPDATE SET
                config = EXCLUDED.config,
                updated_by = EXCLUDED.updated_by,
                updated_at = now()
        ",
        vec![project_id.into(), config.into(), actor_id.into()],
    ))
    .await?;

    Ok(())
}

async fn create_template_ai_placeholders(
    tx: &DatabaseTransaction,
    project_id: Uuid,
    user_id: Uuid,
    template: &ScenarioTemplateRow,
) -> Result<(), ApiError> {
    let Some(roles) = template.ai_roles.as_array() else {
        return Ok(());
    };
    let project_short = project_id.simple().to_string();
    let project_short = &project_short[..8];

    for role in roles {
        let role_key = role
            .get("key")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("assistant");
        let label = role
            .get("label")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(role_key);
        let agent_type = role
            .get("agent_type")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("template");
        let provider = role
            .get("provider")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(agent_type);
        let model = role
            .get("model")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("template-placeholder");
        let api_endpoint = role
            .get("api_endpoint")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let capabilities = role.get("capabilities").cloned().unwrap_or_else(|| json!([]));
        let participant_id = format!("tpl-{project_short}-{role_key}")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(100)
            .collect::<String>();

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO ai_participants (
                    id, project_id, name, model, provider, api_endpoint,
                    capabilities, domain_overrides, max_domain_level,
                    can_veto_human_consensus, reason_min_length, is_active,
                    registered_by, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'advisor', false, 50, false, $9, now())
                ON CONFLICT (id) DO NOTHING
            ",
            vec![
                participant_id.into(),
                project_id.into(),
                label.to_string().into(),
                model.to_string().into(),
                provider.to_string().into(),
                api_endpoint.into(),
                capabilities.into(),
                json!({
                    "source": "scenario_template",
                    "scenario_template_key": template.key,
                    "role": role
                })
                .into(),
                user_id.into(),
            ],
        ))
        .await?;
    }

    Ok(())
}

async fn create_template_connector_suggestions(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    project_key: &str,
    template: &ScenarioTemplateRow,
    causation_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let event_ctx = ScenarioTemplateEventContext {
        workspace_id,
        project_id,
        actor_id,
        template,
        causation_id,
    };
    let Some(suggestions) = template.connector_suggestions.as_array() else {
        return Ok(());
    };

    for suggestion in suggestions {
        let kind = suggestion
            .get("type")
            .or_else(|| suggestion.get("kind"))
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("webhook");
        if !matches!(
            kind,
            "webhook" | "mcp" | "rest" | "cli" | "openprx_tunnel" | "print" | "device"
        ) {
            continue;
        }
        let label = suggestion
            .get("label")
            .or_else(|| suggestion.get("name"))
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Template connection");
        let endpoint = suggestion
            .get("endpoint")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        let connector_id = Uuid::new_v4();

        let inserted_connector = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO connectors (
                    id, workspace_id, project_id, kind, name, description, endpoint,
                    auth_policy, capability_manifest, is_active, created_by, created_at, updated_at
                )
                SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, false, $10, now(), now()
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM connectors
                    WHERE project_id = $3
                      AND kind = $4
                      AND name = $5
                      AND capability_manifest->>'source' = 'scenario_template'
                      AND capability_manifest->>'scenario_template_key' = $11
                )
                RETURNING id
            ",
            vec![
                connector_id.into(),
                workspace_id.into(),
                project_id.into(),
                kind.to_string().into(),
                label.to_string().into(),
                format!(
                    "Suggested by scenario template {} for project {project_key}",
                    template.key
                )
                .into(),
                endpoint.into(),
                json!({
                    "mode": "unconfigured",
                    "template_suggestion": true
                })
                .into(),
                json!({
                    "source": "scenario_template",
                    "scenario_template_key": template.key,
                    "suggestion": suggestion
                })
                .into(),
                actor_id.into(),
                template.key.clone().into(),
            ],
        ))
        .one(tx)
        .await?;
        if let Some(inserted_connector) = inserted_connector {
            insert_scenario_business_event(
                tx,
                &event_ctx,
                ScenarioBusinessEvent {
                    event_type: "connector.created",
                    aggregate_type: "connector",
                    aggregate_id: inserted_connector.id.to_string(),
                    payload: json!({
                        "connector_id": inserted_connector.id,
                        "kind": kind,
                        "name": label,
                        "project_id": project_id,
                        "scenario_template_key": template.key
                    }),
                    metadata: json!({
                        "connector_id": inserted_connector.id,
                        "connector_kind": kind,
                        "project_id": project_id,
                        "scenario_template_initialized": true
                    }),
                },
            )
            .await?;
        }
    }

    Ok(())
}

async fn insert_scenario_form_event(
    tx: &DatabaseTransaction,
    ctx: &ScenarioTemplateEventContext<'_>,
    form_id: Uuid,
    record_id: Option<Uuid>,
    event_type: &str,
    payload: JsonValue,
    metadata: JsonValue,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_events (
                id, workspace_id, project_id, form_id, record_id, event_type,
                actor_id, source, payload, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
        ",
        vec![
            Uuid::new_v4().into(),
            ctx.workspace_id.into(),
            ctx.project_id.into(),
            form_id.into(),
            record_id.into(),
            event_type.to_string().into(),
            ctx.actor_id.into(),
            scenario_template_event_source(ctx).into(),
            payload.clone().into(),
        ],
    ))
    .await?;

    insert_scenario_business_event(
        tx,
        ctx,
        ScenarioBusinessEvent {
            event_type,
            aggregate_type: "form",
            aggregate_id: form_id.to_string(),
            payload,
            metadata: json!({
                "form_id": form_id,
                "record_id": record_id,
                "legacy_form_events": true,
                "scenario_template_initialized": true,
                "template_metadata": metadata
            }),
        },
    )
    .await?;
    Ok(())
}

async fn insert_scenario_business_event(
    tx: &DatabaseTransaction,
    ctx: &ScenarioTemplateEventContext<'_>,
    event: ScenarioBusinessEvent<'_>,
) -> Result<Uuid, ApiError> {
    insert_business_event(
        tx,
        BusinessEventInput {
            workspace_id: ctx.workspace_id,
            project_id: Some(ctx.project_id),
            event_type: event.event_type.to_string(),
            aggregate_type: event.aggregate_type.to_string(),
            aggregate_id: event.aggregate_id,
            actor_id: ctx.actor_id,
            source: scenario_template_event_source(ctx),
            payload: event.payload,
            metadata: event.metadata,
            correlation_id: ctx.causation_id,
            causation_id: ctx.causation_id,
            idempotency_key: None,
        },
    )
    .await
}

fn scenario_template_event_source(ctx: &ScenarioTemplateEventContext<'_>) -> JsonValue {
    json!({
        "type": "system",
        "origin": "scenario_template",
        "scenario_template_key": ctx.template.key,
        "actor_id": ctx.actor_id
    })
}

fn scenario_form_templates(template: &ScenarioTemplateRow) -> Vec<JsonValue> {
    if let Some(forms) = template
        .sample_data
        .get("forms")
        .and_then(JsonValue::as_array)
        .filter(|forms| !forms.is_empty())
    {
        return forms.clone();
    }
    default_form_templates(&template.key)
}

fn scenario_plugin_templates(template: &ScenarioTemplateRow) -> Result<Vec<TemplatePlugin>, ApiError> {
    if let Some(plugins) = template
        .sample_data
        .get("plugins")
        .and_then(JsonValue::as_array)
        .filter(|plugins| !plugins.is_empty())
    {
        return plugins
            .iter()
            .map(template_plugin_from_json)
            .collect::<Result<Vec<_>, _>>();
    }
    default_plugin_templates(&template.key)
}

fn template_plugin_from_json(value: &JsonValue) -> Result<TemplatePlugin, ApiError> {
    let manifest = value
        .get("manifest")
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("template plugin manifest is required".to_string()))?;
    let status = value
        .get("status")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .unwrap_or("active");
    if !matches!(status, "active" | "disabled" | "failed") {
        return Err(ApiError::BadRequest("template plugin status is invalid".to_string()));
    }
    let output = value.get("wasm_output").cloned().unwrap_or_else(|| json!({"ok": true}));
    Ok(TemplatePlugin {
        manifest,
        wasm_bytes: constant_json_wasm(&output)?,
        status: status.to_string(),
    })
}

fn default_plugin_templates(template_key: &str) -> Result<Vec<TemplatePlugin>, ApiError> {
    match template_key {
        "restaurant_ordering_default" => Ok(vec![TemplatePlugin {
            manifest: json!({
                "schema_version": "openpr.plugin.v1",
                "key": "restaurant_calc",
                "name": "Restaurant Calculation",
                "version": "0.1.0",
                "description": "Computes restaurant line totals and exposes restaurant scenario tools.",
                "capabilities": {
                    "hooks": [
                        {"kind": "formula", "form_key": "order_line", "field_key": "line_total"},
                        {"kind": "event_handler", "event_type": "print_job.created"}
                    ],
                    "tools": [
                        {"name": "restaurant_calc.today_revenue", "description": "Read restaurant daily revenue context."}
                    ],
                    "runtime": {"timeout_ms": 500, "fuel": 100_000, "memory_bytes": 1_048_576}
                }
            }),
            wasm_bytes: constant_json_wasm(&json!({
                "ok": true,
                "patch": {"line_total": "19.98"},
                "print_payload": {"format": "text", "template": "kitchen_or_receipt"},
                "today_revenue": "19.98"
            }))?,
            status: "active".to_string(),
        }]),
        _ => Ok(Vec::new()),
    }
}

fn default_form_templates(template_key: &str) -> Vec<JsonValue> {
    match template_key {
        "code_delivery_default" => vec![
            form_template(
                "code_task",
                "Code Task",
                "Repository work item for branch, CI and release risk.",
                "{repo}:{branch}",
                vec![
                    field("repo", "Repository", "text", true),
                    field("directory", "Directory", "text", false),
                    field("branch", "Branch", "text", false),
                    select_field("ci_status", "CI status", false, &["unknown", "passing", "failing"]),
                    select_field("risk", "Risk", false, &["low", "medium", "high"]),
                    field("verification", "Verification", "textarea", false),
                ],
            ),
            form_template(
                "change_record",
                "Change Record",
                "Changed files, summary, tests and impact surface.",
                "{file_path}",
                vec![
                    field("file_path", "File", "text", true),
                    field("summary", "Summary", "textarea", true),
                    field("tests", "Tests", "textarea", false),
                    field("impact", "Impact", "textarea", false),
                ],
            ),
            form_template(
                "release_check",
                "Release Check",
                "Release readiness checklist and evidence.",
                "{check_item}",
                vec![
                    field("check_item", "Check item", "text", true),
                    select_field("result", "Result", true, &["pending", "pass", "fail"]),
                    field("evidence", "Evidence", "textarea", false),
                    field("owner", "Owner", "text", false),
                ],
            ),
        ],
        "contract_review_default" => vec![
            form_template(
                "contract",
                "Contract",
                "Contract master data, amount, dates, attachment and risk status.",
                "{contract_name}",
                vec![
                    field("contract_no", "Contract No.", "text", true),
                    field("contract_name", "Contract Name", "text", true),
                    field("counterparty", "Counterparty", "text", true),
                    amount_field("amount", "Amount", false, "CNY", 2),
                    field("signing_entity", "Signing Entity", "text", false),
                    field("start_date", "Start Date", "date", false),
                    field("end_date", "End Date", "date", false),
                    field("attachment", "Attachment", "attachment", false),
                    select_field("risk_level", "Risk Level", true, &["low", "medium", "high"]),
                    select_field("status", "Status", false, &["draft", "reviewing", "approved", "signed"]),
                ],
            ),
            form_template(
                "risk_clause",
                "Risk Clause",
                "Clause risk extraction and suggested revision.",
                "{clause_type}",
                vec![
                    select_field(
                        "clause_type",
                        "Clause Type",
                        true,
                        &["payment", "liability", "termination", "ip", "other"],
                    ),
                    field("original_text", "Original Text", "textarea", true),
                    field("risk_note", "Risk Note", "textarea", true),
                    field("suggested_revision", "Suggested Revision", "textarea", false),
                    select_field("risk_level", "Risk Level", true, &["low", "medium", "high"]),
                    field("owner", "Owner", "text", false),
                    relation_field("contract_id", "Contract", "contract", false),
                ],
            ),
            form_template(
                "approval_record",
                "Approval Record",
                "Approver decision, opinion and timestamp.",
                "{approver}",
                vec![
                    field("approver", "Approver", "text", true),
                    field("opinion", "Opinion", "textarea", false),
                    field("approved_at", "Approved At", "datetime", false),
                    select_field("decision", "Decision", true, &["pending", "approved", "rejected"]),
                    relation_field("contract_id", "Contract", "contract", false),
                ],
            ),
        ],
        "equipment_maintenance_default" => vec![
            form_template(
                "equipment",
                "Equipment",
                "Equipment asset profile and owner.",
                "{equipment_no}",
                vec![
                    field("equipment_no", "Equipment No.", "text", true),
                    field("name", "Name", "text", true),
                    field("model", "Model", "text", false),
                    field("location", "Location", "text", false),
                    field("owner", "Owner", "text", false),
                    select_field("status", "Status", false, &["running", "stopped", "maintenance"]),
                    field("image", "Image", "image", false),
                ],
            ),
            form_template(
                "repair_order",
                "Repair Order",
                "Fault report, downtime, owner and repair result.",
                "{fault_type}",
                vec![
                    relation_field("equipment_id", "Equipment", "equipment", true),
                    field("fault_type", "Fault Type", "text", true),
                    field("description", "Description", "textarea", true),
                    select_field(
                        "downtime_impact",
                        "Downtime Impact",
                        false,
                        &["none", "minor", "major", "critical"],
                    ),
                    field("reported_at", "Reported At", "datetime", false),
                    field("repair_owner", "Repair Owner", "text", false),
                    field("repair_result", "Repair Result", "textarea", false),
                    field("attachment", "Attachment", "attachment", false),
                ],
            ),
            form_template(
                "inspection_record",
                "Inspection Record",
                "Inspection item, result, exception and evidence.",
                "{inspection_item}",
                vec![
                    relation_field("equipment_id", "Equipment", "equipment", true),
                    field("inspection_item", "Inspection Item", "text", true),
                    select_field("result", "Result", true, &["pending", "pass", "fail"]),
                    field("exception_note", "Exception Note", "textarea", false),
                    field("signature", "Signature", "text", false),
                    field("image", "Image", "image", false),
                ],
            ),
        ],
        "quality_corrective_action_default" => vec![
            form_template(
                "defect_record",
                "Defect Record",
                "Batch/order defect classification and severity.",
                "{defect_type}",
                vec![
                    field("batch", "Batch or Order", "text", true),
                    field("defect_type", "Defect Type", "text", true),
                    field("quantity", "Quantity", "integer", false),
                    field("image", "Image", "image", false),
                    select_field("severity", "Severity", false, &["minor", "major", "critical"]),
                ],
            ),
            form_template(
                "root_cause_analysis",
                "Root Cause Analysis",
                "Cause category, analysis process and evidence.",
                "{cause_category}",
                vec![
                    relation_field("defect_id", "Defect", "defect_record", true),
                    select_field(
                        "cause_category",
                        "Cause Category",
                        true,
                        &["material", "process", "human", "equipment", "other"],
                    ),
                    field("analysis", "Analysis", "textarea", true),
                    field("responsible_department", "Responsible Department", "text", false),
                    field("evidence", "Evidence", "attachment", false),
                ],
            ),
            form_template(
                "corrective_action",
                "Corrective Action",
                "Action owner, deadline and recheck result.",
                "{action}",
                vec![
                    relation_field("defect_id", "Defect", "defect_record", true),
                    field("action", "Action", "textarea", true),
                    field("owner", "Owner", "text", true),
                    field("deadline", "Deadline", "date", false),
                    select_field(
                        "recheck_result",
                        "Recheck Result",
                        false,
                        &["pending", "passed", "failed"],
                    ),
                ],
            ),
        ],
        "customer_delivery_default" => vec![
            form_template(
                "customer",
                "Customer",
                "Customer profile and contact.",
                "{customer_name}",
                vec![
                    field("customer_name", "Customer Name", "text", true),
                    field("contact", "Contact", "text", false),
                    field("phone", "Phone", "text", false),
                    field("industry", "Industry", "text", false),
                    select_field("level", "Level", false, &["standard", "key", "strategic"]),
                ],
            ),
            form_template(
                "delivery_milestone",
                "Delivery Milestone",
                "Plan date, actual date, acceptance status and risk.",
                "{milestone}",
                vec![
                    relation_field("customer_id", "Customer", "customer", true),
                    field("milestone", "Milestone", "text", true),
                    field("planned_date", "Planned Date", "date", false),
                    field("actual_date", "Actual Date", "date", false),
                    select_field(
                        "acceptance_status",
                        "Acceptance Status",
                        false,
                        &["not_started", "submitted", "accepted", "rejected"],
                    ),
                    select_field("risk", "Risk", false, &["low", "medium", "high"]),
                ],
            ),
            form_template(
                "change_request",
                "Change Request",
                "Change content, impact, quotation and status.",
                "{request}",
                vec![
                    relation_field("customer_id", "Customer", "customer", true),
                    field("request", "Request", "textarea", true),
                    field("impact", "Impact", "textarea", false),
                    amount_field("quote", "Quote", false, "CNY", 2),
                    select_field(
                        "status",
                        "Status",
                        false,
                        &["draft", "reviewing", "approved", "rejected"],
                    ),
                ],
            ),
        ],
        "restaurant_ordering_default" => restaurant_form_templates(),
        _ => Vec::new(),
    }
}

fn restaurant_form_templates() -> Vec<JsonValue> {
    vec![
        form_template(
            "menu_category",
            "Menu Category",
            "Menu grouping for front counter and kitchen routing.",
            "{name}",
            vec![
                field("name", "Name", "text", true),
                field("sort_order", "Sort Order", "integer", false),
                select_field("status", "Status", false, &["active", "inactive"]),
            ],
        ),
        form_template(
            "sku",
            "SKU",
            "Menu item SKU, category, price and kitchen station.",
            "{name}",
            vec![
                field("sku", "SKU", "text", true),
                field("name", "Name", "text", true),
                relation_field("category_id", "Category", "menu_category", false),
                amount_field("price", "Price", true, "CNY", 2),
                field("kitchen_station", "Kitchen Station", "text", false),
                select_field("status", "Status", false, &["available", "sold_out", "inactive"]),
            ],
        ),
        form_template(
            "table",
            "Table",
            "Dining table or seat area.",
            "{table_no}",
            vec![
                field("table_no", "Table No.", "text", true),
                field("seat_count", "Seat Count", "integer", false),
                select_field(
                    "status",
                    "Status",
                    false,
                    &["empty", "occupied", "reserved", "cleaning"],
                ),
            ],
        ),
        form_template(
            "order",
            "Order",
            "Restaurant order header, table and total amount.",
            "{order_no}",
            vec![
                field("order_no", "Order No.", "text", true),
                relation_field("table_id", "Table", "table", true),
                select_field(
                    "status",
                    "Status",
                    true,
                    &["draft", "sent_to_kitchen", "served", "paid", "cancelled"],
                ),
                amount_field("total_amount", "Total Amount", false, "CNY", 2),
                field("opened_at", "Opened At", "datetime", false),
                field("paid_at", "Paid At", "datetime", false),
            ],
        ),
        form_template(
            "order_line",
            "Order Line",
            "Order line item as order child table data.",
            "{sku_name}",
            vec![
                relation_field("order_id", "Order", "order", true),
                relation_field("sku_id", "SKU", "sku", true),
                field("sku_name", "SKU Name", "text", true),
                field("quantity", "Quantity", "integer", true),
                amount_field("unit_price", "Unit Price", true, "CNY", 2),
                amount_field("line_total", "Line Total", false, "CNY", 2),
                field("seat_no", "Seat", "text", false),
                select_field(
                    "status",
                    "Status",
                    false,
                    &["draft", "sent_to_kitchen", "served", "cancelled"],
                ),
            ],
        ),
        form_template(
            "print_job",
            "Print Job",
            "Kitchen ticket and cashier receipt printing task.",
            "{job_type}",
            vec![
                relation_field("order_id", "Order", "order", true),
                select_field("job_type", "Job Type", true, &["kitchen", "receipt"]),
                select_field("status", "Status", true, &["pending", "printing", "printed", "failed"]),
                field("printer", "Printer", "text", false),
                field("payload", "Payload", "textarea", false),
                field("retry_count", "Retry Count", "integer", false),
            ],
        ),
        form_template(
            "business_report",
            "Business Report",
            "Daily revenue, order count and printed receipt summary.",
            "{report_date}",
            vec![
                field("report_date", "Report Date", "date", true),
                amount_field("gross_revenue", "Gross Revenue", false, "CNY", 2),
                field("order_count", "Order Count", "integer", false),
                field("item_count", "Item Count", "integer", false),
                field("printed_receipts", "Printed Receipts", "integer", false),
            ],
        ),
    ]
}

fn form_template(key: &str, name: &str, description: &str, title_template: &str, fields: Vec<JsonValue>) -> JsonValue {
    let columns = fields
        .iter()
        .filter_map(|field| field.get("key").and_then(JsonValue::as_str))
        .map(ToOwned::to_owned)
        .take(8)
        .collect::<Vec<_>>();
    json!({
        "key": key,
        "name": name,
        "description": description,
        "title_template": title_template,
        "schema": {"version": "openpr.form.schema.v1", "fields": fields},
        "detail_layout": {"sections": [{"key": "main", "title": name, "fields": columns}]},
        "views": [
            {"key": "grid", "name": format!("All {name}"), "view_type": "grid", "config": {"columns": columns}},
            {"key": "detail", "name": format!("{name} Detail"), "view_type": "detail", "config": {"sections": [{"key": "main", "fields": columns}]}}
        ]
    })
}

fn field(key: &str, label: &str, field_type: &str, required: bool) -> JsonValue {
    json!({"key": key, "label": label, "type": field_type, "required": required})
}

fn select_field(key: &str, label: &str, required: bool, options: &[&str]) -> JsonValue {
    json!({"key": key, "label": label, "type": "single_select", "required": required, "options": options})
}

fn amount_field(key: &str, label: &str, required: bool, currency: &str, scale: u32) -> JsonValue {
    json!({
        "key": key,
        "label": label,
        "type": "amount",
        "required": required,
        "amount": {"currency": currency, "scale": scale, "allow_negative": false, "rounding": "half_up"}
    })
}

fn relation_field(key: &str, label: &str, form_key: &str, required: bool) -> JsonValue {
    json!({
        "key": key,
        "label": label,
        "type": "relation",
        "required": required,
        "relation": {"target_type": "form_record", "form_key": form_key}
    })
}

fn normalize_form_template_schema(schema: JsonValue) -> Result<JsonValue, ApiError> {
    let mut schema = ensure_schema_field_ids(schema).map_err(ApiError::BadRequest)?;
    let object = schema
        .as_object_mut()
        .ok_or_else(|| ApiError::BadRequest("form template schema must be a JSON object".to_string()))?;
    if let Some(fields) = object.get_mut("fields").and_then(JsonValue::as_array_mut) {
        for field in fields {
            if let Some(field_object) = field.as_object_mut()
                && field_object.get("type").and_then(JsonValue::as_str) == Some("select")
            {
                field_object.insert("type".to_string(), json!("single_select"));
            }
        }
    }
    validate_schema(&schema).map_err(ApiError::BadRequest)?;
    Ok(schema)
}

fn template_views(form: &JsonValue, schema: &JsonValue) -> Vec<JsonValue> {
    if let Some(views) = form
        .get("views")
        .and_then(JsonValue::as_array)
        .filter(|views| !views.is_empty())
    {
        return views.clone();
    }
    let columns = schema
        .get("fields")
        .and_then(JsonValue::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("key").and_then(JsonValue::as_str))
                .take(8)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    vec![
        json!({"key": "grid", "name": "Grid", "view_type": "grid", "config": {"columns": columns}}),
        json!({"key": "detail", "name": "Detail", "view_type": "detail", "config": {"sections": [{"key": "main", "fields": columns}]}}),
    ]
}

fn template_string(value: &JsonValue, key: &str, fallback: &str) -> Result<String, ApiError> {
    let raw = value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    if raw.is_empty() {
        return Err(ApiError::BadRequest(format!("form template {key} is required")));
    }
    Ok(raw.to_string())
}

fn template_optional_string(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn ensure_template_object(value: JsonValue, name: &str) -> Result<JsonValue, ApiError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{name} must be a JSON object")))
    }
}

fn constant_json_wasm(output: &JsonValue) -> Result<Vec<u8>, ApiError> {
    let output = serde_json::to_vec(output).map_err(|_| ApiError::Internal)?;
    let output_ptr = 1024_u32;
    let input_ptr = 2048_u32;
    let handle = (i64::from(output_ptr) << 32) | i64::try_from(output.len()).unwrap_or(i64::MAX);
    let type_section = wasm_vec(vec![
        wasm_func_type(&[0x7f], &[0x7f])?,
        wasm_func_type(&[0x7f, 0x7f], &[0x7e])?,
        wasm_func_type(&[], &[0x7f])?,
    ])?;
    let function_section = wasm_vec(vec![vec![0x02], vec![0x00], vec![0x01]])?;
    let memory_section = wasm_vec(vec![vec![0x00, 0x01]])?;
    let export_section = wasm_vec(vec![
        wasm_export("memory", 0x02, 0)?,
        wasm_export("openpr_plugin_abi_version", 0x00, 0)?,
        wasm_export("openpr_alloc", 0x00, 1)?,
        wasm_export("openpr_invoke", 0x00, 2)?,
    ])?;
    let code_section = wasm_vec(vec![
        wasm_body(vec![0x41, 0x01])?,
        wasm_body({
            let mut instructions = vec![0x41];
            instructions.extend(leb_u32(input_ptr));
            instructions
        })?,
        wasm_body({
            let mut instructions = vec![0x42];
            instructions.extend(leb_i64(handle));
            instructions
        })?,
    ])?;
    let data_section = wasm_vec(vec![{
        let mut segment = vec![0x00, 0x41];
        segment.extend(leb_u32(output_ptr));
        segment.push(0x0b);
        segment.extend(leb_u32(
            u32::try_from(output.len()).map_err(|_| ApiError::BadRequest("plugin output is too large".to_string()))?,
        ));
        segment.extend(output);
        segment
    }])?;

    let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    wasm.extend(wasm_section(1, type_section)?);
    wasm.extend(wasm_section(3, function_section)?);
    wasm.extend(wasm_section(5, memory_section)?);
    wasm.extend(wasm_section(7, export_section)?);
    wasm.extend(wasm_section(10, code_section)?);
    wasm.extend(wasm_section(11, data_section)?);
    Ok(wasm)
}

fn wasm_len(len: usize) -> Result<u32, ApiError> {
    u32::try_from(len).map_err(|_| ApiError::BadRequest("generated plugin module is too large".to_string()))
}

fn wasm_func_type(params: &[u8], results: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut bytes = vec![0x60];
    bytes.extend(leb_u32(wasm_len(params.len())?));
    bytes.extend(params);
    bytes.extend(leb_u32(wasm_len(results.len())?));
    bytes.extend(results);
    Ok(bytes)
}

fn wasm_export(name: &str, kind: u8, index: u32) -> Result<Vec<u8>, ApiError> {
    let mut bytes = wasm_str(name)?;
    bytes.push(kind);
    bytes.extend(leb_u32(index));
    Ok(bytes)
}

fn wasm_body(instructions: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    let mut body = vec![0x00];
    body.extend(instructions);
    body.push(0x0b);
    let mut bytes = leb_u32(wasm_len(body.len())?);
    bytes.extend(body);
    Ok(bytes)
}

fn wasm_section(id: u8, content: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    let mut bytes = vec![id];
    bytes.extend(leb_u32(wasm_len(content.len())?));
    bytes.extend(content);
    Ok(bytes)
}

fn wasm_vec(items: Vec<Vec<u8>>) -> Result<Vec<u8>, ApiError> {
    let mut bytes = leb_u32(wasm_len(items.len())?);
    for item in items {
        bytes.extend(item);
    }
    Ok(bytes)
}

fn wasm_str(value: &str) -> Result<Vec<u8>, ApiError> {
    let mut bytes = leb_u32(wasm_len(value.len())?);
    bytes.extend(value.as_bytes());
    Ok(bytes)
}

fn leb_u32(mut value: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if value == 0 {
            break;
        }
    }
    bytes
}

fn leb_i64(mut value: i64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        // Masking with 0x7f proves the encoded chunk is always within the u8 range 0..=127.
        #[allow(clippy::cast_sign_loss)]
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        let sign_bit_set = (byte & 0x40) != 0;
        let done = (value == 0 && !sign_bit_set) || (value == -1 && sign_bit_set);
        if !done {
            byte |= 0x80;
        }
        bytes.push(byte);
        if done {
            break;
        }
    }
    bytes
}

/// POST /`api/v1/workspaces/:workspace_id/projects` - Create a new project
pub async fn create_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.key.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("key and name are required".to_string()));
    }

    // Validate key format (uppercase letters only)
    if !req.key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return Err(ApiError::BadRequest(
            "key must contain only uppercase letters and digits".to_string(),
        ));
    }

    // Check workspace membership
    let _role = get_workspace_role(&state, workspace_id, user_id).await?;

    // Check if key already exists in workspace
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2",
            vec![workspace_id.into(), req.key.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "project key already exists in this workspace".to_string(),
        ));
    }

    let project_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let description = req.description.unwrap_or_default();
    let scenario_template = match req.scenario_template_key.as_deref() {
        Some(key) => Some(load_scenario_template(&state, workspace_id, &normalize_scenario_template_key(key)?).await?),
        None => None,
    };
    let type_key = match (req.type_key, scenario_template.as_ref()) {
        (Some(type_key), Some(template)) if type_key != template.project_type_key => {
            return Err(ApiError::BadRequest(
                "type_key must match scenario template project_type_key".to_string(),
            ));
        }
        (Some(type_key), _) => type_key,
        (None, Some(template)) => template.project_type_key.clone(),
        (None, None) => "code_project".to_string(),
    };
    ensure_project_type_exists(&state, &type_key).await?;
    let type_settings = merge_template_type_settings(req.type_settings, scenario_template.as_ref())?;

    let tx = state.db.begin().await?;
    let workflow_id = match scenario_template.as_ref() {
        Some(template) => create_template_workflow(&tx, workspace_id, &req.name, template).await?,
        None => None,
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects (id, workspace_id, key, name, description, type_key, type_settings, workflow_id, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            project_id.into(),
            workspace_id.into(),
            req.key.clone().into(),
            req.name.clone().into(),
            description.clone().into(),
            type_key.clone().into(),
            type_settings.clone().into(),
            workflow_id.into(),
            user_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;
    let project_event_id = insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: Some(project_id),
            event_type: "project.created".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project_id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": project_id,
                "workspace_id": workspace_id,
                "key": req.key,
                "name": req.name,
                "description": description,
                "type_key": type_key,
                "type_settings": type_settings,
                "scenario_template_key": scenario_template.as_ref().map(|template| template.key.clone())
            }),
            metadata: json!({
                "scenario_template_key": scenario_template.as_ref().map(|template| template.key.clone()),
                "scenario_template_initialized": scenario_template.is_some()
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    if let Some(template) = scenario_template.as_ref() {
        apply_scenario_template(
            &tx,
            workspace_id,
            project_id,
            &req.key,
            Some(user_id),
            user_id,
            template,
            Some(project_event_id),
        )
        .await?;
    }
    tx.commit().await?;

    decision_domain::initialize_default_domains_for_project(&state, project_id).await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectCreated,
            workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": project_id,
                    "workspace_id": workspace_id,
                    "key": req.key,
                    "name": req.name,
                    "description": description,
                    "type_key": type_key.clone(),
                    "type_settings": type_settings.clone(),
                    "scenario_template_key": scenario_template.as_ref().map(|template| template.key.clone()),
                    "created_at": now.to_rfc3339(),
                    "updated_at": now.to_rfc3339(),
                }
            })),
        },
    );

    Ok(ApiResponse::success(ProjectResponse {
        id: project_id,
        workspace_id,
        key: req.key,
        name: req.name,
        description,
        type_key,
        type_settings,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        issue_counts: None,
    }))
}

/// POST /api/v1/projects/:project_id/scenario-templates/:template_key/install
/// Install a complete multi-form scenario template into an existing project.
pub async fn install_project_scenario_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, template_key)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let project = ScenarioProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, key, name, description, type_key, type_settings,
                   workflow_id, created_by, created_at
            FROM projects
            WHERE id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let (actor_id, role, is_bot) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), project.workspace_id).await?;
    if role == "member" && !is_bot {
        return Err(ApiError::Forbidden(
            "only owners and admins can install scenario templates".to_string(),
        ));
    }

    let template = load_scenario_template(
        &state,
        project.workspace_id,
        &normalize_scenario_template_key(&template_key)?,
    )
    .await?;
    let event_actor_id = if is_bot { None } else { Some(actor_id) };
    let type_settings = merge_template_type_settings(Some(project.type_settings.clone()), Some(&template))?;
    let now = chrono::Utc::now();

    let form_keys = scenario_form_templates(&template)
        .into_iter()
        .filter_map(|form| template_string(&form, "key", "form").ok())
        .collect::<Vec<_>>();
    let connector_kinds = template
        .connector_suggestions
        .as_array()
        .map(|suggestions| {
            suggestions
                .iter()
                .filter_map(|suggestion| {
                    suggestion
                        .get("type")
                        .or_else(|| suggestion.get("kind"))
                        .and_then(JsonValue::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let plugin_keys = scenario_plugin_templates(&template)?
        .into_iter()
        .filter_map(|plugin| parse_manifest(&plugin.manifest).ok().map(|manifest| manifest.key))
        .collect::<Vec<_>>();

    let tx = state.db.begin().await?;
    let installed_workflow_id = if project.workflow_id.is_none() {
        create_template_workflow(&tx, project.workspace_id, &project.name, &template).await?
    } else {
        None
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE projects
            SET type_settings = $2,
                workflow_id = COALESCE(workflow_id, $3),
                updated_at = $4
            WHERE id = $1
        ",
        vec![
            project.id.into(),
            type_settings.clone().into(),
            installed_workflow_id.into(),
            now.into(),
        ],
    ))
    .await?;

    let install_event_id = insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: Some(project.id),
            event_type: "project.scenario_template.installed".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project.id.to_string(),
            actor_id: event_actor_id,
            source: json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
            payload: json!({
                "project_id": project.id,
                "workspace_id": project.workspace_id,
                "project_key": project.key.clone(),
                "scenario_template_key": template.key.clone(),
                "scenario_template_name": template.name.clone(),
                "workflow_installed": installed_workflow_id.is_some(),
                "form_keys": form_keys.clone(),
                "connector_kinds": connector_kinds.clone(),
                "plugin_keys": plugin_keys.clone()
            }),
            metadata: json!({
                "project_id": project.id,
                "project_key": project.key.clone(),
                "scenario_template_key": template.key.clone(),
                "scenario_template_initialized": true,
                "install_mode": "existing_project"
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    apply_scenario_template(
        &tx,
        project.workspace_id,
        project.id,
        &project.key,
        event_actor_id,
        project.created_by,
        &template,
        Some(install_event_id),
    )
    .await?;
    tx.commit().await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectUpdated,
            workspace_id: project.workspace_id,
            project_id: project.id,
            actor_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(json!({
                "project": {
                    "id": project.id,
                    "workspace_id": project.workspace_id,
                    "key": project.key.clone(),
                    "name": project.name.clone(),
                    "description": project.description.clone(),
                    "type_key": project.type_key.clone(),
                    "type_settings": type_settings.clone(),
                    "created_at": project.created_at.to_rfc3339(),
                    "updated_at": now.to_rfc3339(),
                },
                "scenario_template": {
                    "key": template.key.clone(),
                    "name": template.name.clone(),
                    "installed_into_existing_project": true
                }
            })),
        },
    );

    let project_response = ProjectResponse {
        id: project.id,
        workspace_id: project.workspace_id,
        key: project.key,
        name: project.name,
        description: project.description,
        type_key: project.type_key.unwrap_or_else(|| "code_project".to_string()),
        type_settings,
        created_at: project.created_at.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        issue_counts: None,
    };

    Ok(ApiResponse::success(json!({
        "project": project_response,
        "scenario_template": {
            "key": template.key,
            "name": template.name,
            "industry": template.industry,
            "project_type_key": template.project_type_key,
            "audience_label": template.audience_label
        },
        "workflow_installed": installed_workflow_id.is_some(),
        "installed": {
            "form_keys": form_keys,
            "connector_kinds": connector_kinds,
            "plugin_keys": plugin_keys
        }
    })))
}

/// GET /`api/v1/workspaces/:workspace_id/projects` - List projects in workspace
pub async fn list_projects(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    // Check workspace membership
    require_workspace_access(&state, &extensions, workspace_id).await?;

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let projects = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, key, name, description, type_key, type_settings, created_at, updated_at FROM projects WHERE workspace_id = $1 ORDER BY created_at DESC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    #[derive(Debug, FromQueryResult)]
    struct IssueCountRow {
        project_id: Uuid,
        state: String,
        count: i64,
    }

    let project_ids: Vec<Uuid> = projects.iter().map(|project| project.id).collect();
    let mut issue_counts_by_project: HashMap<Uuid, IssueCounts> = HashMap::new();

    if !project_ids.is_empty() {
        let issue_counts = IssueCountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id, state, COUNT(*) as count FROM work_items WHERE project_id = ANY($1) GROUP BY project_id, state",
            vec![project_ids.into()],
        ))
        .all(&state.db)
        .await?;

        for row in issue_counts {
            let entry = issue_counts_by_project.entry(row.project_id).or_default();
            *entry.by_state.entry(row.state).or_insert(0) += row.count;
            entry.total += row.count;
        }
    }

    let response: Vec<ProjectResponse> = projects
        .into_iter()
        .map(|p| ProjectResponse {
            id: p.id,
            workspace_id: p.workspace_id,
            key: p.key,
            name: p.name,
            description: p.description,
            type_key: p.type_key.unwrap_or_else(|| "code_project".to_string()),
            type_settings: p.type_settings,
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
            issue_counts: Some(issue_counts_by_project.get(&p.id).cloned().unwrap_or_default()),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// GET /api/v1/projects/:id - Get project details
pub async fn get_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.id, p.workspace_id, p.key, p.name, p.description, p.type_key, p.type_settings, p.created_at, p.updated_at
            FROM projects p
            WHERE p.id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    Ok(ApiResponse::success(ProjectResponse {
        id: project.id,
        workspace_id: project.workspace_id,
        key: project.key,
        name: project.name,
        description: project.description,
        type_key: project.type_key.unwrap_or_else(|| "code_project".to_string()),
        type_settings: project.type_settings,
        created_at: project.created_at.to_rfc3339(),
        updated_at: project.updated_at.to_rfc3339(),
        issue_counts: None,
    }))
}

/// PUT /api/v1/projects/:id - Update project
pub async fn update_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get project's workspace and check permission
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct WorkspaceIdRow {
        workspace_id: Uuid,
    }

    let project = WorkspaceIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let role = get_workspace_role(&state, project.workspace_id, user_id).await?;
    if role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can update projects".to_string(),
        ));
    }

    // Validate key if provided
    if let Some(ref key) = req.key {
        if !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Err(ApiError::BadRequest(
                "key must contain only uppercase letters and digits".to_string(),
            ));
        }

        // Check key uniqueness in workspace
        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2 AND id != $3",
                vec![project.workspace_id.into(), key.clone().into(), project_id.into()],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict("key already exists in this workspace".to_string()));
        }
    }

    // Build update query dynamically
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${param_idx}"));
        values.push(name.into());
        param_idx += 1;
    }

    if let Some(key) = req.key {
        updates.push(format!("key = ${param_idx}"));
        values.push(key.into());
        param_idx += 1;
    }

    if let Some(description) = req.description {
        updates.push(format!("description = ${param_idx}"));
        values.push(description.into());
        param_idx += 1;
    }

    if let Some(type_key) = req.type_key {
        ensure_project_type_exists(&state, &type_key).await?;
        updates.push(format!("type_key = ${param_idx}"));
        values.push(type_key.into());
        param_idx += 1;
    }

    if let Some(type_settings) = req.type_settings {
        updates.push(format!("type_settings = ${param_idx}"));
        values.push(type_settings.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${param_idx}"));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(project_id.into());

    let query = format!("UPDATE projects SET {} WHERE id = ${}", updates.join(", "), param_idx);

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;

    // Fetch updated project
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, key, name, description, type_key, type_settings, created_at, updated_at FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::Internal)?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: updated.workspace_id,
            project_id: Some(updated.id),
            event_type: "project.updated".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: updated.id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": updated.id,
                "workspace_id": updated.workspace_id,
                "key": updated.key,
                "name": updated.name,
                "description": updated.description,
                "type_key": updated.type_key.clone(),
                "type_settings": updated.type_settings.clone(),
                "updated_at": updated.updated_at.to_rfc3339()
            }),
            metadata: json!({
                "project_id": updated.id,
                "project_key": updated.key
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectUpdated,
            workspace_id: updated.workspace_id,
            project_id: updated.id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": updated.id,
                    "workspace_id": updated.workspace_id,
                    "key": updated.key,
                    "name": updated.name,
                    "description": updated.description,
                    "type_key": updated.type_key.clone(),
                    "type_settings": updated.type_settings.clone(),
                    "created_at": updated.created_at.to_rfc3339(),
                    "updated_at": updated.updated_at.to_rfc3339(),
                }
            })),
        },
    );

    Ok(ApiResponse::success(ProjectResponse {
        id: updated.id,
        workspace_id: updated.workspace_id,
        key: updated.key,
        name: updated.name,
        description: updated.description,
        type_key: updated.type_key.unwrap_or_else(|| "code_project".to_string()),
        type_settings: updated.type_settings,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
        issue_counts: None,
    }))
}

/// DELETE /api/v1/projects/:id - Delete project
pub async fn delete_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get project's workspace and check permission
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectDeleteRow {
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
    }

    let project = ProjectDeleteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, key, name, description, type_key, type_settings FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let role = get_workspace_role(&state, project.workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete projects".to_string(),
        ));
    }

    let tx = state.db.begin().await?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: None,
            event_type: "project.deleted".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project_id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": project_id,
                "workspace_id": project.workspace_id,
                "key": project.key.clone(),
                "name": project.name.clone(),
                "description": project.description.clone(),
                "type_key": project.type_key.clone(),
                "type_settings": project.type_settings.clone(),
                "status": "deleted"
            }),
            metadata: json!({
                "project_id": project_id,
                "project_key": project.key
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .await?;
    tx.commit().await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectDeleted,
            workspace_id: project.workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": project_id,
                    "workspace_id": project.workspace_id,
                    "status": "deleted"
                }
            })),
        },
    );

    Ok(ApiResponse::ok())
}

/// Helper: Get user's role in workspace
async fn get_workspace_role(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok(row.role)
}

#[cfg(test)]
mod tests {
    use super::{
        ScenarioTemplateRow, default_form_templates, merge_template_type_settings, normalize_form_template_schema,
        normalize_scenario_template_key, scenario_form_templates, scenario_plugin_templates,
    };
    use crate::plugins::{manifest::parse_manifest, runtime::validate_wasm_module};
    use serde_json::{Value as JsonValue, json};

    fn template_row() -> ScenarioTemplateRow {
        ScenarioTemplateRow {
            key: "contract_review_default".to_string(),
            name: "Contract Review".to_string(),
            industry: "legal".to_string(),
            project_type_key: "contract_review".to_string(),
            audience_label: "Document review assistant".to_string(),
            workflow_template: json!({"default_labels":["legal-risk"]}),
            field_schema: json!({"fields":[]}),
            resource_schema: json!({"resources":[]}),
            ai_roles: json!([]),
            governance_policy: json!({"risk_model":"financial_legal_compliance"}),
            connector_suggestions: json!([]),
            sample_data: json!({"example_project_key":"LEGAL"}),
        }
    }

    #[test]
    fn normalizes_scenario_template_key() {
        assert_eq!(
            normalize_scenario_template_key(" Contract_Review_Default ").unwrap(),
            "contract_review_default"
        );
        assert!(normalize_scenario_template_key("contract-review").is_err());
        assert!(normalize_scenario_template_key("").is_err());
    }

    #[test]
    fn merge_template_settings_preserves_existing_values() {
        let merged = merge_template_type_settings(Some(json!({"owner":"legal"})), Some(&template_row())).unwrap();

        assert_eq!(merged.get("owner").and_then(JsonValue::as_str), Some("legal"));
        assert_eq!(
            merged.get("scenario_template_key").and_then(JsonValue::as_str),
            Some("contract_review_default")
        );
        assert_eq!(
            merged
                .get("scenario_template")
                .and_then(|template| template.get("project_type_key"))
                .and_then(JsonValue::as_str),
            Some("contract_review")
        );
        assert_eq!(
            merged
                .get("scenario_template")
                .and_then(|template| template.get("governance_policy"))
                .and_then(|policy| policy.get("risk_model"))
                .and_then(JsonValue::as_str),
            Some("financial_legal_compliance")
        );
    }

    #[test]
    fn merge_template_settings_rejects_non_object() {
        assert!(merge_template_type_settings(Some(json!([])), Some(&template_row())).is_err());
    }

    #[test]
    fn default_contract_template_generates_three_valid_forms() {
        let forms = default_form_templates("contract_review_default");
        assert_eq!(forms.len(), 3);
        assert_eq!(
            forms
                .first()
                .and_then(|form| form.get("key"))
                .and_then(JsonValue::as_str),
            Some("contract")
        );
        for form in forms {
            let schema = form.get("schema").cloned().unwrap();
            assert!(normalize_form_template_schema(schema).is_ok());
            assert!(
                form.get("views")
                    .and_then(JsonValue::as_array)
                    .is_some_and(|views| views.len() >= 2)
            );
        }
    }

    #[test]
    fn restaurant_template_generates_required_business_forms() {
        let forms = default_form_templates("restaurant_ordering_default");
        let keys = forms
            .iter()
            .filter_map(|form| form.get("key").and_then(JsonValue::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "menu_category",
                "sku",
                "table",
                "order",
                "order_line",
                "print_job",
                "business_report"
            ]
        );
        for form in forms {
            assert!(normalize_form_template_schema(form.get("schema").cloned().unwrap()).is_ok());
        }
    }

    #[test]
    fn scenario_forms_prefer_sample_data_forms() {
        let mut template = template_row();
        template.sample_data = json!({
            "forms": [{
                "key": "custom_contract",
                "name": "Custom Contract",
                "schema": {"fields": [{"key": "status", "type": "select", "options": ["open"]}]}
            }]
        });

        let forms = scenario_form_templates(&template);
        assert_eq!(forms.len(), 1);
        let form = forms.first().expect("sample data form should exist");
        assert_eq!(form.get("key").and_then(JsonValue::as_str), Some("custom_contract"));
        let schema = normalize_form_template_schema(form.get("schema").cloned().unwrap()).unwrap();
        assert_eq!(
            schema
                .get("fields")
                .and_then(JsonValue::as_array)
                .and_then(|fields| fields.first())
                .and_then(|field| field.get("type"))
                .and_then(JsonValue::as_str),
            Some("single_select")
        );
    }

    #[test]
    fn restaurant_template_generates_active_wasm_plugin() {
        let mut template = template_row();
        template.key = "restaurant_ordering_default".to_string();

        let plugins = scenario_plugin_templates(&template).expect("restaurant plugins should build");
        assert_eq!(plugins.len(), 1);
        let plugin = plugins.first().expect("plugin should exist");
        assert_eq!(plugin.status, "active");
        validate_wasm_module(&plugin.wasm_bytes).expect("default plugin wasm should validate");
        let manifest = parse_manifest(&plugin.manifest).expect("default plugin manifest should parse");
        assert_eq!(manifest.key, "restaurant_calc");
        assert!(
            manifest
                .capabilities
                .hooks
                .iter()
                .any(|hook| hook.kind == "formula" && hook.form_key.as_deref() == Some("order_line"))
        );
    }

    #[test]
    fn scenario_plugins_prefer_sample_data_plugins() {
        let mut template = template_row();
        template.sample_data = json!({
            "plugins": [{
                "status": "disabled",
                "manifest": {
                    "schema_version": "openpr.plugin.v1",
                    "key": "custom_calc",
                    "name": "Custom Calc",
                    "version": "0.1.0",
                    "capabilities": {
                        "hooks": [{"kind": "formula", "form_key": "custom_contract", "field_key": "total"}]
                    }
                },
                "wasm_output": {"patch": {"total": "1.00"}}
            }]
        });

        let plugins = scenario_plugin_templates(&template).expect("sample plugins should build");
        assert_eq!(plugins.len(), 1);
        let plugin = plugins.first().expect("sample plugin should exist");
        assert_eq!(plugin.status, "disabled");
        validate_wasm_module(&plugin.wasm_bytes).expect("sample plugin wasm should validate");
        let manifest = parse_manifest(&plugin.manifest).expect("sample plugin manifest should parse");
        assert_eq!(manifest.key, "custom_calc");
    }
}
