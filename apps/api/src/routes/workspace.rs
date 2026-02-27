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
};

#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub role: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkspaceRequest {
    pub name: Option<String>,
    pub slug: Option<String>,
}

/// POST /api/v1/workspaces - Create a new workspace
pub async fn create_workspace(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.slug.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "slug and name are required".to_string(),
        ));
    }

    // Validate slug format (lowercase alphanumeric + hyphens)
    if !req
        .slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ApiError::BadRequest(
            "slug must contain only lowercase letters, numbers, and hyphens".to_string(),
        ));
    }

    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check if slug already exists
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM workspaces WHERE slug = $1",
            vec![req.slug.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict("slug already exists".to_string()));
    }

    // Create workspace
    let workspace_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                workspace_id.into(),
                req.slug.clone().into(),
                req.name.clone().into(),
                user_id.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    // Add creator as owner
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES ($1, $2, $3, $4)",
            vec![
                workspace_id.into(),
                user_id.into(),
                "owner".into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(ApiResponse::success(WorkspaceResponse {
        id: workspace_id,
        slug: req.slug,
        name: req.name,
        role: Some("owner".to_string()),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/workspaces - List user's workspaces
pub async fn list_workspaces(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct WorkspaceRow {
        id: Uuid,
        slug: String,
        name: String,
        role: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let workspaces = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT w.id,
                   w.slug,
                   w.name,
                   COALESCE(
                       wm.role,
                       CASE
                           WHEN wb.permissions @> '["admin"]'::jsonb THEN 'admin'
                           ELSE 'member'
                       END
                   ) AS role,
                   w.created_at,
                   w.updated_at
            FROM workspaces w
            LEFT JOIN workspace_members wm
                   ON w.id = wm.workspace_id
                  AND wm.user_id = $1
            LEFT JOIN workspace_bots wb
                   ON w.id = wb.workspace_id
                  AND wb.id = $1
            WHERE wm.user_id IS NOT NULL OR wb.id IS NOT NULL
            ORDER BY w.created_at DESC
        "#,
        vec![user_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<WorkspaceResponse> = workspaces
        .into_iter()
        .map(|w| WorkspaceResponse {
            id: w.id,
            slug: w.slug,
            name: w.name,
            role: Some(w.role),
            created_at: w.created_at.to_rfc3339(),
            updated_at: w.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// GET /api/v1/workspaces/:id - Get workspace details
pub async fn get_workspace(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct WorkspaceRow {
        id: Uuid,
        slug: String,
        name: String,
        role: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let workspace = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT w.id,
                   w.slug,
                   w.name,
                   COALESCE(
                       wm.role,
                       CASE
                           WHEN wb.permissions @> '["admin"]'::jsonb THEN 'admin'
                           ELSE 'member'
                       END
                   ) AS role,
                   w.created_at,
                   w.updated_at
            FROM workspaces w
            LEFT JOIN workspace_members wm
                   ON w.id = wm.workspace_id
                  AND wm.user_id = $2
            LEFT JOIN workspace_bots wb
                   ON w.id = wb.workspace_id
                  AND wb.id = $2
            WHERE w.id = $1
              AND (wm.user_id IS NOT NULL OR wb.id IS NOT NULL)
        "#,
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok(ApiResponse::success(WorkspaceResponse {
        id: workspace.id,
        slug: workspace.slug,
        name: workspace.name,
        role: Some(workspace.role),
        created_at: workspace.created_at.to_rfc3339(),
        updated_at: workspace.updated_at.to_rfc3339(),
    }))
}

/// PUT /api/v1/workspaces/:id - Update workspace
pub async fn update_workspace(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check permission (owner or admin)
    let role = get_user_role(&state, workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can update workspaces".to_string(),
        ));
    }

    // Validate slug if provided
    if let Some(ref slug) = req.slug {
        if !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(ApiError::BadRequest(
                "slug must contain only lowercase letters, numbers, and hyphens".to_string(),
            ));
        }

        // Check slug uniqueness
        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM workspaces WHERE slug = $1 AND id != $2",
                vec![slug.clone().into(), workspace_id.into()],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict("slug already exists".to_string()));
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

    if let Some(slug) = req.slug {
        updates.push(format!("slug = ${}", param_idx));
        values.push(slug.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;

    values.push(workspace_id.into());

    let query = format!(
        "UPDATE workspaces SET {} WHERE id = ${}",
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

    // Fetch updated workspace
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct WorkspaceRow {
        id: Uuid,
        slug: String,
        name: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let workspace = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, slug, name, created_at, updated_at FROM workspaces WHERE id = $1",
        vec![workspace_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    Ok(ApiResponse::success(WorkspaceResponse {
        id: workspace.id,
        slug: workspace.slug,
        name: workspace.name,
        role: Some(role),
        created_at: workspace.created_at.to_rfc3339(),
        updated_at: workspace.updated_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/workspaces/:id - Delete workspace
pub async fn delete_workspace(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check permission (only owner can delete)
    let role = get_user_role(&state, workspace_id, user_id).await?;
    if role != "owner" {
        return Err(ApiError::Forbidden(
            "only owners can delete workspaces".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM workspaces WHERE id = $1",
            vec![workspace_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

/// Helper: Get user's role in workspace
async fn get_user_role(
    state: &AppState,
    workspace_id: Uuid,
    actor_id: Uuid,
) -> Result<String, ApiError> {
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT role
            FROM (
                SELECT wm.role AS role
                FROM workspace_members wm
                WHERE wm.workspace_id = $1
                  AND wm.user_id = $2
                UNION ALL
                SELECT CASE
                           WHEN wb.permissions @> '["admin"]'::jsonb THEN 'admin'
                           ELSE 'member'
                       END AS role
                FROM workspace_bots wb
                WHERE wb.workspace_id = $1
                  AND wb.id = $2
            ) roles
            LIMIT 1
        "#,
        vec![workspace_id.into(), actor_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok(row.role)
}
