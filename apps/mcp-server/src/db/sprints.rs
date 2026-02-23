use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct SprintInfo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn create_sprint(
    state: &AppState,
    project_id: Uuid,
    name: String,
    start_date: Option<String>,
    end_date: Option<String>,
) -> Result<SprintInfo, String> {
    let sprint_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let parsed_start = if let Some(s) = start_date.as_ref() {
        Some(
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|e| format!("Invalid start_date format: {}", e))?,
        )
    } else {
        None
    };

    let parsed_end = if let Some(e) = end_date.as_ref() {
        Some(
            chrono::NaiveDate::parse_from_str(e, "%Y-%m-%d")
                .map_err(|err| format!("Invalid end_date format: {}", err))?,
        )
    } else {
        None
    };

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO sprints (
                    id,
                    project_id,
                    name,
                    description,
                    start_date,
                    end_date,
                    status,
                    created_at,
                    updated_at
                ) VALUES ($1, $2, $3, '', $4, $5, 'planned', $6, $7)
            "#,
            vec![
                sprint_id.into(),
                project_id.into(),
                name.clone().into(),
                parsed_start.into(),
                parsed_end.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create sprint: {}", e))?;

    Ok(SprintInfo {
        id: sprint_id.to_string(),
        project_id: project_id.to_string(),
        name,
        description: "".to_string(),
        start_date: parsed_start.map(|d| d.to_string()),
        end_date: parsed_end.map(|d| d.to_string()),
        status: "planned".to_string(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    })
}

pub async fn update_sprint(
    state: &AppState,
    sprint_id: Uuid,
    name: Option<String>,
    status: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
) -> Result<SprintInfo, String> {
    let now = chrono::Utc::now();

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(n) = name {
        updates.push(format!("name = ${}", param_idx));
        values.push(n.into());
        param_idx += 1;
    }

    if let Some(s) = status {
        updates.push(format!("status = ${}", param_idx));
        values.push(s.into());
        param_idx += 1;
    }

    if let Some(s) = start_date {
        let parsed = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|e| format!("Invalid start_date format: {}", e))?;
        updates.push(format!("start_date = ${}", param_idx));
        values.push(parsed.into());
        param_idx += 1;
    }

    if let Some(e) = end_date {
        let parsed = chrono::NaiveDate::parse_from_str(&e, "%Y-%m-%d")
            .map_err(|err| format!("Invalid end_date format: {}", err))?;
        updates.push(format!("end_date = ${}", param_idx));
        values.push(parsed.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err("No fields to update".to_string());
    }

    updates.push(format!("updated_at = ${}", param_idx));
    values.push(now.into());
    param_idx += 1;

    values.push(sprint_id.into());

    let sql = format!(
        r#"UPDATE sprints SET {} WHERE id = ${}
           RETURNING id::text, project_id::text, name, description, start_date::text, end_date::text, status, created_at::text, updated_at::text"#,
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
        .map_err(|e| format!("Failed to update sprint: {}", e))?;

    match row {
        Some(r) => Ok(SprintInfo::from_query_result(&r, "").map_err(|e| e.to_string())?),
        None => Err("Sprint not found".to_string()),
    }
}
