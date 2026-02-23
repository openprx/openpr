use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager},
};

use crate::{
    error::ApiError,
    routes::auth::{extract_bearer_token, extract_cookie_token},
};

pub async fn auth_middleware(
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

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
