use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::trust_score_service::{is_project_member, normalize_domain_key},
};

#[derive(Debug, Deserialize)]
pub struct ListTrustScoresQuery {
    pub project_id: Option<Uuid>,
    pub domain: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UserTrustQuery {
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UserTrustHistoryQuery {
    pub project_id: Option<Uuid>,
    pub domain: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct TrustScoreRankRow {
    user_id: Uuid,
    user_type: String,
    project_id: Uuid,
    domain: String,
    score: i32,
    level: String,
    vote_weight: f64,
    consecutive_rejections: i32,
    cooldown_until: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct UserTrustScoreRow {
    user_id: Uuid,
    user_type: String,
    project_id: Uuid,
    domain: String,
    score: i32,
    level: String,
    vote_weight: f64,
    consecutive_rejections: i32,
    cooldown_until: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct UserTrustHistoryRow {
    id: i64,
    user_id: Uuid,
    project_id: Uuid,
    domain: String,
    event_type: String,
    event_id: String,
    score_change: i32,
    old_score: i32,
    new_score: i32,
    old_level: String,
    new_level: String,
    reason: String,
    is_appealed: bool,
    appeal_result: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct UserTrustScoreItem {
    project_id: Uuid,
    domain: String,
    score: i32,
    level: String,
    vote_weight: f64,
    can_veto: bool,
    consecutive_rejections: i32,
    cooldown_until: Option<String>,
    updated_at: String,
}

async fn load_user_trust_scores(
    state: &AppState,
    viewer_id: Uuid,
    target_user_id: Uuid,
    project_id: Option<Uuid>,
    domain: Option<String>,
) -> Result<Vec<UserTrustScoreRow>, ApiError> {
    if let Some(project_id) = project_id
        && !is_project_member(&state.db, project_id, viewer_id).await?
    {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let mut values: Vec<sea_orm::Value> = vec![target_user_id.into(), viewer_id.into()];
    let mut where_parts = vec![String::from(
        r#"
            ts.user_id = $1
            AND EXISTS (
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = ts.project_id
                  AND wm.user_id = $2
            )
        "#,
    )];
    let mut idx = 3;

    if let Some(project_id) = project_id {
        where_parts.push(format!("ts.project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(domain) = domain {
        where_parts.push(format!("ts.domain = ${idx}"));
        values.push(domain.into());
    }

    let sql = format!(
        r#"
            SELECT ts.user_id, ts.user_type::text AS user_type, ts.project_id, ts.domain, ts.score,
                   ts.level::text AS level, ts.vote_weight, ts.consecutive_rejections, ts.cooldown_until, ts.updated_at
            FROM trust_scores ts
            WHERE {}
            ORDER BY ts.project_id ASC, CASE WHEN ts.domain = 'global' THEN 0 ELSE 1 END, ts.domain ASC
        "#,
        where_parts.join(" AND ")
    );

    Ok(UserTrustScoreRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(&state.db)
    .await?)
}

fn build_user_trust_payload(
    target_user_id: Uuid,
    rows: Vec<UserTrustScoreRow>,
) -> Result<serde_json::Value, ApiError> {
    if rows.is_empty() {
        return Err(ApiError::NotFound("trust score not found".to_string()));
    }

    let user_type = rows[0].user_type.clone();
    let scores: Vec<UserTrustScoreItem> = rows
        .into_iter()
        .map(|row| {
            let can_veto = row.level == "vetoer" || row.level == "autonomous";
            UserTrustScoreItem {
                project_id: row.project_id,
                domain: row.domain,
                score: row.score,
                level: row.level,
                vote_weight: row.vote_weight,
                can_veto,
                consecutive_rejections: row.consecutive_rejections,
                cooldown_until: row.cooldown_until.map(|v| v.to_rfc3339()),
                updated_at: row.updated_at.to_rfc3339(),
            }
        })
        .collect();

    Ok(serde_json::json!({
        "user_id": target_user_id,
        "user_type": user_type,
        "scores": scores
    }))
}

pub async fn list_trust_scores(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListTrustScoresQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    if let Some(project_id) = query.project_id
        && !is_project_member(&state.db, project_id, user_id).await?
    {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let domain = query
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("global");
    let domain = if domain.eq_ignore_ascii_case("all") {
        None
    } else {
        let normalized = normalize_domain_key(domain);
        if normalized.is_empty() {
            return Err(ApiError::BadRequest("invalid domain".to_string()));
        }
        Some(normalized)
    };

    let mut where_parts = vec![String::from(
        r#"EXISTS (
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = ts.project_id
                  AND wm.user_id = $1
            )"#,
    )];
    let mut values: Vec<sea_orm::Value> = vec![user_id.into()];
    let mut idx = 2;

    if let Some(project_id) = query.project_id {
        where_parts.push(format!("ts.project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(domain) = domain {
        where_parts.push(format!("ts.domain = ${idx}"));
        values.push(domain.into());
        idx += 1;
    }

    let where_sql = where_parts.join(" AND ");

    let count_sql = format!("SELECT COUNT(*)::bigint AS count FROM trust_scores ts WHERE {where_sql}");
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        values.clone(),
    ))
    .one(&state.db)
    .await?
    .map(|r| r.count)
    .unwrap_or(0);

    let mut list_values = values;
    list_values.push(per_page.into());
    list_values.push(offset.into());
    let list_sql = format!(
        r#"
            SELECT ts.user_id, ts.user_type::text AS user_type, ts.project_id, ts.domain, ts.score,
                   ts.level::text AS level, ts.vote_weight, ts.consecutive_rejections, ts.cooldown_until, ts.updated_at
            FROM trust_scores ts
            WHERE {where_sql}
            ORDER BY ts.score DESC, ts.vote_weight DESC, ts.updated_at DESC
            LIMIT ${idx} OFFSET ${}
        "#,
        idx + 1
    );
    let items = TrustScoreRankRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?;

    let total_pages = if total == 0 { 0 } else { (total + per_page - 1) / per_page };
    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn get_user_trust(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(target_user_id): Path<Uuid>,
    Query(query): Query<UserTrustQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let rows = load_user_trust_scores(&state, user_id, target_user_id, query.project_id, None).await?;
    let payload = build_user_trust_payload(target_user_id, rows)?;
    Ok(ApiResponse::success(payload))
}

pub async fn get_user_trust_by_domain(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((target_user_id, domain)): Path<(Uuid, String)>,
    Query(query): Query<UserTrustQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let domain = normalize_domain_key(&domain);
    if domain.is_empty() {
        return Err(ApiError::BadRequest("invalid domain".to_string()));
    }

    let rows =
        load_user_trust_scores(&state, user_id, target_user_id, query.project_id, Some(domain))
            .await?;
    let payload = build_user_trust_payload(target_user_id, rows)?;
    Ok(ApiResponse::success(payload))
}

pub async fn list_user_trust_history(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(target_user_id): Path<Uuid>,
    Query(query): Query<UserTrustHistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    if let Some(project_id) = query.project_id
        && !is_project_member(&state.db, project_id, user_id).await?
    {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let domain = query
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(normalize_domain_key);
    if let Some(domain) = domain.as_ref()
        && domain.is_empty()
    {
        return Err(ApiError::BadRequest("invalid domain".to_string()));
    }

    let mut where_parts = vec![String::from(
        r#"
            t.user_id = $1
            AND EXISTS (
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = t.project_id
                  AND wm.user_id = $2
            )
        "#,
    )];
    let mut values: Vec<sea_orm::Value> = vec![target_user_id.into(), user_id.into()];
    let mut idx = 3;

    if let Some(project_id) = query.project_id {
        where_parts.push(format!("t.project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(domain) = domain {
        where_parts.push(format!("t.domain = ${idx}"));
        values.push(domain.into());
        idx += 1;
    }

    let where_sql = where_parts.join(" AND ");
    let count_sql =
        format!("SELECT COUNT(*)::bigint AS count FROM trust_score_logs t WHERE {where_sql}");
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        values.clone(),
    ))
    .one(&state.db)
    .await?
    .map(|r| r.count)
    .unwrap_or(0);

    let mut list_values = values;
    list_values.push(per_page.into());
    list_values.push(offset.into());
    let list_sql = format!(
        r#"
            SELECT t.id, t.user_id, t.project_id, t.domain, t.event_type, t.event_id,
                   t.score_change, t.old_score, t.new_score, t.old_level::text AS old_level,
                   t.new_level::text AS new_level, t.reason, t.is_appealed, t.appeal_result, t.created_at
            FROM trust_score_logs t
            WHERE {where_sql}
            ORDER BY t.created_at DESC, t.id DESC
            LIMIT ${idx} OFFSET ${}
        "#,
        idx + 1
    );
    let items = UserTrustHistoryRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?;

    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };
    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}
