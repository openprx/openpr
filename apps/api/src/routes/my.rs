use axum::{
    Extension,
    extract::{Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_per_page")]
    pub per_page: i64,
}

fn default_page() -> i64 {
    1
}

fn default_per_page() -> i64 {
    10
}

#[derive(Debug, Serialize)]
pub struct MyIssueResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub project_name: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct MyActivityResponse {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub project_name: String,
    pub issue_title: String,
    pub user_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub action: String,
    pub detail: serde_json::Value,
    pub created_at: String,
}

/// GET /api/v1/my/issues - List issues assigned to current user across workspaces
pub async fn get_my_issues(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let page = pagination.page.max(1);
    let per_page = pagination.per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;

    #[derive(Debug, FromQueryResult)]
    struct MyIssueRow {
        id: Uuid,
        project_id: Uuid,
        workspace_id: Uuid,
        project_name: String,
        title: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    #[derive(Debug, FromQueryResult)]
    struct CountRow {
        count: i64,
    }

    let total_result = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*) as count
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.assignee_id = $1
              AND (
                EXISTS (
                    SELECT 1
                    FROM workspace_members wm
                    WHERE wm.workspace_id = p.workspace_id
                      AND wm.user_id = $1
                )
                OR EXISTS (
                    SELECT 1
                    FROM workspace_bots wb
                    WHERE wb.workspace_id = p.workspace_id
                      AND wb.id = $1
                )
              )
        "#,
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?;

    let total = total_result.map(|r| r.count).unwrap_or(0);

    let issues = MyIssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.id,
                   wi.project_id,
                   p.workspace_id,
                   p.name AS project_name,
                   wi.title,
                   wi.state,
                   wi.priority,
                   wi.assignee_id,
                   wi.updated_at
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.assignee_id = $1
              AND (
                EXISTS (
                    SELECT 1
                    FROM workspace_members wm
                    WHERE wm.workspace_id = p.workspace_id
                      AND wm.user_id = $1
                )
                OR EXISTS (
                    SELECT 1
                    FROM workspace_bots wb
                    WHERE wb.workspace_id = p.workspace_id
                      AND wb.id = $1
                )
              )
            ORDER BY wi.updated_at DESC
            LIMIT $2 OFFSET $3
        "#,
        vec![
            user_id.into(),
            (per_page as i64).into(),
            (offset as i64).into(),
        ],
    ))
    .all(&state.db)
    .await?;

    let items: Vec<MyIssueResponse> = issues
        .into_iter()
        .map(|issue| MyIssueResponse {
            id: issue.id,
            project_id: issue.project_id,
            workspace_id: issue.workspace_id,
            project_name: issue.project_name,
            title: issue.title,
            state: issue.state,
            priority: issue.priority,
            assignee_id: issue.assignee_id,
            updated_at: issue.updated_at.to_rfc3339(),
        })
        .collect();

    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

/// GET /api/v1/my/activities - List recent workspace activities visible to current user
pub async fn get_my_activities(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let page = pagination.page.max(1);
    let per_page = pagination.per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;

    #[derive(Debug, FromQueryResult)]
    struct MyActivityRow {
        id: Uuid,
        issue_id: Uuid,
        project_id: Uuid,
        workspace_id: Uuid,
        project_name: String,
        issue_title: String,
        user_id: Option<Uuid>,
        author_name: Option<String>,
        action: String,
        detail: serde_json::Value,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    #[derive(Debug, FromQueryResult)]
    struct CountRow {
        count: i64,
    }

    let total_result = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*) as count
            FROM activities a
            INNER JOIN work_items wi ON wi.id = COALESCE(a.issue_id, CASE WHEN a.resource_type = 'issue' THEN a.resource_id END)
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE (
                EXISTS (
                    SELECT 1
                    FROM workspace_members wm
                    WHERE wm.workspace_id = p.workspace_id
                      AND wm.user_id = $1
                )
                OR EXISTS (
                    SELECT 1
                    FROM workspace_bots wb
                    WHERE wb.workspace_id = p.workspace_id
                      AND wb.id = $1
                )
            )
        "#,
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?;

    let total = total_result.map(|r| r.count).unwrap_or(0);

    let activities = MyActivityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT a.id,
                   COALESCE(a.issue_id, CASE WHEN a.resource_type = 'issue' THEN a.resource_id END) AS issue_id,
                   wi.project_id,
                   p.workspace_id,
                   p.name AS project_name,
                   wi.title AS issue_title,
                   COALESCE(a.user_id, a.actor_id) AS user_id,
                   u.name AS author_name,
                   COALESCE(a.action, a.event_type) AS action,
                   COALESCE(a.detail, a.payload, '{}'::jsonb) AS detail,
                   a.created_at
            FROM activities a
            INNER JOIN work_items wi ON wi.id = COALESCE(a.issue_id, CASE WHEN a.resource_type = 'issue' THEN a.resource_id END)
            INNER JOIN projects p ON wi.project_id = p.id
            LEFT JOIN users u ON u.id = COALESCE(a.user_id, a.actor_id)
            WHERE (
                EXISTS (
                    SELECT 1
                    FROM workspace_members wm
                    WHERE wm.workspace_id = p.workspace_id
                      AND wm.user_id = $1
                )
                OR EXISTS (
                    SELECT 1
                    FROM workspace_bots wb
                    WHERE wb.workspace_id = p.workspace_id
                      AND wb.id = $1
                )
            )
            ORDER BY a.created_at DESC
            LIMIT $2 OFFSET $3
        "#,
        vec![user_id.into(), (per_page as i64).into(), (offset as i64).into()],
    ))
    .all(&state.db)
    .await?;

    let items: Vec<MyActivityResponse> = activities
        .into_iter()
        .map(|activity| MyActivityResponse {
            id: activity.id,
            issue_id: activity.issue_id,
            project_id: activity.project_id,
            workspace_id: activity.workspace_id,
            project_name: activity.project_name,
            issue_title: activity.issue_title,
            user_id: activity.user_id,
            author_name: activity.author_name,
            action: activity.action,
            detail: activity.detail,
            created_at: activity.created_at.to_rfc3339(),
        })
        .collect();

    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}
