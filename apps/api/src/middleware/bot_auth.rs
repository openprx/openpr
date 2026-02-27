/// Bot Token authentication middleware.
///
/// Supports two authentication modes:
///   1. Bot Token (`opr_*`) — looks up `workspace_bots` table via SHA-256 hash.
///   2. JWT Bearer / cookie — falls back to the existing JWT path.
///
/// On success the middleware injects ONE of the following extensions:
///   - `JwtClaims`       — for human/JWT auth (existing handlers remain compatible)
///   - `BotAuthContext`  — for bot token auth (new handlers or opt-in handlers)
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager},
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    error::ApiError,
    routes::auth::{extract_bearer_token, extract_cookie_token},
};

/// Auth context injected when a bot token is used.
#[derive(Debug, Clone, Serialize)]
pub struct BotAuthContext {
    pub bot_id: Uuid,
    pub workspace_id: Uuid,
    pub permissions: Vec<String>,
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Middleware: authenticate as bot (opr_ token) or fall through to JWT.
///
/// Injects `BotAuthContext` for bot tokens, `JwtClaims` for JWT tokens.
/// Either extension can be extracted by downstream handlers.
pub async fn bot_or_user_auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(req.headers())
        .or_else(|| extract_cookie_token(req.headers(), "access_token"))
        .ok_or_else(|| ApiError::Unauthorized("missing access token".to_string()))?;

    if token.starts_with("opr_") {
        // ── Bot Token path ──
        let token_hash = sha256_hex(&token);

        #[derive(Debug, FromQueryResult)]
        struct BotRow {
            id: Uuid,
            workspace_id: Uuid,
            permissions: serde_json::Value,
            is_active: bool,
            expires_at: Option<chrono::DateTime<Utc>>,
        }

        let bot = BotRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id, workspace_id, permissions, is_active, expires_at
               FROM workspace_bots
               WHERE token_hash = $1"#,
            vec![token_hash.into()],
        ))
        .one(&state.db)
        .await
        .map_err(|_| ApiError::Internal)?
        .ok_or_else(|| ApiError::Unauthorized("invalid bot token".to_string()))?;

        if !bot.is_active {
            return Err(ApiError::Unauthorized("bot token is disabled".to_string()));
        }
        if let Some(expires_at) = bot.expires_at {
            if expires_at < Utc::now() {
                return Err(ApiError::Unauthorized("bot token has expired".to_string()));
            }
        }

        // Update last_used_at asynchronously (best-effort, don't block request)
        let db = state.db.clone();
        let bot_id = bot.id;
        tokio::spawn(async move {
            let _ = db
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE workspace_bots SET last_used_at = $1 WHERE id = $2",
                    vec![Utc::now().into(), bot_id.into()],
                ))
                .await;
        });

        let permissions: Vec<String> = bot
            .permissions
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        req.extensions_mut().insert(BotAuthContext {
            bot_id: bot.id,
            workspace_id: bot.workspace_id,
            permissions,
        });
    } else {
        // ── JWT path (unchanged behaviour) ──
        let jwt = JwtManager::new(
            &state.cfg.jwt_secret,
            state.cfg.jwt_access_ttl_seconds,
            state.cfg.jwt_refresh_ttl_seconds,
        );
        let claims: JwtClaims = jwt
            .verify_access_token(&token)
            .map_err(|_| ApiError::Unauthorized("invalid access token".to_string()))?;

        req.extensions_mut().insert(claims);
    }

    Ok(next.run(req).await)
}
