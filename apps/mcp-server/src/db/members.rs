use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct MemberInfo {
    pub user_id: String,
    pub email: String,
    pub name: String,
    pub entity_type: String,
    pub agent_type: Option<String>,
    pub role: String,
    pub created_at: String,
}

pub async fn list_members(state: &AppState, workspace_id: Uuid) -> Result<Vec<MemberInfo>, String> {
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    wm.user_id::text AS user_id,
                    u.email,
                    u.name,
                    u.entity_type,
                    u.agent_type,
                    wm.role,
                    wm.created_at::text AS created_at
                FROM workspace_members wm
                INNER JOIN users u ON wm.user_id = u.id
                WHERE wm.workspace_id = $1
                ORDER BY wm.created_at ASC
            "#,
            vec![workspace_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let members = rows
        .into_iter()
        .map(|row| MemberInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(members)
}
