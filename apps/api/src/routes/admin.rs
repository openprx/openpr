use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use bcrypt::{DEFAULT_COST, hash};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Deserialize)]
pub struct ListUsersQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub entity_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
    pub password: Option<String>,
    pub role: Option<String>,
    pub entity_type: Option<String>,
    pub agent_type: Option<String>,
    pub agent_config: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    pub entity_type: Option<String>,
    pub agent_type: Option<String>,
    pub agent_config: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ToggleUserStatusRequest {
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub new_password: String,
}

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub is_active: bool,
    pub entity_type: String,
    pub agent_type: Option<String>,
    pub agent_config: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct UserWorkspace {
    pub workspace_id: Uuid,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub workspace_role: String,
    pub joined_at: String,
}

#[derive(Debug, Serialize)]
pub struct UserDetailResponse {
    pub user: UserSummary,
    pub workspaces: Vec<UserWorkspace>,
}

#[derive(Debug, Serialize)]
pub struct ListUsersResponse {
    pub items: Vec<UserSummary>,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminStats {
    pub total_users: i64,
    pub total_workspaces: i64,
    pub total_projects: i64,
    pub total_issues: i64,
    pub active_users_last_30d: i64,
}

#[derive(Debug, FromQueryResult)]
struct UserRow {
    id: Uuid,
    email: String,
    name: String,
    role: String,
    is_active: bool,
    entity_type: String,
    agent_type: Option<String>,
    agent_config: Option<Value>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct UserWorkspaceRow {
    workspace_id: Uuid,
    workspace_slug: String,
    workspace_name: String,
    workspace_role: String,
    joined_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    total: i64,
}

pub async fn list_users(
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let (where_sql, where_values) = build_search_where(&query.search, &query.entity_type)?;

    let count_sql = format!("SELECT COUNT(*)::bigint as total FROM users {}", where_sql);
    let total_row = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &count_sql,
        where_values.clone(),
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    let mut values = where_values;
    values.push(limit.into());
    values.push(offset.into());
    let list_sql = format!(
        "SELECT id, email, name, role, is_active, entity_type, agent_type, agent_config, created_at, updated_at FROM users {} ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
        where_sql,
        values.len() - 1,
        values.len()
    );

    let users = UserRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &list_sql,
        values,
    ))
    .all(&state.db)
    .await?;

    let items = users.into_iter().map(map_user_summary).collect();

    Ok(ApiResponse::success(ListUsersResponse {
        items,
        page,
        per_page: limit,
        total_pages: ((total_row.total + limit - 1) / limit).max(1),
        total: total_row.total,
    }))
}

pub async fn create_user(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name cannot be empty".to_string()));
    }

    let email = req.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::BadRequest("email cannot be empty".to_string()));
    }

    let role = req.role.unwrap_or_else(|| "user".to_string());
    if role != "admin" && role != "user" {
        return Err(ApiError::BadRequest(
            "role must be 'admin' or 'user'".to_string(),
        ));
    }

    let entity_type = normalize_entity_type(req.entity_type.as_deref())?;
    let agent_type = normalize_agent_type(req.agent_type)?;

    if entity_type == "bot" && agent_type.is_none() {
        return Err(ApiError::BadRequest(
            "agent_type is required for bot user".to_string(),
        ));
    }
    if entity_type == "human" && agent_type.is_some() {
        return Err(ApiError::BadRequest(
            "agent_type is only allowed for bot user".to_string(),
        ));
    }

    let password_hash = if entity_type == "bot" {
        "".to_string()
    } else {
        let password = req
            .password
            .ok_or_else(|| ApiError::BadRequest("password is required".to_string()))?;
        if password.trim().len() < 8 {
            return Err(ApiError::BadRequest(
                "password must be at least 8 characters".to_string(),
            ));
        }
        hash(password, DEFAULT_COST).map_err(|_| ApiError::Internal)?
    };

    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM users WHERE email = $1".to_string(),
            vec![email.clone().into()],
        ))
        .await?;
    if existing.is_some() {
        return Err(ApiError::Conflict("email already exists".to_string()));
    }

    let user_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, agent_config, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9, $10)".to_string(),
            vec![
                user_id.into(),
                email.into(),
                password_hash.into(),
                name.to_string().into(),
                role.into(),
                entity_type.into(),
                agent_type.into(),
                req.agent_config.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    let user = find_user_by_id(&state, user_id).await?;
    Ok(ApiResponse::success(map_user_summary(user)))
}

pub async fn get_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user = find_user_by_id(&state, user_id).await?;

    let workspaces = UserWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                wm.workspace_id,
                w.slug as workspace_slug,
                w.name as workspace_name,
                wm.role as workspace_role,
                wm.created_at as joined_at
            FROM workspace_members wm
            INNER JOIN workspaces w ON w.id = wm.workspace_id
            WHERE wm.user_id = $1
            ORDER BY wm.created_at DESC
        "#,
        vec![user_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(UserDetailResponse {
        user: map_user_summary(user),
        workspaces: workspaces
            .into_iter()
            .map(|w| UserWorkspace {
                workspace_id: w.workspace_id,
                workspace_slug: w.workspace_slug,
                workspace_name: w.workspace_name,
                workspace_role: w.workspace_role,
                joined_at: w.joined_at.to_rfc3339(),
            })
            .collect(),
    }))
}

pub async fn update_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.name.is_none()
        && req.email.is_none()
        && req.role.is_none()
        && req.entity_type.is_none()
        && req.agent_type.is_none()
        && req.agent_config.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    if let Some(name) = &req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }
    }

    let normalized_email = req.email.as_ref().map(|email| email.trim().to_lowercase());
    if let Some(email) = &normalized_email {
        if email.is_empty() {
            return Err(ApiError::BadRequest("email cannot be empty".to_string()));
        }

        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM users WHERE email = $1 AND id != $2".to_string(),
                vec![email.clone().into(), user_id.into()],
            ))
            .await?;
        if existing.is_some() {
            return Err(ApiError::Conflict("email already exists".to_string()));
        }
    }

    if let Some(role) = &req.role {
        if role != "admin" && role != "user" {
            return Err(ApiError::BadRequest(
                "role must be 'admin' or 'user'".to_string(),
            ));
        }
    }

    let current_user = find_user_by_id(&state, user_id).await?;
    let target_entity_type = if let Some(entity_type) = req.entity_type.as_deref() {
        normalize_entity_type(Some(entity_type))?
    } else {
        current_user.entity_type.clone()
    };

    if req.agent_type.is_some() {
        let normalized_agent_type = normalize_agent_type(req.agent_type.clone())?;
        if target_entity_type == "human" && normalized_agent_type.is_some() {
            return Err(ApiError::BadRequest(
                "agent_type is only allowed for bot user".to_string(),
            ));
        }
    }

    if target_entity_type == "bot" && req.agent_type.is_none() && current_user.agent_type.is_none() {
        return Err(ApiError::BadRequest(
            "agent_type is required for bot user".to_string(),
        ));
    }

    let mut set_parts: Vec<String> = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx: i64 = 1;

    if let Some(name) = req.name {
        set_parts.push(format!("name = ${}", param_idx));
        values.push(name.trim().to_string().into());
        param_idx += 1;
    }

    if let Some(email) = normalized_email {
        set_parts.push(format!("email = ${}", param_idx));
        values.push(email.into());
        param_idx += 1;
    }

    if let Some(role) = req.role {
        set_parts.push(format!("role = ${}", param_idx));
        values.push(role.into());
        param_idx += 1;
    }

    if let Some(entity_type) = req.entity_type {
        let normalized = normalize_entity_type(Some(&entity_type))?;
        set_parts.push(format!("entity_type = ${}", param_idx));
        values.push(normalized.into());
        param_idx += 1;
    }

    if req.agent_type.is_some() {
        let normalized = normalize_agent_type(req.agent_type)?;
        set_parts.push(format!("agent_type = ${}", param_idx));
        values.push(normalized.into());
        param_idx += 1;
    }

    if req.agent_config.is_some() {
        set_parts.push(format!("agent_config = ${}", param_idx));
        values.push(req.agent_config.into());
        param_idx += 1;
    }

    set_parts.push(format!("updated_at = ${}", param_idx));
    values.push(chrono::Utc::now().into());
    param_idx += 1;
    values.push(user_id.into());

    let update_sql = format!(
        "UPDATE users SET {} WHERE id = ${}",
        set_parts.join(", "),
        param_idx
    );
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &update_sql,
            values,
        ))
        .await?;

    let user = find_user_by_id(&state, user_id).await?;
    Ok(ApiResponse::success(map_user_summary(user)))
}

pub async fn toggle_user_status(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<ToggleUserStatusRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let is_active = if let Some(is_active) = req.is_active {
        is_active
    } else {
        let user = find_user_by_id(&state, user_id).await?;
        !user.is_active
    };

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE users SET is_active = $1, updated_at = now() WHERE id = $2".to_string(),
            vec![is_active.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("user not found".to_string()));
    }

    let user = find_user_by_id(&state, user_id).await?;
    Ok(ApiResponse::success(map_user_summary(user)))
}

pub async fn reset_user_password(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.new_password.trim().len() < 8 {
        return Err(ApiError::BadRequest(
            "password must be at least 8 characters".to_string(),
        ));
    }

    let password_hash = hash(req.new_password, DEFAULT_COST).map_err(|_| ApiError::Internal)?;
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2".to_string(),
            vec![password_hash.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("user not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

pub async fn delete_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE users SET is_active = false, updated_at = now() WHERE id = $1".to_string(),
            vec![user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("user not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

pub async fn get_stats(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let total_users = fetch_count(&state, "SELECT COUNT(*)::bigint as total FROM users").await?;
    let total_workspaces =
        fetch_count(&state, "SELECT COUNT(*)::bigint as total FROM workspaces").await?;
    let total_projects =
        fetch_count(&state, "SELECT COUNT(*)::bigint as total FROM projects").await?;
    let total_issues =
        fetch_count(&state, "SELECT COUNT(*)::bigint as total FROM work_items").await?;
    let active_users_last_30d = fetch_count(
        &state,
        "SELECT COUNT(*)::bigint as total FROM users WHERE is_active = true AND updated_at >= now() - interval '30 days'",
    )
    .await?;

    Ok(ApiResponse::success(AdminStats {
        total_users,
        total_workspaces,
        total_projects,
        total_issues,
        active_users_last_30d,
    }))
}

async fn fetch_count(state: &AppState, sql: &str) -> Result<i64, ApiError> {
    let row =
        CountRow::find_by_statement(Statement::from_string(DbBackend::Postgres, sql.to_string()))
            .one(&state.db)
            .await?
            .ok_or(ApiError::Internal)?;
    Ok(row.total)
}

async fn find_user_by_id(state: &AppState, user_id: Uuid) -> Result<UserRow, ApiError> {
    UserRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, email, name, role, is_active, entity_type, agent_type, agent_config, created_at, updated_at FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("user not found".to_string()))
}

fn map_user_summary(row: UserRow) -> UserSummary {
    UserSummary {
        id: row.id,
        email: row.email,
        name: row.name,
        role: row.role,
        is_active: row.is_active,
        entity_type: row.entity_type,
        agent_type: row.agent_type,
        agent_config: row.agent_config,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn build_search_where(
    search: &Option<String>,
    entity_type: &Option<String>,
) -> Result<(String, Vec<sea_orm::Value>), ApiError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx: i64 = 1;

    if let Some(keyword) = search {
        let trimmed = keyword.trim();
        if !trimmed.is_empty() {
            clauses.push(format!("(email ILIKE ${0} OR name ILIKE ${0})", idx));
            values.push(format!("%{}%", trimmed).into());
            idx += 1;
        }
    }

    if let Some(raw_entity_type) = entity_type {
        let normalized = normalize_entity_type(Some(raw_entity_type.as_str()))?;
        clauses.push(format!("entity_type = ${}", idx));
        values.push(normalized.into());
    }

    if clauses.is_empty() {
        Ok(("".to_string(), values))
    } else {
        Ok((format!("WHERE {}", clauses.join(" AND ")), values))
    }
}

fn normalize_entity_type(entity_type: Option<&str>) -> Result<String, ApiError> {
    let normalized = entity_type.unwrap_or("human").trim().to_lowercase();
    if normalized != "human" && normalized != "bot" {
        return Err(ApiError::BadRequest(
            "entity_type must be 'human' or 'bot'".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_agent_type(agent_type: Option<String>) -> Result<Option<String>, ApiError> {
    if let Some(raw) = agent_type {
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            return Ok(None);
        }
        if normalized != "openclaw" && normalized != "webhook" && normalized != "custom" {
            return Err(ApiError::BadRequest(
                "agent_type must be 'openclaw', 'webhook' or 'custom'".to_string(),
            ));
        }
        Ok(Some(normalized))
    } else {
        Ok(None)
    }
}
