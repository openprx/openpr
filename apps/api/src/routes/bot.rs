// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::Utc;
use platform::{app::AppState, auth::JwtClaims};
use rand::Rng;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::middleware::bot_auth::{BotAuthContext, BotPermission, bot_permissions_allow, require_workspace_access};
use crate::{error::ApiError, response::ApiResponse};

// ============================================================================
// Request / Response types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateBotRequest {
    pub name: String,
    /// Permissions array, defaults to `["read"]`.
    ///
    /// `read` reaches every safe HTTP method, `write` adds the unsafe ones, `admin` additionally
    /// makes the token count as a workspace administrator. Each implies the ones before it, so
    /// `["write"]` and `["read","write"]` are the same token. An empty array is rejected rather
    /// than issued as a token that can do nothing.
    pub permissions: Option<Vec<String>>,
    /// RFC3339 expiry, None = never
    pub expires_at: Option<String>,
}

/// Response for create — includes raw token (one-time only).
#[derive(Debug, Serialize)]
pub struct CreateBotResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub token: String, // raw token, returned ONCE
    pub token_prefix: String,
    pub permissions: Vec<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// Response for list — never includes token or hash.
#[derive(Debug, Serialize)]
pub struct BotResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub permissions: Vec<String>,
    pub is_active: bool,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

// ============================================================================
// Helpers
// ============================================================================

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen::<u8>()).collect();
    format!("opr_{}", hex::encode(&bytes))
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

fn workspace_role_from_permissions(perms: &[String]) -> &'static str {
    if bot_permissions_allow(perms, BotPermission::Admin) {
        "admin"
    } else {
        "member"
    }
}

/// Validate and canonicalize the permission list a bot token will carry.
///
/// The stored array is what every request is judged against, so it has to be exactly the vocabulary
/// the enforcement side understands: unknown names grant nothing there and would silently produce an
/// inert token here. Duplicates are collapsed so the stored value reads the same as it behaves.
fn normalize_bot_permissions(permissions: Vec<String>) -> Result<Vec<String>, ApiError> {
    const VALID_PERMISSIONS: [&str; 3] = ["read", "write", "admin"];
    let mut normalized: Vec<String> = Vec::with_capacity(permissions.len());
    for permission in permissions {
        let permission = permission.trim();
        if !VALID_PERMISSIONS.contains(&permission) {
            return Err(ApiError::BadRequest(format!(
                "invalid permission '{permission}', must be one of: read, write, admin"
            )));
        }
        if !normalized.iter().any(|kept| kept == permission) {
            normalized.push(permission.to_string());
        }
    }
    if normalized.is_empty() {
        return Err(ApiError::BadRequest(
            "permissions must contain at least one of: read, write, admin".to_string(),
        ));
    }
    Ok(normalized)
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /`api/v1/workspaces/:workspace_id/bots`
pub async fn create_bot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateBotRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    // Only owners/admins can create bots
    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can create bots".to_string(),
        ));
    }

    let perms = normalize_bot_permissions(req.permissions.unwrap_or_else(|| vec!["read".to_string()]))?;

    let expires_at: Option<chrono::DateTime<Utc>> = match req.expires_at {
        Some(ref s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| ApiError::BadRequest("invalid expires_at format".to_string()))?
                .with_timezone(&Utc),
        ),
        None => None,
    };

    let raw_token = generate_token();
    let token_hash = sha256_hex(&raw_token);
    // token_prefix: first 8 chars, e.g. "opr_a1b2"
    let token_prefix: String = raw_token.chars().take(8).collect();

    let bot_id = Uuid::new_v4();
    let now = Utc::now();
    let perms_json = serde_json::to_value(&perms).unwrap_or_else(|_| serde_json::json!(["read"]));
    let workspace_role = workspace_role_from_permissions(&perms);
    let bot_email = format!("{bot_id}@bot.openpr.local");
    let bot_name = req.name.clone();

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO users
           (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $8)",
        vec![
            bot_id.into(),
            bot_email.into(),
            "!".into(),
            bot_name.clone().into(),
            "user".into(),
            "bot_mcp".into(),
            "mcp".into(),
            now.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
           VALUES ($1, $2, $3, $4)",
        vec![workspace_id.into(), bot_id.into(), workspace_role.into(), now.into()],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_bots
           (id, workspace_id, name, token_hash, token_prefix, permissions,
            created_by, expires_at, is_active, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, $9, $9)",
        vec![
            bot_id.into(),
            workspace_id.into(),
            bot_name.clone().into(),
            token_hash.into(),
            token_prefix.clone().into(),
            perms_json.into(),
            user_id.into(),
            expires_at.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.commit().await?;

    Ok(ApiResponse::success(CreateBotResponse {
        id: bot_id,
        workspace_id,
        name: bot_name,
        token: raw_token,
        token_prefix,
        permissions: perms,
        expires_at: expires_at.map(|t| t.to_rfc3339()),
        created_at: now.to_rfc3339(),
    }))
}

/// GET /`api/v1/workspaces/:workspace_id/bots`
pub async fn list_bots(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can list bots".to_string(),
        ));
    }

    #[derive(Debug, FromQueryResult)]
    struct BotRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        token_prefix: String,
        permissions: serde_json::Value,
        is_active: bool,
        last_used_at: Option<chrono::DateTime<Utc>>,
        expires_at: Option<chrono::DateTime<Utc>>,
        created_at: chrono::DateTime<Utc>,
    }

    let bots = BotRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, name, token_prefix, permissions,
                  is_active, last_used_at, expires_at, created_at
           FROM workspace_bots
           WHERE workspace_id = $1
           ORDER BY created_at DESC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<BotResponse> = bots
        .into_iter()
        .map(|b| {
            let perms: Vec<String> = b
                .permissions
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            BotResponse {
                id: b.id,
                workspace_id: b.workspace_id,
                name: b.name,
                token_prefix: b.token_prefix,
                permissions: perms,
                is_active: b.is_active,
                last_used_at: b.last_used_at.map(|t| t.to_rfc3339()),
                expires_at: b.expires_at.map(|t| t.to_rfc3339()),
                created_at: b.created_at.to_rfc3339(),
            }
        })
        .collect();

    Ok(ApiResponse::success(response))
}

/// DELETE /`api/v1/workspaces/:workspace_id/bots/:bot_id`
pub async fn revoke_bot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, bot_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let (_, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only workspace owners and admins can revoke bots".to_string(),
        ));
    }

    // Verify bot belongs to this workspace
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND workspace_id = $2",
            vec![bot_id.into(), workspace_id.into()],
        ))
        .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound("bot not found".to_string()));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE workspace_bots SET is_active = false, updated_at = $1 WHERE id = $2",
            vec![Utc::now().into(), bot_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

#[cfg(test)]
mod tests {
    use super::{normalize_bot_permissions, workspace_role_from_permissions};

    #[test]
    fn permissions_are_canonicalized_and_deduplicated() {
        let normalized = normalize_bot_permissions(vec![" read ".to_string(), "write".to_string(), "read".to_string()])
            .expect("known permissions are accepted");

        assert_eq!(normalized, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn a_token_that_could_do_nothing_is_not_issued() {
        assert!(normalize_bot_permissions(Vec::new()).is_err());
        assert!(normalize_bot_permissions(vec!["Read".to_string()]).is_err());
        assert!(normalize_bot_permissions(vec!["owner".to_string()]).is_err());
    }

    #[test]
    fn only_the_admin_permission_seats_the_bot_as_a_workspace_admin() {
        assert_eq!(workspace_role_from_permissions(&["admin".to_string()]), "admin");
        assert_eq!(
            workspace_role_from_permissions(&["read".to_string(), "write".to_string()]),
            "member"
        );
    }
}
