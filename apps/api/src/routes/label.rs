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
pub struct LabelResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub color: String,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLabelRequest {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Helper: Get user's role in workspace
async fn get_workspace_role(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<String, ApiError> {
    #[derive(Debug, FromQueryResult)]
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

/// POST /api/v1/workspaces/:workspace_id/labels - Create a label
pub async fn create_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateLabelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    // Check workspace membership
    get_workspace_role(&state, workspace_id, user_id).await?;

    // Check name uniqueness
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM labels WHERE workspace_id = $1 AND name = $2",
            vec![workspace_id.into(), req.name.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "label with this name already exists in workspace".to_string(),
        ));
    }

    let label_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let color = req.color.unwrap_or_else(|| "#gray".to_string());
    let description = req.description.unwrap_or_default();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO labels (id, workspace_id, name, color, description, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                label_id.into(),
                workspace_id.into(),
                req.name.clone().into(),
                color.clone().into(),
                description.clone().into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(ApiResponse::success(LabelResponse {
        id: label_id,
        workspace_id,
        name: req.name,
        color,
        description,
        created_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/workspaces/:workspace_id/labels - List labels in workspace
pub async fn list_labels(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check workspace membership
    get_workspace_role(&state, workspace_id, user_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let labels = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, color, description, created_at FROM labels WHERE workspace_id = $1 ORDER BY name ASC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<LabelResponse> = labels
        .into_iter()
        .map(|l| LabelResponse {
            id: l.id,
            workspace_id: l.workspace_id,
            name: l.name,
            color: l.color,
            description: l.description,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// PUT /api/v1/labels/:id - Update label
pub async fn update_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(label_id): Path<Uuid>,
    Json(req): Json<UpdateLabelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get label's workspace and verify access
    #[derive(Debug, FromQueryResult)]
    struct LabelWorkspace {
        workspace_id: Uuid,
    }

    let label = LabelWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("label not found".to_string()))?;

    get_workspace_role(&state, label.workspace_id, user_id).await?;

    // Check name uniqueness if name is being updated
    if let Some(ref name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }

        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM labels WHERE workspace_id = $1 AND name = $2 AND id != $3",
                vec![
                    label.workspace_id.into(),
                    name.clone().into(),
                    label_id.into(),
                ],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict(
                "label with this name already exists in workspace".to_string(),
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

    if let Some(color) = req.color {
        updates.push(format!("color = ${}", param_idx));
        values.push(color.into());
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

    values.push(label_id.into());

    let query = format!(
        "UPDATE labels SET {} WHERE id = ${}",
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

    // Fetch updated label
    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, color, description, created_at FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    Ok(ApiResponse::success(LabelResponse {
        id: updated.id,
        workspace_id: updated.workspace_id,
        name: updated.name,
        color: updated.color,
        description: updated.description,
        created_at: updated.created_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/labels/:id - Delete label
pub async fn delete_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(label_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get label's workspace and check permission
    #[derive(Debug, FromQueryResult)]
    struct LabelWorkspace {
        workspace_id: Uuid,
    }

    let label = LabelWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("label not found".to_string()))?;

    let role = get_workspace_role(&state, label.workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete labels".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM labels WHERE id = $1",
            vec![label_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

// ============================================================================
// Work Item Label Association APIs
// ============================================================================

/// POST /api/v1/issues/:issue_id/labels/:label_id - Add label to issue
pub async fn add_label_to_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((issue_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify issue exists and user has access
    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    let issue_ws = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE wi.id = $1 AND wm.user_id = $2
        "#,
        vec![issue_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found or access denied".to_string()))?;

    // Verify label belongs to same workspace
    let label_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2",
            vec![label_id.into(), issue_ws.workspace_id.into()],
        ))
        .await?;

    if label_exists.is_none() {
        return Err(ApiError::NotFound(
            "label not found in workspace".to_string(),
        ));
    }

    // Check if already associated
    let already_linked = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM work_item_labels WHERE work_item_id = $1 AND label_id = $2",
            vec![issue_id.into(), label_id.into()],
        ))
        .await?;

    if already_linked.is_some() {
        return Err(ApiError::Conflict(
            "label already added to issue".to_string(),
        ));
    }

    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO work_item_labels (work_item_id, label_id, created_at) VALUES ($1, $2, $3)",
            vec![issue_id.into(), label_id.into(), now.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::LabelAdded,
            workspace_id: issue_ws.workspace_id,
            project_id: issue_ws.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: Some(label_id),
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::ok())
}

/// GET /api/v1/issues/:issue_id/labels - Get issue's labels
pub async fn get_issue_labels(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify issue exists and user has access
    let issue_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT 1
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE wi.id = $1 AND wm.user_id = $2
        "#,
        vec![issue_id.into(), user_id.into()],
    ))
        .await?;

    if issue_exists.is_none() {
        return Err(ApiError::NotFound(
            "issue not found or access denied".to_string(),
        ));
    }

    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let labels = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT l.id, l.workspace_id, l.name, l.color, l.description, l.created_at
            FROM labels l
            INNER JOIN work_item_labels wil ON l.id = wil.label_id
            WHERE wil.work_item_id = $1
            ORDER BY l.name ASC
        "#,
        vec![issue_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<LabelResponse> = labels
        .into_iter()
        .map(|l| LabelResponse {
            id: l.id,
            workspace_id: l.workspace_id,
            name: l.name,
            color: l.color,
            description: l.description,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// DELETE /api/v1/issues/:issue_id/labels/:label_id - Remove label from issue
pub async fn remove_label_from_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((issue_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, FromQueryResult)]
    struct IssueContext {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    // Verify issue exists and user has access
    let issue_context = IssueContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE wi.id = $1 AND wm.user_id = $2
        "#,
        vec![issue_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found or access denied".to_string()))?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_item_labels WHERE work_item_id = $1 AND label_id = $2",
            vec![issue_id.into(), label_id.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::LabelRemoved,
            workspace_id: issue_context.workspace_id,
            project_id: issue_context.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: Some(label_id),
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::ok())
}
