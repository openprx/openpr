use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::{ApiError, localize_error, request_lang},
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::{ApiResponse, PaginatedData},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct CommentResponse {
    pub id: Uuid,
    pub work_item_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCommentRequest {
    pub content: Option<String>,
    pub body: Option<String>,
    pub mentions: Option<Vec<Uuid>>,
}

impl CreateCommentRequest {
    fn resolved_content(&self) -> &str {
        self.content
            .as_deref()
            .or(self.body.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateCommentRequest {
    pub content: Option<String>,
    pub body: Option<String>,
    pub mentions: Option<Vec<Uuid>>,
}

impl UpdateCommentRequest {
    fn resolved_content(&self) -> &str {
        self.content
            .as_deref()
            .or(self.body.as_deref())
            .unwrap_or_default()
    }
}

fn normalize_mentions(mentions: Option<Vec<Uuid>>, actor_id: Uuid) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    mentions
        .unwrap_or_default()
        .into_iter()
        .filter(|id| *id != actor_id)
        .filter(|id| seen.insert(*id))
        .collect()
}

fn build_auth_extensions(
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

async fn create_mention_notifications<C: ConnectionTrait>(
    db: &C,
    mention_user_ids: &[Uuid],
    actor_name: &str,
    workspace_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
) -> Result<(), ApiError> {
    if mention_user_ids.is_empty() {
        return Ok(());
    }

    let link = format!(
        "/workspace/{}/projects/{}/issues/{}",
        workspace_id, project_id, issue_id
    );
    let now = chrono::Utc::now();

    for mention_user_id in mention_user_ids {
        let is_workspace_member = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![workspace_id.into(), (*mention_user_id).into()],
            ))
            .await?;

        if is_workspace_member.is_none() {
            continue;
        }

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO notifications (
                    id, user_id, workspace_id, type, kind, payload, title, content, link, related_issue_id, related_comment_id, is_read, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, $10, NULL, false, $11)
            "#,
            vec![
                Uuid::new_v4().into(),
                (*mention_user_id).into(),
                workspace_id.into(),
                "mentioned".into(),
                "mentioned".into(),
                json!({}).into(),
                "notification.mentionedTitle".into(),
                format!("mentioned_by:{}", actor_name).into(),
                link.clone().into(),
                issue_id.into(),
                now.into(),
            ],
        ))
        .await?;
    }

    Ok(())
}

/// POST /api/v1/issues/:issue_id/comments - Create a new comment
pub async fn create_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    headers: HeaderMap,
    Path(issue_id): Path<Uuid>,
    Json(req): Json<CreateCommentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let lang = request_lang(&headers);
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let content = req.resolved_content().trim().to_string();
    if content.is_empty() {
        return Err(ApiError::BadRequest(localize_error(
            "content is required",
            lang,
        )));
    }

    #[derive(Debug, FromQueryResult)]
    struct IssueContext {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    // Verify issue exists and fetch project/workspace context.
    let issue_context = IssueContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        "#,
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound(localize_error("issue not found", lang)))?;

    require_workspace_access(&state, &extensions, issue_context.workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct UserInfo {
        name: String,
        email: String,
    }

    let author = UserInfo::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name, email FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?;

    let actor_name = author
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "User".to_string());

    let mention_user_ids = normalize_mentions(req.mentions, user_id);

    let comment_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO comments (id, work_item_id, author_id, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)",
        vec![
            comment_id.into(),
            issue_id.into(),
            user_id.into(),
            content.clone().into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;

    create_mention_notifications(
        &tx,
        &mention_user_ids,
        &actor_name,
        issue_context.workspace_id,
        issue_context.project_id,
        issue_id,
    )
    .await?;

    tx.commit().await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::CommentCreated,
            workspace_id: issue_context.workspace_id,
            project_id: issue_context.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: Some(comment_id),
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: mention_user_ids.clone(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::success(CommentResponse {
        id: comment_id,
        work_item_id: issue_id,
        author_id: Some(user_id),
        author_name: author.as_ref().map(|a| a.name.clone()),
        author_email: author.as_ref().map(|a| a.email.clone()),
        body: content,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/issues/:issue_id/comments - List comments for an issue
pub async fn list_comments(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        workspace_id: Uuid,
    }

    let issue = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        "#,
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    require_workspace_access(&state, &extensions, issue.workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct CommentRow {
        id: Uuid,
        work_item_id: Uuid,
        author_id: Option<Uuid>,
        author_name: Option<String>,
        author_email: Option<String>,
        body: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let comments = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT c.id, c.work_item_id, c.author_id, u.name as author_name, u.email as author_email,
                   c.body, c.created_at, c.updated_at
            FROM comments c
            LEFT JOIN users u ON c.author_id = u.id
            WHERE c.work_item_id = $1
            ORDER BY c.created_at ASC
        "#,
        vec![issue_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<CommentResponse> = comments
        .into_iter()
        .map(|c| CommentResponse {
            id: c.id,
            work_item_id: c.work_item_id,
            author_id: c.author_id,
            author_name: c.author_name,
            author_email: c.author_email,
            body: c.body,
            created_at: c.created_at.to_rfc3339(),
            updated_at: c.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// PUT /api/v1/comments/:id - Update a comment
pub async fn update_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    headers: HeaderMap,
    Path(comment_id): Path<Uuid>,
    Json(req): Json<UpdateCommentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let lang = request_lang(&headers);
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let content = req.resolved_content().trim().to_string();
    if content.is_empty() {
        return Err(ApiError::BadRequest(localize_error(
            "content cannot be empty",
            lang,
        )));
    }

    // Get comment and verify author.
    #[derive(Debug, FromQueryResult)]
    struct CommentContext {
        author_id: Option<Uuid>,
        work_item_id: Uuid,
        project_id: Uuid,
        workspace_id: Uuid,
    }

    let comment = CommentContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT c.author_id, c.work_item_id, wi.project_id, p.workspace_id
            FROM comments c
            INNER JOIN work_items wi ON c.work_item_id = wi.id
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE c.id = $1
        "#,
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("comment not found".to_string()))?;

    if comment.author_id != Some(user_id) {
        return Err(ApiError::Forbidden(
            "only the comment author can update it".to_string(),
        ));
    }

    #[derive(Debug, FromQueryResult)]
    struct UserInfo {
        name: String,
    }

    let author = UserInfo::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?;

    let actor_name = author
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "User".to_string());

    let mention_user_ids = normalize_mentions(req.mentions, user_id);

    let now = chrono::Utc::now();
    let detail = serde_json::json!({
        "comment_id": comment_id.to_string(),
    });
    let detail_text = detail.to_string();

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE comments SET body = $1, updated_at = $2 WHERE id = $3",
        vec![content.clone().into(), now.into(), comment_id.into()],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO activities (
                id, workspace_id, project_id, issue_id, user_id, action, detail, created_at,
                resource_type, resource_id, event_type, actor_id, payload
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13::jsonb)
        "#,
        vec![
            Uuid::new_v4().into(),
            comment.workspace_id.into(),
            comment.project_id.into(),
            comment.work_item_id.into(),
            user_id.into(),
            "comment_edited".into(),
            detail_text.clone().into(),
            now.into(),
            "comment".into(),
            comment_id.into(),
            "comment_edited".into(),
            user_id.into(),
            "{}".into(),
        ],
    ))
    .await?;

    create_mention_notifications(
        &tx,
        &mention_user_ids,
        &actor_name,
        comment.workspace_id,
        comment.project_id,
        comment.work_item_id,
    )
    .await?;

    tx.commit().await?;

    // Fetch updated comment with author info.
    #[derive(Debug, FromQueryResult)]
    struct CommentRow {
        id: Uuid,
        work_item_id: Uuid,
        author_id: Option<Uuid>,
        author_name: Option<String>,
        author_email: Option<String>,
        body: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT c.id, c.work_item_id, c.author_id, u.name as author_name, u.email as author_email,
                   c.body, c.created_at, c.updated_at
            FROM comments c
            LEFT JOIN users u ON c.author_id = u.id
            WHERE c.id = $1
        "#,
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::CommentUpdated,
            workspace_id: comment.workspace_id,
            project_id: comment.project_id,
            actor_id: user_id,
            issue_id: Some(comment.work_item_id),
            comment_id: Some(comment_id),
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::success(CommentResponse {
        id: updated.id,
        work_item_id: updated.work_item_id,
        author_id: updated.author_id,
        author_name: updated.author_name,
        author_email: updated.author_email,
        body: updated.body,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/comments/:id - Delete a comment
pub async fn delete_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(comment_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    // Get comment context.
    #[derive(Debug, FromQueryResult)]
    struct CommentInfo {
        author_id: Option<Uuid>,
        workspace_id: Uuid,
        project_id: Uuid,
        work_item_id: Uuid,
        body: String,
    }

    let comment_info = CommentInfo::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT c.author_id, p.workspace_id, wi.project_id, c.work_item_id, c.body
            FROM comments c
            INNER JOIN work_items wi ON c.work_item_id = wi.id
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE c.id = $1
        "#,
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("comment not found".to_string()))?;

    // Check if user is the author.
    let is_author = comment_info.author_id == Some(user_id);

    if !is_author {
        let (_, role, _) =
            require_workspace_access(&state, &extensions, comment_info.workspace_id).await?;
        if role != "owner" && role != "admin" {
            return Err(ApiError::Forbidden(
                "only the comment author or workspace owner/admin can delete comments".to_string(),
            ));
        }
    }

    let preview: String = comment_info.body.chars().take(20).collect();
    let detail = serde_json::json!({
        "comment_id": comment_id.to_string(),
        "content_preview": preview,
    });
    let detail_text = detail.to_string();
    let now = chrono::Utc::now();

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM comments WHERE id = $1",
        vec![comment_id.into()],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO activities (
                id, workspace_id, project_id, issue_id, user_id, action, detail, created_at,
                resource_type, resource_id, event_type, actor_id, payload
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13::jsonb)
        "#,
        vec![
            Uuid::new_v4().into(),
            comment_info.workspace_id.into(),
            comment_info.project_id.into(),
            comment_info.work_item_id.into(),
            user_id.into(),
            "comment_deleted".into(),
            detail_text.clone().into(),
            now.into(),
            "comment".into(),
            comment_id.into(),
            "comment_deleted".into(),
            user_id.into(),
            "{}".into(),
        ],
    ))
    .await?;

    tx.commit().await?;

    let deleted_comment_payload = serde_json::json!({
        "comment": {
            "id": comment_id.to_string(),
            "issue_id": comment_info.work_item_id.to_string(),
            "content": comment_info.body,
            "author_id": comment_info.author_id.map(|id| id.to_string()),
            "author_name": "",
            "created_at": now.to_rfc3339(),
            "updated_at": now.to_rfc3339(),
        },
        "issue": {
            "id": comment_info.work_item_id.to_string(),
        }
    });

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::CommentDeleted,
            workspace_id: comment_info.workspace_id,
            project_id: comment_info.project_id,
            actor_id: user_id,
            issue_id: Some(comment_info.work_item_id),
            comment_id: Some(comment_id),
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(deleted_comment_payload),
        },
    );

    Ok(ApiResponse::ok())
}
