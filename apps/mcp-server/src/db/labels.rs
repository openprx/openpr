use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct LabelInfo {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub color: String,
    pub description: String,
    pub created_at: String,
}

pub async fn create_label(
    state: &AppState,
    workspace_id: Uuid,
    name: String,
    color: String,
) -> Result<LabelInfo, String> {
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM labels WHERE workspace_id = $1 AND name = $2",
            vec![workspace_id.into(), name.clone().into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    if existing.is_some() {
        return Err("Label with this name already exists in workspace".to_string());
    }

    let label_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO labels (id, workspace_id, name, color, description, created_at)
                VALUES ($1, $2, $3, $4, '', $5)
            "#,
            vec![
                label_id.into(),
                workspace_id.into(),
                name.clone().into(),
                color.clone().into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create label: {}", e))?;

    Ok(LabelInfo {
        id: label_id.to_string(),
        workspace_id: workspace_id.to_string(),
        name,
        color,
        description: "".to_string(),
        created_at: now.to_rfc3339(),
    })
}
