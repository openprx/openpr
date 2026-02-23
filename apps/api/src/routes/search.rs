use axum::{
    Extension,
    extract::{Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(rename = "type")]
    pub search_type: Option<String>, // 'issue', 'project', 'comment', or empty for all
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct IssueSearchResult {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub state: String,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectSearchResult {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub workspace_id: Uuid,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct CommentSearchResult {
    pub id: Uuid,
    pub body: String,
    pub issue_id: Uuid,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub author_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum SearchResult {
    #[serde(rename = "issue")]
    Issue(IssueSearchResult),
    #[serde(rename = "project")]
    Project(ProjectSearchResult),
    #[serde(rename = "comment")]
    Comment(CommentSearchResult),
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total: usize,
}

/// GET /api/v1/search
pub async fn search(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if query.q.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Search query cannot be empty".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(50).min(100) as i64;
    let search_type = query.search_type.as_deref();

    let mut results = Vec::new();

    // Search issues
    if search_type.is_none() || search_type == Some("issue") {
        let issues = search_issues(&state, user_id, &query.q, limit)
            .await
            .unwrap_or_default();
        results.extend(issues.into_iter().map(SearchResult::Issue));
    }

    // Search projects
    if search_type.is_none() || search_type == Some("project") {
        let projects = search_projects(&state, user_id, &query.q, limit)
            .await
            .unwrap_or_default();
        results.extend(projects.into_iter().map(SearchResult::Project));
    }

    // Search comments
    if search_type.is_none() || search_type == Some("comment") {
        let comments = search_comments(&state, user_id, &query.q, limit)
            .await
            .unwrap_or_default();
        results.extend(comments.into_iter().map(SearchResult::Comment));
    }

    let total = results.len();

    Ok(ApiResponse::success(SearchResponse {
        query: query.q,
        results,
        total,
    }))
}

async fn search_issues(
    state: &AppState,
    user_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<IssueSearchResult>, ApiError> {
    let pattern = format!("%{}%", query);
    let sql = r#"
        SELECT wi.id, wi.title, wi.description, wi.state, wi.project_id, p.workspace_id
        FROM work_items wi
        INNER JOIN projects p ON wi.project_id = p.id
        INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
        WHERE wm.user_id = $1
          AND (wi.title ILIKE $2 OR wi.description ILIKE $2)
        ORDER BY wi.updated_at DESC
        LIMIT $3
    "#;

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![user_id.into(), pattern.into(), limit.into()],
        ))
        .await?;

    rows.iter()
        .map(|r| IssueSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn search_projects(
    state: &AppState,
    user_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<ProjectSearchResult>, ApiError> {
    let pattern = format!("%{}%", query);
    let sql = r#"
        SELECT p.id, p.key, p.name, p.description, p.workspace_id
        FROM projects p
        INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
        WHERE wm.user_id = $1
          AND (p.name ILIKE $2 OR p.key ILIKE $2 OR p.description ILIKE $2)
        ORDER BY p.updated_at DESC
        LIMIT $3
    "#;

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![user_id.into(), pattern.into(), limit.into()],
        ))
        .await?;

    rows.iter()
        .map(|r| ProjectSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn search_comments(
    state: &AppState,
    user_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<CommentSearchResult>, ApiError> {
    let pattern = format!("%{}%", query);
    let sql = r#"
        SELECT c.id, c.body, c.work_item_id AS issue_id, wi.project_id, p.workspace_id, c.author_id, c.created_at
        FROM comments c
        INNER JOIN work_items wi ON c.work_item_id = wi.id
        INNER JOIN projects p ON wi.project_id = p.id
        INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
        WHERE wm.user_id = $1
          AND c.body ILIKE $2
        ORDER BY c.created_at DESC
        LIMIT $3
    "#;

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![user_id.into(), pattern.into(), limit.into()],
        ))
        .await?;

    rows.iter()
        .map(|r| CommentSearchResult::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}
