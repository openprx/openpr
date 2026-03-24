use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use crate::{
    error::ApiError, response::ApiResponse, services::workflow_service::resolve_effective_workflow_for_project,
};

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
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

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
        r"
            SELECT wi.id, wi.title, wi.state, wi.priority, wi.assignee_id,
                   u.name as assignee_name, wi.updated_at
            FROM work_items wi
            LEFT JOIN users u ON wi.assignee_id = u.id
            WHERE wi.project_id = $1
            ORDER BY wi.updated_at DESC
        ",
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
        r"
            SELECT wil.work_item_id, l.name as label_name
            FROM work_item_labels wil
            INNER JOIN labels l ON wil.label_id = l.id
            WHERE wil.work_item_id IN (SELECT id FROM work_items WHERE project_id = $1)
        ",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    // Build label map
    let mut label_map: HashMap<Uuid, Vec<String>> = HashMap::new();
    for il in issue_labels {
        label_map.entry(il.work_item_id).or_default().push(il.label_name);
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
        columns_map.entry(issue.state).or_default().push(board_issue);
    }

    // Build ordered columns by effective workflow states.
    let workflow = resolve_effective_workflow_for_project(&state, project_id).await?;
    let mut columns = Vec::new();
    for state_def in &workflow.states {
        columns.push(BoardColumn {
            state: state_def.key.clone(),
            issues: columns_map.remove(&state_def.key).unwrap_or_default(),
        });
    }

    // Keep legacy/unknown states visible at the end for compatibility.
    let mut leftovers: Vec<(String, Vec<BoardIssue>)> = columns_map.into_iter().collect();
    leftovers.sort_by(|a, b| a.0.cmp(&b.0));
    for (state_key, issues) in leftovers {
        columns.push(BoardColumn {
            state: state_key,
            issues,
        });
    }

    Ok(ApiResponse::success(BoardResponse { project_id, columns }))
}
