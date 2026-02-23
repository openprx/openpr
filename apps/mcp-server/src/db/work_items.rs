use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct WorkItemInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub state: String,
    pub priority: String,
    pub assignee_id: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_work_items(
    state: &AppState,
    project_id: Uuid,
) -> Result<Vec<WorkItemInfo>, String> {
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    id::text,
                    project_id::text,
                    title,
                    description,
                    state,
                    priority,
                    assignee_id::text,
                    due_at::text,
                    created_at::text,
                    updated_at::text
                FROM work_items 
                WHERE project_id = $1
                ORDER BY created_at DESC
            "#,
            vec![project_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let items = rows
        .into_iter()
        .map(|row| WorkItemInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(items)
}

pub async fn get_work_item(
    state: &AppState,
    work_item_id: Uuid,
) -> Result<Option<WorkItemInfo>, String> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    id::text,
                    project_id::text,
                    title,
                    description,
                    state,
                    priority,
                    assignee_id::text,
                    due_at::text,
                    created_at::text,
                    updated_at::text
                FROM work_items 
                WHERE id = $1
            "#,
            vec![work_item_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    match row {
        Some(r) => Ok(Some(
            WorkItemInfo::from_query_result(&r, "").map_err(|e| e.to_string())?,
        )),
        None => Ok(None),
    }
}

pub async fn create_work_item(
    state: &AppState,
    project_id: Uuid,
    title: String,
    description: String,
    state_val: String,
    priority: String,
    assignee_id: Option<Uuid>,
    due_at: Option<String>,
    created_by: Option<Uuid>,
) -> Result<WorkItemInfo, String> {
    let work_item_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let due_at_parsed = if let Some(d) = due_at.as_ref() {
        Some(
            chrono::DateTime::parse_from_rfc3339(d)
                .map_err(|e| format!("Invalid due_at format: {}", e))?
                .with_timezone(&chrono::Utc),
        )
    } else {
        None
    };

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO work_items (id, project_id, title, description, state, priority, assignee_id, due_at, created_by, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
            vec![
                work_item_id.into(),
                project_id.into(),
                title.clone().into(),
                description.clone().into(),
                state_val.clone().into(),
                priority.clone().into(),
                assignee_id.into(),
                due_at_parsed.into(),
                created_by.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create work item: {}", e))?;

    Ok(WorkItemInfo {
        id: work_item_id.to_string(),
        project_id: project_id.to_string(),
        title,
        description,
        state: state_val,
        priority,
        assignee_id: assignee_id.map(|id| id.to_string()),
        due_at: due_at_parsed.map(|d| d.to_rfc3339()),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    })
}

pub async fn update_work_item(
    state: &AppState,
    work_item_id: Uuid,
    title: Option<String>,
    description: Option<String>,
    state_val: Option<String>,
    priority: Option<String>,
    assignee_id: Option<Option<Uuid>>,
    due_at: Option<Option<String>>,
) -> Result<WorkItemInfo, String> {
    let now = chrono::Utc::now();

    let mut updates = vec![];
    let mut values: Vec<sea_orm::Value> = vec![];
    let mut param_idx = 1;

    if let Some(t) = title.as_ref() {
        updates.push(format!("title = ${}", param_idx));
        values.push(t.clone().into());
        param_idx += 1;
    }

    if let Some(d) = description.as_ref() {
        updates.push(format!("description = ${}", param_idx));
        values.push(d.clone().into());
        param_idx += 1;
    }

    if let Some(s) = state_val.as_ref() {
        updates.push(format!("state = ${}", param_idx));
        values.push(s.clone().into());
        param_idx += 1;
    }

    if let Some(p) = priority.as_ref() {
        updates.push(format!("priority = ${}", param_idx));
        values.push(p.clone().into());
        param_idx += 1;
    }

    if let Some(a) = assignee_id {
        updates.push(format!("assignee_id = ${}", param_idx));
        values.push(a.into());
        param_idx += 1;
    }

    if let Some(d) = due_at {
        let due_at_parsed = if let Some(date_str) = d {
            Some(
                chrono::DateTime::parse_from_rfc3339(&date_str)
                    .map_err(|e| format!("Invalid due_at format: {}", e))?
                    .with_timezone(&chrono::Utc),
            )
        } else {
            None
        };
        updates.push(format!("due_at = ${}", param_idx));
        values.push(due_at_parsed.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err("No fields to update".to_string());
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(now.into());
    param_idx += 1;

    values.push(work_item_id.into());

    let sql = format!(
        r#"UPDATE work_items SET {} WHERE id = ${} 
           RETURNING id::text, project_id::text, title, description, state, priority, 
                     assignee_id::text, due_at::text, created_at::text, updated_at::text"#,
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
        .map_err(|e| format!("Failed to update work item: {}", e))?;

    match row {
        Some(r) => Ok(WorkItemInfo::from_query_result(&r, "").map_err(|e| e.to_string())?),
        None => Err("Work item not found".to_string()),
    }
}

pub async fn search_work_items(
    state: &AppState,
    workspace_id: Uuid,
    query: &str,
) -> Result<Vec<WorkItemInfo>, String> {
    let search_pattern = format!("%{}%", query);

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    wi.id::text,
                    wi.project_id::text,
                    wi.title,
                    wi.description,
                    wi.state,
                    wi.priority,
                    wi.assignee_id::text,
                    wi.due_at::text,
                    wi.created_at::text,
                    wi.updated_at::text
                FROM work_items wi
                JOIN projects p ON wi.project_id = p.id
                WHERE p.workspace_id = $1
                  AND (wi.title ILIKE $2 OR wi.description ILIKE $2)
                ORDER BY wi.created_at DESC
                LIMIT 50
            "#,
            vec![workspace_id.into(), search_pattern.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let items = rows
        .into_iter()
        .map(|row| WorkItemInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(items)
}

pub async fn delete_work_item(state: &AppState, work_item_id: Uuid) -> Result<bool, String> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_items WHERE id = $1",
            vec![work_item_id.into()],
        ))
        .await
        .map_err(|e| format!("Failed to delete work item: {}", e))?;

    Ok(result.rows_affected() > 0)
}
