use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct SprintResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateSprintRequest {
    pub name: String,
    pub description: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSprintRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: Option<String>,
}

/// Validate sprint status
fn validate_status(status: &str) -> Result<(), ApiError> {
    match status {
        "planned" | "active" | "completed" | "cancelled" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "status must be one of: planned, active, completed, cancelled".to_string(),
        )),
    }
}

fn build_auth_extensions(
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

/// POST /api/v1/projects/:project_id/sprints - Create sprint
pub async fn create_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateSprintRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let sprint_status = req.status.unwrap_or_else(|| "planned".to_string());
    validate_status(&sprint_status)?;

    // Check name uniqueness
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM sprints WHERE project_id = $1 AND name = $2",
            vec![project_id.into(), req.name.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "sprint with this name already exists in project".to_string(),
        ));
    }

    let sprint_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let description = req.description.unwrap_or_default();

    let start_date: Option<chrono::NaiveDate> = if let Some(s) = req.start_date {
        Some(
            chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| {
                ApiError::BadRequest("invalid start_date format (use YYYY-MM-DD)".to_string())
            })?,
        )
    } else {
        None
    };

    let end_date: Option<chrono::NaiveDate> = if let Some(e) = req.end_date {
        Some(
            chrono::NaiveDate::parse_from_str(&e, "%Y-%m-%d").map_err(|_| {
                ApiError::BadRequest("invalid end_date format (use YYYY-MM-DD)".to_string())
            })?,
        )
    } else {
        None
    };

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO sprints (id, project_id, name, description, start_date, end_date, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            vec![
                sprint_id.into(),
                project_id.into(),
                req.name.clone().into(),
                description.clone().into(),
                start_date.into(),
                end_date.into(),
                sprint_status.clone().into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(ApiResponse::success(SprintResponse {
        id: sprint_id,
        project_id,
        name: req.name,
        description,
        start_date: start_date.map(|d| d.to_string()),
        end_date: end_date.map(|d| d.to_string()),
        status: sprint_status,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/projects/:project_id/sprints - List sprints
pub async fn list_sprints(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct SprintRow {
        id: Uuid,
        project_id: Uuid,
        name: String,
        description: String,
        start_date: Option<chrono::NaiveDate>,
        end_date: Option<chrono::NaiveDate>,
        status: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let sprints = SprintRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id, name, description, start_date, end_date, status, created_at, updated_at FROM sprints WHERE project_id = $1 ORDER BY start_date DESC NULLS LAST, created_at DESC",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<SprintResponse> = sprints
        .into_iter()
        .map(|s| SprintResponse {
            id: s.id,
            project_id: s.project_id,
            name: s.name,
            description: s.description,
            start_date: s.start_date.map(|d| d.to_string()),
            end_date: s.end_date.map(|d| d.to_string()),
            status: s.status,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// PUT /api/v1/sprints/:id - Update sprint
pub async fn update_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(sprint_id): Path<Uuid>,
    Json(req): Json<UpdateSprintRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    // Get sprint's project and verify access
    #[derive(Debug, FromQueryResult)]
    struct SprintProject {
        project_id: Uuid,
        workspace_id: Uuid,
        status: String,
    }

    let sprint = SprintProject::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT s.project_id, p.workspace_id, s.status
            FROM sprints s
            INNER JOIN projects p ON s.project_id = p.id
            WHERE s.id = $1
        "#,
        vec![sprint_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;

    require_workspace_access(&state, &extensions, sprint.workspace_id).await?;

    let requested_status = req.status.clone();

    if let Some(ref status) = req.status {
        validate_status(status)?;
    }

    // Check name uniqueness if updating
    if let Some(ref name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }

        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM sprints WHERE project_id = $1 AND name = $2 AND id != $3",
                vec![
                    sprint.project_id.into(),
                    name.clone().into(),
                    sprint_id.into(),
                ],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict(
                "sprint with this name already exists".to_string(),
            ));
        }
    }

    // Build update query
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${}", param_idx));
        values.push(name.into());
        param_idx += 1;
    }

    if let Some(description) = req.description {
        updates.push(format!("description = ${}", param_idx));
        values.push(description.into());
        param_idx += 1;
    }

    if let Some(start) = req.start_date {
        let date = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest("invalid start_date format".to_string()))?;
        updates.push(format!("start_date = ${}", param_idx));
        values.push(date.into());
        param_idx += 1;
    }

    if let Some(end) = req.end_date {
        let date = chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest("invalid end_date format".to_string()))?;
        updates.push(format!("end_date = ${}", param_idx));
        values.push(date.into());
        param_idx += 1;
    }

    if let Some(status) = req.status {
        updates.push(format!("status = ${}", param_idx));
        values.push(status.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(sprint_id.into());

    let query = format!(
        "UPDATE sprints SET {} WHERE id = ${}",
        updates.join(", "),
        param_idx
    );

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &query,
            values,
        ))
        .await?;

    // Fetch updated sprint
    #[derive(Debug, FromQueryResult)]
    struct SprintRow {
        id: Uuid,
        project_id: Uuid,
        name: String,
        description: String,
        start_date: Option<chrono::NaiveDate>,
        end_date: Option<chrono::NaiveDate>,
        status: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = SprintRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id, name, description, start_date, end_date, status, created_at, updated_at FROM sprints WHERE id = $1",
        vec![sprint_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    if let Some(new_status) = requested_status {
        if new_status != sprint.status {
            let event = if new_status == "active" {
                Some(WebhookEvent::SprintStarted)
            } else if new_status == "completed" {
                Some(WebhookEvent::SprintCompleted)
            } else {
                None
            };

            if let Some(event) = event {
                trigger_webhooks(
                    state.clone(),
                    TriggerContext {
                        event,
                        workspace_id: sprint.workspace_id,
                        project_id: sprint.project_id,
                        actor_id: user_id,
                        issue_id: None,
                        comment_id: None,
                        label_id: None,
                        sprint_id: Some(sprint_id),
                        changes: Some(serde_json::json!({
                            "status": {"old": sprint.status, "new": new_status}
                        })),
                        mentions: Vec::new(),
                        extra_data: None,
                    },
                );
            }
        }
    }

    Ok(ApiResponse::success(SprintResponse {
        id: updated.id,
        project_id: updated.project_id,
        name: updated.name,
        description: updated.description,
        start_date: updated.start_date.map(|d| d.to_string()),
        end_date: updated.end_date.map(|d| d.to_string()),
        status: updated.status,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/sprints/:id - Delete sprint
pub async fn delete_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(sprint_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct SprintWorkspace {
        workspace_id: Uuid,
    }

    let sprint = SprintWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.workspace_id
            FROM sprints s
            INNER JOIN projects p ON s.project_id = p.id
            WHERE s.id = $1
        "#,
        vec![sprint_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;

    require_workspace_access(&state, &extensions, sprint.workspace_id).await?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM sprints WHERE id = $1",
            vec![sprint_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}
