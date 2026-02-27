use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::trust_score_service::{is_project_admin_or_owner, is_project_member},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct AiAgentRow {
    pub id: String,
    pub project_id: Uuid,
    pub name: String,
    pub model: String,
    pub provider: String,
    pub api_endpoint: Option<String>,
    pub capabilities: Value,
    pub domain_overrides: Option<Value>,
    pub max_domain_level: String,
    pub can_veto_human_consensus: bool,
    pub reason_min_length: i32,
    pub is_active: bool,
    pub registered_by: Uuid,
    pub last_active_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAiAgentRequest {
    pub id: String,
    pub name: String,
    pub model: String,
    pub provider: String,
    pub api_endpoint: Option<String>,
    pub capabilities: Value,
    pub domain_overrides: Option<Value>,
    pub max_domain_level: Option<String>,
    pub can_veto_human_consensus: Option<bool>,
    pub reason_min_length: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAiAgentRequest {
    pub name: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub api_endpoint: Option<String>,
    pub capabilities: Option<Value>,
    pub domain_overrides: Option<Value>,
    pub max_domain_level: Option<String>,
    pub can_veto_human_consensus: Option<bool>,
    pub reason_min_length: Option<i32>,
    pub is_active: Option<bool>,
}

#[derive(Debug, FromQueryResult)]
struct AiAgentPolicyRow {
    max_domain_level: String,
    can_veto_human_consensus: bool,
}

#[derive(Debug, FromQueryResult)]
struct AiAgentVoteStatsRow {
    total_votes: i64,
    yes_votes: i64,
    no_votes: i64,
    abstain_votes: i64,
    last_voted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct AiAgentCommentStatsRow {
    total_comments: i64,
    last_commented_at: Option<DateTime<Utc>>,
}

fn valid_max_level(value: &str) -> bool {
    matches!(
        value,
        "observer" | "advisor" | "voter" | "vetoer" | "autonomous"
    )
}

pub async fn list_ai_agents(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let items = AiAgentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, model, provider, api_endpoint,
                   capabilities, domain_overrides, max_domain_level,
                   can_veto_human_consensus, reason_min_length, is_active,
                   registered_by, last_active_at, created_at
            FROM ai_participants
            WHERE project_id = $1
            ORDER BY created_at DESC
        "#,
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn create_ai_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateAiAgentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    if req.id.trim().is_empty()
        || req.name.trim().is_empty()
        || req.model.trim().is_empty()
        || req.provider.trim().is_empty()
    {
        return Err(ApiError::BadRequest(
            "id, name, model and provider are required".to_string(),
        ));
    }
    if req.id.trim().len() > 100 {
        return Err(ApiError::BadRequest(
            "id length must be <= 100 characters".to_string(),
        ));
    }

    let max_domain_level = req
        .max_domain_level
        .unwrap_or_else(|| "voter".to_string())
        .to_lowercase();
    if !valid_max_level(&max_domain_level) {
        return Err(ApiError::BadRequest("invalid max_domain_level".to_string()));
    }

    if req.can_veto_human_consensus.unwrap_or(false)
        && !matches!(max_domain_level.as_str(), "vetoer" | "autonomous")
    {
        return Err(ApiError::BadRequest(
            "can_veto_human_consensus requires max_domain_level >= vetoer".to_string(),
        ));
    }

    let reason_min_length = req.reason_min_length.unwrap_or(50).max(0);

    let insert = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO ai_participants (
                    id, project_id, name, model, provider, api_endpoint,
                    capabilities, domain_overrides, max_domain_level,
                    can_veto_human_consensus, reason_min_length, is_active,
                    registered_by, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, true, $12, $13)
            "#,
            vec![
                req.id.trim().to_string().into(),
                project_id.into(),
                req.name.trim().to_string().into(),
                req.model.trim().to_string().into(),
                req.provider.trim().to_string().into(),
                req.api_endpoint.clone().into(),
                req.capabilities.clone().into(),
                req.domain_overrides.clone().into(),
                max_domain_level.into(),
                req.can_veto_human_consensus.unwrap_or(false).into(),
                reason_min_length.into(),
                user_id.into(),
                Utc::now().into(),
            ],
        ))
        .await;

    if let Err(err) = insert {
        let message = err.to_string();
        if message.contains("duplicate key value") || message.contains("ai_participants_pkey") {
            return Err(ApiError::Conflict("ai agent id already exists".to_string()));
        }
        return Err(ApiError::Database(err));
    }

    get_ai_agent(
        State(state),
        Extension(claims),
        Path((project_id, req.id.trim().to_string())),
    )
    .await
}

pub async fn get_ai_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let item = AiAgentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, model, provider, api_endpoint,
                   capabilities, domain_overrides, max_domain_level,
                   can_veto_human_consensus, reason_min_length, is_active,
                   registered_by, last_active_at, created_at
            FROM ai_participants
            WHERE project_id = $1 AND id = $2
        "#,
        vec![project_id.into(), id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai agent not found".to_string()))?;

    Ok(ApiResponse::success(item))
}

pub async fn update_ai_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, id)): Path<(Uuid, String)>,
    Json(req): Json<UpdateAiAgentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    if req.name.is_none()
        && req.model.is_none()
        && req.provider.is_none()
        && req.api_endpoint.is_none()
        && req.capabilities.is_none()
        && req.domain_overrides.is_none()
        && req.max_domain_level.is_none()
        && req.can_veto_human_consensus.is_none()
        && req.reason_min_length.is_none()
        && req.is_active.is_none()
    {
        return Err(ApiError::BadRequest("no changes provided".to_string()));
    }

    let current = AiAgentPolicyRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT max_domain_level, can_veto_human_consensus
            FROM ai_participants
            WHERE project_id = $1 AND id = $2
        "#,
        vec![project_id.into(), id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai agent not found".to_string()))?;

    let merged_max_domain_level = req
        .max_domain_level
        .as_deref()
        .unwrap_or(&current.max_domain_level)
        .to_lowercase();
    let merged_can_veto_human_consensus = req
        .can_veto_human_consensus
        .unwrap_or(current.can_veto_human_consensus);

    if !valid_max_level(&merged_max_domain_level) {
        return Err(ApiError::BadRequest("invalid max_domain_level".to_string()));
    }
    if merged_can_veto_human_consensus
        && !matches!(merged_max_domain_level.as_str(), "vetoer" | "autonomous")
    {
        return Err(ApiError::BadRequest(
            "can_veto_human_consensus requires max_domain_level >= vetoer".to_string(),
        ));
    }

    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut sets: Vec<String> = Vec::new();
    let mut idx = 1;

    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }
        sets.push(format!("name = ${idx}"));
        values.push(name.trim().to_string().into());
        idx += 1;
    }
    if let Some(model) = req.model {
        if model.trim().is_empty() {
            return Err(ApiError::BadRequest("model cannot be empty".to_string()));
        }
        sets.push(format!("model = ${idx}"));
        values.push(model.trim().to_string().into());
        idx += 1;
    }
    if let Some(provider) = req.provider {
        if provider.trim().is_empty() {
            return Err(ApiError::BadRequest("provider cannot be empty".to_string()));
        }
        sets.push(format!("provider = ${idx}"));
        values.push(provider.trim().to_string().into());
        idx += 1;
    }
    if let Some(api_endpoint) = req.api_endpoint {
        sets.push(format!("api_endpoint = ${idx}"));
        values.push(api_endpoint.into());
        idx += 1;
    }
    if let Some(capabilities) = req.capabilities {
        sets.push(format!("capabilities = ${idx}"));
        values.push(capabilities.into());
        idx += 1;
    }
    if let Some(domain_overrides) = req.domain_overrides {
        sets.push(format!("domain_overrides = ${idx}"));
        values.push(domain_overrides.into());
        idx += 1;
    }
    if let Some(max_domain_level) = req.max_domain_level {
        let normalized = max_domain_level.to_lowercase();
        if !valid_max_level(&normalized) {
            return Err(ApiError::BadRequest("invalid max_domain_level".to_string()));
        }
        sets.push(format!("max_domain_level = ${idx}"));
        values.push(normalized.into());
        idx += 1;
    }
    if let Some(can_veto_human_consensus) = req.can_veto_human_consensus {
        sets.push(format!("can_veto_human_consensus = ${idx}"));
        values.push(can_veto_human_consensus.into());
        idx += 1;
    }
    if let Some(reason_min_length) = req.reason_min_length {
        sets.push(format!("reason_min_length = ${idx}"));
        values.push(reason_min_length.max(0).into());
        idx += 1;
    }
    if let Some(is_active) = req.is_active {
        sets.push(format!("is_active = ${idx}"));
        values.push(is_active.into());
        idx += 1;
    }

    values.push(project_id.into());
    values.push(id.clone().into());

    let sql = format!(
        "UPDATE ai_participants SET {} WHERE project_id = ${idx} AND id = ${} RETURNING id",
        sets.join(", "),
        idx + 1
    );

    let updated = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;

    if updated.is_none() {
        return Err(ApiError::NotFound("ai agent not found".to_string()));
    }

    get_ai_agent(State(state), Extension(claims), Path((project_id, id))).await
}

pub async fn delete_ai_agent(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let deleted = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE ai_participants SET is_active = false WHERE project_id = $1 AND id = $2",
            vec![project_id.into(), id.into()],
        ))
        .await?;

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound("ai agent not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

pub async fn get_ai_agent_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let agent = AiAgentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, model, provider, api_endpoint,
                   capabilities, domain_overrides, max_domain_level,
                   can_veto_human_consensus, reason_min_length, is_active,
                   registered_by, last_active_at, created_at
            FROM ai_participants
            WHERE project_id = $1 AND id = $2
        "#,
        vec![project_id.into(), id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai agent not found".to_string()))?;

    let vote_stats = AiAgentVoteStatsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                COUNT(*)::bigint AS total_votes,
                COUNT(*) FILTER (WHERE v.choice = 'yes')::bigint AS yes_votes,
                COUNT(*) FILTER (WHERE v.choice = 'no')::bigint AS no_votes,
                COUNT(*) FILTER (WHERE v.choice = 'abstain')::bigint AS abstain_votes,
                MAX(v.voted_at) AS last_voted_at
            FROM votes v
            WHERE v.voter_id = $2
              AND v.voter_type = 'ai'::author_type
              AND EXISTS (
                  SELECT 1
                  FROM proposal_issue_links pil
                  INNER JOIN work_items wi ON wi.id = pil.issue_id
                  WHERE pil.proposal_id = v.proposal_id
                    AND wi.project_id = $1
              )
        "#,
        vec![project_id.into(), id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    let comment_stats = AiAgentCommentStatsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                COUNT(*)::bigint AS total_comments,
                MAX(pc.created_at) AS last_commented_at
            FROM proposal_comments pc
            WHERE pc.author_id = $2
              AND pc.author_type = 'ai'::author_type
              AND EXISTS (
                  SELECT 1
                  FROM proposal_issue_links pil
                  INNER JOIN work_items wi ON wi.id = pil.issue_id
                  WHERE pil.proposal_id = pc.proposal_id
                    AND wi.project_id = $1
              )
        "#,
        vec![project_id.into(), id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;

    let latest_activity = [
        agent.last_active_at,
        vote_stats.last_voted_at,
        comment_stats.last_commented_at,
    ]
    .into_iter()
    .flatten()
    .max()
    .map(|time| time.to_rfc3339());

    Ok(ApiResponse::success(json!({
        "id": agent.id,
        "project_id": project_id,
        "total_votes": vote_stats.total_votes,
        "yes_votes": vote_stats.yes_votes,
        "no_votes": vote_stats.no_votes,
        "abstain_votes": vote_stats.abstain_votes,
        "total_comments": comment_stats.total_comments,
        "last_voted_at": vote_stats.last_voted_at.map(|time| time.to_rfc3339()),
        "last_commented_at": comment_stats.last_commented_at.map(|time| time.to_rfc3339()),
        "last_active_at": latest_activity
    })))
}
