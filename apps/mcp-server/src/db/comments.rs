use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct CommentInfo {
    pub id: String,
    pub work_item_id: String,
    pub content: String,
    pub author_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_comments(
    state: &AppState,
    work_item_id: Uuid,
) -> Result<Vec<CommentInfo>, String> {
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 
                    id::text,
                    work_item_id::text,
                    body as content,
                    author_id::text,
                    created_at::text,
                    updated_at::text
                FROM comments 
                WHERE work_item_id = $1
                ORDER BY created_at ASC
            "#,
            vec![work_item_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let comments = rows
        .into_iter()
        .map(|row| CommentInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(comments)
}

pub async fn create_comment(
    state: &AppState,
    work_item_id: Uuid,
    content: String,
    author_id: Option<Uuid>,
) -> Result<CommentInfo, String> {
    // Verify work item exists
    let work_item_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM work_items WHERE id = $1",
            vec![work_item_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    if work_item_exists.is_none() {
        return Err("Work item not found".to_string());
    }

    let comment_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO comments (id, work_item_id, body, author_id, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            vec![
                comment_id.into(),
                work_item_id.into(),
                content.clone().into(),
                author_id.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create comment: {}", e))?;

    Ok(CommentInfo {
        id: comment_id.to_string(),
        work_item_id: work_item_id.to_string(),
        content,
        author_id: author_id.map(|id| id.to_string()),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    })
}

pub async fn delete_comment(state: &AppState, comment_id: Uuid) -> Result<bool, String> {
    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM comments WHERE id = $1",
            vec![comment_id.into()],
        ))
        .await
        .map_err(|e| format!("Failed to delete comment: {}", e))?;

    Ok(result.rows_affected() > 0)
}
