use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>, // 'json' or 'csv', default 'json'
}

#[derive(Debug, Serialize, FromQueryResult)]
struct ProjectExportData {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct IssueExportData {
    pub id: Uuid,
    pub key: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: Option<String>,
    pub issue_type: String,
    pub assignee_id: Option<Uuid>,
    pub reporter_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct CommentExportData {
    pub id: Uuid,
    pub work_item_id: Uuid,
    pub content: String,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct ProjectExport {
    pub project: ProjectExportData,
    pub issues: Vec<IssueExportData>,
    pub comments: Vec<CommentExportData>,
    pub exported_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    format: String,
    filename: String,
    content: String,
}

/// GET /api/v1/export/project/:id
pub async fn export_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify user has access to project
    verify_project_access(&state, project_id, user_id).await?;

    // Get project data
    let project_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, key, name, description, created_at, updated_at FROM projects WHERE id = $1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Project not found".to_string()))?;

    let project = ProjectExportData::from_query_result(&project_row, "")?;

    // Get issues
    let issue_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id, key, title, description, status, priority, type as issue_type, 
                      assignee_id, reporter_id, created_at, updated_at 
               FROM work_items WHERE project_id = $1 ORDER BY created_at"#,
            vec![project_id.into()],
        ))
        .await?;

    let issues: Vec<IssueExportData> = issue_rows
        .iter()
        .map(|r| IssueExportData::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    // Get comments for all issues in the project
    let comment_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT c.id, c.work_item_id, c.content, c.created_by, c.created_at, c.updated_at
               FROM comments c
               INNER JOIN work_items wi ON c.work_item_id = wi.id
               WHERE wi.project_id = $1
               ORDER BY c.created_at"#,
            vec![project_id.into()],
        ))
        .await?;

    let comments: Vec<CommentExportData> = comment_rows
        .iter()
        .map(|r| CommentExportData::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    let export = ProjectExport {
        project,
        issues,
        comments,
        exported_at: chrono::Utc::now(),
    };

    let format = query.format.as_deref().unwrap_or("json");

    match format {
        "json" => {
            let json_data =
                serde_json::to_string_pretty(&export).map_err(|_| ApiError::Internal)?;
            let filename = format!("project_{}_export.json", export.project.key);
            Ok(ApiResponse::success(ExportResponse {
                format: "json".to_string(),
                filename,
                content: json_data,
            })
            .into_response())
        }
        "csv" => {
            // CSV export: create one CSV with all issues
            let mut csv_output = String::new();
            csv_output
                .push_str("Key,Title,Status,Priority,Type,Description,Created At,Updated At\n");

            for issue in &export.issues {
                let description = issue.description.replace('"', "\"\"");
                csv_output.push_str(&format!(
                    "\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\"\n",
                    issue.key,
                    issue.title,
                    issue.status,
                    issue.priority.as_deref().unwrap_or(""),
                    issue.issue_type,
                    description,
                    issue.created_at,
                    issue.updated_at
                ));
            }

            let filename = format!("project_{}_export.csv", export.project.key);
            Ok(ApiResponse::success(ExportResponse {
                format: "csv".to_string(),
                filename,
                content: csv_output,
            })
            .into_response())
        }
        _ => Err(ApiError::BadRequest(
            "Invalid format. Use 'json' or 'csv'".to_string(),
        )),
    }
}

async fn verify_project_access(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT 1 FROM projects p
               INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
               WHERE p.id = $1 AND wm.user_id = $2"#,
            vec![project_id.into(), user_id.into()],
        ))
        .await?;

    if result.is_none() {
        return Err(ApiError::Forbidden("Access denied".to_string()));
    }

    Ok(())
}
