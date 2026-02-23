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
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Serialize)]
pub struct ActivityResponse {
    pub id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub event_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub actor_email: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct IssueActivityResponse {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub user_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub action: String,
    pub detail: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ListActivitiesQuery {
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/v1/workspaces/:workspace_id/activities - Get workspace activity feed
pub async fn get_workspace_activities(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListActivitiesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify user is workspace member
    let is_member = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;

    if is_member.is_none() {
        return Err(ApiError::NotFound(
            "workspace not found or access denied".to_string(),
        ));
    }

    // Build query to get activities for workspace resources
    let mut where_clauses = Vec::new();
    let mut values: Vec<sea_orm::Value> = vec![workspace_id.into()];
    let mut param_idx = 2;

    // Base condition: activities for workspace, projects in workspace, or issues in projects
    where_clauses.push(
        "(a.resource_type = 'workspace' AND a.resource_id = $1) OR \
         (a.resource_type = 'project' AND a.resource_id IN (SELECT id FROM projects WHERE workspace_id = $1)) OR \
         (a.resource_type = 'issue' AND a.resource_id IN (SELECT wi.id FROM work_items wi INNER JOIN projects p ON wi.project_id = p.id WHERE p.workspace_id = $1))".to_string()
    );

    if let Some(resource_type) = query.resource_type {
        where_clauses.push(format!("a.resource_type = ${}", param_idx));
        values.push(resource_type.into());
        param_idx += 1;
    }

    if let Some(resource_id) = query.resource_id {
        where_clauses.push(format!("a.resource_id = ${}", param_idx));
        values.push(resource_id.into());
        param_idx += 1;
    }

    if let Some(event_type) = query.event_type {
        where_clauses.push(format!("a.event_type = ${}", param_idx));
        values.push(event_type.into());
        param_idx += 1;
    }

    let limit = query.limit.unwrap_or(50).min(200);

    let sql = format!(
        r#"
        SELECT a.id, a.resource_type, a.resource_id, a.event_type, a.actor_id,
               u.name as actor_name, u.email as actor_email,
               a.payload, a.created_at
        FROM activities a
        LEFT JOIN users u ON a.actor_id = u.id
        WHERE {}
        ORDER BY a.created_at DESC
        LIMIT {}
        "#,
        where_clauses.join(" AND "),
        limit
    );

    #[derive(Debug, FromQueryResult)]
    struct ActivityRow {
        id: Uuid,
        resource_type: String,
        resource_id: Uuid,
        event_type: String,
        actor_id: Option<Uuid>,
        actor_name: Option<String>,
        actor_email: Option<String>,
        payload: serde_json::Value,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let activities = ActivityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        values,
    ))
    .all(&state.db)
    .await?;

    let response: Vec<ActivityResponse> = activities
        .into_iter()
        .map(|a| ActivityResponse {
            id: a.id,
            resource_type: a.resource_type,
            resource_id: a.resource_id,
            event_type: a.event_type,
            actor_id: a.actor_id,
            actor_name: a.actor_name,
            actor_email: a.actor_email,
            payload: a.payload,
            created_at: a.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// GET /api/v1/projects/:project_id/activities - Get project activity feed
pub async fn get_project_activities(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListActivitiesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify user has access to project
    let has_access = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = $1 AND wm.user_id = $2
            "#,
            vec![project_id.into(), user_id.into()],
        ))
        .await?;

    if has_access.is_none() {
        return Err(ApiError::NotFound(
            "project not found or access denied".to_string(),
        ));
    }

    // Build query
    let mut where_clauses = vec![
        "(a.resource_type = 'project' AND a.resource_id = $1) OR \
         (a.resource_type = 'issue' AND a.resource_id IN (SELECT id FROM work_items WHERE project_id = $1))"
            .to_string(),
    ];
    let mut values: Vec<sea_orm::Value> = vec![project_id.into()];
    let mut param_idx = 2;

    if let Some(resource_type) = query.resource_type {
        where_clauses.push(format!("a.resource_type = ${}", param_idx));
        values.push(resource_type.into());
        param_idx += 1;
    }

    if let Some(resource_id) = query.resource_id {
        where_clauses.push(format!("a.resource_id = ${}", param_idx));
        values.push(resource_id.into());
        param_idx += 1;
    }

    if let Some(event_type) = query.event_type {
        where_clauses.push(format!("a.event_type = ${}", param_idx));
        values.push(event_type.into());
    }

    let limit = query.limit.unwrap_or(50).min(200);

    let sql = format!(
        r#"
        SELECT a.id, a.resource_type, a.resource_id, a.event_type, a.actor_id,
               u.name as actor_name, u.email as actor_email,
               a.payload, a.created_at
        FROM activities a
        LEFT JOIN users u ON a.actor_id = u.id
        WHERE {}
        ORDER BY a.created_at DESC
        LIMIT {}
        "#,
        where_clauses.join(" AND "),
        limit
    );

    #[derive(Debug, FromQueryResult)]
    struct ActivityRow {
        id: Uuid,
        resource_type: String,
        resource_id: Uuid,
        event_type: String,
        actor_id: Option<Uuid>,
        actor_name: Option<String>,
        actor_email: Option<String>,
        payload: serde_json::Value,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let activities = ActivityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        values,
    ))
    .all(&state.db)
    .await?;

    let response: Vec<ActivityResponse> = activities
        .into_iter()
        .map(|a| ActivityResponse {
            id: a.id,
            resource_type: a.resource_type,
            resource_id: a.resource_id,
            event_type: a.event_type,
            actor_id: a.actor_id,
            actor_name: a.actor_name,
            actor_email: a.actor_email,
            payload: a.payload,
            created_at: a.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// GET /api/v1/issues/:issue_id/activities - Get issue activity feed
pub async fn get_issue_activities(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify user has access to issue
    let has_access = state
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

    if has_access.is_none() {
        return Err(ApiError::NotFound(
            "issue not found or access denied".to_string(),
        ));
    }

    #[derive(Debug, FromQueryResult)]
    struct ActivityRow {
        id: Uuid,
        issue_id: Uuid,
        user_id: Option<Uuid>,
        author_name: Option<String>,
        action: String,
        detail: serde_json::Value,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let activities = ActivityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT a.id,
                   COALESCE(a.issue_id, a.resource_id) AS issue_id,
                   COALESCE(a.user_id, a.actor_id) AS user_id,
                   u.name AS author_name,
                   COALESCE(a.action, a.event_type) AS action,
                   COALESCE(a.detail, a.payload, '{}'::jsonb) AS detail,
                   a.created_at
            FROM activities a
            LEFT JOIN users u ON u.id = COALESCE(a.user_id, a.actor_id)
            WHERE (a.issue_id = $1) OR (a.resource_type = 'issue' AND a.resource_id = $1)
            ORDER BY a.created_at DESC
            LIMIT 100
        "#,
        vec![issue_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<IssueActivityResponse> = activities
        .into_iter()
        .map(|a| IssueActivityResponse {
            id: a.id,
            issue_id: a.issue_id,
            user_id: a.user_id,
            actor_id: a.user_id,
            author_name: a.author_name,
            action: a.action,
            detail: a.detail,
            created_at: a.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}
