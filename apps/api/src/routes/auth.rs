use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use bcrypt::{DEFAULT_COST, hash, verify};
use platform::{
    app::AppState,
    auth::{JwtClaims, JwtManager},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, response::ApiResponse};

#[derive(Debug, Serialize)]
struct UserResponse {
    id: String,
    email: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    email: String,
    password: String,
    name: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthTokens {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    access_expires_in: i64,
    refresh_expires_in: i64,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    user: UserResponse,
    tokens: AuthTokens,
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    user: UserResponse,
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<Response, ApiError> {
    if req.email.trim().is_empty() || req.password.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "email, password and name are required".to_string(),
        ));
    }

    if req.password.len() < 8 {
        return Err(ApiError::BadRequest(
            "password must be at least 8 characters".to_string(),
        ));
    }

    let normalized_email = req.email.trim().to_lowercase();

    let user_count_row = state
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint as total FROM users".to_string(),
        ))
        .await?
        .ok_or(ApiError::Internal)?;
    let user_count = user_count_row
        .try_get::<i64>("", "total")
        .map_err(|_| ApiError::Internal)?;

    let role = if user_count == 0 {
        "admin".to_string()
    } else {
        let access_token = extract_bearer_token(&headers)
            .or_else(|| extract_cookie_token(&headers, "access_token"))
            .ok_or_else(|| ApiError::Forbidden("admin access required".to_string()))?;
        let claims = jwt_manager(&state)
            .verify_access_token(&access_token)
            .map_err(|_| ApiError::Forbidden("admin access required".to_string()))?;
        ensure_admin_user(&state, &claims.sub).await?;
        "user".to_string()
    };

    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM users WHERE email = $1".to_string(),
            vec![normalized_email.clone().into()],
        ))
        .await?;
    if existing.is_some() {
        return Err(ApiError::Conflict("email already registered".to_string()));
    }

    let password_hash = hash(req.password, DEFAULT_COST).map_err(|_| ApiError::Internal)?;
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (email, password_hash, name, role, is_active, created_at, updated_at) VALUES ($1, $2, $3, $4, true, now(), now())"
                .to_string(),
            vec![
                normalized_email.clone().into(),
                password_hash.into(),
                req.name.trim().to_string().into(),
                role.into(),
            ],
        ))
        .await?;

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id::text as id, email, name, role FROM users WHERE email = $1".to_string(),
            vec![normalized_email.into()],
        ))
        .await?
        .ok_or(ApiError::Internal)?;

    let user = UserResponse {
        id: row
            .try_get::<String>("", "id")
            .map_err(|_| ApiError::Internal)?,
        email: row
            .try_get::<String>("", "email")
            .map_err(|_| ApiError::Internal)?,
        name: row
            .try_get::<String>("", "name")
            .map_err(|_| ApiError::Internal)?,
        role: row
            .try_get::<String>("", "role")
            .ok(),
    };

    let jwt = jwt_manager(&state);
    let (tokens, cookies) = build_auth_response(&jwt, &state, &user)?;
    let mut resp = ApiResponse::success(AuthResponse { user, tokens }).into_response();
    resp.headers_mut().append(header::SET_COOKIE, cookies.0);
    resp.headers_mut().append(header::SET_COOKIE, cookies.1);
    Ok(resp)
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let normalized_email = req.email.trim().to_lowercase();

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id::text as id, email, password_hash, name, role, entity_type FROM users WHERE email = $1"
                .to_string(),
            vec![normalized_email.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid email or password".to_string()))?;

    let entity_type = row
        .try_get::<String>("", "entity_type")
        .map_err(|_| ApiError::Internal)?;
    if entity_type == "bot" {
        return Err(ApiError::Unauthorized(
            "bot user cannot login with password".to_string(),
        ));
    }

    let password_hash = row
        .try_get::<String>("", "password_hash")
        .map_err(|_| ApiError::Unauthorized("invalid email or password".to_string()))?;
    if password_hash.is_empty() {
        return Err(ApiError::Unauthorized("invalid email or password".to_string()));
    }
    let verified = verify(req.password, &password_hash)
        .map_err(|_| ApiError::Unauthorized("invalid email or password".to_string()))?;
    if !verified {
        return Err(ApiError::Unauthorized(
            "invalid email or password".to_string(),
        ));
    }

    let user = UserResponse {
        id: row
            .try_get::<String>("", "id")
            .map_err(|_| ApiError::Internal)?,
        email: row
            .try_get::<String>("", "email")
            .map_err(|_| ApiError::Internal)?,
        name: row
            .try_get::<String>("", "name")
            .map_err(|_| ApiError::Internal)?,
        role: row
            .try_get::<String>("", "role")
            .ok(),
    };

    let jwt = jwt_manager(&state);
    let (tokens, response) = build_auth_response(&jwt, &state, &user)?;

    let mut resp = ApiResponse::success(AuthResponse { user, tokens }).into_response();
    resp.headers_mut().append(header::SET_COOKIE, response.0);
    resp.headers_mut().append(header::SET_COOKIE, response.1);
    Ok(resp)
}

pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    let refresh_token = req
        .refresh_token
        .or_else(|| extract_cookie_token(&headers, "refresh_token"))
        .or_else(|| extract_bearer_token(&headers))
        .ok_or_else(|| ApiError::Unauthorized("missing refresh token".to_string()))?;

    let jwt = jwt_manager(&state);
    let claims = jwt
        .verify_refresh_token(&refresh_token)
        .map_err(|_| ApiError::Unauthorized("invalid refresh token".to_string()))?;

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id::text as id, email, name, role FROM users WHERE id = $1::uuid".to_string(),
            vec![claims.sub.clone().into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".to_string()))?;

    let user = UserResponse {
        id: row
            .try_get::<String>("", "id")
            .map_err(|_| ApiError::Internal)?,
        email: row
            .try_get::<String>("", "email")
            .map_err(|_| ApiError::Internal)?,
        name: row
            .try_get::<String>("", "name")
            .map_err(|_| ApiError::Internal)?,
        role: row
            .try_get::<String>("", "role")
            .ok(),
    };

    let (tokens, cookies) = build_auth_response(&jwt, &state, &user)?;
    let mut resp = ApiResponse::success(AuthResponse { user, tokens }).into_response();
    resp.headers_mut().append(header::SET_COOKIE, cookies.0);
    resp.headers_mut().append(header::SET_COOKIE, cookies.1);
    Ok(resp)
}

pub async fn logout() -> Result<Response, ApiError> {
    let mut resp = ApiResponse::ok().into_response();
    resp.headers_mut()
        .append(header::SET_COOKIE, clear_cookie_header("access_token"));
    resp.headers_mut()
        .append(header::SET_COOKIE, clear_cookie_header("refresh_token"));
    Ok(resp)
}

pub async fn me(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id::text as id, email, name, role FROM users WHERE id = $1::uuid".to_string(),
            vec![claims.sub.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".to_string()))?;

    let user = UserResponse {
        id: row
            .try_get::<String>("", "id")
            .map_err(|_| ApiError::Internal)?,
        email: row
            .try_get::<String>("", "email")
            .map_err(|_| ApiError::Internal)?,
        name: row
            .try_get::<String>("", "name")
            .map_err(|_| ApiError::Internal)?,
        role: row
            .try_get::<String>("", "role")
            .ok(),
    };

    Ok(ApiResponse::success(MeResponse { user }))
}

fn jwt_manager(state: &AppState) -> JwtManager {
    JwtManager::new(
        &state.cfg.jwt_secret,
        state.cfg.jwt_access_ttl_seconds,
        state.cfg.jwt_refresh_ttl_seconds,
    )
}

fn build_auth_response(
    jwt: &JwtManager,
    state: &AppState,
    user: &UserResponse,
) -> Result<(AuthTokens, (HeaderValue, HeaderValue)), ApiError> {
    let access_token = jwt
        .issue_access_token(&user.id, &user.email)
        .map_err(|_| ApiError::Internal)?;
    let refresh_token = jwt
        .issue_refresh_token(&user.id, &user.email)
        .map_err(|_| ApiError::Internal)?;

    let tokens = AuthTokens {
        access_token: access_token.clone(),
        refresh_token: refresh_token.clone(),
        token_type: "Bearer",
        access_expires_in: state.cfg.jwt_access_ttl_seconds,
        refresh_expires_in: state.cfg.jwt_refresh_ttl_seconds,
    };

    let access_cookie = auth_cookie_header(
        "access_token",
        &access_token,
        state.cfg.jwt_access_ttl_seconds,
    )?;
    let refresh_cookie = auth_cookie_header(
        "refresh_token",
        &refresh_token,
        state.cfg.jwt_refresh_ttl_seconds,
    )?;

    Ok((tokens, (access_cookie, refresh_cookie)))
}

fn auth_cookie_header(
    name: &str,
    value: &str,
    max_age_seconds: i64,
) -> Result<HeaderValue, ApiError> {
    let cookie =
        format!("{name}={value}; HttpOnly; Path=/; Max-Age={max_age_seconds}; SameSite=Lax");
    HeaderValue::from_str(&cookie).map_err(|_| ApiError::Internal)
}

fn clear_cookie_header(name: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax"
    ))
    .expect("static cookie header should be valid")
}

pub fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .map(|token| token.trim().to_string())
}

pub fn extract_cookie_token(headers: &HeaderMap, key: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|cookie| {
        let mut parts = cookie.trim().splitn(2, '=');
        let k = parts.next()?;
        let v = parts.next()?;
        if k == key { Some(v.to_string()) } else { None }
    })
}

async fn ensure_admin_user(state: &AppState, user_id: &str) -> Result<(), ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role, is_active FROM users WHERE id = $1::uuid".to_string(),
            vec![user_id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Forbidden("admin access required".to_string()))?;

    let role = row
        .try_get::<String>("", "role")
        .map_err(|_| ApiError::Internal)?;
    let is_active = row
        .try_get::<bool>("", "is_active")
        .map_err(|_| ApiError::Internal)?;

    if !is_active || role != "admin" {
        return Err(ApiError::Forbidden("admin access required".to_string()));
    }

    Ok(())
}
