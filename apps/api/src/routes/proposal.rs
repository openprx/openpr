use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{self, MissedTickBehavior};
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::ai_task_service::queue_vote_requested_tasks_for_project,
    services::governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
    services::impact_review_service::ImpactReviewService,
    services::permission_service::PermissionService,
    services::trust_score_service::{
        TrustScoreService, is_project_member, normalize_domain_key, parse_domains,
        parse_participant_type,
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
pub struct CreateProposalRequest {
    pub title: Option<String>,
    pub proposal_type: Option<String>,
    pub content: Option<String>,
    pub domains: Option<Vec<String>>,
    pub voting_rule: Option<String>,
    pub cycle_template: Option<String>,
    pub template_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProposalRequest {
    pub title: Option<String>,
    pub proposal_type: Option<String>,
    pub content: Option<String>,
    pub domains: Option<Vec<String>>,
    pub voting_rule: Option<String>,
    pub cycle_template: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListProposalsQuery {
    pub status: Option<String>,
    pub proposal_type: Option<String>,
    pub domain: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub sort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VoteRequest {
    pub choice: String,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProposalCommentRequest {
    pub comment_type: Option<String>,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct LinkIssueRequest {
    pub issue_id: Uuid,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct ProposalRow {
    pub id: String,
    pub title: String,
    pub proposal_type: String,
    pub status: String,
    pub author_id: String,
    pub author_type: String,
    pub content: String,
    pub domains: Value,
    pub voting_rule: String,
    pub cycle_template: String,
    pub template_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub voting_started_at: Option<DateTime<Utc>>,
    pub voting_ended_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProposalCommentRow {
    pub id: i64,
    pub proposal_id: String,
    pub author_id: String,
    pub author_name: Option<String>,
    pub author_avatar: Option<String>,
    pub author_type: String,
    pub comment_type: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct VoteRow {
    pub id: i64,
    pub proposal_id: String,
    pub voter_id: String,
    pub voter_type: String,
    pub choice: String,
    pub weight: f64,
    pub reason: Option<String>,
    pub voted_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProposalIssueLinkRow {
    pub id: i64,
    pub proposal_id: String,
    pub issue_id: Uuid,
    pub issue_title: String,
    pub issue_state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ProposalListItem {
    pub id: String,
    pub title: String,
    pub proposal_type: String,
    pub status: String,
    pub author_id: String,
    pub author_type: String,
    pub voting_rule: String,
    pub cycle_template: String,
    pub template_id: Option<String>,
    pub domains: Value,
    pub created_at: String,
    pub submitted_at: Option<String>,
    pub voting_started_at: Option<String>,
    pub voting_ended_at: Option<String>,
    pub archived_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProposalDetailResponse {
    pub proposal: ProposalListItem,
    pub tally: VoteTally,
    pub decision_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoteTally {
    pub yes: i64,
    pub no: i64,
    pub abstain: i64,
}

#[derive(Debug, FromQueryResult)]
struct DecisionIdRow {
    id: String,
}

#[derive(Debug, FromQueryResult)]
struct ActorRow {
    role: String,
    entity_type: String,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, FromQueryResult)]
struct TallyRow {
    yes: Option<i64>,
    no: Option<i64>,
    abstain: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct WeightedTallyRow {
    yes: Option<i64>,
    no: Option<i64>,
    abstain: Option<i64>,
    weighted_yes: Option<f64>,
    weighted_no: Option<f64>,
}

#[derive(Debug, FromQueryResult)]
struct VoteForFinalizeRow {
    id: i64,
    voter_id: String,
    voter_type: String,
    choice: String,
    weight: f64,
}

#[derive(Debug, FromQueryResult)]
struct WeightRow {
    vote_weight: f64,
}

#[derive(Debug, FromQueryResult)]
struct ProposalTemplateForCreateRow {
    id: String,
    project_id: Uuid,
    template_type: String,
    content: Value,
    is_active: bool,
}

#[derive(Debug)]
struct ActorContext {
    user_id: Uuid,
    user_id_str: String,
    author_type: &'static str,
    is_admin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionResult {
    Approved,
    Rejected,
    Vetoed,
}

impl DecisionResult {
    fn as_db_value(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Vetoed => "vetoed",
        }
    }

    fn as_proposal_status(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Vetoed => "vetoed",
        }
    }
}

fn validate_proposal_type(value: &str) -> bool {
    matches!(
        value,
        "feature" | "architecture" | "priority" | "resource" | "governance" | "bugfix"
    )
}

fn validate_status(value: &str) -> bool {
    matches!(
        value,
        "draft" | "open" | "voting" | "approved" | "rejected" | "vetoed" | "archived"
    )
}

fn validate_voting_rule(value: &str) -> bool {
    matches!(value, "simple_majority" | "absolute_majority" | "consensus")
}

fn validate_cycle_template(value: &str) -> bool {
    matches!(value, "rapid" | "fast" | "standard" | "critical")
}

fn validate_vote_choice(value: &str) -> bool {
    matches!(value, "yes" | "no" | "abstain")
}

fn validate_comment_type(value: &str) -> bool {
    matches!(
        value,
        "support" | "concern" | "objection" | "amendment" | "general"
    )
}

fn default_cycle_template(proposal_type: &str) -> &'static str {
    match proposal_type {
        "feature" | "priority" | "bugfix" => "rapid",
        "architecture" | "resource" => "standard",
        "governance" => "critical",
        _ => "rapid",
    }
}

fn cycle_hours(cycle_template: &str) -> (i64, i64) {
    match cycle_template {
        "rapid" => (1, 1),
        "fast" => (24, 24),
        "standard" => (72, 48),
        "critical" => (168, 72),
        _ => (1, 1),
    }
}

fn format_dt(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn gen_prefixed_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn optional_dt(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(format_dt)
}

fn proposal_to_item(row: &ProposalRow) -> ProposalListItem {
    ProposalListItem {
        id: row.id.clone(),
        title: row.title.clone(),
        proposal_type: row.proposal_type.clone(),
        status: row.status.clone(),
        author_id: row.author_id.clone(),
        author_type: row.author_type.clone(),
        voting_rule: row.voting_rule.clone(),
        cycle_template: row.cycle_template.clone(),
        template_id: row.template_id.clone(),
        domains: row.domains.clone(),
        created_at: format_dt(row.created_at),
        submitted_at: optional_dt(row.submitted_at),
        voting_started_at: optional_dt(row.voting_started_at),
        voting_ended_at: optional_dt(row.voting_ended_at),
        archived_at: optional_dt(row.archived_at),
    }
}

fn take_template_string(content: &Value, key: &str) -> Option<String> {
    content
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}

fn take_template_domains(content: &Value) -> Option<Vec<String>> {
    content.get("domains").and_then(|value| {
        value.as_array().map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
    })
}

async fn build_actor_context(state: &AppState, claims: &JwtClaims) -> Result<ActorContext, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let actor = ActorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role, COALESCE(entity_type, 'human') AS entity_type FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("user not found".to_string()))?;

    let author_type = if actor.entity_type == "bot" || actor.entity_type == "ai" {
        "ai"
    } else {
        "human"
    };

    Ok(ActorContext {
        user_id,
        user_id_str: user_id.to_string(),
        author_type,
        is_admin: actor.role.trim().eq_ignore_ascii_case("admin"),
    })
}

async fn find_proposal(state: &AppState, proposal_id: &str) -> Result<ProposalRow, ApiError> {
    ProposalRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, title, proposal_type::text AS proposal_type, status::text AS status,
                   author_id, author_type::text AS author_type, content, domains,
                   voting_rule::text AS voting_rule, cycle_template::text AS cycle_template,
                   template_id,
                   created_at, submitted_at, voting_started_at, voting_ended_at, archived_at
            FROM proposals
            WHERE id = $1
        "#,
        vec![proposal_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))
}

async fn proposal_tally(state: &AppState, proposal_id: &str) -> Result<VoteTally, ApiError> {
    let tally = TallyRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
              SUM(CASE WHEN choice = 'yes' THEN 1 ELSE 0 END) AS yes,
              SUM(CASE WHEN choice = 'no' THEN 1 ELSE 0 END) AS no,
              SUM(CASE WHEN choice = 'abstain' THEN 1 ELSE 0 END) AS abstain
            FROM votes
            WHERE proposal_id = $1
        "#,
        vec![proposal_id.into()],
    ))
    .one(&state.db)
    .await?
    .unwrap_or(TallyRow {
        yes: Some(0),
        no: Some(0),
        abstain: Some(0),
    });

    Ok(VoteTally {
        yes: tally.yes.unwrap_or(0),
        no: tally.no.unwrap_or(0),
        abstain: tally.abstain.unwrap_or(0),
    })
}

fn calculate_result(yes: f64, no: f64, rule: &str) -> DecisionResult {
    let total = yes + no;
    if total <= 0.0 {
        return DecisionResult::Rejected;
    }

    match rule {
        "simple_majority" => {
            if yes > no {
                DecisionResult::Approved
            } else {
                DecisionResult::Rejected
            }
        }
        "absolute_majority" => {
            if yes / total >= 0.67 {
                DecisionResult::Approved
            } else {
                DecisionResult::Rejected
            }
        }
        "consensus" => {
            if yes / total >= 0.80 {
                DecisionResult::Approved
            } else {
                DecisionResult::Rejected
            }
        }
        _ => DecisionResult::Rejected,
    }
}

async fn ensure_voting_finalized_if_needed(state: &AppState, proposal: &ProposalRow) -> Result<(), ApiError> {
    if proposal.status != "voting" {
        return Ok(());
    }

    let Some(voting_started_at) = proposal.voting_started_at else {
        return Ok(());
    };

    let (_, voting_hours) = cycle_hours(&proposal.cycle_template);
    if Utc::now() < voting_started_at + Duration::hours(voting_hours) {
        return Ok(());
    }

    let decision_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM decisions WHERE proposal_id = $1",
            vec![proposal.id.clone().into()],
        ))
        .await?
        .is_some();

    if decision_exists {
        return Ok(());
    }

    finalize_voting(state, proposal).await
}

async fn finalize_voting(state: &AppState, proposal: &ProposalRow) -> Result<(), ApiError> {
    let tx = state.db.begin().await?;
    let tally = recalculate_and_tally_votes_with_conn(&tx, proposal).await?;
    let yes = tally.yes.unwrap_or(0);
    let no = tally.no.unwrap_or(0);
    let abstain = tally.abstain.unwrap_or(0);
    let weighted_yes = tally.weighted_yes.unwrap_or(0.0);
    let weighted_no = tally.weighted_no.unwrap_or(0.0);
    let total = yes + no + abstain;
    let result = calculate_result(weighted_yes, weighted_no, &proposal.voting_rule);

    let approval_rate = if yes + no > 0 {
        Some((yes as f64) / ((yes + no) as f64))
    } else {
        None
    };

    let weighted_approval_rate = if weighted_yes + weighted_no > 0.0 {
        Some(weighted_yes / (weighted_yes + weighted_no))
    } else {
        None
    };

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE proposals
            SET status = $2::proposal_status,
                voting_ended_at = $3
            WHERE id = $1
        "#,
        vec![
            proposal.id.clone().into(),
            result.as_proposal_status().into(),
            Utc::now().into(),
        ],
    ))
    .await?;

    let decision_id = gen_prefixed_id("DEC");
    let decision_insert = tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO decisions (
                id, proposal_id, result, approval_rate, total_votes, yes_votes, no_votes, abstain_votes,
                weighted_yes, weighted_no, weighted_approval_rate, is_weighted, decided_at
            ) VALUES ($1, $2, $3::decision_result, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
        vec![
            decision_id.into(),
            proposal.id.clone().into(),
            result.as_db_value().into(),
            approval_rate.into(),
            (total as i32).into(),
            (yes as i32).into(),
            (no as i32).into(),
            (abstain as i32).into(),
            Some(weighted_yes).into(),
            Some(weighted_no).into(),
            weighted_approval_rate.into(),
            true.into(),
            Utc::now().into(),
        ],
    ))
    .await;

    if let Err(err) = decision_insert {
        let message = err.to_string();
        if message.contains("decisions_proposal_id_key") || message.contains("duplicate key value") {
            let _ = tx.rollback().await;
            tracing::warn!(
                proposal_id = proposal.id,
                "skip finalize: decision already exists"
            );
            return Ok(());
        }
        return Err(ApiError::Database(err));
    }

    apply_trust_score_after_finalize_with_conn(state, &tx, proposal, result).await?;
    if matches!(result, DecisionResult::Approved) {
        let review_svc = ImpactReviewService::new(state.db.clone());
        review_svc
            .schedule_review_with_conn(&tx, &proposal.id, true)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct VotingTransitionRow {
    id: String,
    author_id: String,
}

#[derive(Debug, FromQueryResult)]
struct ProposalProjectRow {
    project_id: Uuid,
}

pub fn start_governance_watcher(state: AppState) {
    tokio::spawn(async move {
        let mut interval = time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            if let Err(err) = governance_watcher_tick(&state).await {
                tracing::error!(error = %err, "governance watcher tick failed");
            }
        }
    });
}

async fn governance_watcher_tick(state: &AppState) -> Result<(), ApiError> {
    let now = Utc::now();

    let open_to_voting = VotingTransitionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE proposals
            SET status = 'voting',
                voting_started_at = COALESCE(voting_started_at, $1)
            WHERE status = 'open'
              AND submitted_at IS NOT NULL
              AND (
                    CASE cycle_template::text
                        WHEN 'rapid' THEN submitted_at + INTERVAL '1 hours'
                        WHEN 'fast' THEN submitted_at + INTERVAL '24 hours'
                        WHEN 'standard' THEN submitted_at + INTERVAL '72 hours'
                        WHEN 'critical' THEN submitted_at + INTERVAL '168 hours'
                        ELSE submitted_at + INTERVAL '1 hours'
                    END
                  ) <= $1
            RETURNING id, author_id
        "#,
        vec![now.into()],
    ))
    .all(&state.db)
    .await?;

    if !open_to_voting.is_empty() {
        tracing::info!(count = open_to_voting.len(), "governance watcher moved proposals to voting");
        for moved in &open_to_voting {
            if let Some(project_id) =
                resolve_project_id_for_proposal(state, &moved.id, &moved.author_id).await?
            {
                let _ = queue_vote_requested_tasks_for_project(
                    &state.db,
                    project_id,
                    &moved.id,
                    json!({
                        "proposal_id": moved.id,
                        "project_id": project_id.to_string(),
                        "trigger": "proposal.auto_voting_transition",
                    }),
                )
                .await?;
            }
        }
    }

    let expired_voting = ProposalRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, title, proposal_type::text AS proposal_type, status::text AS status,
                   author_id, author_type::text AS author_type, content, domains,
                   voting_rule::text AS voting_rule, cycle_template::text AS cycle_template,
                   created_at, submitted_at, voting_started_at, voting_ended_at, archived_at
            FROM proposals p
            WHERE p.status = 'voting'
              AND p.voting_started_at IS NOT NULL
              AND (
                    CASE p.cycle_template::text
                        WHEN 'rapid' THEN p.voting_started_at + INTERVAL '1 hours'
                        WHEN 'fast' THEN p.voting_started_at + INTERVAL '24 hours'
                        WHEN 'standard' THEN p.voting_started_at + INTERVAL '48 hours'
                        WHEN 'critical' THEN p.voting_started_at + INTERVAL '72 hours'
                        ELSE p.voting_started_at + INTERVAL '1 hours'
                    END
                  ) <= $1
              AND NOT EXISTS (
                    SELECT 1 FROM decisions d WHERE d.proposal_id = p.id
                  )
        "#,
        vec![now.into()],
    ))
    .all(&state.db)
    .await?;

    for proposal in expired_voting {
        finalize_voting(state, &proposal).await?;
    }

    Ok(())
}

async fn resolve_project_id_for_proposal(
    state: &AppState,
    proposal_id: &str,
    author_id: &str,
) -> Result<Option<Uuid>, ApiError> {
    resolve_project_id_for_proposal_with_conn(&state.db, proposal_id, author_id).await
}

async fn resolve_workspace_id_for_project(
    state: &AppState,
    project_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct WorkspaceRow {
        workspace_id: Uuid,
    }

    let row = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?;

    Ok(row.map(|v| v.workspace_id))
}

async fn resolve_project_id_for_proposal_with_conn<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
    author_id: &str,
) -> Result<Option<Uuid>, ApiError> {
    let linked_projects = ProposalProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT DISTINCT wi.project_id
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON wi.id = pil.issue_id
            WHERE pil.proposal_id = $1
        "#,
        vec![proposal_id.to_string().into()],
    ))
    .all(db)
    .await?;

    if linked_projects.len() == 1 {
        return Ok(Some(linked_projects[0].project_id));
    }
    if linked_projects.len() > 1 {
        tracing::warn!(
            proposal_id = proposal_id,
            count = linked_projects.len(),
            "skip trust score update: proposal links multiple projects"
        );
        return Ok(None);
    }

    let fallback = ProposalProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.id AS project_id
            FROM projects p
            INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id
            WHERE wm.user_id = $1::uuid
            ORDER BY p.created_at DESC
            LIMIT 2
        "#,
        vec![author_id.to_string().into()],
    ))
    .all(db)
    .await;

    match fallback {
        Ok(rows) if rows.len() == 1 => Ok(Some(rows[0].project_id)),
        Ok(rows) if rows.len() > 1 => {
            tracing::warn!(
                proposal_id = proposal_id,
                author_id = author_id,
                "skip trust score update: author belongs to multiple projects"
            );
            Ok(None)
        }
        Ok(_) => Ok(None),
        Err(err) => {
            tracing::warn!(
                proposal_id = proposal_id,
                author_id = author_id,
                error = %err,
                "skip trust score update: failed to resolve project"
            );
            Ok(None)
        }
    }
}

async fn resolve_vote_weight_for_proposal(
    state: &AppState,
    proposal_id: &str,
    voter_id: &str,
    voter_type: &str,
) -> Result<f64, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ProposalAuthorRow {
        author_id: String,
    }

    let Ok(voter_uuid) = Uuid::parse_str(voter_id) else {
        return Ok(1.0);
    };

    let author = ProposalAuthorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT author_id FROM proposals WHERE id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .one(&state.db)
    .await?;

    let Some(author) = author else {
        return Ok(1.0);
    };

    let project_id = match resolve_project_id_for_proposal(state, proposal_id, &author.author_id).await? {
        Some(id) => id,
        None => return Ok(1.0),
    };
    let participant = parse_participant_type(voter_type);
    let participant_str = match participant {
        ParticipantType::Ai => "ai",
        ParticipantType::Human => "human",
    };

    let proposal_domains = parse_domains(&find_proposal(state, proposal_id).await?.domains);
    resolve_vote_weight_from_trust_scores(
        &state.db,
        voter_uuid,
        participant_str,
        project_id,
        &proposal_domains,
    )
    .await
}

fn normalize_proposal_domains(domains: &[String]) -> Vec<String> {
    let mut unique = std::collections::HashSet::new();
    let mut out = Vec::new();
    for domain in domains {
        let normalized = normalize_domain_key(domain);
        if normalized.is_empty() || normalized == "global" {
            continue;
        }
        if unique.insert(normalized.clone()) {
            out.push(normalized);
        }
    }
    out
}

fn primary_domain_for_proposal(proposal: &ProposalRow) -> String {
    parse_domains(&proposal.domains)
        .into_iter()
        .map(|d| normalize_domain_key(&d))
        .find(|d| !d.is_empty() && d != "global")
        .unwrap_or_else(|| "global".to_string())
}

async fn resolve_vote_weight_from_trust_scores<C: ConnectionTrait>(
    db: &C,
    voter_uuid: Uuid,
    participant_str: &str,
    project_id: Uuid,
    proposal_domains: &[String],
) -> Result<f64, ApiError> {
    let domains = normalize_proposal_domains(proposal_domains);
    if domains.is_empty() {
        let row = WeightRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT vote_weight
                FROM trust_scores
                WHERE user_id = $1
                  AND user_type = $2::participant_type
                  AND project_id = $3
                  AND domain = 'global'
                LIMIT 1
            "#,
            vec![voter_uuid.into(), participant_str.into(), project_id.into()],
        ))
        .one(db)
        .await?;
        return Ok(row.map(|item| item.vote_weight.clamp(0.5, 2.0)).unwrap_or(1.0));
    }

    let mut values: Vec<sea_orm::Value> = vec![
        voter_uuid.into(),
        participant_str.to_string().into(),
        project_id.into(),
    ];
    let mut domain_placeholders = Vec::with_capacity(domains.len());
    let mut index = 4;
    for domain in domains {
        domain_placeholders.push(format!("${index}"));
        values.push(domain.into());
        index += 1;
    }

    let sql = format!(
        r#"
            SELECT MAX(vote_weight) AS vote_weight
            FROM trust_scores
            WHERE user_id = $1
              AND user_type = $2::participant_type
              AND project_id = $3
              AND (domain = 'global' OR domain IN ({}))
        "#,
        domain_placeholders.join(", ")
    );

    let row = WeightRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .one(db)
    .await?;

    Ok(row.map(|item| item.vote_weight.clamp(0.5, 2.0)).unwrap_or(1.0))
}

async fn recalculate_and_tally_votes_with_conn<C: ConnectionTrait>(
    db: &C,
    proposal: &ProposalRow,
) -> Result<WeightedTallyRow, ApiError> {
    let mut tally = WeightedTallyRow {
        yes: Some(0),
        no: Some(0),
        abstain: Some(0),
        weighted_yes: Some(0.0),
        weighted_no: Some(0.0),
    };
    let proposal_domains = parse_domains(&proposal.domains);
    let project_id = resolve_project_id_for_proposal_with_conn(db, &proposal.id, &proposal.author_id).await?;

    let votes = VoteForFinalizeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, voter_id, voter_type::text AS voter_type, choice::text AS choice, weight
            FROM votes
            WHERE proposal_id = $1
        "#,
        vec![proposal.id.clone().into()],
    ))
    .all(db)
    .await?;

    for vote in votes {
        let mut effective_weight = vote.weight.clamp(0.5, 2.0);
        if let (Some(project_id), Ok(voter_uuid)) = (project_id, Uuid::parse_str(&vote.voter_id)) {
            let participant = parse_participant_type(&vote.voter_type);
            let participant_str = match participant {
                ParticipantType::Ai => "ai",
                ParticipantType::Human => "human",
            };
            effective_weight = resolve_vote_weight_from_trust_scores(
                db,
                voter_uuid,
                participant_str,
                project_id,
                &proposal_domains,
            )
            .await?;
        }

        if (effective_weight - vote.weight).abs() > f64::EPSILON {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE votes SET weight = $2 WHERE id = $1",
                vec![vote.id.into(), effective_weight.into()],
            ))
            .await?;
        }

        match vote.choice.as_str() {
            "yes" => {
                tally.yes = Some(tally.yes.unwrap_or(0) + 1);
                tally.weighted_yes = Some(tally.weighted_yes.unwrap_or(0.0) + effective_weight);
            }
            "no" => {
                tally.no = Some(tally.no.unwrap_or(0) + 1);
                tally.weighted_no = Some(tally.weighted_no.unwrap_or(0.0) + effective_weight);
            }
            "abstain" => {
                tally.abstain = Some(tally.abstain.unwrap_or(0) + 1);
            }
            _ => {}
        }
    }

    Ok(tally)
}

async fn apply_trust_score_after_finalize_with_conn<C: ConnectionTrait>(
    state: &AppState,
    db: &C,
    proposal: &ProposalRow,
    result: DecisionResult,
) -> Result<(), ApiError> {
    if !matches!(result, DecisionResult::Approved | DecisionResult::Rejected) {
        return Ok(());
    }

    let Ok(author_id) = Uuid::parse_str(&proposal.author_id) else {
        tracing::warn!(
            proposal_id = proposal.id,
            author_id = proposal.author_id,
            "skip trust score update: non-uuid author id"
        );
        return Ok(());
    };

    let project_id =
        match resolve_project_id_for_proposal_with_conn(db, &proposal.id, &proposal.author_id).await? {
            Some(id) => id,
            None => return Ok(()),
        };

    let user_type: ParticipantType = parse_participant_type(&proposal.author_type);
    let domains = parse_domains(&proposal.domains);
    let trust = TrustScoreService::new(state.db.clone());
    trust
        .apply_proposal_result_with_conn(
            db,
            author_id,
            user_type,
            project_id,
            &proposal.id,
            matches!(result, DecisionResult::Approved),
            &domains,
        )
        .await
}

fn ensure_author_or_admin(proposal: &ProposalRow, actor: &ActorContext) -> Result<(), ApiError> {
    if proposal.author_id == actor.user_id_str || actor.is_admin {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "only proposal author or admin can perform this action".to_string(),
        ))
    }
}

async fn write_proposal_audit_log(
    state: &AppState,
    proposal: &ProposalRow,
    actor_id: Uuid,
    action: &str,
    resource_type: &str,
    resource_id: Option<String>,
    old_value: Option<Value>,
    new_value: Option<Value>,
    metadata: Option<Value>,
) -> Result<(), ApiError> {
    let project_id =
        resolve_project_id_for_proposal(state, &proposal.id, &proposal.author_id).await?;
    let Some(project_id) = project_id else {
        return Ok(());
    };

    write_governance_audit_log(
        &state.db,
        GovernanceAuditLogInput {
            project_id,
            actor_id: Some(actor_id),
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id,
            old_value,
            new_value,
            metadata,
        },
    )
    .await
}

pub async fn create_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<CreateProposalRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let mut title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);
    let mut proposal_type = req
        .proposal_type
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);
    let mut content = req
        .content
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);
    let mut domains = req.domains.clone();
    let mut template_voting_rule: Option<String> = None;
    let mut template_cycle_template: Option<String> = None;
    let mut template_project_id: Option<Uuid> = None;

    if let Some(template_id) = req.template_id.as_deref() {
        let template = ProposalTemplateForCreateRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, project_id, template_type, content, is_active
                FROM proposal_templates
                WHERE id = $1
            "#,
            vec![template_id.to_string().into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::NotFound("proposal template not found".to_string()))?;

        if !template.is_active {
            return Err(ApiError::BadRequest(
                "proposal template is inactive".to_string(),
            ));
        }
        if !actor.is_admin && !is_project_member(&state.db, template.project_id, actor.user_id).await? {
            return Err(ApiError::Forbidden("project access denied".to_string()));
        }

        template_project_id = Some(template.project_id);
        if title.is_none() {
            title = take_template_string(&template.content, "title");
        }
        if proposal_type.is_none() {
            proposal_type = take_template_string(&template.content, "proposal_type")
                .or_else(|| Some(template.template_type.clone()));
        }
        if content.is_none() {
            content = take_template_string(&template.content, "content");
        }
        if domains.is_none() {
            domains = take_template_domains(&template.content);
        }
        template_voting_rule = take_template_string(&template.content, "voting_rule");
        template_cycle_template = take_template_string(&template.content, "cycle_template");
    }

    let title = title.ok_or_else(|| ApiError::BadRequest("title is required".to_string()))?;
    let proposal_type = proposal_type
        .ok_or_else(|| ApiError::BadRequest("proposal_type is required".to_string()))?;
    let content = content.ok_or_else(|| ApiError::BadRequest("content is required".to_string()))?;
    let domains = domains.ok_or_else(|| ApiError::BadRequest("domains is required".to_string()))?;

    if title.len() < 10 || title.len() > 200 {
        return Err(ApiError::BadRequest(
            "title must be between 10 and 200 characters".to_string(),
        ));
    }
    if content.len() < 50 {
        return Err(ApiError::BadRequest(
            "content must be at least 50 characters".to_string(),
        ));
    }
    if domains.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one domain is required".to_string(),
        ));
    }
    if !validate_proposal_type(&proposal_type) {
        return Err(ApiError::BadRequest("invalid proposal_type".to_string()));
    }

    let voting_rule = req
        .voting_rule
        .clone()
        .or(template_voting_rule)
        .unwrap_or_else(|| "simple_majority".to_string());
    if !validate_voting_rule(&voting_rule) {
        return Err(ApiError::BadRequest("invalid voting_rule".to_string()));
    }

    let cycle_template = req
        .cycle_template
        .clone()
        .or(template_cycle_template)
        .unwrap_or_else(|| default_cycle_template(&proposal_type).to_string());
    if !validate_cycle_template(&cycle_template) {
        return Err(ApiError::BadRequest("invalid cycle_template".to_string()));
    }

    let proposal_id = gen_prefixed_id("PROP");
    let now = Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO proposals (
                    id, title, proposal_type, status, author_id, author_type, content,
                    domains, voting_rule, cycle_template, template_id, created_at
                ) VALUES ($1, $2, $3::proposal_type, 'draft', $4, $5::author_type, $6, $7, $8::voting_rule, $9::cycle_template, $10, $11)
            "#,
            vec![
                proposal_id.clone().into(),
                title.clone().into(),
                proposal_type.clone().into(),
                actor.user_id_str.clone().into(),
                actor.author_type.into(),
                content.clone().into(),
                json!(domains).into(),
                voting_rule.clone().into(),
                cycle_template.clone().into(),
                req.template_id.clone().into(),
                now.into(),
            ],
        ))
        .await?;

    let project_id = if let Some(project_id) = template_project_id {
        Some(project_id)
    } else {
        resolve_project_id_for_proposal(&state, &proposal_id, &actor.user_id_str).await?
    };
    if let Some(project_id) = project_id {
        write_governance_audit_log(
            &state.db,
            GovernanceAuditLogInput {
                project_id,
                actor_id: Some(actor.user_id),
                action: "proposal.created".to_string(),
                resource_type: "proposal".to_string(),
                resource_id: Some(proposal_id.clone()),
                old_value: None,
                new_value: Some(json!({
                    "title": title,
                    "proposal_type": proposal_type,
                    "voting_rule": voting_rule,
                    "cycle_template": cycle_template,
                    "template_id": req.template_id.clone(),
                })),
                metadata: Some(json!({ "domains": domains })),
            },
        )
        .await?;

        if let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await? {
            trigger_webhooks(
                state.clone(),
                TriggerContext {
                    event: WebhookEvent::ProposalCreated,
                    workspace_id,
                    project_id,
                    actor_id: actor.user_id,
                    issue_id: None,
                    comment_id: None,
                    label_id: None,
                    sprint_id: None,
                    changes: None,
                    mentions: Vec::new(),
                    extra_data: Some(json!({
                        "proposal": {
                            "id": proposal_id,
                            "title": title,
                            "proposal_type": proposal_type,
                            "status": "draft",
                            "voting_rule": voting_rule,
                            "cycle_template": cycle_template,
                            "template_id": req.template_id,
                            "created_at": now.to_rfc3339(),
                        }
                    })),
                },
            );
        }
    }

    let (discussion_hours, voting_hours) = cycle_hours(&cycle_template);

    Ok(ApiResponse::success(json!({
        "id": proposal_id,
        "status": "draft",
        "cycle_template": cycle_template,
        "template_id": req.template_id.clone(),
        "discussion_duration_hours": discussion_hours,
        "voting_duration_hours": voting_hours,
        "voting_deadline": Value::Null,
        "created_at": now.to_rfc3339()
    })))
}

pub async fn list_proposals(
    State(state): State<AppState>,
    Query(query): Query<ListProposalsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    if let Some(status) = query.status.as_deref() {
        if !validate_status(status) {
            return Err(ApiError::BadRequest("invalid status".to_string()));
        }
    }
    if let Some(proposal_type) = query.proposal_type.as_deref() {
        if !validate_proposal_type(proposal_type) {
            return Err(ApiError::BadRequest("invalid proposal_type".to_string()));
        }
    }

    let mut where_parts: Vec<String> = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx = 1;

    if let Some(status) = query.status {
        where_parts.push(format!("status::text = ${}", idx));
        values.push(status.into());
        idx += 1;
    }
    if let Some(proposal_type) = query.proposal_type {
        where_parts.push(format!("proposal_type::text = ${}", idx));
        values.push(proposal_type.into());
        idx += 1;
    }
    if let Some(domain) = query.domain {
        where_parts.push(format!("domains ? ${}", idx));
        values.push(domain.into());
        idx += 1;
    }

    let where_sql = if where_parts.is_empty() {
        "".to_string()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let sort_field = query
        .sort
        .as_deref()
        .and_then(|sort| sort.split(':').next())
        .unwrap_or("created_at");
    let sort_order = query
        .sort
        .as_deref()
        .and_then(|sort| sort.split(':').nth(1))
        .unwrap_or("desc");

    let sort_field = match sort_field {
        "created_at" | "title" | "status" => sort_field,
        _ => "created_at",
    };
    let sort_order = if sort_order.eq_ignore_ascii_case("asc") {
        "ASC"
    } else {
        "DESC"
    };

    let mut count_values = values.clone();
    let count_sql = format!("SELECT COUNT(*)::bigint AS count FROM proposals {}", where_sql);

    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        std::mem::take(&mut count_values),
    ))
    .one(&state.db)
    .await?
    .map(|r| r.count)
    .unwrap_or(0);

    let mut list_values = values;
    list_values.push(per_page.into());
    list_values.push(offset.into());
    let limit_idx = idx;
    let offset_idx = idx + 1;

    let list_sql = format!(
        r#"
            SELECT id, title, proposal_type::text AS proposal_type, status::text AS status,
                   author_id, author_type::text AS author_type, content, domains,
                   voting_rule::text AS voting_rule, cycle_template::text AS cycle_template,
                   template_id,
                   created_at, submitted_at, voting_started_at, voting_ended_at, archived_at
            FROM proposals
            {where_sql}
            ORDER BY {sort_field} {sort_order}
            LIMIT ${limit_idx}
            OFFSET ${offset_idx}
        "#
    );

    let rows = ProposalRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        list_sql,
        list_values,
    ))
    .all(&state.db)
    .await?;

    let items = rows.iter().map(proposal_to_item).collect::<Vec<_>>();
    let total_pages = if total == 0 {
        1
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn get_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let proposal = find_proposal(&state, &id).await?;
    ensure_voting_finalized_if_needed(&state, &proposal).await?;
    let proposal = find_proposal(&state, &id).await?;

    let tally = proposal_tally(&state, &proposal.id).await?;
    let decision_id = DecisionIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM decisions WHERE proposal_id = $1",
        vec![proposal.id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .map(|r| r.id);

    Ok(ApiResponse::success(ProposalDetailResponse {
        proposal: proposal_to_item(&proposal),
        tally,
        decision_id,
    }))
}

pub async fn update_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProposalRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    let old_snapshot = serde_json::to_value(proposal_to_item(&proposal)).map_err(|_| ApiError::Internal)?;
    let old_snapshot = serde_json::to_value(proposal_to_item(&proposal)).map_err(|_| ApiError::Internal)?;

    if proposal.author_id != actor.user_id_str {
        return Err(ApiError::Forbidden(
            "only proposal author can update".to_string(),
        ));
    }
    if proposal.status != "draft" {
        return Err(ApiError::BadRequest(
            "only draft proposal can be updated".to_string(),
        ));
    }

    if req.title.is_none()
        && req.proposal_type.is_none()
        && req.content.is_none()
        && req.domains.is_none()
        && req.voting_rule.is_none()
        && req.cycle_template.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let mut set_parts: Vec<String> = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut idx = 1;

    if let Some(title) = req.title {
        let title = title.trim().to_string();
        if title.len() < 10 || title.len() > 200 {
            return Err(ApiError::BadRequest(
                "title must be between 10 and 200 characters".to_string(),
            ));
        }
        set_parts.push(format!("title = ${}", idx));
        values.push(title.into());
        idx += 1;
    }

    if let Some(proposal_type) = req.proposal_type {
        if !validate_proposal_type(&proposal_type) {
            return Err(ApiError::BadRequest("invalid proposal_type".to_string()));
        }
        set_parts.push(format!("proposal_type = ${}::proposal_type", idx));
        values.push(proposal_type.into());
        idx += 1;
    }

    if let Some(content) = req.content {
        let content = content.trim().to_string();
        if content.len() < 50 {
            return Err(ApiError::BadRequest(
                "content must be at least 50 characters".to_string(),
            ));
        }
        set_parts.push(format!("content = ${}", idx));
        values.push(content.into());
        idx += 1;
    }

    if let Some(domains) = req.domains {
        if domains.is_empty() {
            return Err(ApiError::BadRequest(
                "at least one domain is required".to_string(),
            ));
        }
        set_parts.push(format!("domains = ${}", idx));
        values.push(json!(domains).into());
        idx += 1;
    }

    if let Some(voting_rule) = req.voting_rule {
        if !validate_voting_rule(&voting_rule) {
            return Err(ApiError::BadRequest("invalid voting_rule".to_string()));
        }
        set_parts.push(format!("voting_rule = ${}::voting_rule", idx));
        values.push(voting_rule.into());
        idx += 1;
    }

    if let Some(cycle_template) = req.cycle_template {
        if !validate_cycle_template(&cycle_template) {
            return Err(ApiError::BadRequest("invalid cycle_template".to_string()));
        }
        set_parts.push(format!("cycle_template = ${}::cycle_template", idx));
        values.push(cycle_template.into());
        idx += 1;
    }

    values.push(id.clone().into());

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("UPDATE proposals SET {} WHERE id = ${}", set_parts.join(", "), idx),
            values,
        ))
        .await?;

    let updated = find_proposal(&state, &id).await?;
    let new_snapshot = serde_json::to_value(proposal_to_item(&updated)).map_err(|_| ApiError::Internal)?;
    write_proposal_audit_log(
        &state,
        &updated,
        actor.user_id,
        "proposal.updated",
        "proposal",
        Some(id),
        Some(old_snapshot),
        Some(new_snapshot),
        None,
    )
    .await?;

    if let Some(project_id) =
        resolve_project_id_for_proposal(&state, &updated.id, &updated.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalUpdated,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({ "proposal": proposal_to_item(&updated) })),
            },
        );
    }
    Ok(ApiResponse::success(proposal_to_item(&updated)))
}

pub async fn delete_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    let old_snapshot = serde_json::to_value(proposal_to_item(&proposal)).map_err(|_| ApiError::Internal)?;

    if proposal.author_id != actor.user_id_str {
        return Err(ApiError::Forbidden(
            "only proposal author can delete".to_string(),
        ));
    }
    if proposal.status != "draft" {
        return Err(ApiError::BadRequest(
            "only draft proposal can be deleted".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM proposals WHERE id = $1",
            vec![id.clone().into()],
        ))
        .await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.deleted",
        "proposal",
        Some(id),
        Some(old_snapshot),
        None,
        None,
    )
    .await?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalDeleted,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal": {
                        "id": proposal.id,
                        "title": proposal.title,
                        "status": "deleted",
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::ok())
}

pub async fn submit_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;

    if proposal.author_id != actor.user_id_str {
        return Err(ApiError::Forbidden(
            "only proposal author can submit".to_string(),
        ));
    }
    if proposal.status != "draft" {
        return Err(ApiError::BadRequest(
            "only draft proposal can be submitted".to_string(),
        ));
    }

    let now = Utc::now();
    let (discussion_hours, _) = cycle_hours(&proposal.cycle_template);
    let discussion_ends_at = now + Duration::hours(discussion_hours);

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE proposals SET status = 'open', submitted_at = $2 WHERE id = $1",
            vec![id.clone().into(), now.into()],
        ))
        .await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.submitted",
        "proposal",
        Some(id.clone()),
        Some(json!({ "status": "draft" })),
        Some(json!({ "status": "open", "submitted_at": now.to_rfc3339() })),
        None,
    )
    .await?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalSubmitted,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal": {
                        "id": proposal.id,
                        "status": "open",
                        "submitted_at": now.to_rfc3339(),
                        "discussion_ends_at": discussion_ends_at.to_rfc3339(),
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::success(json!({
        "id": id,
        "status": "open",
        "discussion_ends_at": discussion_ends_at.to_rfc3339(),
        "notified_members": 0
    })))
}

pub async fn start_voting(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;

    if proposal.author_id != actor.user_id_str {
        return Err(ApiError::Forbidden(
            "only proposal author can start voting".to_string(),
        ));
    }
    if proposal.status != "open" {
        return Err(ApiError::BadRequest(
            "proposal must be in open status".to_string(),
        ));
    }

    let now = Utc::now();
    let (_, voting_hours) = cycle_hours(&proposal.cycle_template);
    let voting_ends_at = now + Duration::hours(voting_hours);

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE proposals SET status = 'voting', voting_started_at = $2 WHERE id = $1",
            vec![id.clone().into(), now.into()],
        ))
        .await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.voting_started",
        "proposal",
        Some(id.clone()),
        Some(json!({ "status": "open" })),
        Some(json!({ "status": "voting", "voting_started_at": now.to_rfc3339() })),
        None,
    )
    .await?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await? {
        let _ = queue_vote_requested_tasks_for_project(
            &state.db,
            project_id,
            &proposal.id,
            json!({
                "proposal_id": proposal.id,
                "project_id": project_id.to_string(),
                "trigger": "proposal.start_voting",
            }),
        )
        .await?;

        if let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await? {
            trigger_webhooks(
                state.clone(),
                TriggerContext {
                    event: WebhookEvent::ProposalVotingStarted,
                    workspace_id,
                    project_id,
                    actor_id: actor.user_id,
                    issue_id: None,
                    comment_id: None,
                    label_id: None,
                    sprint_id: None,
                    changes: None,
                    mentions: Vec::new(),
                    extra_data: Some(json!({
                        "proposal": {
                            "id": proposal.id,
                            "status": "voting",
                            "voting_started_at": now.to_rfc3339(),
                            "voting_ends_at": voting_ends_at.to_rfc3339(),
                        }
                    })),
                },
            );
        }
    }

    Ok(ApiResponse::success(json!({
        "id": id,
        "status": "voting",
        "voting_started_at": now.to_rfc3339(),
        "voting_ends_at": voting_ends_at.to_rfc3339()
    })))
}

pub async fn archive_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    ensure_author_or_admin(&proposal, &actor)?;

    let now = Utc::now();
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE proposals SET status = 'archived', archived_at = $2 WHERE id = $1",
            vec![id.clone().into(), now.into()],
        ))
        .await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.archived",
        "proposal",
        Some(id.clone()),
        Some(json!({ "status": proposal.status })),
        Some(json!({ "status": "archived", "archived_at": now.to_rfc3339() })),
        None,
    )
    .await?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalArchived,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal": {
                        "id": proposal.id,
                        "status": "archived",
                        "archived_at": now.to_rfc3339(),
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::success(json!({
        "id": id,
        "status": "archived",
        "archived_at": now.to_rfc3339()
    })))
}

pub async fn create_vote(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<VoteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    ensure_voting_finalized_if_needed(&state, &proposal).await?;
    let proposal = find_proposal(&state, &id).await?;

    if proposal.status != "voting" {
        return Err(ApiError::BadRequest(
            "proposal is not in voting status".to_string(),
        ));
    }

    let choice = req.choice.trim().to_lowercase();
    if !validate_vote_choice(&choice) {
        return Err(ApiError::BadRequest("invalid vote choice".to_string()));
    }

    if actor.author_type == "ai" {
        let project_id =
            resolve_project_id_for_proposal(&state, &id, &proposal.author_id).await?;
        let Some(project_id) = project_id else {
            return Err(ApiError::Forbidden(
                "ai voting requires project context".to_string(),
            ));
        };

        let domain = primary_domain_for_proposal(&proposal);
        let permission = PermissionService::new(state.db.clone());
        let can_vote = permission
            .can_vote(actor.user_id, project_id, &domain, ParticipantType::Ai)
            .await?;
        if !can_vote {
            return Err(ApiError::Forbidden(
                "ai participant has no voting permission in this domain".to_string(),
            ));
        }

        let min_reason = permission
            .ai_reason_min_length(actor.user_id, project_id)
            .await?
            .unwrap_or(50)
            .max(0) as usize;

        let reason_len = req
            .reason
            .as_deref()
            .map(str::trim)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0);
        if reason_len < min_reason {
            return Err(ApiError::BadRequest(format!(
                "reason is required for AI vote and must be at least {} characters",
                min_reason
            )));
        }
    }

    let now = Utc::now();
    let reason = req.reason.map(|value| value.trim().to_string());
    let weight =
        resolve_vote_weight_for_proposal(&state, &id, &actor.user_id_str, actor.author_type).await?;

    let res = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO votes (proposal_id, voter_id, voter_type, choice, weight, reason, voted_at)
                VALUES ($1, $2, $3::author_type, $4::vote_choice, $5, $6, $7)
            "#,
            vec![
                id.clone().into(),
                actor.user_id_str.clone().into(),
                actor.author_type.into(),
                choice.clone().into(),
                weight.into(),
                reason.into(),
                now.into(),
            ],
        ))
        .await;

    if let Err(err) = res {
        let message = err.to_string();
        if message.contains("uq_votes_proposal_voter") {
            return Err(ApiError::Conflict(
                "you have already voted on this proposal".to_string(),
            ));
        }
        return Err(ApiError::Database(err));
    }

    let vote_row = VoteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, voter_id, voter_type::text AS voter_type,
                   choice::text AS choice, weight, reason, voted_at
            FROM votes
            WHERE proposal_id = $1 AND voter_id = $2
        "#,
        vec![id.clone().into(), actor.user_id_str.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    let tally = proposal_tally(&state, &id).await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "vote.created",
        "vote",
        Some(vote_row.id.to_string()),
        None,
        Some(json!({
            "proposal_id": id,
            "voter_id": vote_row.voter_id,
            "choice": choice,
            "weight": vote_row.weight,
        })),
        None,
    )
    .await?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalVoteCast,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal_id": vote_row.proposal_id,
                    "vote": {
                        "id": vote_row.id,
                        "voter_id": vote_row.voter_id,
                        "choice": vote_row.choice,
                        "weight": vote_row.weight,
                        "reason": vote_row.reason,
                        "voted_at": vote_row.voted_at.to_rfc3339(),
                    },
                    "tally": tally,
                })),
            },
        );
    }

    Ok(ApiResponse::success(json!({
        "vote_id": vote_row.id,
        "weight": vote_row.weight,
        "voted_at": vote_row.voted_at.to_rfc3339(),
        "current_tally": tally
    })))
}

pub async fn list_votes(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let proposal = find_proposal(&state, &id).await?;
    ensure_voting_finalized_if_needed(&state, &proposal).await?;
    let proposal = find_proposal(&state, &id).await?;

    let tally = proposal_tally(&state, &id).await?;

    if proposal.status == "voting" {
        return Ok(ApiResponse::success(json!({
            "proposal_id": id,
            "is_hidden": true,
            "tally": tally,
            "items": []
        })));
    }

    let votes = VoteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, voter_id, voter_type::text AS voter_type,
                   choice::text AS choice, weight, reason, voted_at
            FROM votes
            WHERE proposal_id = $1
            ORDER BY voted_at ASC
        "#,
        vec![id.clone().into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(json!({
        "proposal_id": id,
        "is_hidden": false,
        "tally": tally,
        "items": votes
    })))
}

pub async fn delete_my_vote(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    ensure_voting_finalized_if_needed(&state, &proposal).await?;
    let proposal = find_proposal(&state, &id).await?;

    if proposal.status != "voting" {
        return Err(ApiError::BadRequest(
            "vote can only be withdrawn during voting".to_string(),
        ));
    }

    let existing_vote = VoteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, voter_id, voter_type::text AS voter_type,
                   choice::text AS choice, weight, reason, voted_at
            FROM votes
            WHERE proposal_id = $1 AND voter_id = $2
            LIMIT 1
        "#,
        vec![id.clone().into(), actor.user_id_str.clone().into()],
    ))
    .one(&state.db)
    .await?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM votes WHERE proposal_id = $1 AND voter_id = $2",
            vec![id.clone().into(), actor.user_id_str.into()],
        ))
        .await?;

    if let Some(vote) = existing_vote {
        write_proposal_audit_log(
            &state,
            &proposal,
            actor.user_id,
            "vote.deleted",
            "vote",
            Some(vote.id.to_string()),
            Some(json!({
                "proposal_id": id,
                "voter_id": vote.voter_id,
                "choice": vote.choice,
                "weight": vote.weight,
            })),
            None,
            None,
        )
        .await?;

        if let Some(project_id) =
            resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
            && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
        {
            trigger_webhooks(
                state.clone(),
                TriggerContext {
                    event: WebhookEvent::ProposalUpdated,
                    workspace_id,
                    project_id,
                    actor_id: actor.user_id,
                    issue_id: None,
                    comment_id: None,
                    label_id: None,
                    sprint_id: None,
                    changes: None,
                    mentions: Vec::new(),
                    extra_data: Some(json!({
                        "proposal_id": proposal.id,
                        "vote_deleted": {
                            "id": vote.id,
                            "voter_id": vote.voter_id,
                            "choice": vote.choice,
                            "weight": vote.weight,
                        }
                    })),
                },
            );
        }
    }

    Ok(ApiResponse::ok())
}

pub async fn create_proposal_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<CreateProposalCommentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;

    let content = req.content.trim();
    if content.is_empty() {
        return Err(ApiError::BadRequest("content is required".to_string()));
    }

    if actor.author_type == "ai" {
        let project_id =
            resolve_project_id_for_proposal(&state, &id, &proposal.author_id).await?;
        let Some(project_id) = project_id else {
            return Err(ApiError::Forbidden(
                "ai commenting requires project context".to_string(),
            ));
        };

        let domain = primary_domain_for_proposal(&proposal);
        let permission = PermissionService::new(state.db.clone());
        let can_comment = permission
            .can_comment(actor.user_id, project_id, &domain, ParticipantType::Ai)
            .await?;
        if !can_comment {
            return Err(ApiError::Forbidden(
                "ai participant has no comment permission in this domain".to_string(),
            ));
        }
    }

    let comment_type = req.comment_type.unwrap_or_else(|| "general".to_string());
    if !validate_comment_type(&comment_type) {
        return Err(ApiError::BadRequest("invalid comment_type".to_string()));
    }

    let comment = ProposalCommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"

            WITH inserted AS (
                INSERT INTO proposal_comments (proposal_id, author_id, author_type, comment_type, content, created_at)
                VALUES ($1, $2, $3::author_type, $4, $5, $6)
                RETURNING id, proposal_id, author_id, author_type, comment_type, content, created_at
            )
            SELECT c.id,
                   c.proposal_id,
                   c.author_id::text AS author_id,
                   COALESCE(u.name, '') AS author_name,
                   NULL::text AS author_avatar,
                   c.author_type::text AS author_type,
                   c.comment_type,
                   c.content,
                   c.created_at
            FROM inserted c
            LEFT JOIN users u ON u.id::text = c.author_id::text
        "#,
        vec![
            id.into(),
            actor.user_id_str.clone().into(),
            actor.author_type.into(),
            comment_type.into(),
            content.into(),
            Utc::now().into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    if let Some(project_id) = resolve_project_id_for_proposal(&state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalUpdated,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal_id": proposal.id,
                    "comment": comment,
                })),
            },
        );
    }

    Ok(ApiResponse::success(comment))
}

pub async fn list_proposal_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    find_proposal(&state, &id).await?;

    let items = ProposalCommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT pc.id,
                   pc.proposal_id,
                   pc.author_id::text AS author_id,
                   COALESCE(to_jsonb(u)->>'name', to_jsonb(u)->>'display_name') AS author_name,
                   NULL::text AS author_avatar,
                   pc.author_type::text AS author_type,
                   pc.comment_type,
                   pc.content,
                   pc.created_at
            FROM proposal_comments pc
            LEFT JOIN users u ON u.id::text = pc.author_id::text
            WHERE proposal_id = $1
            ORDER BY pc.created_at ASC
        "#,
        vec![id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn delete_proposal_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(comment_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;

    let comment = ProposalCommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT pc.id,
                   pc.proposal_id,
                   pc.author_id::text AS author_id,
                   COALESCE(to_jsonb(u)->>'name', to_jsonb(u)->>'display_name') AS author_name,
                   NULL::text AS author_avatar,
                   pc.author_type::text AS author_type,
                   pc.comment_type,
                   pc.content,
                   pc.created_at
            FROM proposal_comments pc
            LEFT JOIN users u ON u.id::text = pc.author_id::text
            WHERE pc.id = $1
        "#,
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("comment not found".to_string()))?;

    if comment.author_id != actor.user_id_str && !actor.is_admin {
        return Err(ApiError::Forbidden(
            "only comment author or admin can delete".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM proposal_comments WHERE id = $1",
            vec![comment_id.into()],
        ))
        .await?;

    let proposal = find_proposal(&state, &comment.proposal_id).await?;
    if let Some(project_id) =
        resolve_project_id_for_proposal(&state, &comment.proposal_id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(&state, project_id).await?
    {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::ProposalUpdated,
                workspace_id,
                project_id,
                actor_id: actor.user_id,
                issue_id: None,
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: Some(json!({
                    "proposal_id": comment.proposal_id,
                    "comment_deleted": {
                        "id": comment.id,
                        "author_id": comment.author_id,
                        "comment_type": comment.comment_type,
                    }
                })),
            },
        );
    }

    Ok(ApiResponse::ok())
}

pub async fn delete_proposal_comment_under_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((proposal_id, comment_id)): Path<(String, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    find_proposal(&state, &proposal_id).await?;

    let comment = ProposalCommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT pc.id,
                   pc.proposal_id,
                   pc.author_id::text AS author_id,
                   COALESCE(to_jsonb(u)->>'name', to_jsonb(u)->>'display_name') AS author_name,
                   NULL::text AS author_avatar,
                   pc.author_type::text AS author_type,
                   pc.comment_type,
                   pc.content,
                   pc.created_at
            FROM proposal_comments pc
            LEFT JOIN users u ON u.id::text = pc.author_id::text
            WHERE pc.id = $1 AND pc.proposal_id = $2
        "#,
        vec![comment_id.into(), proposal_id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("comment not found".to_string()))?;

    if comment.author_id != actor.user_id_str && !actor.is_admin {
        return Err(ApiError::Forbidden(
            "only comment author or admin can delete".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM proposal_comments WHERE id = $1 AND proposal_id = $2",
            vec![comment_id.into(), proposal_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

pub async fn link_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<LinkIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &id).await?;
    ensure_author_or_admin(&proposal, &actor)?;

    if proposal.status != "approved" {
        return Err(ApiError::BadRequest(
            "issue can only be linked after proposal is approved".to_string(),
        ));
    }

    let issue_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM work_items WHERE id = $1",
            vec![req.issue_id.into()],
        ))
        .await?
        .is_some();

    if !issue_exists {
        return Err(ApiError::NotFound("issue not found".to_string()));
    }

    let res = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO proposal_issue_links (proposal_id, issue_id, created_at) VALUES ($1, $2, $3)",
            vec![id.clone().into(), req.issue_id.into(), Utc::now().into()],
        ))
        .await;

    if let Err(err) = res {
        let message = err.to_string();
        if message.contains("uq_proposal_issue_link") {
            return Err(ApiError::Conflict(
                "issue is already linked to this proposal".to_string(),
            ));
        }
        return Err(ApiError::Database(err));
    }

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.issue_linked",
        "proposal_issue_link",
        Some(format!("{}:{}", id, req.issue_id)),
        None,
        Some(json!({
            "proposal_id": id,
            "issue_id": req.issue_id,
        })),
        None,
    )
    .await?;

    Ok(ApiResponse::ok())
}

pub async fn list_linked_issues(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    find_proposal(&state, &id).await?;

    let items = ProposalIssueLinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT pil.id, pil.proposal_id, pil.issue_id, wi.title AS issue_title,
                   wi.state AS issue_state, pil.created_at
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON pil.issue_id = wi.id
            WHERE pil.proposal_id = $1
            ORDER BY pil.created_at ASC
        "#,
        vec![id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn unlink_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((proposal_id, issue_id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = build_actor_context(&state, &claims).await?;
    let proposal = find_proposal(&state, &proposal_id).await?;
    ensure_author_or_admin(&proposal, &actor)?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM proposal_issue_links WHERE proposal_id = $1 AND issue_id = $2",
            vec![proposal_id.clone().into(), issue_id.into()],
        ))
        .await?;

    write_proposal_audit_log(
        &state,
        &proposal,
        actor.user_id,
        "proposal.issue_unlinked",
        "proposal_issue_link",
        Some(format!("{}:{}", proposal_id, issue_id)),
        Some(json!({
            "proposal_id": proposal_id,
            "issue_id": issue_id,
        })),
        None,
        None,
    )
    .await?;

    Ok(ApiResponse::ok())
}
