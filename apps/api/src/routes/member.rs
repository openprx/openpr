use axum::{
    Extension, Json,
    extract::{Path, Query, State},
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
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub entity_type: String,
    pub agent_type: Option<String>,
    pub role: String,
    pub joined_at: String,
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    pub user_id: Uuid,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemberRoleRequest {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchUsersQuery {
    pub search: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct SearchUserResponse {
    pub id: Uuid,
    pub email: String,
    pub name: String,
}

/// POST /api/v1/workspaces/:workspace_id/members - Add existing user to workspace
pub async fn add_member(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Validate role
    if req.role != "admin" && req.role != "member" {
        return Err(ApiError::BadRequest(
            "role must be 'admin' or 'member'".to_string(),
        ));
    }

    // Check permission (only owner and admin can add members)
    let operator_role = get_workspace_role(&state, workspace_id, user_id).await?;
    if operator_role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can add members".to_string(),
        ));
    }

    // Find user by id
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct UserRow {
        id: Uuid,
        email: String,
        name: String,
        entity_type: String,
        agent_type: Option<String>,
    }

    let target_user = UserRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, email, name, entity_type, agent_type FROM users WHERE id = $1",
        vec![req.user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("user not found".to_string()))?;

    // Check if already a member
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT user_id FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), target_user.id.into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "user is already a member of this workspace".to_string(),
        ));
    }

    let now = chrono::Utc::now();

    // Add member
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) VALUES ($1, $2, $3, $4)",
            vec![
                workspace_id.into(),
                target_user.id.into(),
                req.role.clone().into(),
                now.into(),
            ],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::MemberAdded,
            workspace_id,
            project_id: Uuid::nil(),
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "member": {
                    "user_id": target_user.id,
                    "email": target_user.email,
                    "name": target_user.name,
                    "entity_type": target_user.entity_type,
                    "agent_type": target_user.agent_type,
                    "role": req.role,
                    "joined_at": now.to_rfc3339(),
                }
            })),
        },
    );

    Ok(ApiResponse::success(MemberResponse {
        user_id: target_user.id,
        email: target_user.email,
        name: target_user.name,
        entity_type: target_user.entity_type,
        agent_type: target_user.agent_type,
        role: req.role,
        joined_at: now.to_rfc3339(),
    }))
}

/// PATCH /api/v1/workspaces/:workspace_id/members/:user_id - Update member role
pub async fn update_member_role(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateMemberRoleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.role != "admin" && req.role != "member" {
        return Err(ApiError::BadRequest(
            "role must be 'admin' or 'member'".to_string(),
        ));
    }

    let operator_role = get_workspace_role(&state, workspace_id, user_id).await?;
    if operator_role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can update member roles".to_string(),
        ));
    }

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let target_role = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), target_user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("member not found in workspace".to_string()))?;

    if target_role.role == "owner" {
        return Err(ApiError::Forbidden(
            "cannot change workspace owner role".to_string(),
        ));
    }

    if operator_role == "admin" && (target_role.role == "admin" || req.role == "admin") {
        return Err(ApiError::Forbidden(
            "only owners can assign or modify admin roles".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE workspace_members SET role = $3 WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), target_user_id.into(), req.role.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::MemberRemoved,
            workspace_id,
            project_id: Uuid::nil(),
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "member": {
                    "user_id": target_user_id,
                    "workspace_id": workspace_id,
                    "status": "removed",
                }
            })),
        },
    );

    Ok(ApiResponse::ok())
}

/// GET /api/v1/workspaces/:workspace_id/members - List workspace members
pub async fn list_members(
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
    struct MemberRow {
        user_id: Uuid,
        email: String,
        name: String,
        entity_type: String,
        agent_type: Option<String>,
        role: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let members = MemberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wm.user_id, u.email, u.name, u.entity_type, u.agent_type, wm.role, wm.created_at
            FROM workspace_members wm
            INNER JOIN users u ON wm.user_id = u.id
            WHERE wm.workspace_id = $1
            ORDER BY wm.created_at ASC
        "#,
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<MemberResponse> = members
        .into_iter()
        .map(|m| MemberResponse {
            user_id: m.user_id,
            email: m.email,
            name: m.name,
            entity_type: m.entity_type,
            agent_type: m.agent_type,
            role: m.role,
            joined_at: m.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// DELETE /api/v1/workspaces/:workspace_id/members/:user_id - Remove member from workspace
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check permission (only owner and admin can remove members)
    let remover_role = get_workspace_role(&state, workspace_id, user_id).await?;
    if remover_role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can remove members".to_string(),
        ));
    }

    // Get target user's role
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let target_role = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), target_user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("member not found in workspace".to_string()))?;

    // Cannot remove owner
    if target_role.role == "owner" {
        return Err(ApiError::Forbidden(
            "cannot remove workspace owner".to_string(),
        ));
    }

    // Admin cannot remove another admin, only owner can
    if target_role.role == "admin" && remover_role != "owner" {
        return Err(ApiError::Forbidden(
            "only owners can remove admins".to_string(),
        ));
    }

    // Remove member
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), target_user_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

/// GET /api/v1/workspaces/:workspace_id/users?search=xxx - Search users for adding to workspace
pub async fn search_users(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<SearchUsersQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let role = get_workspace_role(&state, workspace_id, user_id).await?;
    if role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can search users".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let search = query.search.unwrap_or_default().trim().to_lowercase();

    let users = if search.is_empty() {
        SearchUserResponse::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, email, name
                FROM users
                ORDER BY created_at DESC
                LIMIT $1
            "#,
            vec![limit.into()],
        ))
        .all(&state.db)
        .await?
    } else {
        SearchUserResponse::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, email, name
                FROM users
                WHERE LOWER(name) LIKE $1 OR LOWER(email) LIKE $1
                ORDER BY created_at DESC
                LIMIT $2
            "#,
            vec![format!("%{}%", search).into(), limit.into()],
        ))
        .all(&state.db)
        .await?
    };

    Ok(ApiResponse::success(PaginatedData::from_items(users)))
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
