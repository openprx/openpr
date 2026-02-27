use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::Utc;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::{
        ai_task_service::{
            AiTaskRow, CreateAiTaskInput, create_ai_task, insert_ai_task_event, next_retry_time,
            valid_reference_type, valid_task_type,
        },
        trust_score_service::{is_project_admin_or_owner, is_project_member, is_system_admin},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct ListAiTasksQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAiTaskRequest {
    pub ai_participant_id: Uuid,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub priority: Option<i32>,
    pub payload: Option<Value>,
    pub idempotency_key: Option<String>,
    pub max_attempts: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct FailTaskRequest {
    pub error_message: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct AiTaskResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub ai_participant_id: Uuid,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub status: String,
    pub priority: i32,
    pub payload: Value,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_retry_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    workspace_id: Uuid,
}

impl From<AiTaskRow> for AiTaskResponse {
    fn from(value: AiTaskRow) -> Self {
        Self {
            id: value.id,
            project_id: value.project_id,
            ai_participant_id: value.ai_participant_id,
            task_type: value.task_type,
            reference_type: value.reference_type,
            reference_id: value.reference_id,
            status: value.status,
            priority: value.priority,
            payload: value.payload,
            result: value.result,
            error_message: value.error_message,
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            next_retry_at: value.next_retry_at.map(|v| v.to_rfc3339()),
            started_at: value.started_at.map(|v| v.to_rfc3339()),
            completed_at: value.completed_at.map(|v| v.to_rfc3339()),
            created_at: value.created_at.to_rfc3339(),
            updated_at: value.updated_at.to_rfc3339(),
        }
    }
}

pub async fn complete_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Json(result): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let mut task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    if task.status != "processing" {
        return Err(ApiError::BadRequest(
            "task is not in processing status".to_string(),
        ));
    }

    let now = Utc::now();
    let updated = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE ai_tasks
            SET status = 'completed',
                result = $2,
                error_message = NULL,
                completed_at = $3,
                updated_at = $3
            WHERE id = $1
            RETURNING
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
        "#,
        vec![task_id.into(), result.clone().into(), now.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    task = updated.clone();
    insert_ai_task_event(&state.db, task_id, "completed", json!({ "result": result })).await?;

    let project = find_project(&state, task.project_id).await?;
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::AiTaskCompleted,
            workspace_id: project.workspace_id,
            project_id: task.project_id,
            actor_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(json!({
                "task_id": task.id.to_string(),
                "task_type": task.task_type,
                "status": task.status,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id.map(|v| v.to_string()),
                "result": task.result,
            })),
        },
    );

    Ok(ApiResponse::success(AiTaskResponse::from(task)))
}

pub async fn fail_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<FailTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let mut task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    if task.status != "processing" && task.status != "pending" {
        return Err(ApiError::BadRequest(
            "task cannot be failed in current status".to_string(),
        ));
    }

    let now = Utc::now();
    let message = req
        .error_message
        .clone()
        .unwrap_or_else(|| "task execution failed".to_string());

    let will_retry = task.attempts < task.max_attempts;
    let (next_status, next_retry_at, event_type) = if will_retry {
        (
            "pending",
            Some(next_retry_time(task.attempts + 1)),
            "retried".to_string(),
        )
    } else {
        ("failed", None, "failed".to_string())
    };

    let updated = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE ai_tasks
            SET status = $2,
                error_message = $3,
                next_retry_at = $4,
                completed_at = CASE WHEN $2 = 'failed' THEN $5 ELSE completed_at END,
                updated_at = $5
            WHERE id = $1
            RETURNING
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
        "#,
        vec![
            task_id.into(),
            next_status.into(),
            message.clone().into(),
            next_retry_at.into(),
            now.into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    task = updated.clone();
    insert_ai_task_event(
        &state.db,
        task_id,
        &event_type,
        json!({
            "error_message": message,
            "payload": req.payload,
            "attempts": task.attempts,
            "max_attempts": task.max_attempts,
            "next_retry_at": task.next_retry_at.map(|v| v.to_rfc3339()),
        }),
    )
    .await?;

    if task.status == "failed" {
        let project = find_project(&state, task.project_id).await?;
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::AiTaskFailed,
                workspace_id: project.workspace_id,
                project_id: task.project_id,
                actor_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "task_id": task.id.to_string(),
                    "task_type": task.task_type,
                    "status": task.status,
                    "reference_type": task.reference_type,
                    "reference_id": task.reference_id.map(|v| v.to_string()),
                    "error_message": task.error_message,
                })),
            },
        );
    }

    Ok(ApiResponse::success(AiTaskResponse::from(task)))
}

pub async fn report_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(task_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_user_id(&claims)?;
    let task = find_task(&state, task_id).await?;
    ensure_actor_can_operate(&state, &task, actor_id).await?;

    insert_ai_task_event(&state.db, task_id, "progress", payload).await?;
    Ok(ApiResponse::ok())
}

pub async fn list_project_ai_tasks(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListAiTasksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user_id(&claims)?;
    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    if let Some(status) = query.status.as_ref() {
        if !matches!(
            status.as_str(),
            "pending" | "processing" | "completed" | "failed" | "cancelled"
        ) {
            return Err(ApiError::BadRequest("invalid status filter".to_string()));
        }
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let (count_sql, count_values) = if let Some(status) = query.status.clone() {
        (
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE project_id = $1 AND status = $2"
                .to_string(),
            vec![project_id.into(), status.into()],
        )
    } else {
        (
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE project_id = $1".to_string(),
            vec![project_id.into()],
        )
    };

    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &count_sql,
            count_values,
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    let (list_sql, list_values) = if let Some(status) = query.status {
        (
            r#"
                SELECT
                    id,
                    project_id,
                    ai_participant_id,
                    task_type,
                    reference_type,
                    reference_id,
                    status,
                    priority,
                    payload,
                    result,
                    error_message,
                    idempotency_key,
                    attempts,
                    max_attempts,
                    next_retry_at,
                    started_at,
                    completed_at,
                    created_at,
                    updated_at
                FROM ai_tasks
                WHERE project_id = $1 AND status = $2
                ORDER BY priority DESC, created_at DESC
                LIMIT $3 OFFSET $4
            "#
            .to_string(),
            vec![
                project_id.into(),
                status.into(),
                per_page.into(),
                offset.into(),
            ],
        )
    } else {
        (
            r#"
                SELECT
                    id,
                    project_id,
                    ai_participant_id,
                    task_type,
                    reference_type,
                    reference_id,
                    status,
                    priority,
                    payload,
                    result,
                    error_message,
                    idempotency_key,
                    attempts,
                    max_attempts,
                    next_retry_at,
                    started_at,
                    completed_at,
                    created_at,
                    updated_at
                FROM ai_tasks
                WHERE project_id = $1
                ORDER BY priority DESC, created_at DESC
                LIMIT $2 OFFSET $3
            "#
            .to_string(),
            vec![project_id.into(), per_page.into(), offset.into()],
        )
    };

    let items = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?
    .into_iter()
    .map(AiTaskResponse::from)
    .collect::<Vec<_>>();

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn create_project_ai_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateAiTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user_id(&claims)?;
    let is_project_admin = is_project_admin_or_owner(&state.db, project_id, user_id).await?;
    let is_sys_admin = is_system_admin(&state.db, user_id).await?;
    if !is_project_admin && !is_sys_admin {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let task_type = req.task_type.trim().to_ascii_lowercase();
    if !valid_task_type(&task_type) {
        return Err(ApiError::BadRequest("invalid task_type".to_string()));
    }

    if let Some(reference_type) = req.reference_type.as_deref()
        && !valid_reference_type(reference_type)
    {
        return Err(ApiError::BadRequest("invalid reference_type".to_string()));
    }

    ensure_project_ai_participant(&state, project_id, req.ai_participant_id).await?;

    let task = create_ai_task(
        &state.db,
        CreateAiTaskInput {
            project_id,
            ai_participant_id: req.ai_participant_id,
            task_type,
            reference_type: req.reference_type,
            reference_id: req.reference_id,
            priority: req.priority.unwrap_or(0),
            payload: req.payload.unwrap_or_else(|| json!({})),
            idempotency_key: req.idempotency_key,
            max_attempts: req.max_attempts.unwrap_or(3),
        },
    )
    .await?;

    let task =
        task.ok_or_else(|| ApiError::Conflict("idempotency key already exists".to_string()))?;
    Ok(ApiResponse::success(AiTaskResponse::from(task)))
}

async fn ensure_project_ai_participant(
    state: &AppState,
    project_id: Uuid,
    ai_participant_id: Uuid,
) -> Result<(), ApiError> {
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 1
                FROM ai_participants ap
                INNER JOIN users u ON u.id::text = ap.id
                WHERE ap.project_id = $1
                  AND ap.is_active = true
                  AND u.id = $2
                  AND u.entity_type = 'bot'
                LIMIT 1
            "#,
            vec![project_id.into(), ai_participant_id.into()],
        ))
        .await?
        .is_some();

    if !exists {
        return Err(ApiError::BadRequest(
            "ai participant is not active in this project".to_string(),
        ));
    }

    Ok(())
}

async fn ensure_actor_can_operate(
    state: &AppState,
    task: &AiTaskRow,
    actor_id: Uuid,
) -> Result<(), ApiError> {
    if task.ai_participant_id == actor_id {
        return Ok(());
    }
    if is_system_admin(&state.db, actor_id).await? {
        return Ok(());
    }
    Err(ApiError::Forbidden(
        "task can only be updated by assigned ai participant".to_string(),
    ))
}

async fn find_task(state: &AppState, task_id: Uuid) -> Result<AiTaskRow, ApiError> {
    AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
            FROM ai_tasks
            WHERE id = $1
        "#,
        vec![task_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai task not found".to_string()))
}

async fn find_project(state: &AppState, project_id: Uuid) -> Result<ProjectRow, ApiError> {
    ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

fn parse_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}
