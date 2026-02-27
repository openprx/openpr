use axum::{
    Extension, Json,
    extract::{Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::{
        governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
        trust_score_service::{is_project_admin_or_owner, is_project_member, is_system_admin},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct GovernanceConfigQuery {
    pub project_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGovernanceConfigRequest {
    pub project_id: Uuid,
    pub review_required: Option<bool>,
    pub auto_review_days: Option<i32>,
    pub review_reminder_days: Option<i32>,
    pub audit_report_cron: Option<String>,
    pub trust_update_mode: Option<String>,
    pub config: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListGovernanceAuditLogsQuery {
    pub project_id: Option<Uuid>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub actor_id: Option<Uuid>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct GovernanceConfigRow {
    pub id: i64,
    pub project_id: Uuid,
    pub review_required: bool,
    pub auto_review_days: i32,
    pub review_reminder_days: i32,
    pub audit_report_cron: String,
    pub trust_update_mode: String,
    pub config: Value,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct GovernanceAuditLogRow {
    pub id: i64,
    pub project_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

pub async fn get_governance_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<GovernanceConfigQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_project_member_or_admin(&state, query.project_id, user_id).await?;

    let config = load_or_init_config(&state, query.project_id, Some(user_id), true).await?;
    Ok(ApiResponse::success(config))
}

pub async fn update_governance_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<UpdateGovernanceConfigRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_claim_user_id(&claims)?;
    ensure_project_admin_or_owner_or_system_admin(&state, req.project_id, actor_id).await?;
    if let Some(value) = req.auto_review_days
        && value < 0
    {
        return Err(ApiError::BadRequest(
            "auto_review_days must be >= 0".to_string(),
        ));
    }
    if let Some(value) = req.review_reminder_days
        && value < 0
    {
        return Err(ApiError::BadRequest(
            "review_reminder_days must be >= 0".to_string(),
        ));
    }
    if let Some(mode) = req.trust_update_mode.as_deref() {
        let mode = mode.trim();
        if mode.is_empty() || mode.len() > 30 {
            return Err(ApiError::BadRequest(
                "invalid trust_update_mode".to_string(),
            ));
        }
    }
    if let Some(cron) = req.audit_report_cron.as_deref() {
        let cron = cron.trim();
        if cron.is_empty() || cron.len() > 100 {
            return Err(ApiError::BadRequest(
                "invalid audit_report_cron".to_string(),
            ));
        }
    }
    if let Some(config) = req.config.as_ref()
        && !config.is_object()
    {
        return Err(ApiError::BadRequest(
            "config must be a JSON object".to_string(),
        ));
    }

    let old_row = load_or_init_config(&state, req.project_id, None, false).await?;
    let old_value = serde_json::to_value(&old_row).map_err(|_| ApiError::Internal)?;
    let review_required_changed = req.review_required.is_some();
    let auto_review_days_changed = req.auto_review_days.is_some();
    let review_reminder_days_changed = req.review_reminder_days.is_some();
    let audit_report_cron_changed = req.audit_report_cron.is_some();
    let trust_update_mode_changed = req.trust_update_mode.is_some();
    let config_changed = req.config.is_some();

    let new_review_required = req.review_required.unwrap_or(old_row.review_required);
    let new_auto_review_days = req.auto_review_days.unwrap_or(old_row.auto_review_days);
    let new_review_reminder_days = req
        .review_reminder_days
        .unwrap_or(old_row.review_reminder_days);
    let new_audit_report_cron = req
        .audit_report_cron
        .as_deref()
        .unwrap_or(old_row.audit_report_cron.as_str())
        .trim()
        .to_string();
    let new_trust_update_mode = req
        .trust_update_mode
        .as_deref()
        .unwrap_or(old_row.trust_update_mode.as_str())
        .trim()
        .to_string();
    let new_config = req.config.unwrap_or(old_row.config.clone());
    let now = Utc::now();

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
                INSERT INTO governance_configs (
                    project_id,
                    review_required,
                    auto_review_days,
                    review_reminder_days,
                    audit_report_cron,
                    trust_update_mode,
                    config,
                    updated_by,
                    created_at,
                    updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (project_id)
                DO UPDATE SET
                    review_required = EXCLUDED.review_required,
                    auto_review_days = EXCLUDED.auto_review_days,
                    review_reminder_days = EXCLUDED.review_reminder_days,
                    audit_report_cron = EXCLUDED.audit_report_cron,
                    trust_update_mode = EXCLUDED.trust_update_mode,
                    config = EXCLUDED.config,
                    updated_by = EXCLUDED.updated_by,
                    updated_at = EXCLUDED.updated_at
            "#,
        vec![
            req.project_id.into(),
            new_review_required.into(),
            new_auto_review_days.into(),
            new_review_reminder_days.into(),
            new_audit_report_cron.into(),
            new_trust_update_mode.into(),
            new_config.clone().into(),
            Some(actor_id).into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;

    let updated = load_or_init_config_by_conn(&tx, req.project_id).await?;
    let new_value = serde_json::to_value(&updated).map_err(|_| ApiError::Internal)?;

    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id: req.project_id,
            actor_id: Some(actor_id),
            action: "governance.config.updated".to_string(),
            resource_type: "governance_config".to_string(),
            resource_id: Some(req.project_id.to_string()),
            old_value: Some(old_value),
            new_value: Some(new_value),
            metadata: Some(json!({
                "source": "api",
                "updated_fields": {
                    "review_required": review_required_changed,
                    "auto_review_days": auto_review_days_changed,
                    "review_reminder_days": review_reminder_days_changed,
                    "audit_report_cron": audit_report_cron_changed,
                    "trust_update_mode": trust_update_mode_changed,
                    "config": config_changed,
                }
            })),
        },
    )
    .await?;
    tx.commit().await?;

    if let Some(workspace_id) = resolve_workspace_id_for_project(&state, req.project_id).await? {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::GovernanceConfigUpdated,
                workspace_id,
                project_id: req.project_id,
                actor_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "governance_config": {
                        "project_id": updated.project_id,
                        "review_required": updated.review_required,
                        "auto_review_days": updated.auto_review_days,
                        "review_reminder_days": updated.review_reminder_days,
                        "audit_report_cron": updated.audit_report_cron,
                        "trust_update_mode": updated.trust_update_mode,
                        "config": updated.config,
                        "updated_by": updated.updated_by,
                        "updated_at": updated.updated_at.to_rfc3339(),
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::success(updated))
}

pub async fn list_governance_audit_logs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListGovernanceAuditLogsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;

    if let Some(project_id) = query.project_id {
        let allowed = is_project_member(&state.db, project_id, user_id).await?
            || is_system_admin(&state.db, user_id).await?;
        if !allowed {
            return Err(ApiError::Forbidden("project access denied".to_string()));
        }
    } else if !is_system_admin(&state.db, user_id).await? {
        return Err(ApiError::Forbidden(
            "admin access required for global audit logs".to_string(),
        ));
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let mut where_parts: Vec<String> = vec![];
    let mut values: Vec<sea_orm::Value> = vec![];
    let mut idx = 1;

    if let Some(project_id) = query.project_id {
        where_parts.push(format!("project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(action) = query.action {
        where_parts.push(format!("action = ${idx}"));
        values.push(action.into());
        idx += 1;
    }
    if let Some(resource_type) = query.resource_type {
        where_parts.push(format!("resource_type = ${idx}"));
        values.push(resource_type.into());
        idx += 1;
    }
    if let Some(actor_id) = query.actor_id {
        where_parts.push(format!("actor_id = ${idx}"));
        values.push(actor_id.into());
        idx += 1;
    }
    if let Some(start_at) = query.start_at {
        where_parts.push(format!("created_at >= ${idx}"));
        values.push(start_at.into());
        idx += 1;
    }
    if let Some(end_at) = query.end_at {
        where_parts.push(format!("created_at <= ${idx}"));
        values.push(end_at.into());
        idx += 1;
    }

    let where_sql = if where_parts.is_empty() {
        "".to_string()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let count_sql =
        format!("SELECT COUNT(*)::bigint AS count FROM governance_audit_logs {where_sql}");
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        values.clone(),
    ))
    .one(&state.db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    let mut list_values = values;
    list_values.push(per_page.into());
    list_values.push(offset.into());
    let list_sql = format!(
        r#"
            SELECT id, project_id, actor_id, action, resource_type, resource_id,
                   old_value, new_value, metadata, created_at
            FROM governance_audit_logs
            {where_sql}
            ORDER BY created_at DESC, id DESC
            LIMIT ${idx} OFFSET ${}
        "#,
        idx + 1
    );

    let items = GovernanceAuditLogRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?;

    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

async fn load_or_init_config(
    state: &AppState,
    project_id: Uuid,
    actor_id: Option<Uuid>,
    audit_on_init: bool,
) -> Result<GovernanceConfigRow, ApiError> {
    let tx = state.db.begin().await?;

    let project_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM projects WHERE id = $1",
            vec![project_id.into()],
        ))
        .await?
        .is_some();
    if !project_exists {
        return Err(ApiError::NotFound("project not found".to_string()));
    }

    let insert_result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO governance_configs (
                    project_id,
                    review_required,
                    auto_review_days,
                    review_reminder_days,
                    audit_report_cron,
                    trust_update_mode,
                    config,
                    created_at,
                    updated_at
                ) VALUES ($1, true, 30, 7, '0 0 1 * *', 'review_based', '{}'::jsonb, $2, $2)
                ON CONFLICT (project_id) DO NOTHING
            "#,
            vec![project_id.into(), Utc::now().into()],
        ))
        .await?;

    let config = load_or_init_config_by_conn(&tx, project_id).await?;
    if audit_on_init && insert_result.rows_affected() > 0 {
        write_governance_audit_log(
            &tx,
            GovernanceAuditLogInput {
                project_id,
                actor_id,
                action: "governance.config.initialized".to_string(),
                resource_type: "governance_config".to_string(),
                resource_id: Some(project_id.to_string()),
                old_value: None,
                new_value: Some(serde_json::to_value(&config).map_err(|_| ApiError::Internal)?),
                metadata: Some(json!({
                    "source": "api.get",
                    "auto_initialized": true
                })),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(config)
}

async fn load_or_init_config_by_conn<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
) -> Result<GovernanceConfigRow, ApiError> {
    GovernanceConfigRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, review_required, auto_review_days, review_reminder_days,
                   audit_report_cron, trust_update_mode, config, updated_by, created_at, updated_at
            FROM governance_configs
            WHERE project_id = $1
        "#,
        vec![project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)
}

fn parse_claim_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

async fn ensure_project_member_or_admin(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_member(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}

async fn ensure_project_admin_or_owner_or_system_admin(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_admin_or_owner(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden(
            "admin or owner required for governance config update".to_string(),
        ));
    }
    Ok(())
}

async fn resolve_workspace_id_for_project(
    state: &AppState,
    project_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct WorkspaceRow {
        workspace_id: Uuid,
    }

    let row = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?;

    Ok(row.map(|v| v.workspace_id))
}
