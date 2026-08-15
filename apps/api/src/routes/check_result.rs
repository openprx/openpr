// Public and framework-facing signatures remain stable during this behavior-neutral cleanup.
#![allow(clippy::needless_pass_by_value)]

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::{ApiResponse, PaginatedData},
    services::governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
};

#[derive(Debug, Deserialize)]
pub struct ListCheckResultsQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCheckResultRequest {
    pub invocation_id: Option<Uuid>,
    pub connector_id: Option<Uuid>,
    pub action_class: Option<String>,
    pub risk_level: Option<String>,
    pub title: String,
    pub summary: String,
    pub result: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProposalFromResultRequest {
    pub title: Option<String>,
    pub proposal_type: Option<String>,
    pub content: Option<String>,
    pub domains: Option<Vec<String>>,
    pub voting_rule: Option<String>,
    pub cycle_template: Option<String>,
    pub submit: Option<bool>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct CheckResultRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub connector_id: Option<Uuid>,
    pub action_class: String,
    pub risk_level: String,
    pub title: String,
    pub summary: String,
    pub result: Value,
    pub status: String,
    pub proposal_id: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_by_kind: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct CheckResultApplyRow {
    id: Uuid,
    project_id: Uuid,
    result: Value,
    created_by: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApprovedEffect {
    tool: String,
    arguments: Value,
}

struct CheckResultEventInput<'a> {
    event_type: &'a str,
    workspace_id: Uuid,
    project_id: Uuid,
    check_result_id: Uuid,
    invocation_id: Option<Uuid>,
    connector_id: Option<Uuid>,
    action_class: &'a str,
    risk_level: &'a str,
    title: &'a str,
    summary: &'a str,
    result: &'a Value,
    status: &'a str,
    proposal_id: Option<&'a str>,
    actor_id: Option<Uuid>,
    actor_kind: &'a str,
}

struct ProposalFromCheckResultEventInput<'a> {
    workspace_id: Uuid,
    project_id: Uuid,
    proposal_id: &'a str,
    check_result_id: Uuid,
    proposal_type: &'a str,
    proposal_status: &'a str,
    voting_rule: &'a str,
    cycle_template: &'a str,
    domains: &'a [String],
    action_class: &'a str,
    risk_level: &'a str,
    actor_id: Option<Uuid>,
    actor_kind: &'a str,
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

pub async fn list_project_check_results(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListCheckResultsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let project = ensure_project_access(&state, claims, bot, project_id).await?;
    let _ = project;
    if let Some(status) = query.status.as_deref() {
        normalize_status(status)?;
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    let mut where_sql = "WHERE project_id = $1".to_string();
    let mut values = vec![project_id.into()];
    let mut idx = 2;
    if let Some(status) = query.status {
        let _ = write!(where_sql, " AND status = ${idx}");
        values.push(normalize_status(&status)?.into());
        idx += 1;
    }

    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*)::bigint AS count FROM check_results {where_sql}"),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    values.push(per_page.into());
    values.push(offset.into());
    let items = CheckResultRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                SELECT id, workspace_id, project_id, invocation_id, connector_id,
                       action_class, risk_level, title, summary, result, status,
                       proposal_id, created_by, created_by_kind, created_at, updated_at
                FROM check_results
                {where_sql}
                ORDER BY created_at DESC
                LIMIT ${idx} OFFSET ${}
            ",
            idx + 1
        ),
        values,
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: if total == 0 {
            0
        } else {
            total / per_page + i64::from(total % per_page != 0)
        },
    }))
}

pub async fn create_project_check_result(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateCheckResultRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (project, actor_id, actor_kind) = ensure_project_actor(&state, claims, bot, project_id).await?;
    let action_class = normalize_action_class(req.action_class.as_deref().unwrap_or("high_risk_mutation"))?;
    let risk_level = normalize_risk_level(req.risk_level.as_deref().unwrap_or("high"))?;
    let title = normalize_title(&req.title)?;
    let summary = normalize_summary(&req.summary)?;
    let result = ensure_json_object(req.result.unwrap_or_else(|| json!({})), "result")?;
    let status = default_status_for(&action_class, &risk_level);
    let check_result_id = Uuid::new_v4();
    let now = Utc::now();

    if let Some(invocation_id) = req.invocation_id {
        ensure_invocation_in_project(&state, project_id, invocation_id).await?;
    }
    if let Some(connector_id) = req.connector_id {
        ensure_connector_in_project_scope(&state, project.workspace_id, project_id, connector_id).await?;
    }

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO check_results (
                    id, workspace_id, project_id, invocation_id, connector_id, action_class,
                    risk_level, title, summary, result, status, created_by, created_by_kind,
                    created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)
            ",
        vec![
            check_result_id.into(),
            project.workspace_id.into(),
            project_id.into(),
            req.invocation_id.into(),
            req.connector_id.into(),
            action_class.clone().into(),
            risk_level.clone().into(),
            title.clone().into(),
            summary.clone().into(),
            result.clone().into(),
            status.into(),
            actor_id.into(),
            actor_kind.to_string().into(),
            now.into(),
        ],
    ))
    .await?;

    insert_check_result_business_event(
        &tx,
        CheckResultEventInput {
            event_type: "check_result.created",
            workspace_id: project.workspace_id,
            project_id,
            check_result_id,
            invocation_id: req.invocation_id,
            connector_id: req.connector_id,
            action_class: &action_class,
            risk_level: &risk_level,
            title: &title,
            summary: &summary,
            result: &result,
            status,
            proposal_id: None,
            actor_id: Some(actor_id),
            actor_kind,
        },
    )
    .await?;

    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id,
            actor_id: Some(actor_id),
            action: "check_result.created".to_string(),
            resource_type: "check_result".to_string(),
            resource_id: Some(check_result_id.to_string()),
            old_value: None,
            new_value: Some(json!({
                "title": title,
                "action_class": action_class,
                "risk_level": risk_level,
                "status": status,
            })),
            metadata: Some(json!({
                "invocation_id": req.invocation_id,
                "connector_id": req.connector_id,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    let row = find_check_result(&state, check_result_id).await?;
    Ok(ApiResponse::success(row))
}

pub async fn get_check_result(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(check_result_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let row = find_check_result(&state, check_result_id).await?;
    ensure_project_access(&state, claims, bot, row.project_id).await?;
    Ok(ApiResponse::success(row))
}

pub async fn create_proposal_from_result(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(check_result_id): Path<Uuid>,
    Json(req): Json<CreateProposalFromResultRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let row = find_check_result(&state, check_result_id).await?;
    if row.proposal_id.is_some() {
        return Err(ApiError::Conflict("check result already has a proposal".to_string()));
    }

    let (_, actor_id, actor_kind) = ensure_project_actor(&state, claims, bot, row.project_id).await?;
    let proposal_type = normalize_proposal_type(
        req.proposal_type
            .as_deref()
            .unwrap_or_else(|| default_proposal_type_for(&row.action_class)),
    )?;
    let title = normalize_title(req.title.as_deref().unwrap_or(row.title.as_str()))?;
    let content = normalize_proposal_content(req.content.unwrap_or_else(|| default_proposal_content(&row)))?;
    let domains = normalize_domains(req.domains.unwrap_or_else(|| {
        vec![
            "governance".to_string(),
            row.action_class.clone(),
            row.risk_level.clone(),
        ]
    }))?;
    let voting_rule = normalize_voting_rule(req.voting_rule.as_deref().unwrap_or("simple_majority"))?;
    let cycle_template = normalize_cycle_template(req.cycle_template.as_deref().unwrap_or_else(|| {
        if row.risk_level == "critical" {
            "critical"
        } else {
            "standard"
        }
    }))?;
    let proposal_id = format!("PROP-{}", Uuid::new_v4());
    let now = Utc::now();
    let submit = req.submit.unwrap_or(false);
    let status = if submit { "open" } else { "draft" };
    let submitted_at = submit.then_some(now);

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO proposals (
                id, title, proposal_type, status, author_id, author_type, content,
                domains, voting_rule, cycle_template, created_at, submitted_at
            )
            VALUES ($1, $2, $3::proposal_type, $4::proposal_status, $5, $6::author_type,
                    $7, $8, $9::voting_rule, $10::cycle_template, $11, $12)
        ",
        vec![
            proposal_id.clone().into(),
            title.clone().into(),
            proposal_type.clone().into(),
            status.into(),
            actor_id.to_string().into(),
            actor_kind.into(),
            content.clone().into(),
            json!(domains).into(),
            voting_rule.clone().into(),
            cycle_template.clone().into(),
            now.into(),
            submitted_at.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE check_results
            SET status = 'proposed', proposal_id = $2, updated_at = $3
            WHERE id = $1
        ",
        vec![check_result_id.into(), proposal_id.clone().into(), now.into()],
    ))
    .await?;

    insert_proposal_from_check_result_business_event(
        &tx,
        ProposalFromCheckResultEventInput {
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            proposal_id: &proposal_id,
            check_result_id,
            proposal_type: &proposal_type,
            proposal_status: status,
            voting_rule: &voting_rule,
            cycle_template: &cycle_template,
            domains: &domains,
            action_class: &row.action_class,
            risk_level: &row.risk_level,
            actor_id: Some(actor_id),
            actor_kind,
        },
    )
    .await?;

    insert_check_result_business_event(
        &tx,
        CheckResultEventInput {
            event_type: "check_result.proposed",
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            check_result_id,
            invocation_id: row.invocation_id,
            connector_id: row.connector_id,
            action_class: &row.action_class,
            risk_level: &row.risk_level,
            title: &row.title,
            summary: &row.summary,
            result: &row.result,
            status: "proposed",
            proposal_id: Some(proposal_id.as_str()),
            actor_id: Some(actor_id),
            actor_kind,
        },
    )
    .await?;

    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id: row.project_id,
            actor_id: Some(actor_id),
            action: "proposal.created_from_check_result".to_string(),
            resource_type: "proposal".to_string(),
            resource_id: Some(proposal_id.clone()),
            old_value: None,
            new_value: Some(json!({
                "proposal_id": proposal_id,
                "status": status,
                "proposal_type": proposal_type,
                "voting_rule": voting_rule,
                "cycle_template": cycle_template,
            })),
            metadata: Some(json!({
                "check_result_id": check_result_id,
                "action_class": row.action_class,
                "risk_level": row.risk_level,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(json!({
        "id": proposal_id,
        "status": status,
        "check_result_id": check_result_id,
        "created_at": now.to_rfc3339(),
        "submitted_at": submitted_at.map(|value| value.to_rfc3339()),
    })))
}

async fn ensure_project_access(
    state: &AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    project_id: Uuid,
) -> Result<ProjectWorkspaceRow, ApiError> {
    let project = find_project_workspace(state, project_id).await?;
    let extensions = build_auth_extensions(claims, bot);
    require_workspace_access(state, &extensions, project.workspace_id).await?;
    Ok(project)
}

async fn ensure_project_actor(
    state: &AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    project_id: Uuid,
) -> Result<(ProjectWorkspaceRow, Uuid, &'static str), ApiError> {
    let project = find_project_workspace(state, project_id).await?;
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, _, is_bot) = require_workspace_access(state, &extensions, project.workspace_id).await?;
    Ok((project, actor_id, if is_bot { "ai" } else { "human" }))
}

async fn find_project_workspace(state: &AppState, project_id: Uuid) -> Result<ProjectWorkspaceRow, ApiError> {
    ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

async fn find_check_result(state: &AppState, id: Uuid) -> Result<CheckResultRow, ApiError> {
    CheckResultRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, invocation_id, connector_id,
                   action_class, risk_level, title, summary, result, status,
                   proposal_id, created_by, created_by_kind, created_at, updated_at
            FROM check_results
            WHERE id = $1
        ",
        vec![id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("check result not found".to_string()))
}

async fn insert_check_result_business_event<C>(db: &C, input: CheckResultEventInput<'_>) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: input.workspace_id,
            project_id: Some(input.project_id),
            event_type: input.event_type.to_string(),
            aggregate_type: "check_result".to_string(),
            aggregate_id: input.check_result_id.to_string(),
            actor_id: input.actor_id,
            source: json!({
                "type": input.actor_kind,
                "actor_id": input.actor_id
            }),
            payload: json!({
                "check_result_id": input.check_result_id,
                "workspace_id": input.workspace_id,
                "project_id": input.project_id,
                "invocation_id": input.invocation_id,
                "connector_id": input.connector_id,
                "action_class": input.action_class,
                "risk_level": input.risk_level,
                "title": input.title,
                "summary": input.summary,
                "result": input.result,
                "status": input.status,
                "proposal_id": input.proposal_id
            }),
            metadata: json!({
                "check_result_id": input.check_result_id,
                "action_class": input.action_class,
                "risk_level": input.risk_level,
                "actor_kind": input.actor_kind,
                "proposal_id": input.proposal_id
            }),
            correlation_id: None,
            causation_id: input.invocation_id,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn insert_proposal_from_check_result_business_event<C>(
    db: &C,
    input: ProposalFromCheckResultEventInput<'_>,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: input.workspace_id,
            project_id: Some(input.project_id),
            event_type: "proposal.created_from_check_result".to_string(),
            aggregate_type: "proposal".to_string(),
            aggregate_id: input.proposal_id.to_string(),
            actor_id: input.actor_id,
            source: json!({
                "type": input.actor_kind,
                "actor_id": input.actor_id
            }),
            payload: json!({
                "proposal_id": input.proposal_id,
                "check_result_id": input.check_result_id,
                "workspace_id": input.workspace_id,
                "project_id": input.project_id,
                "proposal_type": input.proposal_type,
                "status": input.proposal_status,
                "voting_rule": input.voting_rule,
                "cycle_template": input.cycle_template,
                "domains": input.domains,
                "action_class": input.action_class,
                "risk_level": input.risk_level
            }),
            metadata: json!({
                "proposal_id": input.proposal_id,
                "check_result_id": input.check_result_id,
                "source": "check_result",
                "actor_kind": input.actor_kind
            }),
            correlation_id: None,
            causation_id: Some(input.check_result_id),
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn ensure_invocation_in_project(state: &AppState, project_id: Uuid, invocation_id: Uuid) -> Result<(), ApiError> {
    let ok = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM agent_invocations WHERE id = $1 AND project_id = $2",
            vec![invocation_id.into(), project_id.into()],
        ))
        .await?
        .is_some();
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "invocation does not belong to this project".to_string(),
        ))
    }
}

async fn ensure_connector_in_project_scope(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    connector_id: Uuid,
) -> Result<(), ApiError> {
    let ok = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT 1 FROM connectors
                WHERE id = $1
                  AND workspace_id = $2
                  AND (project_id IS NULL OR project_id = $3)
            ",
            vec![connector_id.into(), workspace_id.into(), project_id.into()],
        ))
        .await?
        .is_some();
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "connector does not belong to this project".to_string(),
        ))
    }
}

fn normalize_action_class(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "read_only"
            | "comment_result"
            | "low_risk_mutation"
            | "high_risk_mutation"
            | "external_side_effect"
            | "financial_legal_compliance"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid action_class".to_string()))
    }
}

fn normalize_risk_level(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "low" | "medium" | "high" | "critical") {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid risk_level".to_string()))
    }
}

fn normalize_status(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "recorded" | "requires_proposal" | "proposed" | "dismissed" | "applied"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid status".to_string()))
    }
}

fn normalize_title(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_string();
    if normalized.len() < 10 || normalized.len() > 200 {
        return Err(ApiError::BadRequest(
            "title must be between 10 and 200 characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_summary(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_string();
    if normalized.len() < 10 {
        return Err(ApiError::BadRequest(
            "summary must be at least 10 characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_proposal_content(value: String) -> Result<String, ApiError> {
    let normalized = value.trim().to_string();
    if normalized.len() < 50 {
        return Err(ApiError::BadRequest(
            "content must be at least 50 characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_proposal_type(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "feature" | "architecture" | "priority" | "resource" | "governance" | "bugfix"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid proposal_type".to_string()))
    }
}

fn normalize_voting_rule(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "simple_majority" | "absolute_majority" | "consensus"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid voting_rule".to_string()))
    }
}

fn normalize_cycle_template(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "rapid" | "fast" | "standard" | "critical") {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid cycle_template".to_string()))
    }
}

fn normalize_domains(domains: Vec<String>) -> Result<Vec<String>, ApiError> {
    let items = domains
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if items.is_empty() {
        return Err(ApiError::BadRequest("at least one domain is required".to_string()));
    }
    Ok(items)
}

fn ensure_json_object(value: Value, field: &str) -> Result<Value, ApiError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{field} must be a JSON object")))
    }
}

fn default_status_for(action_class: &str, risk_level: &str) -> &'static str {
    if matches!(
        action_class,
        "high_risk_mutation" | "external_side_effect" | "financial_legal_compliance"
    ) || matches!(risk_level, "high" | "critical")
    {
        "requires_proposal"
    } else {
        "recorded"
    }
}

fn check_result_status_for_decision(decision_result: &str) -> Option<&'static str> {
    match decision_result {
        "approved" => Some("applied"),
        "rejected" | "vetoed" => Some("dismissed"),
        _ => None,
    }
}

pub async fn sync_check_result_after_proposal_decision<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
    decision_result: &str,
    decided_at: chrono::DateTime<Utc>,
) -> Result<(), ApiError> {
    let Some(status) = check_result_status_for_decision(decision_result) else {
        return Ok(());
    };

    if decision_result == "approved" {
        return apply_approved_check_results(db, proposal_id, decided_at).await;
    }

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH updated AS (
                UPDATE check_results
                SET status = $2,
                    updated_at = $3
                WHERE proposal_id = $1
                  AND status = 'proposed'
                RETURNING project_id, id
            )
            INSERT INTO governance_audit_logs (
                project_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                old_value,
                new_value,
                metadata,
                created_at
            )
            SELECT
                project_id,
                NULL,
                'check_result.proposal_decision_synced',
                'check_result',
                id::text,
                jsonb_build_object('status', 'proposed'),
                jsonb_build_object('status', $2),
                jsonb_build_object('proposal_id', $1, 'decision_result', $4),
                $3
            FROM updated
        ",
        vec![
            proposal_id.to_string().into(),
            status.into(),
            decided_at.into(),
            decision_result.to_string().into(),
        ],
    ))
    .await?;

    Ok(())
}

async fn apply_approved_check_results<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
    decided_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    let rows = CheckResultApplyRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, project_id, result, created_by
            FROM check_results
            WHERE proposal_id = $1
              AND status = 'proposed'
            ORDER BY created_at ASC
        ",
        vec![proposal_id.to_string().into()],
    ))
    .all(db)
    .await?;

    for row in rows {
        let effects = approved_effects_from_result(&row.result)?;
        let mut applied = Vec::new();
        for effect in effects {
            let applied_effect = apply_approved_effect(db, &row, &effect, decided_at).await?;
            applied.push(applied_effect);
        }

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE check_results
                SET status = 'applied',
                    updated_at = $2
                WHERE id = $1
                  AND status = 'proposed'
            ",
            vec![row.id.into(), decided_at.into()],
        ))
        .await?;

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO governance_audit_logs (
                    project_id,
                    actor_id,
                    action,
                    resource_type,
                    resource_id,
                    old_value,
                    new_value,
                    metadata,
                    created_at
                )
                VALUES (
                    $1, NULL, 'check_result.approved_effects_applied', 'check_result', $2,
                    jsonb_build_object('status', 'proposed'),
                    jsonb_build_object('status', 'applied'),
                    $3,
                    $4
                )
            ",
            vec![
                row.project_id.into(),
                row.id.to_string().into(),
                json!({
                    "proposal_id": proposal_id,
                    "applied_effects": applied,
                    "applied_effect_count": applied.len(),
                })
                .into(),
                decided_at.into(),
            ],
        ))
        .await?;
    }

    Ok(())
}

async fn apply_approved_effect<C: ConnectionTrait>(
    db: &C,
    row: &CheckResultApplyRow,
    effect: &ApprovedEffect,
    applied_at: DateTime<Utc>,
) -> Result<Value, ApiError> {
    match effect.tool.as_str() {
        "comments.create" => apply_comment_create_effect(db, row, &effect.arguments, applied_at).await,
        "work_items.update" => apply_work_item_update_effect(db, row, &effect.arguments, applied_at).await,
        other => Err(ApiError::BadRequest(format!(
            "unsupported approved check-result effect: {other}"
        ))),
    }
}

async fn apply_comment_create_effect<C: ConnectionTrait>(
    db: &C,
    row: &CheckResultApplyRow,
    args: &Value,
    applied_at: DateTime<Utc>,
) -> Result<Value, ApiError> {
    let work_item_id = required_uuid_arg(args, "work_item_id")?;
    let content = required_string_arg(args, "content")?;
    let comment_id = Uuid::new_v4();

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO comments (id, work_item_id, author_id, body, created_at, updated_at)
                SELECT $1, wi.id, $2, $3, $4, $4
                FROM work_items wi
                WHERE wi.id = $5
                  AND wi.project_id = $6
            ",
            vec![
                comment_id.into(),
                row.created_by.into(),
                content.clone().into(),
                applied_at.into(),
                work_item_id.into(),
                row.project_id.into(),
            ],
        ))
        .await?;

    if result.rows_affected() != 1 {
        return Err(ApiError::BadRequest(
            "comments.create approved effect targets a work item outside the check-result project".to_string(),
        ));
    }

    Ok(json!({
        "tool": "comments.create",
        "comment_id": comment_id,
        "work_item_id": work_item_id,
    }))
}

async fn apply_work_item_update_effect<C: ConnectionTrait>(
    db: &C,
    row: &CheckResultApplyRow,
    args: &Value,
    applied_at: DateTime<Utc>,
) -> Result<Value, ApiError> {
    let work_item_id = required_uuid_arg(args, "work_item_id")?;
    let mut sets = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx = 1;
    let mut changed_fields = Vec::new();

    if let Some(title) = optional_string_arg(args, "title")? {
        sets.push(format!("title = ${idx}"));
        values.push(title.into());
        changed_fields.push("title");
        idx += 1;
    }
    if let Some(description) = optional_string_arg(args, "description")? {
        sets.push(format!("description = ${idx}"));
        values.push(description.into());
        changed_fields.push("description");
        idx += 1;
    }
    if let Some(state) = optional_string_arg(args, "state")? {
        sets.push(format!("state = ${idx}"));
        values.push(state.into());
        changed_fields.push("state");
        idx += 1;
    }
    if let Some(priority) = optional_string_arg(args, "priority")? {
        validate_priority_arg(&priority)?;
        sets.push(format!("priority = ${idx}"));
        values.push(priority.into());
        changed_fields.push("priority");
        idx += 1;
    }
    if args.get("assignee_id").is_some() {
        let assignee_id = optional_uuid_arg(args, "assignee_id")?;
        sets.push(format!("assignee_id = ${idx}"));
        values.push(assignee_id.into());
        changed_fields.push("assignee_id");
        idx += 1;
    }
    if args.get("due_at").is_some() {
        let due_at = optional_datetime_arg(args, "due_at")?;
        sets.push(format!("due_at = ${idx}"));
        values.push(due_at.into());
        changed_fields.push("due_at");
        idx += 1;
    }

    if sets.is_empty() {
        return Err(ApiError::BadRequest(
            "work_items.update approved effect needs at least one field to update".to_string(),
        ));
    }

    sets.push(format!("updated_at = ${idx}"));
    values.push(applied_at.into());
    idx += 1;
    values.push(work_item_id.into());
    let work_item_idx = idx;
    idx += 1;
    values.push(row.project_id.into());
    let project_idx = idx;

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "UPDATE work_items SET {} WHERE id = ${work_item_idx} AND project_id = ${project_idx}",
                sets.join(", ")
            ),
            values,
        ))
        .await?;

    if result.rows_affected() != 1 {
        return Err(ApiError::BadRequest(
            "work_items.update approved effect targets a work item outside the check-result project".to_string(),
        ));
    }

    Ok(json!({
        "tool": "work_items.update",
        "work_item_id": work_item_id,
        "changed_fields": changed_fields,
    }))
}

fn approved_effects_from_result(result: &Value) -> Result<Vec<ApprovedEffect>, ApiError> {
    let Some(value) = result
        .get("approved_effects")
        .or_else(|| result.get("approved_effect"))
        .or_else(|| result.get("side_effects"))
        .or_else(|| result.get("side_effect"))
        .or_else(|| result.get("apply"))
    else {
        return Ok(Vec::new());
    };

    let items = value
        .as_array()
        .map_or_else(|| vec![value.clone()], std::clone::Clone::clone);

    items.into_iter().map(parse_approved_effect).collect()
}

fn parse_approved_effect(value: Value) -> Result<ApprovedEffect, ApiError> {
    let tool = value
        .get("tool")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .ok_or_else(|| ApiError::BadRequest("approved effect tool is required".to_string()))?
        .to_string();
    let arguments = value
        .get("arguments")
        .or_else(|| value.get("args"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(ApiError::BadRequest(
            "approved effect arguments must be a JSON object".to_string(),
        ));
    }
    Ok(ApprovedEffect { tool, arguments })
}

fn required_string_arg(args: &Value, key: &str) -> Result<String, ApiError> {
    optional_string_arg(args, key)?.ok_or_else(|| ApiError::BadRequest(format!("{key} is required")))
}

fn optional_string_arg(args: &Value, key: &str) -> Result<Option<String>, ApiError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_str().map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(ApiError::BadRequest(format!("{key} must be a non-empty string")));
    };
    Ok(Some(value.to_string()))
}

fn required_uuid_arg(args: &Value, key: &str) -> Result<Uuid, ApiError> {
    optional_uuid_arg(args, key)?.ok_or_else(|| ApiError::BadRequest(format!("{key} is required")))
}

fn optional_uuid_arg(args: &Value, key: &str) -> Result<Option<Uuid>, ApiError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(raw) = value.as_str() else {
        return Err(ApiError::BadRequest(format!("{key} must be a UUID string or null")));
    };
    Uuid::parse_str(raw)
        .map(Some)
        .map_err(|_| ApiError::BadRequest(format!("{key} must be a valid UUID")))
}

fn optional_datetime_arg(args: &Value, key: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(raw) = value.as_str() else {
        return Err(ApiError::BadRequest(format!(
            "{key} must be an RFC3339 datetime string or null"
        )));
    };
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|_| ApiError::BadRequest(format!("{key} must be a valid RFC3339 datetime")))
}

fn validate_priority_arg(value: &str) -> Result<(), ApiError> {
    if matches!(value, "none" | "low" | "medium" | "high" | "urgent") {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "priority must be none, low, medium, high, or urgent".to_string(),
        ))
    }
}

fn default_proposal_type_for(action_class: &str) -> &'static str {
    match action_class {
        "high_risk_mutation" => "architecture",
        "low_risk_mutation" => "feature",
        _ => "governance",
    }
}

fn default_proposal_content(row: &CheckResultRow) -> String {
    format!(
        "AI/MCP check result requires governance review.\n\nCheck result: {}\nAction class: {}\nRisk level: {}\n\nSummary:\n{}\n\nResult payload:\n```json\n{}\n```",
        row.id, row.action_class, row.risk_level, row.summary, row.result
    )
}

#[cfg(test)]
mod tests {
    use super::{
        approved_effects_from_result, check_result_status_for_decision, default_status_for, normalize_action_class,
        normalize_domains, normalize_risk_level, validate_priority_arg,
    };
    use serde_json::json;

    #[test]
    fn high_risk_check_results_require_proposal() {
        assert_eq!(default_status_for("high_risk_mutation", "medium"), "requires_proposal");
        assert_eq!(default_status_for("comment_result", "critical"), "requires_proposal");
        assert_eq!(default_status_for("comment_result", "low"), "recorded");
    }

    #[test]
    fn validates_action_class_and_risk_level() {
        assert!(normalize_action_class("external_side_effect").is_ok());
        assert!(normalize_action_class("unknown").is_err());
        assert!(normalize_risk_level("critical").is_ok());
        assert!(normalize_risk_level("unknown").is_err());
    }

    #[test]
    fn domains_are_trimmed_lowercased_and_deduplicated() {
        let domains = normalize_domains(vec![
            " Governance ".to_string(),
            "governance".to_string(),
            " Risk ".to_string(),
        ])
        .unwrap();
        assert_eq!(domains, vec!["governance".to_string(), "risk".to_string()]);
    }

    #[test]
    fn rejected_decisions_dismiss_linked_check_results_without_apply() {
        assert_eq!(check_result_status_for_decision("rejected"), Some("dismissed"));
        assert_eq!(check_result_status_for_decision("vetoed"), Some("dismissed"));
    }

    #[test]
    fn approved_decisions_mark_linked_check_results_applied() {
        assert_eq!(check_result_status_for_decision("approved"), Some("applied"));
    }

    #[test]
    fn approved_effects_parse_single_and_array_shapes() {
        let single = approved_effects_from_result(&json!({
            "approved_effect": {
                "tool": "comments.create",
                "arguments": {
                    "work_item_id": "00000000-0000-0000-0000-000000000001",
                    "content": "approved"
                }
            }
        }))
        .unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(
            single.first().map(|effect| effect.tool.as_str()),
            Some("comments.create")
        );

        let many = approved_effects_from_result(&json!({
            "side_effects": [
                {
                    "name": "comments.create",
                    "args": {
                        "work_item_id": "00000000-0000-0000-0000-000000000001",
                        "content": "approved"
                    }
                },
                {
                    "tool": "work_items.update",
                    "arguments": {
                        "work_item_id": "00000000-0000-0000-0000-000000000001",
                        "state": "done"
                    }
                }
            ]
        }))
        .unwrap();
        assert_eq!(many.len(), 2);
        assert_eq!(
            many.get(1).map(|effect| effect.tool.as_str()),
            Some("work_items.update")
        );
    }

    #[test]
    fn approved_effects_reject_missing_tool_or_non_object_arguments() {
        assert!(approved_effects_from_result(&json!({ "approved_effect": { "arguments": {} } })).is_err());
        assert!(
            approved_effects_from_result(&json!({
                "approved_effect": {
                    "tool": "comments.create",
                    "arguments": []
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn approved_work_item_priority_is_allowlisted() {
        assert!(validate_priority_arg("urgent").is_ok());
        assert!(validate_priority_arg("blocker").is_err());
    }
}
