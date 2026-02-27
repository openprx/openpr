use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct NotificationResponse {
    pub id: Uuid,
    pub user_id: Uuid,
    #[serde(rename = "type")]
    #[sea_orm(column_name = "type")]
    pub notification_type: String,
    pub title: String,
    pub content: String,
    pub link: Option<String>,
    pub related_issue_id: Option<Uuid>,
    pub related_comment_id: Option<Uuid>,
    pub related_project_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub issue_title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListNotificationsQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub limit: Option<u32>,
    pub unread_only: Option<bool>,
}

#[derive(Serialize)]
pub struct NotificationListData {
    #[serde(flatten)]
    pub page_data: PaginatedData<NotificationResponse>,
    pub unread_count: i64,
}

/// GET /api/v1/notifications
pub async fn list_notifications(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListNotificationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.limit.or(query.per_page).unwrap_or(20).min(100);
    let offset = (page - 1) * per_page;
    let unread_only = query.unread_only.unwrap_or(false);

    // Get total count
    let count_query = if unread_only {
        "SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false"
    } else {
        "SELECT COUNT(*) as count FROM notifications WHERE user_id = $1"
    };

    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            count_query,
            vec![user_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;

    // Get unread count
    let unread_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false",
            vec![user_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let unread_count: i64 = unread_result.try_get("", "count")?;

    // Get notifications
    let list_query = if unread_only {
        "SELECT n.id, n.user_id, n.type as notification_type, n.title, n.content, n.link,
                n.related_issue_id, n.related_comment_id, n.related_project_id,
                n.is_read, n.created_at, n.read_at,
                i.title as issue_title
         FROM notifications n
         LEFT JOIN work_items i ON n.related_issue_id = i.id
         WHERE n.user_id = $1 AND n.is_read = false
         ORDER BY n.created_at DESC
         LIMIT $2 OFFSET $3"
    } else {
        "SELECT n.id, n.user_id, n.type as notification_type, n.title, n.content, n.link,
                n.related_issue_id, n.related_comment_id, n.related_project_id,
                n.is_read, n.created_at, n.read_at,
                i.title as issue_title
         FROM notifications n
         LEFT JOIN work_items i ON n.related_issue_id = i.id
         WHERE n.user_id = $1
         ORDER BY n.created_at DESC
         LIMIT $2 OFFSET $3"
    };

    let notifications_result = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            list_query,
            vec![
                user_id.into(),
                (per_page as i64).into(),
                (offset as i64).into(),
            ],
        ))
        .await?;

    let notifications: Vec<NotificationResponse> = notifications_result
        .iter()
        .map(|r| NotificationResponse::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    let total_pages = ((total as f64) / (per_page as f64)).ceil() as u32;

    Ok(ApiResponse::success(NotificationListData {
        page_data: PaginatedData {
            items: notifications,
            total,
            page: page as i64,
            per_page: per_page as i64,
            total_pages: total_pages as i64,
        },
        unread_count,
    }))
}

#[derive(Debug, Serialize)]
pub struct UnreadCountData {
    pub count: i64,
}

/// GET /api/v1/notifications/unread-count
pub async fn get_unread_count(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let unread_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false",
            vec![user_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let unread_count: i64 = unread_result.try_get("", "count")?;

    Ok(ApiResponse::success(UnreadCountData { count: unread_count }))
}

/// PATCH /api/v1/notifications/:id/read
pub async fn mark_notification_read(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(notification_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let now = chrono::Utc::now();

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE notifications SET is_read = true, read_at = $1 WHERE id = $2 AND user_id = $3 AND is_read = false",
            vec![now.into(), notification_id.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(
            "Notification not found or already read".to_string(),
        ));
    }

    Ok(ApiResponse::ok())
}

/// PATCH /api/v1/notifications/read-all
pub async fn mark_all_read(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE notifications SET is_read = true, read_at = $1 WHERE user_id = $2 AND is_read = false",
            vec![now.into(), user_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

/// DELETE /api/v1/notifications/:id
pub async fn delete_notification(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(notification_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM notifications WHERE id = $1 AND user_id = $2",
            vec![notification_id.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Notification not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

/// Helper function to create a notification (called by other parts of the system)
pub async fn create_notification(
    state: &AppState,
    user_id: Uuid,
    notification_type: &str,
    title: String,
    content: String,
    link: Option<String>,
    related_issue_id: Option<Uuid>,
    related_comment_id: Option<Uuid>,
    related_project_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let notification_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO notifications 
               (id, user_id, type, kind, payload, title, content, link, related_issue_id, 
                related_comment_id, related_project_id, is_read, created_at)
               VALUES ($1, $2, $3, $4, $5::jsonb, $6, $7, $8, $9, $10, $11, false, $12)"#,
            vec![
                notification_id.into(),
                user_id.into(),
                notification_type.into(),
                notification_type.into(),
                json!({}).into(),
                title.into(),
                content.into(),
                link.into(),
                related_issue_id.into(),
                related_comment_id.into(),
                related_project_id.into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(notification_id)
}
