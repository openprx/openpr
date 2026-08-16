// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements)]

/// Bot Token authentication middleware.
///
/// Supports two authentication modes:
///   1. Bot Token (`opr_*`) — looks up `workspace_bots` table via SHA-256 hash.
///   2. JWT Bearer / cookie — falls back to the existing JWT path.
///
/// On success the middleware injects:
///   - bot token auth: `BotAuthContext` + synthetic `JwtClaims`
///   - JWT auth: `JwtClaims`
use axum::{
    extract::{Request, State},
    http::Extensions,
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager, TokenType},
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::OperationResponseMeta,
    routes::auth::{extract_bearer_token, extract_cookie_token},
};

const MCP_TOOL_HEADER: &str = "x-openpr-mcp-tool";
const MCP_SURFACE_HEADER: &str = "x-openpr-mcp-surface";

struct BotOperationContext {
    bot_id: Uuid,
    workspace_id: Uuid,
    tool_name: Option<String>,
    surface: &'static str,
    method: String,
    path: String,
    request_id: Uuid,
}

fn operation_surface(headers: &axum::http::HeaderMap) -> &'static str {
    match headers.get(MCP_SURFACE_HEADER).and_then(|value| value.to_str().ok()) {
        Some("mcp_http") => "mcp_http",
        Some("mcp_sse") => "mcp_sse",
        Some("mcp_stdio") => "mcp_stdio",
        Some("cli") => "cli",
        _ => "rest",
    }
}

fn operation_tool_name(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(MCP_TOOL_HEADER)?.to_str().ok()?.trim();
    let mut segments = value.split('.');
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 64
            && segment.starts_with(|character: char| character.is_ascii_lowercase())
            && segment
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')
    };
    let first = segments.next()?;
    let second = segments.next()?;
    if value.len() <= 128 && valid_segment(first) && valid_segment(second) && segments.all(valid_segment) {
        Some(value.to_string())
    } else {
        None
    }
}

fn spawn_operation_log(
    db: sea_orm::DatabaseConnection,
    context: BotOperationContext,
    business_code: i32,
    error_message: Option<&'static str>,
    duration_ms: i64,
) {
    tokio::spawn(async move {
        let outcome = if business_code == 0 { "ok" } else { "error" };
        let result = db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"INSERT INTO bot_operation_logs
                   (id, workspace_id, bot_id, tool_name, surface, method, path,
                    business_code, outcome, error_message, duration_ms, request_id, created_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
                vec![
                    Uuid::new_v4().into(),
                    context.workspace_id.into(),
                    context.bot_id.into(),
                    context.tool_name.into(),
                    context.surface.into(),
                    context.method.into(),
                    context.path.into(),
                    business_code.into(),
                    outcome.into(),
                    error_message.map(str::to_string).into(),
                    duration_ms.into(),
                    context.request_id.into(),
                    Utc::now().into(),
                ],
            ))
            .await;
        if let Err(error) = result {
            tracing::warn!(
                request_id = %context.request_id,
                error = %error,
                "bot operation log write failed"
            );
        }
    });
}

/// Auth context injected when a bot token is used.
#[derive(Debug, Clone, Serialize)]
pub struct BotAuthContext {
    pub bot_id: Uuid,
    pub workspace_id: Uuid,
    pub permissions: Vec<String>,
}

pub fn extract_bot_context(extensions: &Extensions) -> Option<&BotAuthContext> {
    extensions.get::<BotAuthContext>()
}

/// What a bot token is allowed to do, as stored in `workspace_bots.permissions`.
///
/// The three names are the ones `POST /workspaces/{id}/bots` accepts and the ones the column
/// comment documents. Until now none of them was ever consulted: every bot was mapped to a
/// workspace role and the `read` / `write` distinction existed only on paper, so a token issued as
/// read-only could create, update and delete anything its workspace contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotPermission {
    /// Retrieve data. Implied by `write` and `admin` — a write returns what it wrote.
    Read,
    /// Change data. Implied by `admin`.
    Write,
    /// Act as a workspace administrator: bypasses form field-level policies and record scoping
    /// exactly like a human `admin` member, and nothing beyond that.
    Admin,
}

impl BotPermission {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}

/// Whether `permissions` grants `required`, honouring `admin` > `write` > `read`.
///
/// Unknown entries grant nothing: the issuing endpoint rejects them, so a token carrying one is
/// either forged or predates a permission rename, and neither deserves the benefit of the doubt.
pub fn bot_permissions_allow(permissions: &[String], required: BotPermission) -> bool {
    let granted = |name: &str| permissions.iter().any(|permission| permission == name);
    match required {
        BotPermission::Read => granted("read") || granted("write") || granted("admin"),
        BotPermission::Write => granted("write") || granted("admin"),
        BotPermission::Admin => granted("admin"),
    }
}

/// Reject a bot token that does not carry `required`.
pub fn ensure_bot_permission(bot: &BotAuthContext, required: BotPermission) -> Result<(), ApiError> {
    if bot_permissions_allow(&bot.permissions, required) {
        return Ok(());
    }
    Err(ApiError::Forbidden(format!(
        "bot token lacks the '{}' permission",
        required.as_str()
    )))
}

fn bot_role_from_permissions(permissions: &[String]) -> String {
    if bot_permissions_allow(permissions, BotPermission::Admin) {
        "admin".to_string()
    } else {
        "member".to_string()
    }
}

/// Unified workspace access check for both bot-token and JWT auth paths.
///
/// Returns `(actor_id, role, is_bot)`:
/// - `actor_id`: user id (JWT) or bot id (bot token)
/// - `role`: workspace role for user, or a synthesized role from bot permissions
/// - `is_bot`: whether the request used a bot token
pub async fn require_workspace_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    let claims = extensions
        .get::<JwtClaims>()
        .ok_or_else(|| ApiError::Unauthorized("missing auth context".to_string()))?;
    let bot = extract_bot_context(extensions);

    require_workspace_access_from_auth(state, claims, bot, workspace_id).await
}

pub async fn require_workspace_access_from_auth(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    if let Some(bot_ctx) = bot {
        if bot_ctx.workspace_id != workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this workspace".to_string()));
        }
        // A token with no usable permission at all reaches nothing, the same way a user with no
        // workspace membership row does.
        ensure_bot_permission(bot_ctx, BotPermission::Read)?;
        let role = bot_role_from_permissions(&bot_ctx.permissions);
        return Ok((bot_ctx.bot_id, role, true));
    }

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok((user_id, row.role, false))
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// The permission an HTTP method demands of a bot token.
///
/// Safe methods only read, everything else can change state. Kept separate from the middleware so
/// the mapping is testable without standing up a router.
fn required_bot_permission(method: &axum::http::Method) -> BotPermission {
    if method.is_safe() {
        BotPermission::Read
    } else {
        BotPermission::Write
    }
}

/// Middleware: authenticate as bot (opr_ token) or fall through to JWT.
///
/// Injects `JwtClaims` for both paths to keep existing handlers compatible.
/// For bot tokens, also injects `BotAuthContext`.
pub async fn bot_or_user_auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let started = Instant::now();
    let mut operation_context = None;
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
            r"SELECT id, workspace_id, permissions, is_active, expires_at
               FROM workspace_bots
               WHERE token_hash = $1",
            vec![token_hash.into()],
        ))
        .one(&state.db)
        .await
        .map_err(|_| ApiError::Internal)?
        .ok_or_else(|| ApiError::Unauthorized("invalid bot token".to_string()))?;

        if !bot.is_active {
            return Err(ApiError::Unauthorized("bot token is disabled".to_string()));
        }
        if let Some(expires_at) = bot.expires_at
            && expires_at < Utc::now()
        {
            return Err(ApiError::Unauthorized("bot token has expired".to_string()));
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
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let context = BotAuthContext {
            bot_id: bot.id,
            workspace_id: bot.workspace_id,
            permissions,
        };
        // Every bot-reachable route passes through here, which makes this the one place the
        // `read` / `write` split can be enforced without trusting each handler to remember. The
        // HTTP method is the authority: safe methods (GET/HEAD/OPTIONS/TRACE) need `read`,
        // everything else needs `write`. Handlers that mutate behind a POST are covered; the price
        // is that a read-shaped POST (preview, signed-url) also needs `write`, which is the
        // direction to fail in.
        let authenticated_operation = BotOperationContext {
            bot_id: context.bot_id,
            workspace_id: context.workspace_id,
            tool_name: operation_tool_name(req.headers()),
            surface: operation_surface(req.headers()),
            method: req.method().as_str().to_string(),
            path: req.uri().path().to_string(),
            request_id: Uuid::new_v4(),
        };
        if let Err(error) = ensure_bot_permission(&context, required_bot_permission(req.method())) {
            let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
            spawn_operation_log(
                state.db.clone(),
                authenticated_operation,
                403,
                Some("forbidden"),
                duration_ms,
            );
            return Err(error);
        }
        operation_context = Some(authenticated_operation);

        req.extensions_mut().insert(context);
        req.extensions_mut().insert(JwtClaims {
            sub: bot.id.to_string(),
            email: format!("bot+{}@openpr.local", bot.id),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        });
    } else {
        // ── JWT path (unchanged behaviour) ──
        let jwt = JwtManager::new(
            state.cfg.jwt_secret.expose(),
            state.cfg.jwt_access_ttl_seconds,
            state.cfg.jwt_refresh_ttl_seconds,
        );
        let claims: JwtClaims = jwt
            .verify_access_token(&token)
            .map_err(|_| ApiError::Unauthorized("invalid access token".to_string()))?;

        req.extensions_mut().insert(claims);
    }

    let response = next.run(req).await;
    if let Some(context) = operation_context {
        let meta = response.extensions().get::<OperationResponseMeta>().copied();
        let (business_code, error_message) = meta.map_or_else(
            || {
                if response.status().is_success() {
                    (0, None)
                } else {
                    (i32::from(response.status().as_u16()), Some("http request rejected"))
                }
            },
            |value| (value.business_code, value.error_summary),
        );
        let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        spawn_operation_log(state.db.clone(), context, business_code, error_message, duration_ms);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        BotAuthContext, BotPermission, bot_permissions_allow, bot_role_from_permissions, ensure_bot_permission,
        operation_surface, operation_tool_name, required_bot_permission,
    };
    use axum::http::{HeaderMap, HeaderValue, Method};
    use uuid::Uuid;

    fn bot(permissions: &[&str]) -> BotAuthContext {
        BotAuthContext {
            bot_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            permissions: permissions.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    #[test]
    fn a_read_only_bot_cannot_perform_write_operations() {
        let read_only = bot(&["read"]);

        assert!(ensure_bot_permission(&read_only, BotPermission::Read).is_ok());
        assert!(ensure_bot_permission(&read_only, BotPermission::Write).is_err());
        assert!(ensure_bot_permission(&read_only, BotPermission::Admin).is_err());
    }

    #[test]
    fn write_and_admin_imply_the_weaker_permissions() {
        assert!(bot_permissions_allow(&["write".to_string()], BotPermission::Read));
        assert!(bot_permissions_allow(&["write".to_string()], BotPermission::Write));
        assert!(!bot_permissions_allow(&["write".to_string()], BotPermission::Admin));

        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Read));
        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Write));
        assert!(bot_permissions_allow(&["admin".to_string()], BotPermission::Admin));
    }

    #[test]
    fn unknown_and_empty_permissions_grant_nothing() {
        assert!(!bot_permissions_allow(&[], BotPermission::Read));
        assert!(!bot_permissions_allow(&["readonly".to_string()], BotPermission::Read));
        assert!(!bot_permissions_allow(&["ADMIN".to_string()], BotPermission::Admin));
        assert!(ensure_bot_permission(&bot(&[]), BotPermission::Read).is_err());
    }

    #[test]
    fn only_the_admin_permission_synthesizes_the_admin_workspace_role() {
        assert_eq!(bot_role_from_permissions(&["admin".to_string()]), "admin");
        assert_eq!(
            bot_role_from_permissions(&["read".to_string(), "write".to_string()]),
            "member"
        );
        assert_eq!(bot_role_from_permissions(&[]), "member");
    }

    #[test]
    fn unsafe_methods_require_the_write_permission() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert_eq!(required_bot_permission(&method), BotPermission::Read);
        }
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(required_bot_permission(&method), BotPermission::Write);
        }
    }

    #[test]
    fn operation_headers_are_bounded_observability_labels_only() {
        let mut headers = HeaderMap::new();
        headers.insert("x-openpr-mcp-surface", HeaderValue::from_static("mcp_http"));
        headers.insert("x-openpr-mcp-tool", HeaderValue::from_static("form_records.list"));
        assert_eq!(operation_surface(&headers), "mcp_http");
        assert_eq!(operation_tool_name(&headers).as_deref(), Some("form_records.list"));

        headers.insert("x-openpr-mcp-surface", HeaderValue::from_static("admin"));
        headers.insert("x-openpr-mcp-tool", HeaderValue::from_static("invalid"));
        assert_eq!(operation_surface(&headers), "rest");
        assert!(operation_tool_name(&headers).is_none());
    }
}
