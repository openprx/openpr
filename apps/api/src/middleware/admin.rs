use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};

use crate::{
    error::ApiError,
    routes::auth::{extract_bearer_token, extract_cookie_token},
};

pub async fn admin_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(req.headers())
        .or_else(|| extract_cookie_token(req.headers(), "access_token"))
        .ok_or_else(|| ApiError::Unauthorized("missing access token".to_string()))?;

    let jwt = JwtManager::new(
        &state.cfg.jwt_secret,
        state.cfg.jwt_access_ttl_seconds,
        state.cfg.jwt_refresh_ttl_seconds,
    );
    let claims: JwtClaims = jwt
        .verify_access_token(&token)
        .map_err(|_| ApiError::Unauthorized("invalid access token".to_string()))?;

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role, is_active FROM users WHERE id = $1::uuid".to_string(),
            vec![claims.sub.clone().into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".to_string()))?;

    let role = row
        .try_get::<String>("", "role")
        .map_err(|_| ApiError::Internal)?;
    let is_active = row
        .try_get::<bool>("", "is_active")
        .map_err(|_| ApiError::Internal)?;

    tracing::debug!(
        user_id = %claims.sub,
        role = %role,
        is_active = is_active,
        "admin middleware role check"
    );

    if !is_active {
        tracing::warn!(user_id = %claims.sub, "admin middleware denied disabled user");
        return Err(ApiError::Forbidden("user is disabled".to_string()));
    }

    if role.trim().to_lowercase() != "admin" {
        tracing::warn!(
            user_id = %claims.sub,
            role = %role,
            "admin middleware denied non-admin role"
        );
        return Err(ApiError::Forbidden("admin access required".to_string()));
    }

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
