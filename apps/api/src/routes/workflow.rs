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
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::{ApiResponse, PaginatedData},
    services::workflow_service::resolve_effective_workflow_for_project,
};

// ============================================================================
// Response types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct WorkflowResponse {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub is_system_default: bool,
    pub state_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowDetailResponse {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub is_system_default: bool,
    pub states: Vec<WorkflowStateResponse>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkflowStateResponse {
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

// ============================================================================
// Request types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateStateRequest {
    pub key: String,
    pub display_name: String,
    pub category: Option<String>,
    pub position: Option<i32>,
    pub color: Option<String>,
    pub is_initial: Option<bool>,
    pub is_terminal: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStateRequest {
    pub display_name: Option<String>,
    pub category: Option<String>,
    pub position: Option<i32>,
    pub color: Option<String>,
    pub is_initial: Option<bool>,
    pub is_terminal: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderStatesRequest {
    pub state_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct SetProjectWorkflowRequest {
    pub workflow_id: Option<Uuid>,
}

// ============================================================================
// Helper functions
// ============================================================================

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

/// Get user's role in workspace
async fn get_workspace_role(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
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

/// Fetch a workflow that is NOT a system default; returns (workflow_id, workspace_id).
/// Returns Forbidden if it IS a system default.
async fn get_editable_workflow(state: &AppState, workflow_id: Uuid) -> Result<(Uuid, Option<Uuid>), ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct WorkflowMeta {
        id: Uuid,
        workspace_id: Option<Uuid>,
        is_system_default: bool,
    }

    let wf = WorkflowMeta::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, is_system_default FROM workflows WHERE id = $1",
        vec![workflow_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workflow not found".to_string()))?;

    if wf.is_system_default {
        return Err(ApiError::Forbidden(
            "system default workflow cannot be modified".to_string(),
        ));
    }

    Ok((wf.id, wf.workspace_id))
}

/// Validate state key: lowercase letter start, then lowercase letters/digits/underscores, 1-50 chars.
fn validate_state_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || key.len() > 50 {
        return Err(ApiError::BadRequest(
            "state key must be between 1 and 50 characters".to_string(),
        ));
    }
    let mut chars = key.chars();
    let first = chars.next().ok_or_else(|| ApiError::BadRequest("state key is empty".to_string()))?;
    if !first.is_ascii_lowercase() {
        return Err(ApiError::BadRequest(
            "state key must start with a lowercase letter".to_string(),
        ));
    }
    for c in chars {
        if !matches!(c, 'a'..='z' | '0'..='9' | '_') {
            return Err(ApiError::BadRequest(
                "state key may only contain lowercase letters, digits, and underscores".to_string(),
            ));
        }
    }
    Ok(())
}

/// Validate category value.
fn validate_category(category: &str) -> Result<(), ApiError> {
    match category {
        "unstarted" | "active" | "completed" | "cancelled" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "category must be one of: unstarted, active, completed, cancelled".to_string(),
        )),
    }
}

// ============================================================================
// Existing handler (preserved)
// ============================================================================

/// GET /api/v1/projects/:project_id/workflow/effective
pub async fn get_effective_workflow_by_project(
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
    let workflow = resolve_effective_workflow_for_project(&state, project_id).await?;
    if let Some(workspace_id) = workflow.workspace_id {
        require_workspace_access(&state, &extensions, workspace_id).await?;
    }
    Ok(ApiResponse::success(workflow))
}

// ============================================================================
// Workflow CRUD
// ============================================================================

/// GET /api/v1/workspaces/:workspace_id/workflows
pub async fn list_workflows(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    require_workspace_access(&state, &extensions, workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct WorkflowRow {
        id: Uuid,
        workspace_id: Option<Uuid>,
        name: String,
        description: String,
        is_system_default: bool,
        state_count: i64,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let rows = WorkflowRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT
                w.id,
                w.workspace_id,
                w.name,
                COALESCE(w.description, '') AS description,
                w.is_system_default,
                COUNT(ws.id) AS state_count,
                w.created_at,
                w.updated_at
            FROM workflows w
            LEFT JOIN workflow_states ws ON ws.workflow_id = w.id
            WHERE w.workspace_id = $1 OR w.is_system_default = TRUE
            GROUP BY w.id
            ORDER BY w.is_system_default DESC, w.created_at ASC
        ",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let items: Vec<WorkflowResponse> = rows
        .into_iter()
        .map(|r| WorkflowResponse {
            id: r.id,
            workspace_id: r.workspace_id,
            name: r.name,
            description: r.description,
            is_system_default: r.is_system_default,
            state_count: r.state_count,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

/// POST /api/v1/workspaces/:workspace_id/workflows
pub async fn create_workflow(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can create workflows".to_string(),
        ));
    }

    // Name uniqueness within workspace
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workflows WHERE workspace_id = $1 AND name = $2",
            vec![workspace_id.into(), req.name.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "workflow with this name already exists in workspace".to_string(),
        ));
    }

    let workflow_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let description = req.description.unwrap_or_default();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO workflows (id, workspace_id, name, description, is_system_default, created_at, updated_at)
                VALUES ($1, $2, $3, $4, FALSE, $5, $6)
            ",
            vec![
                workflow_id.into(),
                workspace_id.into(),
                req.name.clone().into(),
                description.clone().into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    // Create 4 default states
    let default_states = [
        ("backlog", "Backlog", "unstarted", 0, false, false),
        ("todo", "To Do", "unstarted", 1, true, false),
        ("in_progress", "In Progress", "active", 2, false, false),
        ("done", "Done", "completed", 3, false, true),
    ];

    for (key, display_name, category, position, is_initial, is_terminal) in &default_states {
        let state_id = Uuid::new_v4();
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO workflow_states
                        (id, workflow_id, key, display_name, category, position, is_initial, is_terminal, created_at, updated_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                ",
                vec![
                    state_id.into(),
                    workflow_id.into(),
                    (*key).into(),
                    (*display_name).into(),
                    (*category).into(),
                    (*position).into(),
                    (*is_initial).into(),
                    (*is_terminal).into(),
                    now.into(),
                    now.into(),
                ],
            ))
            .await?;
    }

    Ok(ApiResponse::success(WorkflowResponse {
        id: workflow_id,
        workspace_id: Some(workspace_id),
        name: req.name,
        description,
        is_system_default: false,
        state_count: 4,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/workflows/:workflow_id
pub async fn get_workflow(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct WorkflowRow {
        id: Uuid,
        workspace_id: Option<Uuid>,
        name: String,
        description: String,
        is_system_default: bool,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let wf = WorkflowRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, COALESCE(description, '') AS description, is_system_default, created_at, updated_at FROM workflows WHERE id = $1",
        vec![workflow_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workflow not found".to_string()))?;

    // Access check: system default workflows are public to all; workspace workflows require membership
    if let Some(ws_id) = wf.workspace_id {
        require_workspace_access(&state, &extensions, ws_id).await?;
    }

    #[derive(Debug, FromQueryResult)]
    struct StateRow {
        id: Uuid,
        workflow_id: Uuid,
        key: String,
        display_name: String,
        category: String,
        position: i32,
        color: Option<String>,
        is_initial: bool,
        is_terminal: bool,
    }

    let state_rows = StateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workflow_id, key, display_name, category, position, color, is_initial, is_terminal
            FROM workflow_states
            WHERE workflow_id = $1
            ORDER BY position ASC
        ",
        vec![workflow_id.into()],
    ))
    .all(&state.db)
    .await?;

    let states: Vec<WorkflowStateResponse> = state_rows
        .into_iter()
        .map(|s| WorkflowStateResponse {
            id: s.id,
            workflow_id: s.workflow_id,
            key: s.key,
            display_name: s.display_name,
            category: s.category,
            position: s.position,
            color: s.color,
            is_initial: s.is_initial,
            is_terminal: s.is_terminal,
        })
        .collect();

    Ok(ApiResponse::success(WorkflowDetailResponse {
        id: wf.id,
        workspace_id: wf.workspace_id,
        name: wf.name,
        description: wf.description,
        is_system_default: wf.is_system_default,
        states,
        created_at: wf.created_at.to_rfc3339(),
        updated_at: wf.updated_at.to_rfc3339(),
    }))
}

/// PUT /api/v1/workflows/:workflow_id
pub async fn update_workflow(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
    Json(req): Json<UpdateWorkflowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_id, workspace_id_opt) = get_editable_workflow(&state, workflow_id).await?;

    let workspace_id = workspace_id_opt.ok_or_else(|| {
        ApiError::Forbidden("system default workflow cannot be modified".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can update workflows".to_string(),
        ));
    }

    // Validate name if provided
    if let Some(ref name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }
        // Check uniqueness
        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM workflows WHERE workspace_id = $1 AND name = $2 AND id != $3",
                vec![workspace_id.into(), name.clone().into(), workflow_id.into()],
            ))
            .await?;
        if existing.is_some() {
            return Err(ApiError::Conflict(
                "workflow with this name already exists in workspace".to_string(),
            ));
        }
    }

    let mut updates: Vec<String> = Vec::new();
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

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(workflow_id.into());

    let query = format!("UPDATE workflows SET {} WHERE id = ${}", updates.join(", "), param_idx);
    state
        .db
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;

    #[derive(Debug, FromQueryResult)]
    struct WorkflowRow {
        id: Uuid,
        workspace_id: Option<Uuid>,
        name: String,
        description: String,
        is_system_default: bool,
        state_count: i64,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = WorkflowRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT w.id, w.workspace_id, w.name,
                   COALESCE(w.description, '') AS description,
                   w.is_system_default,
                   COUNT(ws.id) AS state_count,
                   w.created_at, w.updated_at
            FROM workflows w
            LEFT JOIN workflow_states ws ON ws.workflow_id = w.id
            WHERE w.id = $1
            GROUP BY w.id
        ",
        vec![workflow_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    Ok(ApiResponse::success(WorkflowResponse {
        id: updated.id,
        workspace_id: updated.workspace_id,
        name: updated.name,
        description: updated.description,
        is_system_default: updated.is_system_default,
        state_count: updated.state_count,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/workflows/:workflow_id
pub async fn delete_workflow(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_id, workspace_id_opt) = get_editable_workflow(&state, workflow_id).await?;

    let workspace_id = workspace_id_opt.ok_or_else(|| {
        ApiError::Forbidden("system default workflow cannot be deleted".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete workflows".to_string(),
        ));
    }

    // Check references: projects using this workflow
    let project_ref = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE workflow_id = $1 LIMIT 1",
            vec![workflow_id.into()],
        ))
        .await?;

    if project_ref.is_some() {
        return Err(ApiError::Conflict(
            "workflow is currently assigned to one or more projects".to_string(),
        ));
    }

    // Check references: workspace default
    let workspace_ref = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workspaces WHERE workflow_id = $1 LIMIT 1",
            vec![workflow_id.into()],
        ))
        .await?;

    if workspace_ref.is_some() {
        return Err(ApiError::Conflict(
            "workflow is set as workspace default".to_string(),
        ));
    }

    // workflow_states will cascade-delete
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM workflows WHERE id = $1",
            vec![workflow_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

// ============================================================================
// Workflow State handlers
// ============================================================================

/// GET /api/v1/workflows/:workflow_id/states
pub async fn list_workflow_states(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct WorkflowMeta {
        workspace_id: Option<Uuid>,
    }

    let wf = WorkflowMeta::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM workflows WHERE id = $1",
        vec![workflow_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workflow not found".to_string()))?;

    if let Some(ws_id) = wf.workspace_id {
        require_workspace_access(&state, &extensions, ws_id).await?;
    }

    #[derive(Debug, FromQueryResult)]
    struct StateRow {
        id: Uuid,
        workflow_id: Uuid,
        key: String,
        display_name: String,
        category: String,
        position: i32,
        color: Option<String>,
        is_initial: bool,
        is_terminal: bool,
    }

    let rows = StateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workflow_id, key, display_name, category, position, color, is_initial, is_terminal
            FROM workflow_states
            WHERE workflow_id = $1
            ORDER BY position ASC
        ",
        vec![workflow_id.into()],
    ))
    .all(&state.db)
    .await?;

    let items: Vec<WorkflowStateResponse> = rows
        .into_iter()
        .map(|s| WorkflowStateResponse {
            id: s.id,
            workflow_id: s.workflow_id,
            key: s.key,
            display_name: s.display_name,
            category: s.category,
            position: s.position,
            color: s.color,
            is_initial: s.is_initial,
            is_terminal: s.is_terminal,
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

/// POST /api/v1/workflows/:workflow_id/states
pub async fn create_workflow_state(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
    Json(req): Json<CreateStateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_id, workspace_id_opt) = get_editable_workflow(&state, workflow_id).await?;

    let workspace_id = workspace_id_opt.ok_or_else(|| {
        ApiError::Forbidden("cannot add states to system default workflow".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can modify workflow states".to_string(),
        ));
    }

    validate_state_key(&req.key)?;

    if req.display_name.trim().is_empty() {
        return Err(ApiError::BadRequest("display_name is required".to_string()));
    }

    let category = req.category.unwrap_or_else(|| "active".to_string());
    validate_category(&category)?;

    // Check key uniqueness within this workflow
    let existing_key = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workflow_states WHERE workflow_id = $1 AND key = $2",
            vec![workflow_id.into(), req.key.clone().into()],
        ))
        .await?;

    if existing_key.is_some() {
        return Err(ApiError::Conflict(
            "a state with this key already exists in the workflow".to_string(),
        ));
    }

    // Determine position
    let position = if let Some(pos) = req.position {
        pos
    } else {
        #[derive(Debug, FromQueryResult)]
        struct MaxPos {
            max_pos: Option<i32>,
        }
        let row = MaxPos::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT MAX(position) AS max_pos FROM workflow_states WHERE workflow_id = $1",
            vec![workflow_id.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or(ApiError::Internal)?;

        row.max_pos.map(|p| p + 1).unwrap_or(0)
    };

    let is_initial = req.is_initial.unwrap_or(false);
    let is_terminal = req.is_terminal.unwrap_or(false);

    // Clear other is_initial flags if this state is initial
    if is_initial {
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE workflow_states SET is_initial = FALSE, updated_at = NOW() WHERE workflow_id = $1",
                vec![workflow_id.into()],
            ))
            .await?;
    }

    let state_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO workflow_states
                    (id, workflow_id, key, display_name, category, position, color, is_initial, is_terminal, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ",
            vec![
                state_id.into(),
                workflow_id.into(),
                req.key.clone().into(),
                req.display_name.clone().into(),
                category.clone().into(),
                position.into(),
                req.color.clone().into(),
                is_initial.into(),
                is_terminal.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(ApiResponse::success(WorkflowStateResponse {
        id: state_id,
        workflow_id,
        key: req.key,
        display_name: req.display_name,
        category,
        position,
        color: req.color,
        is_initial,
        is_terminal,
    }))
}

/// PUT /api/v1/workflow-states/:state_id
pub async fn update_workflow_state(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(state_id): Path<Uuid>,
    Json(req): Json<UpdateStateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    // Fetch state and its workflow
    #[derive(Debug, FromQueryResult)]
    struct StateContext {
        workflow_id: Uuid,
        workspace_id: Option<Uuid>,
        is_system_default: bool,
    }

    let ctx = StateContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT ws.workflow_id, w.workspace_id, w.is_system_default
            FROM workflow_states ws
            INNER JOIN workflows w ON w.id = ws.workflow_id
            WHERE ws.id = $1
        ",
        vec![state_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workflow state not found".to_string()))?;

    if ctx.is_system_default {
        return Err(ApiError::Forbidden(
            "cannot modify states of system default workflow".to_string(),
        ));
    }

    let workspace_id = ctx.workspace_id.ok_or_else(|| {
        ApiError::Forbidden("cannot modify states of system default workflow".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can modify workflow states".to_string(),
        ));
    }

    if let Some(ref display_name) = req.display_name
        && display_name.trim().is_empty()
    {
        return Err(ApiError::BadRequest("display_name cannot be empty".to_string()));
    }

    if let Some(ref category) = req.category {
        validate_category(category)?;
    }

    // If setting is_initial = true, clear others first
    if req.is_initial == Some(true) {
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE workflow_states SET is_initial = FALSE, updated_at = NOW() WHERE workflow_id = $1",
                vec![ctx.workflow_id.into()],
            ))
            .await?;
    }

    let mut updates: Vec<String> = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(display_name) = req.display_name {
        updates.push(format!("display_name = ${}", param_idx));
        values.push(display_name.into());
        param_idx += 1;
    }

    if let Some(category) = req.category {
        updates.push(format!("category = ${}", param_idx));
        values.push(category.into());
        param_idx += 1;
    }

    if let Some(position) = req.position {
        updates.push(format!("position = ${}", param_idx));
        values.push(position.into());
        param_idx += 1;
    }

    if let Some(color) = req.color {
        updates.push(format!("color = ${}", param_idx));
        values.push(color.into());
        param_idx += 1;
    }

    if let Some(is_initial) = req.is_initial {
        updates.push(format!("is_initial = ${}", param_idx));
        values.push(is_initial.into());
        param_idx += 1;
    }

    if let Some(is_terminal) = req.is_terminal {
        updates.push(format!("is_terminal = ${}", param_idx));
        values.push(is_terminal.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(state_id.into());

    let query = format!("UPDATE workflow_states SET {} WHERE id = ${}", updates.join(", "), param_idx);
    state
        .db
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;

    #[derive(Debug, FromQueryResult)]
    struct StateRow {
        id: Uuid,
        workflow_id: Uuid,
        key: String,
        display_name: String,
        category: String,
        position: i32,
        color: Option<String>,
        is_initial: bool,
        is_terminal: bool,
    }

    let updated = StateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workflow_id, key, display_name, category, position, color, is_initial, is_terminal FROM workflow_states WHERE id = $1",
        vec![state_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    Ok(ApiResponse::success(WorkflowStateResponse {
        id: updated.id,
        workflow_id: updated.workflow_id,
        key: updated.key,
        display_name: updated.display_name,
        category: updated.category,
        position: updated.position,
        color: updated.color,
        is_initial: updated.is_initial,
        is_terminal: updated.is_terminal,
    }))
}

/// DELETE /api/v1/workflow-states/:state_id
pub async fn delete_workflow_state(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(state_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct StateContext {
        workflow_id: Uuid,
        workspace_id: Option<Uuid>,
        is_system_default: bool,
    }

    let ctx = StateContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT ws.workflow_id, w.workspace_id, w.is_system_default
            FROM workflow_states ws
            INNER JOIN workflows w ON w.id = ws.workflow_id
            WHERE ws.id = $1
        ",
        vec![state_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workflow state not found".to_string()))?;

    if ctx.is_system_default {
        return Err(ApiError::Forbidden(
            "cannot delete states of system default workflow".to_string(),
        ));
    }

    let workspace_id = ctx.workspace_id.ok_or_else(|| {
        ApiError::Forbidden("cannot delete states of system default workflow".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete workflow states".to_string(),
        ));
    }

    // Ensure at least 2 states remain
    #[derive(Debug, FromQueryResult)]
    struct CountRow {
        cnt: i64,
    }

    let count_row = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*) AS cnt FROM workflow_states WHERE workflow_id = $1",
        vec![ctx.workflow_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    if count_row.cnt <= 2 {
        return Err(ApiError::Conflict(
            "workflow must have at least 2 states".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM workflow_states WHERE id = $1",
            vec![state_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

/// PUT /api/v1/workflows/:workflow_id/states/reorder
pub async fn reorder_workflow_states(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workflow_id): Path<Uuid>,
    Json(req): Json<ReorderStatesRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_id, workspace_id_opt) = get_editable_workflow(&state, workflow_id).await?;

    let workspace_id = workspace_id_opt.ok_or_else(|| {
        ApiError::Forbidden("cannot reorder states of system default workflow".to_string())
    })?;

    require_workspace_access(&state, &extensions, workspace_id).await?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can reorder workflow states".to_string(),
        ));
    }

    if req.state_ids.is_empty() {
        return Err(ApiError::BadRequest("state_ids cannot be empty".to_string()));
    }

    // Phase 1: assign large offset values to avoid UNIQUE(workflow_id, position) conflicts
    for (idx, sid) in req.state_ids.iter().enumerate() {
        let offset_pos = 10000 + idx as i32;
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE workflow_states SET position = $1, updated_at = NOW() WHERE id = $2 AND workflow_id = $3",
                vec![offset_pos.into(), (*sid).into(), workflow_id.into()],
            ))
            .await?;
    }

    // Phase 2: assign final positions
    for (idx, sid) in req.state_ids.iter().enumerate() {
        let final_pos = idx as i32;
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE workflow_states SET position = $1, updated_at = NOW() WHERE id = $2 AND workflow_id = $3",
                vec![final_pos.into(), (*sid).into(), workflow_id.into()],
            ))
            .await?;
    }

    Ok(ApiResponse::ok())
}

// ============================================================================
// Project workflow assignment
// ============================================================================

/// PUT /api/v1/projects/:project_id/workflow
pub async fn set_project_workflow(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<SetProjectWorkflowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct ProjectMeta {
        workspace_id: Uuid,
    }

    let project = ProjectMeta::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let role = get_workspace_role(&state, project.workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can change project workflow".to_string(),
        ));
    }

    if let Some(wf_id) = req.workflow_id {
        // Validate the workflow belongs to same workspace OR is system default
        #[derive(Debug, FromQueryResult)]
        struct WorkflowCheck {
            workspace_id: Option<Uuid>,
            is_system_default: bool,
        }

        let wf = WorkflowCheck::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id, is_system_default FROM workflows WHERE id = $1",
            vec![wf_id.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::NotFound("workflow not found".to_string()))?;

        let belongs = wf.is_system_default
            || wf.workspace_id == Some(project.workspace_id);

        if !belongs {
            return Err(ApiError::Forbidden(
                "workflow does not belong to this workspace".to_string(),
            ));
        }

        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE projects SET workflow_id = $1, updated_at = NOW() WHERE id = $2",
                vec![wf_id.into(), project_id.into()],
            ))
            .await?;
    } else {
        // null → clear project-level override
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE projects SET workflow_id = NULL, updated_at = NOW() WHERE id = $1",
                vec![project_id.into()],
            ))
            .await?;
    }

    Ok(ApiResponse::ok())
}
