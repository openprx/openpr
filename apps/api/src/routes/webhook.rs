use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::entities::webhook::{CreateWebhookRequest, UpdateWebhookRequest, WEBHOOK_EVENTS};
use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, FromQueryResult)]
struct WebhookRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub url: String,
    pub events: serde_json::Value,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ListDeliveriesQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct WebhookDeliveryResponse {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub request_headers: Option<serde_json::Value>,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub delivered_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<WebhookRow> for WebhookResponse {
    type Error = ApiError;

    fn try_from(row: WebhookRow) -> Result<Self, Self::Error> {
        let events: Vec<String> = serde_json::from_value(row.events).map_err(|_| ApiError::Internal)?;
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            name: row.name,
            url: row.url,
            events,
            active: row.active,
            bot_user_id: row.bot_user_id,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_triggered_at: row.last_triggered_at,
        })
    }
}

/// GET /api/v1/workspaces/:workspace_id/webhooks/:webhook_id/deliveries
pub async fn list_deliveries(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ListDeliveriesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;
    ensure_webhook_in_workspace(&state, workspace_id, webhook_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS count FROM webhook_deliveries WHERE webhook_id = $1",
            vec![webhook_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    id,
                    webhook_id,
                    event_type,
                    payload,
                    request_headers,
                    response_status,
                    response_body,
                    error,
                    CASE
                        WHEN delivered_at IS NULL THEN NULL
                        ELSE (EXTRACT(EPOCH FROM (delivered_at - created_at)) * 1000)::BIGINT
                    END AS duration_ms,
                    CASE
                        WHEN response_status BETWEEN 200 AND 299 THEN true
                        ELSE false
                    END AS success,
                    delivered_at,
                    created_at
                FROM webhook_deliveries
                WHERE webhook_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
            "#,
            vec![
                webhook_id.into(),
                (per_page as i64).into(),
                (offset as i64).into(),
            ],
        ))
        .await?;

    let items: Vec<WebhookDeliveryResponse> = rows
        .iter()
        .map(|r| WebhookDeliveryResponse::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page: page as i64,
        per_page: per_page as i64,
        total_pages,
    }))
}

/// GET /api/v1/workspaces/:workspace_id/webhooks/:webhook_id/deliveries/:delivery_id
pub async fn get_delivery(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id, delivery_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;
    ensure_webhook_in_workspace(&state, workspace_id, webhook_id).await?;

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    id,
                    webhook_id,
                    event_type,
                    payload,
                    request_headers,
                    response_status,
                    response_body,
                    error,
                    CASE
                        WHEN delivered_at IS NULL THEN NULL
                        ELSE (EXTRACT(EPOCH FROM (delivered_at - created_at)) * 1000)::BIGINT
                    END AS duration_ms,
                    CASE
                        WHEN response_status BETWEEN 200 AND 299 THEN true
                        ELSE false
                    END AS success,
                    delivered_at,
                    created_at
                FROM webhook_deliveries
                WHERE webhook_id = $1 AND id = $2
            "#,
            vec![webhook_id.into(), delivery_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook delivery not found".to_string()))?;

    let delivery = WebhookDeliveryResponse::from_query_result(&row, "")?;
    Ok(ApiResponse::success(delivery))
}

/// POST /api/v1/workspaces/:workspace_id/webhooks
pub async fn create_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = req.name.clone().unwrap_or_else(|| req.url.clone());
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Creating a webhook only requires authenticated workspace access.
    verify_workspace_access(&state, workspace_id, user_id).await?;

    // Validate events
    for event in &req.events {
        if !WEBHOOK_EVENTS.contains(&event.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "Invalid event type: {}",
                event
            )));
        }
    }

    if let Some(bot_user_id) = req.bot_user_id {
        ensure_bot_user(&state, bot_user_id).await?;
    }

    // Prefer explicit secret, otherwise generate one.
    let secret = req.secret.unwrap_or_else(generate_secret);

    let webhook_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let active = req.enabled.or(req.active).unwrap_or(true);

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO webhooks 
               (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
            vec![
                webhook_id.into(),
                workspace_id.into(),
                name.into(),
                req.url.into(),
                secret.into(),
                serde_json::to_value(&req.events).unwrap().into(),
                active.into(),
                req.bot_user_id.into(),
                user_id.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await?;

    let webhook = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE id = $1",
            vec![webhook_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let response = WebhookResponse::try_from(WebhookRow::from_query_result(&webhook, "")?)?;

    Ok(ApiResponse::success(response))
}

/// GET /api/v1/workspaces/:workspace_id/webhooks
pub async fn list_webhooks(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;

    let webhooks = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE workspace_id = $1 ORDER BY created_at DESC",
            vec![workspace_id.into()],
        ))
        .await?;

    let responses: Vec<WebhookResponse> = webhooks
        .iter()
        .map(|r| {
            let row = WebhookRow::from_query_result(r, "")?;
            WebhookResponse::try_from(row)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ApiResponse::success(PaginatedData::from_items(responses)))
}

/// GET /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
pub async fn get_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;

    let webhook = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;

    let response = WebhookResponse::try_from(WebhookRow::from_query_result(&webhook, "")?)?;

    Ok(ApiResponse::success(response))
}

/// PATCH /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
pub async fn update_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateWebhookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_admin(&state, workspace_id, user_id).await?;

    // Validate events if provided
    if let Some(ref events) = req.events {
        for event in events {
            if !WEBHOOK_EVENTS.contains(&event.as_str()) {
                return Err(ApiError::BadRequest(format!(
                    "Invalid event type: {}",
                    event
                )));
            }
        }
    }
    if let Some(bot_user_id) = req.bot_user_id {
        ensure_bot_user(&state, bot_user_id).await?;
    }

    // Build dynamic UPDATE query
    let mut updates = Vec::new();
    let mut params: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${}", param_idx));
        params.push(name.into());
        param_idx += 1;
    }
    if let Some(url) = req.url {
        updates.push(format!("url = ${}", param_idx));
        params.push(url.into());
        param_idx += 1;
    }
    if let Some(secret) = req.secret {
        updates.push(format!("secret = ${}", param_idx));
        params.push(secret.into());
        param_idx += 1;
    }
    if let Some(events) = req.events {
        updates.push(format!("events = ${}", param_idx));
        params.push(serde_json::to_value(&events).unwrap().into());
        param_idx += 1;
    }
    if let Some(active) = req.enabled.or(req.active) {
        updates.push(format!("active = ${}", param_idx));
        params.push(active.into());
        param_idx += 1;
    }
    if req.bot_user_id.is_some() {
        updates.push(format!("bot_user_id = ${}", param_idx));
        params.push(req.bot_user_id.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("No fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${}", param_idx));
    params.push(chrono::Utc::now().into());
    param_idx += 1;

    params.push(webhook_id.into());
    params.push(workspace_id.into());

    let sql = format!(
        "UPDATE webhooks SET {} WHERE id = ${} AND workspace_id = ${}",
        updates.join(", "),
        param_idx - 1,
        param_idx
    );

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            params,
        ))
        .await?;

    let webhook = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE id = $1",
            vec![webhook_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;

    let response = WebhookResponse::try_from(WebhookRow::from_query_result(&webhook, "")?)?;

    Ok(ApiResponse::success(response))
}

/// DELETE /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
pub async fn delete_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_admin(&state, workspace_id, user_id).await?;

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

// Helper functions
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
            let normalized_role = role.trim().to_lowercase();
            if normalized_role != "admin" && normalized_role != "owner" {
                return Err(ApiError::Forbidden(
                    "Workspace admin or owner required".to_string(),
                ));
            }
            Ok(())
        }
        None => Err(ApiError::Forbidden("Not a workspace member".to_string())),
    }
}

async fn verify_workspace_access(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;

    if result.is_none() {
        return Err(ApiError::Forbidden("Access denied".to_string()));
    }

    Ok(())
}

async fn ensure_bot_user(state: &AppState, bot_user_id: Uuid) -> Result<(), ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM users WHERE id = $1 AND entity_type = 'bot'",
            vec![bot_user_id.into()],
        ))
        .await?;
    if row.is_none() {
        return Err(ApiError::BadRequest(
            "bot_user_id must reference a bot user".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_webhook_in_workspace(
    state: &AppState,
    workspace_id: Uuid,
    webhook_id: Uuid,
) -> Result<(), ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?;

    if row.is_none() {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    Ok(())
}

#[allow(dead_code)]
pub async fn build_webhook_payload(
    state: &AppState,
    event_type: &str,
    issue_data: &Value,
    webhook_bot_user_id: Option<Uuid>,
) -> Value {
    let mut payload = json!({
        "event": event_type,
        "issue": issue_data,
        "timestamp": chrono::Utc::now(),
    });

    if let Some(bot_id) = webhook_bot_user_id {
        let is_bot_task = check_is_bot_task(event_type, issue_data, &bot_id);
        payload["is_bot_task"] = json!(is_bot_task);
        payload["bot_id"] = json!(bot_id);

        if let Ok(Some(agent_type)) = get_bot_agent_type_by_id(state, bot_id).await {
            payload["bot_agent_type"] = json!(agent_type);
        }

        payload["trigger_reason"] = json!(get_trigger_reason(event_type, issue_data, &bot_id));
    }

    payload
}

fn check_is_bot_task(event: &str, issue: &Value, bot_id: &Uuid) -> bool {
    match event {
        "issue.created" | "issue.updated" => issue
            .get("assignee_id")
            .and_then(|v| v.as_str())
            .map(|id| id == bot_id.to_string())
            .unwrap_or(false),
        "comment.created" => true,
        _ => false,
    }
}

fn get_trigger_reason(event: &str, issue: &Value, bot_id: &Uuid) -> String {
    let bot_id_str = bot_id.to_string();
    if event == "comment.created" {
        "mentioned".to_string()
    } else if issue.get("assignee_id").and_then(|v| v.as_str()) == Some(bot_id_str.as_str()) {
        "assigned".to_string()
    } else {
        "status_changed".to_string()
    }
}

async fn get_bot_agent_type_by_id(
    state: &AppState,
    bot_id: Uuid,
) -> Result<Option<String>, ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT agent_type FROM users WHERE id = $1 AND entity_type = 'bot'",
            vec![bot_id.into()],
        ))
        .await?;

    if let Some(row) = row {
        let agent_type: Option<String> = row.try_get("", "agent_type")?;
        Ok(agent_type)
    } else {
        Ok(None)
    }
}

fn generate_secret() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..64)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}
