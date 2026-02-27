use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::trust_score_service::{
        is_project_admin_or_owner, is_project_member, normalize_domain_key, scoped_domain_id,
    },
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct DecisionDomainRow {
    pub id: String,
    pub project_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub default_voting_rule: String,
    pub default_cycle_template: String,
    pub veto_threshold: i32,
    pub autonomous_threshold: i32,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDecisionDomainRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub default_voting_rule: Option<String>,
    pub default_cycle_template: Option<String>,
    pub veto_threshold: Option<i32>,
    pub autonomous_threshold: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDecisionDomainRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub default_voting_rule: Option<String>,
    pub default_cycle_template: Option<String>,
    pub veto_threshold: Option<i32>,
    pub autonomous_threshold: Option<i32>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ListDecisionDomainsQuery {
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct DomainMemberRow {
    pub user_id: Uuid,
    pub name: Option<String>,
    pub email: String,
    pub role: String,
    pub entity_type: String,
    pub trust_level: Option<String>,
    pub trust_score: Option<i32>,
    pub vote_weight: Option<f64>,
}

fn validate_voting_rule(value: &str) -> bool {
    matches!(value, "simple_majority" | "absolute_majority" | "consensus")
}

fn validate_cycle_template(value: &str) -> bool {
    matches!(value, "rapid" | "fast" | "standard" | "critical")
}

pub async fn initialize_default_domains_for_project(
    state: &AppState,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let defaults = [
        (
            "code_quality",
            "Code Quality",
            "simple_majority",
            "fast",
            200,
            300,
        ),
        (
            "architecture",
            "Architecture",
            "absolute_majority",
            "standard",
            200,
            350,
        ),
        ("priority", "Priority", "simple_majority", "fast", 200, 300),
        (
            "ux_design",
            "UX Design",
            "simple_majority",
            "fast",
            200,
            300,
        ),
        ("security", "Security", "consensus", "critical", 180, 400),
        (
            "business",
            "Business",
            "absolute_majority",
            "standard",
            250,
            999,
        ),
        (
            "governance",
            "Governance",
            "consensus",
            "critical",
            300,
            999,
        ),
    ];

    for (key, name, rule, cycle, veto_threshold, autonomous_threshold) in defaults {
        let domain_id = scoped_domain_id(project_id, key);
        state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO decision_domains (
                        id, project_id, name, description, default_voting_rule, default_cycle_template,
                        veto_threshold, autonomous_threshold, is_active, created_at
                    ) VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, true, $8)
                    ON CONFLICT (id) DO NOTHING
                "#,
                vec![
                    domain_id.into(),
                    project_id.into(),
                    name.to_string().into(),
                    rule.to_string().into(),
                    cycle.to_string().into(),
                    veto_threshold.into(),
                    autonomous_threshold.into(),
                    Utc::now().into(),
                ],
            ))
            .await?;
    }
    Ok(())
}

pub async fn list_decision_domains(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    initialize_default_domains_for_project(&state, project_id).await?;

    let items = DecisionDomainRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, description, default_voting_rule, default_cycle_template,
                   veto_threshold, autonomous_threshold, is_active, created_at
            FROM decision_domains
            WHERE project_id = $1
            ORDER BY created_at ASC
        "#,
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn list_decision_domains_global(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListDecisionDomainsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if let Some(project_id) = query.project_id {
        if !is_project_member(&state.db, project_id, user_id).await? {
            return Err(ApiError::Forbidden("project access denied".to_string()));
        }
        initialize_default_domains_for_project(&state, project_id).await?;
    }

    let mut values: Vec<sea_orm::Value> = vec![user_id.into()];
    let mut where_parts = vec![String::from(
        r#"EXISTS (
                SELECT 1
                FROM projects p
                WHERE p.id = decision_domains.project_id
                  AND (
                    EXISTS (
                        SELECT 1
                        FROM workspace_members wm
                        WHERE wm.workspace_id = p.workspace_id
                          AND wm.user_id = $1
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM workspace_bots wb
                        WHERE wb.workspace_id = p.workspace_id
                          AND wb.id = $1
                    )
                  )
            )"#,
    )];
    if let Some(project_id) = query.project_id {
        where_parts.push("project_id = $2".to_string());
        values.push(project_id.into());
    }

    let sql = format!(
        r#"
            SELECT id, project_id, name, description, default_voting_rule, default_cycle_template,
                   veto_threshold, autonomous_threshold, is_active, created_at
            FROM decision_domains
            WHERE {}
            ORDER BY project_id ASC, created_at ASC
        "#,
        where_parts.join(" AND ")
    );

    let items = DecisionDomainRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn create_decision_domain(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateDecisionDomainRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    if req.name.trim().is_empty() || req.key.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "key and name are required".to_string(),
        ));
    }

    let voting_rule = req
        .default_voting_rule
        .unwrap_or_else(|| "simple_majority".to_string());
    if !validate_voting_rule(&voting_rule) {
        return Err(ApiError::BadRequest(
            "invalid default_voting_rule".to_string(),
        ));
    }

    let cycle_template = req
        .default_cycle_template
        .unwrap_or_else(|| "fast".to_string());
    if !validate_cycle_template(&cycle_template) {
        return Err(ApiError::BadRequest(
            "invalid default_cycle_template".to_string(),
        ));
    }

    let veto_threshold = req.veto_threshold.unwrap_or(200).max(0);
    let autonomous_threshold = req.autonomous_threshold.unwrap_or(300).max(0);
    let domain_key = normalize_domain_key(&req.key);
    let domain_id = scoped_domain_id(project_id, &domain_key);
    let now = Utc::now();

    let insert = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO decision_domains (
                    id, project_id, name, description, default_voting_rule, default_cycle_template,
                    veto_threshold, autonomous_threshold, is_active, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, $9)
            "#,
            vec![
                domain_id.clone().into(),
                project_id.into(),
                req.name.trim().into(),
                req.description.unwrap_or_default().into(),
                voting_rule.clone().into(),
                cycle_template.clone().into(),
                veto_threshold.into(),
                autonomous_threshold.into(),
                now.into(),
            ],
        ))
        .await;

    if let Err(err) = insert {
        let message = err.to_string();
        if message.contains("duplicate key value") || message.contains("decision_domains_pkey") {
            return Err(ApiError::Conflict("domain key already exists".to_string()));
        }
        return Err(ApiError::Database(err));
    }

    Ok(ApiResponse::success(serde_json::json!({
        "id": domain_id,
        "project_id": project_id,
        "name": req.name.trim(),
        "default_voting_rule": voting_rule,
        "default_cycle_template": cycle_template,
        "veto_threshold": veto_threshold,
        "autonomous_threshold": autonomous_threshold,
        "is_active": true,
        "created_at": now.to_rfc3339(),
    })))
}

pub async fn update_decision_domain(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, domain_id)): Path<(Uuid, String)>,
    Json(req): Json<UpdateDecisionDomainRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    if let Some(rule) = req.default_voting_rule.as_deref() {
        if !validate_voting_rule(rule) {
            return Err(ApiError::BadRequest(
                "invalid default_voting_rule".to_string(),
            ));
        }
    }
    if let Some(cycle) = req.default_cycle_template.as_deref() {
        if !validate_cycle_template(cycle) {
            return Err(ApiError::BadRequest(
                "invalid default_cycle_template".to_string(),
            ));
        }
    }

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${idx}"));
        values.push(name.trim().to_string().into());
        idx += 1;
    }
    if let Some(description) = req.description {
        updates.push(format!("description = ${idx}"));
        values.push(description.into());
        idx += 1;
    }
    if let Some(rule) = req.default_voting_rule {
        updates.push(format!("default_voting_rule = ${idx}"));
        values.push(rule.into());
        idx += 1;
    }
    if let Some(cycle) = req.default_cycle_template {
        updates.push(format!("default_cycle_template = ${idx}"));
        values.push(cycle.into());
        idx += 1;
    }
    if let Some(veto_threshold) = req.veto_threshold {
        updates.push(format!("veto_threshold = ${idx}"));
        values.push(veto_threshold.max(0).into());
        idx += 1;
    }
    if let Some(autonomous_threshold) = req.autonomous_threshold {
        updates.push(format!("autonomous_threshold = ${idx}"));
        values.push(autonomous_threshold.max(0).into());
        idx += 1;
    }
    if let Some(is_active) = req.is_active {
        updates.push(format!("is_active = ${idx}"));
        values.push(is_active.into());
        idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    values.push(domain_id.clone().into());
    values.push(project_id.into());

    let sql = format!(
        "UPDATE decision_domains SET {} WHERE id = ${} AND project_id = ${}",
        updates.join(", "),
        idx,
        idx + 1
    );

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("domain not found".to_string()));
    }

    let domain = DecisionDomainRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, description, default_voting_rule, default_cycle_template,
                   veto_threshold, autonomous_threshold, is_active, created_at
            FROM decision_domains
            WHERE id = $1 AND project_id = $2
        "#,
        vec![domain_id.into(), project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("domain not found".to_string()))?;

    Ok(ApiResponse::success(domain))
}

pub async fn delete_decision_domain(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, domain_id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_admin_or_owner(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM decision_domains WHERE id = $1 AND project_id = $2",
            vec![domain_id.into(), project_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("domain not found".to_string()));
    }

    Ok(ApiResponse::ok())
}

pub async fn list_domain_members(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, domain_id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if !is_project_member(&state.db, project_id, user_id).await? {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }

    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM decision_domains WHERE id = $1 AND project_id = $2",
            vec![domain_id.clone().into(), project_id.into()],
        ))
        .await?
        .is_some();
    if !exists {
        return Err(ApiError::NotFound("domain not found".to_string()));
    }

    let items = DomainMemberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wm.user_id,
                   COALESCE(to_jsonb(u)->>'name', to_jsonb(u)->>'display_name') AS name,
                   u.email,
                   wm.role,
                   COALESCE(u.entity_type, 'human') AS entity_type,
                   ts.level::text AS trust_level,
                   ts.score AS trust_score,
                   ts.vote_weight
            FROM projects p
            INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id
            INNER JOIN users u ON u.id = wm.user_id
            LEFT JOIN trust_scores ts ON ts.project_id = p.id
                                      AND ts.user_id = wm.user_id
                                      AND ts.domain = $2
            WHERE p.id = $1
            ORDER BY wm.role ASC, u.email ASC
        "#,
        vec![project_id.into(), domain_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}
