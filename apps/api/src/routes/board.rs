use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Serialize)]
pub struct BoardColumn {
    pub state: String,
    pub issues: Vec<BoardIssue>,
}

#[derive(Debug, Serialize)]
pub struct BoardIssue {
    pub id: Uuid,
    pub title: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub labels: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct BoardResponse {
    pub project_id: Uuid,
    pub columns: Vec<BoardColumn>,
}

/// GET /api/v1/projects/:project_id/board - Get kanban board view
pub async fn get_project_board(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify access
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

    // Get all issues with labels
    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        title: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        assignee_name: Option<String>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let issues = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.id, wi.title, wi.state, wi.priority, wi.assignee_id,
                   u.name as assignee_name, wi.updated_at
            FROM work_items wi
            LEFT JOIN users u ON wi.assignee_id = u.id
            WHERE wi.project_id = $1
            ORDER BY wi.updated_at DESC
        "#,
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    // Get labels for each issue
    #[derive(Debug, FromQueryResult)]
    struct IssueLabelRow {
        work_item_id: Uuid,
        label_name: String,
    }

    let issue_labels = IssueLabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wil.work_item_id, l.name as label_name
            FROM work_item_labels wil
            INNER JOIN labels l ON wil.label_id = l.id
            WHERE wil.work_item_id IN (SELECT id FROM work_items WHERE project_id = $1)
        "#,
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    // Build label map
    let mut label_map: HashMap<Uuid, Vec<String>> = HashMap::new();
    for il in issue_labels {
        label_map
            .entry(il.work_item_id)
            .or_insert_with(Vec::new)
            .push(il.label_name);
    }

    // Group issues by state
    let mut columns_map: HashMap<String, Vec<BoardIssue>> = HashMap::new();
    for issue in issues {
        let labels = label_map.get(&issue.id).cloned().unwrap_or_default();
        let board_issue = BoardIssue {
            id: issue.id,
            title: issue.title,
            priority: issue.priority,
            assignee_id: issue.assignee_id,
            assignee_name: issue.assignee_name,
            labels,
            updated_at: issue.updated_at.to_rfc3339(),
        };
        columns_map
            .entry(issue.state)
            .or_insert_with(Vec::new)
            .push(board_issue);
    }

    // Build ordered columns (todo, in_progress, done)
    let ordered_states = vec!["todo", "in_progress", "done"];
    let mut columns = Vec::new();
    for state in ordered_states {
        columns.push(BoardColumn {
            state: state.to_string(),
            issues: columns_map.remove(state).unwrap_or_default(),
        });
    }

    Ok(ApiResponse::success(BoardResponse {
        project_id,
        columns,
    }))
}
