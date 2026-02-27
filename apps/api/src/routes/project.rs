use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
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
    pub created_at: String,
    pub updated_at: String,
    pub issue_counts: Option<IssueCounts>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct IssueCounts {
    pub backlog: i64,
    pub todo: i64,
    pub in_progress: i64,
    pub done: i64,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub key: Option<String>,
    pub description: Option<String>,
}

/// POST /api/v1/workspaces/:workspace_id/projects - Create a new project
pub async fn create_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.key.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "key and name are required".to_string(),
        ));
    }

    // Validate key format (uppercase letters only)
    if !req
        .key
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
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

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects (id, workspace_id, key, name, description, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        vec![
            project_id.into(),
            workspace_id.into(),
            req.key.clone().into(),
            req.name.clone().into(),
            description.clone().into(),
            user_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;
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
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        issue_counts: None,
    }))
}

/// GET /api/v1/workspaces/:workspace_id/projects - List projects in workspace
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
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let projects = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, key, name, description, created_at, updated_at FROM projects WHERE workspace_id = $1 ORDER BY created_at DESC",
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
            match row.state.as_str() {
                "backlog" => entry.backlog += row.count,
                "todo" => entry.todo += row.count,
                "in_progress" => entry.in_progress += row.count,
                "done" => entry.done += row.count,
                _ => {}
            }
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
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
            issue_counts: Some(
                issue_counts_by_project
                    .get(&p.id)
                    .cloned()
                    .unwrap_or_default(),
            ),
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
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
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
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.id, p.workspace_id, p.key, p.name, p.description, p.created_at, p.updated_at
            FROM projects p
            WHERE p.id = $1
        "#,
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
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

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
        if !key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
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
                vec![
                    project.workspace_id.into(),
                    key.clone().into(),
                    project_id.into(),
                ],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict(
                "key already exists in this workspace".to_string(),
            ));
        }
    }

    // Build update query dynamically
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${}", param_idx));
        values.push(name.into());
        param_idx += 1;
    }

    if let Some(key) = req.key {
        updates.push(format!("key = ${}", param_idx));
        values.push(key.into());
        param_idx += 1;
    }

    if let Some(description) = req.description {
        updates.push(format!("description = ${}", param_idx));
        values.push(description.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(project_id.into());

    let query = format!(
        "UPDATE projects SET {} WHERE id = ${}",
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

    // Fetch updated project
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, key, name, description, created_at, updated_at FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

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
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

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
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete projects".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM projects WHERE id = $1",
            vec![project_id.into()],
        ))
        .await?;

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
async fn get_workspace_role(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<String, ApiError> {
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
