use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct ProjectInfo {
    pub id: String,
    pub workspace_id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_projects(
    state: &AppState,
    workspace_id: Uuid,
) -> Result<Vec<ProjectInfo>, String> {
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    id::text, 
                    workspace_id::text, 
                    key, 
                    name, 
                    description,
                    created_at::text,
                    updated_at::text
                FROM projects 
                WHERE workspace_id = $1
                ORDER BY created_at DESC
            "#,
            vec![workspace_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let projects = rows
        .into_iter()
        .map(|row| ProjectInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(projects)
}

pub async fn get_project(
    state: &AppState,
    project_id: Uuid,
) -> Result<Option<ProjectInfo>, String> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    id::text, 
                    workspace_id::text, 
                    key, 
                    name, 
                    description,
                    created_at::text,
                    updated_at::text
                FROM projects 
                WHERE id = $1
            "#,
            vec![project_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    match row {
        Some(r) => Ok(Some(
            ProjectInfo::from_query_result(&r, "").map_err(|e| e.to_string())?,
        )),
        None => Ok(None),
    }
}

pub async fn create_project(
    state: &AppState,
    workspace_id: Uuid,
    key: String,
    name: String,
    description: String,
    created_by: Option<Uuid>,
) -> Result<ProjectInfo, String> {
    // Validate key format
    if !key.chars().all(|c| c.is_ascii_uppercase()) {
        return Err("Project key must contain only uppercase letters".to_string());
    }

    // Check if key exists
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2",
            vec![workspace_id.into(), key.clone().into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    if existing.is_some() {
        return Err("Project key already exists in this workspace".to_string());
    }

    let project_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO projects (id, workspace_id, key, name, description, created_by, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            vec![
                project_id.into(),
                workspace_id.into(),
                key.clone().into(),
                name.clone().into(),
                description.clone().into(),
                created_by.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create project: {}", e))?;

    Ok(ProjectInfo {
        id: project_id.to_string(),
        workspace_id: workspace_id.to_string(),
        key,
        name,
        description,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    })
}

pub async fn update_project(
    state: &AppState,
    project_id: Uuid,
    name: Option<String>,
    description: Option<String>,
) -> Result<ProjectInfo, String> {
    let now = chrono::Utc::now();

    let mut updates = vec![];
    let mut values: Vec<sea_orm::Value> = vec![];
    let mut param_idx = 1;

    if let Some(n) = name.as_ref() {
        updates.push(format!("name = ${}", param_idx));
        values.push(n.clone().into());
        param_idx += 1;
    }

    if let Some(d) = description.as_ref() {
        updates.push(format!("description = ${}", param_idx));
        values.push(d.clone().into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err("No fields to update".to_string());
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(now.into());
    param_idx += 1;

    values.push(project_id.into());

    let sql = format!(
        "UPDATE projects SET {} WHERE id = ${} RETURNING id::text, workspace_id::text, key, name, description, created_at::text, updated_at::text",
        updates.join(", "),
        param_idx
    );

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            values,
        ))
        .await
        .map_err(|e| format!("Failed to update project: {}", e))?;

    match row {
        Some(r) => Ok(ProjectInfo::from_query_result(&r, "").map_err(|e| e.to_string())?),
        None => Err("Project not found".to_string()),
    }
}

pub async fn delete_project(state: &AppState, project_id: Uuid) -> Result<bool, String> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM projects WHERE id = $1",
            vec![project_id.into()],
        ))
        .await
        .map_err(|e| format!("Failed to delete project: {}", e))?;

    Ok(result.rows_affected() > 0)
}
