use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Deserialize)]
pub struct ImportIssue {
    pub key: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: Option<String>,
    #[serde(rename = "type")]
    pub issue_type: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportProjectRequest {
    pub project_key: Option<String>, // If provided, import into existing project
    pub project_name: Option<String>, // For new project creation
    pub project_description: Option<String>,
    pub issues: Vec<ImportIssue>,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub project_id: Uuid,
    pub project_key: String,
    pub issues_created: usize,
    pub issues_failed: usize,
    pub errors: Vec<String>,
}

/// POST /api/v1/workspaces/:workspace_id/import/project
pub async fn import_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<ImportProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Verify user is admin of workspace
    verify_workspace_admin(&state, workspace_id, user_id).await?;

    // Start transaction
    let txn = state.db.begin().await?;

    let mut errors = Vec::new();
    let mut issues_created = 0;
    let mut issues_failed = 0;

    // Determine project
    let (project_id, project_key) = if let Some(project_key) = req.project_key {
        // Import into existing project
        let result = txn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2",
                vec![workspace_id.into(), project_key.clone().into()],
            ))
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Project {} not found", project_key)))?;

        let id: Uuid = result.try_get("", "id")?;
        (id, project_key)
    } else {
        // Create new project
        let project_name = req.project_name.ok_or_else(|| {
            ApiError::BadRequest("project_name required for new project".to_string())
        })?;

        let project_key = generate_project_key(&project_name);
        let project_id = Uuid::new_v4();
        let now = chrono::Utc::now();

        txn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO projects (id, workspace_id, key, name, description, created_by, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
            vec![
                project_id.into(),
                workspace_id.into(),
                project_key.clone().into(),
                project_name.into(),
                req.project_description.unwrap_or_default().into(),
                user_id.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

        (project_id, project_key)
    };

    // Get next issue number
    let next_num_result = txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COALESCE(MAX(CAST(SUBSTRING(key FROM '[0-9]+$') AS INTEGER)), 0) + 1 as next_num FROM work_items WHERE project_id = $1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let mut next_num: i32 = next_num_result.try_get("", "next_num")?;

    // Import issues
    for import_issue in req.issues {
        let issue_id = Uuid::new_v4();
        let issue_key = format!("{}-{}", project_key, next_num);
        let now = chrono::Utc::now();
        let title = import_issue.title.clone();

        let result = txn
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO work_items 
                   (id, project_id, key, title, description, status, priority, type, reporter_id, created_at, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
                vec![
                    issue_id.into(),
                    project_id.into(),
                    issue_key.clone().into(),
                    import_issue.title.into(),
                    import_issue.description.into(),
                    import_issue.status.into(),
                    import_issue.priority.into(),
                    import_issue.issue_type.into(),
                    user_id.into(),
                    now.into(),
                    now.into(),
                ],
            ))
            .await;

        match result {
            Ok(_) => {
                issues_created += 1;
                next_num += 1;
            }
            Err(e) => {
                issues_failed += 1;
                errors.push(format!("Failed to import issue '{}': {}", title, e));
            }
        }
    }

    // Commit transaction
    txn.commit().await?;

    let result = ImportResult {
        project_id,
        project_key,
        issues_created,
        issues_failed,
        errors,
    };

    Ok(ApiResponse::success(result))
}

async fn verify_workspace_admin(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;

    match result {
        Some(row) => {
            let role: String = row.try_get("", "role")?;
            if role != "admin" {
                return Err(ApiError::Forbidden("Admin access required".to_string()));
            }
            Ok(())
        }
        None => Err(ApiError::Forbidden("Not a workspace member".to_string())),
    }
}

fn generate_project_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .take(5)
        .collect::<String>()
        .to_uppercase()
}
