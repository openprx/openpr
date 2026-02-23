use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait, TryGetable};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::trust_score_service::{
        TrustEventType, TrustScoreService, is_project_admin_or_owner,
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct CreateAppealRequest {
    pub log_id: i64,
    pub reason: String,
    pub evidence: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListAppealsQuery {
    pub status: Option<String>,
    pub mine: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAppealRequest {
    pub status: String,
    pub review_note: Option<String>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct AppealRow {
    pub id: i64,
    pub log_id: i64,
    pub project_id: Uuid,
    pub domain: String,
    pub appellant_id: Uuid,
    pub reason: String,
    pub evidence: Option<Value>,
    pub status: String,
    pub reviewer_id: Option<Uuid>,
    pub review_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct TrustScoreLogRow {
    id: i64,
    user_id: Uuid,
    project_id: Uuid,
    domain: String,
    score_change: i32,
}

#[derive(Debug, FromQueryResult)]
struct VetoerCountRow {
    count: i64,
}

pub async fn create_appeal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<CreateAppealRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let appellant_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.reason.trim().is_empty() {
        return Err(ApiError::BadRequest("reason is required".to_string()));
    }

    let log = find_log(&state, req.log_id).await?;
    if log.user_id != appellant_id {
        return Err(ApiError::Forbidden(
            "only the score owner can submit an appeal".to_string(),
        ));
    }

    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM appeals WHERE log_id = $1 AND status = 'pending'::appeal_status",
            vec![req.log_id.into()],
        ))
        .await?
        .is_some();
    if exists {
        return Err(ApiError::Conflict("pending appeal already exists".to_string()));
    }

    let row = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO appeals (
                log_id, appellant_id, reason, evidence, status, created_at
            ) VALUES ($1, $2, $3, $4, 'pending'::appeal_status, $5)
            RETURNING id, log_id, appellant_id, reason, evidence,
                      status::text AS status, reviewer_id, review_note,
                      created_at, resolved_at,
                      (SELECT project_id FROM trust_score_logs WHERE id = log_id) AS project_id,
                      (SELECT domain FROM trust_score_logs WHERE id = log_id) AS domain
        "#,
        vec![
            req.log_id.into(),
            appellant_id.into(),
            req.reason.trim().to_string().into(),
            req.evidence.into(),
            Utc::now().into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    if let Some(workspace_id) = resolve_workspace_id_for_project(&state, row.project_id).await? {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::AppealCreated,
                workspace_id,
                project_id: row.project_id,
                actor_id: appellant_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(serde_json::json!({
                    "appeal": {
                        "id": row.id,
                        "log_id": row.log_id,
                        "project_id": row.project_id,
                        "domain": row.domain,
                        "appellant_id": row.appellant_id,
                        "reason": row.reason,
                        "evidence": row.evidence,
                        "status": row.status,
                        "created_at": row.created_at.to_rfc3339(),
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::success(row))
}

pub async fn list_appeals(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListAppealsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let mut where_parts = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx = 1;

    let mine = query.mine.unwrap_or(false);
    if mine {
        where_parts.push(format!("a.appellant_id = ${idx}"));
        values.push(user_id.into());
        idx += 1;
    } else {
        where_parts.push(format!(
            r#"(
                a.appellant_id = ${0}
                OR EXISTS (
                    SELECT 1
                    FROM trust_score_logs t
                    INNER JOIN vetoers v ON v.project_id = t.project_id AND v.domain = t.domain
                    WHERE t.id = a.log_id AND v.user_id = ${0}
                )
                OR EXISTS (
                    SELECT 1
                    FROM trust_score_logs t
                    WHERE t.id = a.log_id
                      AND EXISTS (
                        SELECT 1
                        FROM projects p
                        INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id
                        WHERE p.id = t.project_id
                          AND wm.user_id = ${0}
                          AND wm.role IN ('owner', 'admin')
                      )
                )
            )"#,
            idx
        ));
        values.push(user_id.into());
        idx += 1;
    }

    if let Some(status) = query.status {
        let normalized = status.to_lowercase();
        if !matches!(normalized.as_str(), "pending" | "accepted" | "rejected") {
            return Err(ApiError::BadRequest("invalid status".to_string()));
        }
        where_parts.push(format!("a.status = ${idx}::appeal_status"));
        values.push(normalized.into());
    }

    let sql = format!(
        r#"
            SELECT a.id, a.log_id, a.appellant_id, a.reason, a.evidence,
                   a.status::text AS status, a.reviewer_id, a.review_note,
                   a.created_at, a.resolved_at,
                   t.project_id, t.domain
            FROM appeals a
            INNER JOIN trust_score_logs t ON t.id = a.log_id
            WHERE {}
            ORDER BY a.created_at DESC
        "#,
        where_parts.join(" AND ")
    );

    let items = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn get_appeal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let row = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT a.id, a.log_id, a.appellant_id, a.reason, a.evidence,
                   a.status::text AS status, a.reviewer_id, a.review_note,
                   a.created_at, a.resolved_at,
                   t.project_id, t.domain
            FROM appeals a
            INNER JOIN trust_score_logs t ON t.id = a.log_id
            WHERE a.id = $1
        "#,
        vec![id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("appeal not found".to_string()))?;

    ensure_can_view_appeal(&state, &row, user_id).await?;
    Ok(ApiResponse::success(row))
}

pub async fn update_appeal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateAppealRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let reviewer_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let normalized = req.status.to_lowercase();
    if !matches!(normalized.as_str(), "accepted" | "rejected") {
        return Err(ApiError::BadRequest(
            "status must be accepted or rejected".to_string(),
        ));
    }

    let tx = state.db.begin().await?;

    let appeal = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT a.id, a.log_id, a.appellant_id, a.reason, a.evidence,
                   a.status::text AS status, a.reviewer_id, a.review_note,
                   a.created_at, a.resolved_at,
                   t.project_id, t.domain
            FROM appeals a
            INNER JOIN trust_score_logs t ON t.id = a.log_id
            WHERE a.id = $1
            FOR UPDATE
        "#,
        vec![id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("appeal not found".to_string()))?;

    if appeal.status != "pending" {
        return Err(ApiError::Conflict("appeal is already resolved".to_string()));
    }

    let log = find_log_with_conn(&tx, appeal.log_id).await?;
    ensure_can_review_appeal(&state, &tx, reviewer_id, &log).await?;

    if normalized == "accepted" {
        let trust_service = TrustScoreService::new(state.db.clone());
        let event_id = format!("APL-{}", appeal.id);
        let participant_type = resolve_user_participant_type_with_conn(&tx, log.user_id).await?;
        trust_service
            .apply_manual_adjustment_with_conn(
                &tx,
                log.user_id,
                participant_type,
                log.project_id,
                &log.domain,
                -log.score_change,
                TrustEventType::AppealAccepted,
                &event_id,
                &format!("appeal {} accepted", appeal.id),
            )
            .await?;
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE trust_score_logs
            SET is_appealed = true,
                appeal_result = $2
            WHERE id = $1
        "#,
        vec![log.id.into(), normalized.clone().into()],
    ))
    .await?;

    let updated = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE appeals
            SET status = $2::appeal_status,
                reviewer_id = $3,
                review_note = $4,
                resolved_at = $5
            WHERE id = $1
            RETURNING id, log_id, appellant_id, reason, evidence,
                      status::text AS status, reviewer_id, review_note,
                      created_at, resolved_at,
                      (SELECT project_id FROM trust_score_logs WHERE id = log_id) AS project_id,
                      (SELECT domain FROM trust_score_logs WHERE id = log_id) AS domain
        "#,
        vec![
            id.into(),
            normalized.into(),
            reviewer_id.into(),
            req.review_note.into(),
            Utc::now().into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?;

    tx.commit().await?;

    Ok(ApiResponse::success(updated))
}

pub async fn delete_appeal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let appeal = AppealRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, log_id, appellant_id, reason, evidence,
                   status::text AS status, reviewer_id, review_note,
                   created_at, resolved_at,
                   (SELECT project_id FROM trust_score_logs WHERE id = log_id) AS project_id,
                   (SELECT domain FROM trust_score_logs WHERE id = log_id) AS domain
            FROM appeals
            WHERE id = $1
        "#,
        vec![id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("appeal not found".to_string()))?;

    if appeal.appellant_id != user_id {
        return Err(ApiError::Forbidden(
            "only appellant can delete appeal".to_string(),
        ));
    }

    if appeal.status != "pending" {
        return Err(ApiError::BadRequest(
            "only pending appeal can be deleted".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM appeals WHERE id = $1",
            vec![id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

async fn find_log(state: &AppState, log_id: i64) -> Result<TrustScoreLogRow, ApiError> {
    find_log_with_conn(&state.db, log_id).await
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

async fn find_log_with_conn<C: ConnectionTrait>(db: &C, log_id: i64) -> Result<TrustScoreLogRow, ApiError> {
    TrustScoreLogRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, user_id, project_id, domain, score_change
            FROM trust_score_logs
            WHERE id = $1
        "#,
        vec![log_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("trust score log not found".to_string()))
}

async fn ensure_can_view_appeal(
    state: &AppState,
    appeal: &AppealRow,
    user_id: Uuid,
) -> Result<(), ApiError> {
    if appeal.appellant_id == user_id {
        return Ok(());
    }

    let log = find_log(state, appeal.log_id).await?;
    if can_review_in_project(state, user_id, &log).await? {
        return Ok(());
    }

    Err(ApiError::Forbidden("appeal access denied".to_string()))
}

async fn ensure_can_review_appeal<C: ConnectionTrait>(
    state: &AppState,
    db: &C,
    user_id: Uuid,
    log: &TrustScoreLogRow,
) -> Result<(), ApiError> {
    if is_project_admin_or_owner(&state.db, log.project_id, user_id).await? {
        return Ok(());
    }

    let vetoer_count = VetoerCountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*)::bigint AS count
            FROM vetoers
            WHERE user_id = $1
              AND project_id = $2
              AND domain = $3
        "#,
        vec![user_id.into(), log.project_id.into(), log.domain.clone().into()],
    ))
    .one(db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    if vetoer_count > 0 {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "admin or domain vetoer required".to_string(),
        ))
    }
}

async fn can_review_in_project(
    state: &AppState,
    user_id: Uuid,
    log: &TrustScoreLogRow,
) -> Result<bool, ApiError> {
    if is_project_admin_or_owner(&state.db, log.project_id, user_id).await? {
        return Ok(true);
    }

    let count = VetoerCountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*)::bigint AS count
            FROM vetoers
            WHERE user_id = $1
              AND project_id = $2
              AND domain = $3
        "#,
        vec![user_id.into(), log.project_id.into(), log.domain.clone().into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    Ok(count > 0)
}

async fn resolve_user_participant_type_with_conn<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<ParticipantType, ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COALESCE(entity_type, 'human') AS entity_type FROM users WHERE id = $1",
            vec![user_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".to_string()))?;

    let entity_type: String = row
        .try_get("", "entity_type")
        .map_err(|_| ApiError::Internal)?;

    if entity_type == "bot" || entity_type == "ai" {
        Ok(ParticipantType::Ai)
    } else {
        Ok(ParticipantType::Human)
    }
}
