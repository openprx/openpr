use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct ListRecordCommentsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRecordCommentRequest {
    pub body: Option<String>,
    pub content: Option<String>,
    pub metadata: Option<Value>,
}

impl CreateRecordCommentRequest {
    pub fn resolved_body(&self) -> &str {
        self.body.as_deref().or(self.content.as_deref()).unwrap_or_default()
    }
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct FormRecordCommentResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub record_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub body: String,
    pub metadata: Value,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordCommentPage {
    pub page: i64,
    pub per_page: i64,
    pub offset: i64,
}

pub struct InsertRecordCommentInput {
    pub comment_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub record_id: Uuid,
    pub author_id: Option<Uuid>,
    pub body: String,
    pub metadata: Value,
}

pub fn record_comment_page(query: &ListRecordCommentsQuery) -> RecordCommentPage {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    RecordCommentPage { page, per_page, offset }
}

pub fn required_record_comment_body(req: &CreateRecordCommentRequest) -> Result<String, ApiError> {
    let value = req.resolved_body().trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest("body is required".to_string()));
    }
    Ok(value.to_string())
}

pub async fn count_record_comments(state: &AppState, record_id: Uuid) -> Result<i64, ApiError> {
    state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM form_record_comments WHERE record_id = $1 AND archived_at IS NULL",
            vec![record_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")
        .map_err(ApiError::from)
}

pub async fn list_record_comments(
    state: &AppState,
    record_id: Uuid,
    page: &RecordCommentPage,
) -> Result<Vec<FormRecordCommentResponse>, ApiError> {
    FormRecordCommentResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT comments.id, comments.workspace_id, comments.project_id, comments.form_id,
                   comments.record_id, comments.author_id, users.name AS author_name,
                   users.email AS author_email, comments.body, comments.metadata,
                   comments.archived_at, comments.created_at, comments.updated_at
            FROM form_record_comments comments
            LEFT JOIN users ON users.id = comments.author_id
            WHERE comments.record_id = $1 AND comments.archived_at IS NULL
            ORDER BY comments.created_at DESC
            LIMIT $2 OFFSET $3
        ",
        vec![record_id.into(), page.per_page.into(), page.offset.into()],
    ))
    .all(&state.db)
    .await
    .map_err(ApiError::from)
}

pub async fn insert_record_comment(tx: &DatabaseTransaction, input: InsertRecordCommentInput) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO form_record_comments (
                id, workspace_id, project_id, form_id, record_id, author_id, body,
                metadata, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), now())
        ",
        vec![
            input.comment_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.form_id.into(),
            input.record_id.into(),
            input.author_id.into(),
            input.body.into(),
            input.metadata.into(),
        ],
    ))
    .await?;
    Ok(())
}

pub async fn find_record_comment(state: &AppState, comment_id: Uuid) -> Result<FormRecordCommentResponse, ApiError> {
    FormRecordCommentResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT comments.id, comments.workspace_id, comments.project_id, comments.form_id,
                   comments.record_id, comments.author_id, users.name AS author_name,
                   users.email AS author_email, comments.body, comments.metadata,
                   comments.archived_at, comments.created_at, comments.updated_at
            FROM form_record_comments comments
            LEFT JOIN users ON users.id = comments.author_id
            WHERE comments.id = $1
        ",
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form record comment not found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        CreateRecordCommentRequest, ListRecordCommentsQuery, record_comment_page, required_record_comment_body,
    };
    use serde_json::json;

    #[test]
    fn normalizes_comment_pagination() {
        let page = record_comment_page(&ListRecordCommentsQuery {
            page: Some(-4),
            per_page: Some(500),
        });
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 100);
        assert_eq!(page.offset, 0);

        let page = record_comment_page(&ListRecordCommentsQuery {
            page: Some(3),
            per_page: Some(25),
        });
        assert_eq!(page.offset, 50);
    }

    #[test]
    fn resolves_comment_body_from_body_or_content() {
        let req = CreateRecordCommentRequest {
            body: Some("  direct body  ".to_string()),
            content: Some("fallback".to_string()),
            metadata: Some(json!({})),
        };
        assert_eq!(required_record_comment_body(&req).unwrap(), "direct body");

        let req = CreateRecordCommentRequest {
            body: None,
            content: Some(" legacy content ".to_string()),
            metadata: None,
        };
        assert_eq!(required_record_comment_body(&req).unwrap(), "legacy content");
    }

    #[test]
    fn rejects_blank_comment_body() {
        let req = CreateRecordCommentRequest {
            body: Some("   ".to_string()),
            content: None,
            metadata: None,
        };
        assert!(required_record_comment_body(&req).is_err());
    }
}
