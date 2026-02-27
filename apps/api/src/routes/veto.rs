use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TryGetable};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::{
        governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
        trust_score_service::{is_project_admin_or_owner, is_project_member, normalize_domain_key},
        veto_service::VetoService,
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct ExerciseVetoRequest {
    pub domain: Option<String>,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct EscalationVoteRequest {
    pub overturn: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateVetoerRequest {
    pub user_id: Uuid,
    pub project_id: Uuid,
    pub domain: String,
    pub granted_by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteVetoerRequest {
    pub user_id: Uuid,
    pub project_id: Uuid,
    pub domain: String,
}

#[derive(Debug, Deserialize)]
pub struct ListVetoersQuery {
    pub project_id: Option<Uuid>,
    pub domain: Option<String>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct VetoerRow {
    pub id: i64,
    pub user_id: Uuid,
    pub project_id: Uuid,
    pub domain: String,
    pub granted_by: String,
    pub granted_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct VetoEventViewRow {
    pub id: i64,
    pub proposal_id: String,
    pub vetoer_id: Uuid,
    pub domain: String,
    pub reason: String,
    pub status: String,
    pub escalation_started_at: Option<DateTime<Utc>>,
    pub escalation_result: Option<String>,
    pub escalation_votes: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub async fn exercise_veto(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<ExerciseVetoRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let author_type = fetch_actor_author_type(&state, user_id).await?;

    let service = VetoService::new(state.db.clone());
    let event = service
        .exercise_veto(
            &id,
            user_id,
            &req.reason,
            req.domain.as_deref(),
            author_type,
        )
        .await?;

    write_veto_audit_log(
        &state,
        &id,
        user_id,
        "veto.exercised",
        Some(event.id.to_string()),
        None,
        Some(serde_json::json!({
            "proposal_id": event.proposal_id,
            "domain": event.domain,
            "reason": event.reason,
            "status": event.status,
        })),
    )
    .await?;

    if let Some(project_id) = resolve_project_for_proposal(&state, &id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::VetoExercised,
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
                    "proposal_id": event.proposal_id,
                    "veto": {
                        "id": event.id,
                        "domain": event.domain,
                        "reason": event.reason,
                        "status": event.status,
                        "created_at": event.created_at.to_rfc3339(),
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::success(serde_json::json!({
        "veto_id": event.id,
        "proposal_id": event.proposal_id,
        "proposal_status": "vetoed",
        "domain": event.domain,
        "status": event.status,
        "created_at": event.created_at.to_rfc3339(),
        "next_steps": [
            "发起人可修改提案后重新提交",
            "或在 48h 内申请 Escalation（发起人权限）",
            "或接受否决并归档"
        ]
    })))
}

pub async fn get_veto(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let proposal_project = resolve_project_for_proposal(&state, &id).await?;
    let Some(project_id) = proposal_project else {
        return Err(ApiError::NotFound("proposal not found".to_string()));
    };
    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let row = VetoEventViewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, vetoer_id, domain, reason,
                   status::text AS status, escalation_started_at,
                   escalation_result, escalation_votes, created_at
            FROM veto_events
            WHERE proposal_id = $1
            ORDER BY created_at DESC
            LIMIT 1
        "#,
        vec![id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("veto event not found".to_string()))?;

    Ok(ApiResponse::success(row))
}

pub async fn start_escalation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let service = VetoService::new(state.db.clone());
    let event = service.start_escalation(&id, user_id).await?;
    write_veto_audit_log(
        &state,
        &id,
        user_id,
        "veto.escalation_started",
        Some(event.id.to_string()),
        None,
        Some(serde_json::json!({
            "proposal_id": event.proposal_id,
            "status": event.status,
            "escalation_started_at": event.escalation_started_at.map(|v| v.to_rfc3339()),
        })),
    )
    .await?;

    if let Some(project_id) = resolve_project_for_proposal(&state, &id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::EscalationStarted,
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
                    "proposal_id": event.proposal_id,
                    "veto": {
                        "id": event.id,
                        "status": event.status,
                        "escalation_started_at": event.escalation_started_at.map(|v| v.to_rfc3339()),
                    }
                })),
            },
        );
    }
    Ok(ApiResponse::success(event))
}

pub async fn withdraw_veto(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let service = VetoService::new(state.db.clone());
    let event = service.withdraw_veto(&id, user_id).await?;
    write_veto_audit_log(
        &state,
        &id,
        user_id,
        "veto.withdrawn",
        Some(event.id.to_string()),
        Some(serde_json::json!({
            "proposal_id": event.proposal_id,
            "status": "active",
        })),
        Some(serde_json::json!({
            "proposal_id": event.proposal_id,
            "status": event.status,
        })),
    )
    .await?;

    if let Some(project_id) = resolve_project_for_proposal(&state, &id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::VetoWithdrawn,
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
                    "proposal_id": event.proposal_id,
                    "veto": {
                        "id": event.id,
                        "status": event.status,
                    }
                })),
            },
        );
    }
    Ok(ApiResponse::success(event))
}

pub async fn vote_escalation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<EscalationVoteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let service = VetoService::new(state.db.clone());
    let event = service
        .cast_escalation_vote(&id, user_id, req.overturn)
        .await?;
    write_veto_audit_log(
        &state,
        &id,
        user_id,
        "veto.escalation_voted",
        Some(event.id.to_string()),
        None,
        Some(serde_json::json!({
            "proposal_id": event.proposal_id,
            "overturn": req.overturn,
            "status": event.status,
            "escalation_result": event.escalation_result,
        })),
    )
    .await?;
    Ok(ApiResponse::success(event))
}

pub async fn list_vetoers(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListVetoersQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let mut values: Vec<sea_orm::Value> = vec![user_id.into()];
    let mut where_parts = vec![String::from(
        r#"EXISTS (
            SELECT 1
            FROM projects p
            WHERE p.id = v.project_id
              AND (
                EXISTS (
                    SELECT 1
                    FROM workspace_members wm
                    WHERE wm.workspace_id = p.workspace_id
                      AND wm.user_id = $1
                )
                OR EXISTS (
                    SELECT 1
                    FROM workspace_bots wb
                    WHERE wb.workspace_id = p.workspace_id
                      AND wb.id = $1
                )
              )
        )"#,
    )];
    let mut idx = 2;

    if let Some(project_id) = query.project_id {
        if !is_project_member(&state.db, project_id, user_id).await? {
            return Err(ApiError::Forbidden("project access denied".to_string()));
        }
        where_parts.push(format!("v.project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }

    if let Some(domain) = query.domain {
        let normalized = normalize_domain_key(&domain);
        if normalized.is_empty() {
            return Err(ApiError::BadRequest("invalid domain".to_string()));
        }
        where_parts.push(format!("v.domain = ${idx}"));
        values.push(normalized.into());
    }

    let sql = format!(
        r#"
            SELECT v.id, v.user_id, v.project_id, v.domain, v.granted_by, v.granted_at
            FROM vetoers v
            WHERE {}
            ORDER BY v.granted_at DESC
        "#,
        where_parts.join(" AND ")
    );

    let items = VetoerRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn create_vetoer(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<CreateVetoerRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let requester_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, req.project_id, requester_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let domain = normalize_domain_key(&req.domain);
    if domain.is_empty() {
        return Err(ApiError::BadRequest("domain is required".to_string()));
    }

    let grant_mode = req
        .granted_by
        .unwrap_or_else(|| "manual_grant".to_string())
        .to_lowercase();
    if !matches!(grant_mode.as_str(), "manual_grant" | "trust_score") {
        return Err(ApiError::BadRequest(
            "granted_by must be manual_grant or trust_score".to_string(),
        ));
    }

    let insert = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO vetoers (user_id, project_id, domain, granted_by, granted_at)
                VALUES ($1, $2, $3, $4, $5)
            "#,
            vec![
                req.user_id.into(),
                req.project_id.into(),
                domain.into(),
                grant_mode.into(),
                Utc::now().into(),
            ],
        ))
        .await;

    if let Err(err) = insert {
        let message = err.to_string();
        if message.contains("uq_vetoers") || message.contains("duplicate key value") {
            return Err(ApiError::Conflict("vetoer already exists".to_string()));
        }
        return Err(ApiError::Database(err));
    }

    Ok(ApiResponse::ok())
}

pub async fn delete_vetoer(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<DeleteVetoerRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let requester_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, req.project_id, requester_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let domain = normalize_domain_key(&req.domain);
    if domain.is_empty() {
        return Err(ApiError::BadRequest("domain is required".to_string()));
    }

    let deleted = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM vetoers WHERE user_id = $1 AND project_id = $2 AND domain = $3",
            vec![req.user_id.into(), req.project_id.into(), domain.into()],
        ))
        .await?;

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound("vetoer not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

async fn resolve_project_for_proposal(
    state: &AppState,
    proposal_id: &str,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ProjectRow {
        project_id: Uuid,
    }

    let by_issue = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON wi.id = pil.issue_id
            WHERE pil.proposal_id = $1
            LIMIT 1
        "#,
        vec![proposal_id.to_string().into()],
    ))
    .one(&state.db)
    .await?;

    if let Some(row) = by_issue {
        return Ok(Some(row.project_id));
    }

    let author = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT author_id FROM proposals WHERE id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .await?;

    let Some(author) = author else {
        return Ok(None);
    };
    let author_id: String = author
        .try_get("", "author_id")
        .map_err(|_| ApiError::Internal)?;
    let Ok(author_uuid) = Uuid::parse_str(&author_id) else {
        return Ok(None);
    };

    let fallback = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.id AS project_id
            FROM projects p
            WHERE EXISTS (
                      SELECT 1
                      FROM workspace_members wm
                      WHERE wm.workspace_id = p.workspace_id
                        AND wm.user_id = $1
                  )
               OR EXISTS (
                      SELECT 1
                      FROM workspace_bots wb
                      WHERE wb.workspace_id = p.workspace_id
                        AND wb.id = $1
                  )
            ORDER BY p.created_at DESC
            LIMIT 2
        "#,
        vec![author_uuid.into()],
    ))
    .all(&state.db)
    .await?;

    if fallback.len() == 1 {
        Ok(Some(fallback[0].project_id))
    } else {
        Ok(None)
    }
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

async fn write_veto_audit_log(
    state: &AppState,
    proposal_id: &str,
    actor_id: Uuid,
    action: &str,
    resource_id: Option<String>,
    old_value: Option<serde_json::Value>,
    new_value: Option<serde_json::Value>,
) -> Result<(), ApiError> {
    let Some(project_id) = resolve_project_for_proposal(state, proposal_id).await? else {
        return Ok(());
    };

    write_governance_audit_log(
        &state.db,
        GovernanceAuditLogInput {
            project_id,
            actor_id: Some(actor_id),
            action: action.to_string(),
            resource_type: "veto_event".to_string(),
            resource_id,
            old_value,
            new_value,
            metadata: Some(serde_json::json!({ "proposal_id": proposal_id })),
        },
    )
    .await
}

async fn fetch_actor_author_type(
    state: &AppState,
    user_id: Uuid,
) -> Result<ParticipantType, ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COALESCE(entity_type, 'human') AS entity_type FROM users WHERE id = $1",
            vec![user_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".to_string()))?;

    let entity_type: String = row
        .try_get("", "entity_type")
        .map_err(|_| ApiError::Internal)?;

    if entity_type == "bot" || entity_type == "ai" {
        Ok(ParticipantType::Ai)
    } else {
        Ok(ParticipantType::Human)
    }
}
