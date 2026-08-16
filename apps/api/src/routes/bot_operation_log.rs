use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::ApiResponse,
};

const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 100;

#[derive(Debug, Deserialize)]
pub struct ListBotOperationLogsQuery {
    pub bot_id: Option<Uuid>,
    pub tool_name: Option<String>,
    pub outcome: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct BotOperationLogResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub bot_id: Uuid,
    pub bot_name: Option<String>,
    pub tool_name: Option<String>,
    pub surface: String,
    pub method: String,
    pub path: String,
    pub business_code: i32,
    pub outcome: String,
    pub error_message: Option<String>,
    pub duration_ms: i64,
    pub request_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct BotOperationLogPage {
    pub items: Vec<BotOperationLogResponse>,
    pub next_cursor: Option<String>,
}

fn normalize_tool_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    let mut segments = value.split('.');
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 64
            && segment.starts_with(|character: char| character.is_ascii_lowercase())
            && segment
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')
    };
    let first = segments.next().unwrap_or_default();
    let second = segments.next().unwrap_or_default();
    if value.len() <= 128 && valid_segment(first) && valid_segment(second) && segments.all(valid_segment) {
        Ok(value.to_string())
    } else {
        Err(ApiError::BadRequest("invalid tool_name filter".to_string()))
    }
}

fn decode_cursor(value: &str) -> Result<(DateTime<Utc>, Uuid), ApiError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ApiError::BadRequest("invalid cursor".to_string()))?;
    let decoded = std::str::from_utf8(&decoded).map_err(|_| ApiError::BadRequest("invalid cursor".to_string()))?;
    let (created_at, id) = decoded
        .split_once('|')
        .ok_or_else(|| ApiError::BadRequest("invalid cursor".to_string()))?;
    let created_at = DateTime::parse_from_rfc3339(created_at)
        .map_err(|_| ApiError::BadRequest("invalid cursor".to_string()))?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id).map_err(|_| ApiError::BadRequest("invalid cursor".to_string()))?;
    Ok((created_at, id))
}

fn encode_cursor(created_at: DateTime<Utc>, id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}|{id}", created_at.to_rfc3339()))
}

/// GET `/api/v1/workspaces/{workspace_id}/bot-operation-logs`.
pub async fn list_bot_operation_logs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListBotOperationLogsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_context)) = bot {
        extensions.insert(bot_context);
    }
    require_workspace_access(&state, &extensions, workspace_id).await?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest("limit must be between 1 and 100".to_string()));
    }
    let fetch_limit = i64::try_from(limit + 1).map_err(|_| ApiError::Internal)?;

    let mut clauses = vec!["logs.workspace_id = $1".to_string()];
    let mut values: Vec<Value> = vec![workspace_id.into()];

    if let Some(bot_id) = query.bot_id {
        values.push(bot_id.into());
        clauses.push(format!("logs.bot_id = ${}", values.len()));
    }
    if let Some(tool_name) = query.tool_name.as_deref() {
        values.push(normalize_tool_name(tool_name)?.into());
        clauses.push(format!("logs.tool_name = ${}", values.len()));
    }
    if let Some(outcome) = query.outcome.as_deref() {
        if !matches!(outcome, "ok" | "error") {
            return Err(ApiError::BadRequest("outcome must be ok or error".to_string()));
        }
        values.push(outcome.to_string().into());
        clauses.push(format!("logs.outcome = ${}", values.len()));
    }
    if let Some(cursor) = query.cursor.as_deref() {
        let (created_at, id) = decode_cursor(cursor)?;
        values.push(created_at.into());
        let created_at_index = values.len();
        values.push(id.into());
        let id_index = values.len();
        clauses.push(format!(
            "(logs.created_at, logs.id) < (${created_at_index}, ${id_index})"
        ));
    }

    values.push(fetch_limit.into());
    let limit_index = values.len();
    let sql = format!(
        r"SELECT logs.id, logs.workspace_id, logs.bot_id, bots.name AS bot_name,
                  logs.tool_name, logs.surface, logs.method, logs.path, logs.business_code,
                  logs.outcome, logs.error_message, logs.duration_ms, logs.request_id, logs.created_at
           FROM bot_operation_logs logs
           LEFT JOIN workspace_bots bots ON bots.id = logs.bot_id
           WHERE {}
           ORDER BY logs.created_at DESC, logs.id DESC
           LIMIT ${limit_index}",
        clauses.join(" AND ")
    );
    let mut items =
        BotOperationLogResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(&state.db)
            .await?;

    let has_more = items.len() > usize::try_from(limit).map_err(|_| ApiError::Internal)?;
    if has_more {
        items.pop();
    }
    let next_cursor = if has_more {
        items.last().map(|item| encode_cursor(item.created_at, item.id))
    } else {
        None
    };

    Ok(Json(ApiResponse {
        code: 0,
        message: "success".to_string(),
        data: Some(BotOperationLogPage { items, next_cursor }),
    }))
}

#[cfg(test)]
mod tests {
    use super::{decode_cursor, encode_cursor, normalize_tool_name};
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn cursor_round_trips_without_exposing_query_data() {
        let timestamp = Utc::now();
        let id = Uuid::new_v4();
        let encoded = encode_cursor(timestamp, id);
        let decoded = decode_cursor(&encoded).ok();
        assert_eq!(decoded.as_ref().map(|value| value.0), Some(timestamp));
        assert_eq!(decoded.as_ref().map(|value| value.1), Some(id));
        assert!(!encoded.contains('|'));
    }

    #[test]
    fn tool_filter_uses_the_same_shape_as_registered_tools() {
        assert!(normalize_tool_name("form_records.list").is_ok_and(|value| value == "form_records.list"));
        assert!(normalize_tool_name("invalid").is_err());
        assert!(normalize_tool_name("Forms.List").is_err());
    }
}
