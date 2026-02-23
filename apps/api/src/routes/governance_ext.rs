use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::trust_score_service::{is_project_admin_or_owner, is_project_member, is_system_admin},
};

#[derive(Debug, Serialize, FromQueryResult)]
struct ChainProposalRow {
    id: String,
    title: String,
    proposal_type: String,
    status: String,
    author_id: String,
    author_type: String,
    created_at: DateTime<Utc>,
    submitted_at: Option<DateTime<Utc>>,
    voting_started_at: Option<DateTime<Utc>>,
    voting_ended_at: Option<DateTime<Utc>>,
    archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct ChainDecisionRow {
    id: String,
    result: String,
    approval_rate: Option<f64>,
    total_votes: i32,
    yes_votes: i32,
    no_votes: i32,
    abstain_votes: i32,
    decided_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct ChainVoteRow {
    voter_id: String,
    voter_type: String,
    choice: String,
    weight: f64,
    reason: Option<String>,
    voted_at: DateTime<Utc>,
    trust_delta: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct ChainIssueRow {
    issue_id: Uuid,
    issue_title: String,
    issue_state: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct ChainReviewRow {
    id: String,
    status: String,
    rating: Option<String>,
    reviewer_id: Option<Uuid>,
    trust_score_applied: bool,
    scheduled_at: Option<DateTime<Utc>>,
    conducted_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct TimelineRow {
    timestamp: DateTime<Utc>,
    event_type: String,
    description: String,
    actor: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProposalChainResponse {
    proposal: ChainProposalRow,
    issues: Vec<ChainIssueRow>,
    link_status: String,
    decision: Option<ChainDecisionRow>,
    votes: Vec<ChainVoteRow>,
    timeline: Vec<TimelineRow>,
    impact_review: Option<ChainReviewRow>,
    feedback_proposals: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct DecisionAnalyticsQuery {
    pub project_id: Option<Uuid>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct DecisionOverviewRow {
    total_decisions: i64,
    approved_count: i64,
    rejected_count: i64,
    vetoed_count: i64,
    avg_cycle_hours: Option<f64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct DecisionTypeRow {
    proposal_type: String,
    total_decisions: i64,
    approved_count: i64,
    rejected_count: i64,
    vetoed_count: i64,
    avg_cycle_hours: Option<f64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct DecisionDomainRow {
    domain: String,
    total_decisions: i64,
    approved_count: i64,
    rejected_count: i64,
    vetoed_count: i64,
    avg_cycle_hours: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateAuditReportRequest {
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct AuditReportRow {
    id: String,
    project_id: Uuid,
    period_start: NaiveDate,
    period_end: NaiveDate,
    total_proposals: i32,
    approved_proposals: i32,
    rejected_proposals: i32,
    vetoed_proposals: i32,
    reviewed_proposals: i32,
    avg_review_rating: Option<f64>,
    rating_distribution: Value,
    top_contributors: Option<Value>,
    domain_stats: Option<Value>,
    ai_participation_stats: Option<Value>,
    key_insights: Option<Value>,
    generated_at: DateTime<Utc>,
    generated_by: String,
}

#[derive(Debug, Deserialize)]
pub struct ListAuditReportsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct AiLearningRow {
    id: i64,
    ai_participant_id: String,
    review_id: String,
    proposal_id: String,
    domain: String,
    review_rating: String,
    ai_vote_choice: Option<String>,
    ai_vote_reason: Option<String>,
    outcome_alignment: String,
    lesson_learned: Option<String>,
    will_change: Option<String>,
    follow_up_improvement: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AiLearningQuery {
    pub domain: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
struct AlignmentStatsRow {
    total: i64,
    aligned: i64,
    misaligned: i64,
    neutral: i64,
    recent_total: i64,
    recent_aligned: i64,
}

#[derive(Debug, FromQueryResult)]
struct ProjectIdRow {
    project_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct NullableProjectIdRow {
    project_id: Option<Uuid>,
}

struct ProposalProjectScope {
    project_ids: Vec<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, FromQueryResult)]
struct RatingAggRow {
    reviewed_proposals: i64,
    avg_review_rating: Option<f64>,
    rating_distribution: Value,
}

#[derive(Debug, FromQueryResult)]
struct DomainStatsRow {
    domain: String,
    total_decisions: i64,
    approved_count: i64,
    rejected_count: i64,
    vetoed_count: i64,
    avg_review_score: Option<f64>,
}

fn parse_claim_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

async fn ensure_project_visible(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_member(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}

async fn get_project_id_for_proposal(
    db: &impl ConnectionTrait,
    proposal_id: &str,
) -> Result<ProposalProjectScope, ApiError> {
    let proposal_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS count FROM proposals WHERE id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .one(db)
    .await?
    .unwrap_or(CountRow { count: 0 });

    if proposal_count.count == 0 {
        return Err(ApiError::NotFound("proposal not found".to_string()));
    }

    let has_direct_project_id_column = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*)::bigint AS count
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'proposals'
              AND column_name = 'project_id'
        "#,
        vec![],
    ))
    .one(db)
    .await?
    .map(|row| row.count > 0)
    .unwrap_or(false);

    if has_direct_project_id_column {
        let row = NullableProjectIdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id FROM proposals WHERE id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .one(db)
        .await?;
        if let Some(row) = row {
            if let Some(project_id) = row.project_id {
                return Ok(ProposalProjectScope {
                    project_ids: vec![project_id],
                });
            }
        }
    }

    let rows = ProjectIdRow::find_by_statement(Statement::from_sql_and_values(
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

    Ok(ProposalProjectScope {
        project_ids: rows.into_iter().map(|row| row.project_id).collect(),
    })
}

async fn ensure_proposal_visible(
    state: &AppState,
    proposal_id: &str,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let scope = get_project_id_for_proposal(&state.db, proposal_id).await?;
    for project_id in scope.project_ids {
        ensure_project_visible(state, project_id, user_id).await?;
    }
    Ok(())
}

fn filter_clause(
    project_id: Option<Uuid>,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
) -> (String, Vec<sea_orm::Value>) {
    let mut where_parts: Vec<String> = vec![];
    let mut values: Vec<sea_orm::Value> = vec![];
    let mut idx = 1;

    if let Some(project_id) = project_id {
        where_parts.push(format!("fp.project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(start_at) = start_at {
        where_parts.push(format!("d.decided_at >= ${idx}"));
        values.push(start_at.into());
        idx += 1;
    }
    if let Some(end_at) = end_at {
        where_parts.push(format!("d.decided_at <= ${idx}"));
        values.push(end_at.into());
    }

    if where_parts.is_empty() {
        ("".to_string(), values)
    } else {
        (format!("WHERE {}", where_parts.join(" AND ")), values)
    }
}

pub async fn get_proposal_chain(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_proposal_visible(&state, &proposal_id, user_id).await?;

    let proposal = ChainProposalRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id,
                title,
                proposal_type::text AS proposal_type,
                status::text AS status,
                author_id,
                author_type::text AS author_type,
                created_at,
                submitted_at,
                voting_started_at,
                voting_ended_at,
                archived_at
            FROM proposals
            WHERE id = $1
        "#,
        vec![proposal_id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;

    let decision = ChainDecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id,
                result::text AS result,
                approval_rate,
                total_votes,
                yes_votes,
                no_votes,
                abstain_votes,
                decided_at
            FROM decisions
            WHERE proposal_id = $1
        "#,
        vec![proposal_id.clone().into()],
    ))
    .one(&state.db)
    .await?;

    let votes = ChainVoteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                v.voter_id,
                v.voter_type::text AS voter_type,
                v.choice::text AS choice,
                v.weight,
                v.reason,
                v.voted_at,
                (
                    SELECT SUM(tsl.score_change)::bigint
                    FROM trust_score_logs tsl
                    WHERE tsl.event_id = ir.id
                      AND tsl.user_id::text = v.voter_id
                      AND tsl.event_type = 'impact_review_completed'
                ) AS trust_delta
            FROM votes v
            LEFT JOIN impact_reviews ir ON ir.proposal_id = v.proposal_id
            WHERE v.proposal_id = $1
            ORDER BY v.voted_at ASC
        "#,
        vec![proposal_id.clone().into()],
    ))
    .all(&state.db)
    .await?;

    let issues = ChainIssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                pil.issue_id,
                wi.title AS issue_title,
                wi.state AS issue_state,
                pil.created_at
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON wi.id = pil.issue_id
            WHERE pil.proposal_id = $1
            ORDER BY pil.created_at ASC
        "#,
        vec![proposal_id.clone().into()],
    ))
    .all(&state.db)
    .await?;

    let impact_review = ChainReviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id,
                status::text AS status,
                rating::text AS rating,
                reviewer_id,
                trust_score_applied,
                scheduled_at,
                conducted_at,
                created_at
            FROM impact_reviews
            WHERE proposal_id = $1
        "#,
        vec![proposal_id.clone().into()],
    ))
    .one(&state.db)
    .await?;

    let feedback_proposals = Value::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT json_build_object(
                'proposal_id', p.id,
                'title', p.title,
                'status', p.status::text,
                'link_type', fll.link_type,
                'created_at', fll.created_at
            ) AS value
            FROM impact_reviews ir
            INNER JOIN feedback_loop_links fll ON fll.source_review_id = ir.id
            INNER JOIN proposals p ON p.id = fll.derived_proposal_id
            WHERE ir.proposal_id = $1
            ORDER BY fll.created_at DESC
        "#,
        vec![proposal_id.clone().into()],
    ))
    .all(&state.db)
    .await?;

    let timeline = build_timeline(&state.db, &proposal_id).await?;
    let link_status = if issues.is_empty() {
        "unlinked".to_string()
    } else {
        "linked".to_string()
    };

    Ok(ApiResponse::success(ProposalChainResponse {
        proposal,
        issues,
        link_status,
        decision,
        votes,
        timeline,
        impact_review,
        feedback_proposals: feedback_proposals
            .into_iter()
            .map(|item| item.get("value").cloned().unwrap_or(Value::Null))
            .collect(),
    }))
}

pub async fn get_proposal_timeline(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_proposal_visible(&state, &proposal_id, user_id).await?;
    let timeline = build_timeline(&state.db, &proposal_id).await?;
    Ok(ApiResponse::success(json!({
        "proposal_id": proposal_id,
        "events": timeline
    })))
}

async fn build_timeline(
    db: &impl ConnectionTrait,
    proposal_id: &str,
) -> Result<Vec<TimelineRow>, ApiError> {
    let mut events = TimelineRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT created_at AS timestamp,
                   'proposal_created' AS event_type,
                   'proposal created' AS description,
                   author_id AS actor
            FROM proposals
            WHERE id = $1

            UNION ALL
            SELECT submitted_at AS timestamp,
                   'proposal_submitted' AS event_type,
                   'proposal submitted for governance discussion' AS description,
                   author_id AS actor
            FROM proposals
            WHERE id = $1 AND submitted_at IS NOT NULL

            UNION ALL
            SELECT voting_started_at AS timestamp,
                   'voting_started' AS event_type,
                   'voting started' AS description,
                   NULL::text AS actor
            FROM proposals
            WHERE id = $1 AND voting_started_at IS NOT NULL

            UNION ALL
            SELECT voting_ended_at AS timestamp,
                   'voting_ended' AS event_type,
                   'voting ended' AS description,
                   NULL::text AS actor
            FROM proposals
            WHERE id = $1 AND voting_ended_at IS NOT NULL

            UNION ALL
            SELECT archived_at AS timestamp,
                   'proposal_archived' AS event_type,
                   'proposal archived' AS description,
                   NULL::text AS actor
            FROM proposals
            WHERE id = $1 AND archived_at IS NOT NULL

            UNION ALL
            SELECT decided_at AS timestamp,
                   CONCAT('decision_', d.result::text) AS event_type,
                   CONCAT('decision ', d.result::text) AS description,
                   NULL::text AS actor
            FROM decisions d
            WHERE d.proposal_id = $1

            UNION ALL
            SELECT pc.created_at AS timestamp,
                   'comment' AS event_type,
                   CONCAT('comment: ', left(pc.content, 80)) AS description,
                   pc.author_id AS actor
            FROM proposal_comments pc
            WHERE pc.proposal_id = $1

            UNION ALL
            SELECT v.voted_at AS timestamp,
                   'vote' AS event_type,
                   CONCAT('vote ', v.choice::text) AS description,
                   v.voter_id AS actor
            FROM votes v
            WHERE v.proposal_id = $1

            UNION ALL
            SELECT pil.created_at AS timestamp,
                   'issue_linked' AS event_type,
                   CONCAT('issue linked: ', wi.id::text) AS description,
                   NULL::text AS actor
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON wi.id = pil.issue_id
            WHERE pil.proposal_id = $1

            UNION ALL
            SELECT ir.scheduled_at AS timestamp,
                   'review_scheduled' AS event_type,
                   'impact review scheduled' AS description,
                   ir.reviewer_id::text AS actor
            FROM impact_reviews ir
            WHERE ir.proposal_id = $1 AND ir.scheduled_at IS NOT NULL

            UNION ALL
            SELECT ir.conducted_at AS timestamp,
                   'review_completed' AS event_type,
                   CONCAT('impact review completed with rating ', COALESCE(ir.rating::text, 'N/A')) AS description,
                   ir.reviewer_id::text AS actor
            FROM impact_reviews ir
            WHERE ir.proposal_id = $1 AND ir.conducted_at IS NOT NULL

            UNION ALL
            SELECT ve.created_at AS timestamp,
                   'veto_event' AS event_type,
                   CONCAT('veto ', ve.status::text, ' in ', ve.domain) AS description,
                   ve.vetoer_id::text AS actor
            FROM veto_events ve
            WHERE ve.proposal_id = $1

            UNION ALL
            SELECT ve.escalation_started_at AS timestamp,
                   'veto_escalation_started' AS event_type,
                   'veto escalation started' AS description,
                   NULL::text AS actor
            FROM veto_events ve
            WHERE ve.proposal_id = $1 AND ve.escalation_started_at IS NOT NULL

            UNION ALL
            SELECT a.created_at AS timestamp,
                   'appeal_created' AS event_type,
                   CONCAT('appeal submitted: ', a.status::text) AS description,
                   a.appellant_id::text AS actor
            FROM appeals a
            INNER JOIN trust_score_logs tsl ON tsl.id = a.log_id
            INNER JOIN impact_reviews ir ON ir.id::text = tsl.event_id
            WHERE ir.proposal_id = $1

            UNION ALL
            SELECT a.resolved_at AS timestamp,
                   CONCAT('appeal_', a.status::text) AS event_type,
                   CONCAT('appeal ', a.status::text) AS description,
                   a.reviewer_id::text AS actor
            FROM appeals a
            INNER JOIN trust_score_logs tsl ON tsl.id = a.log_id
            INNER JOIN impact_reviews ir ON ir.id::text = tsl.event_id
            WHERE ir.proposal_id = $1 AND a.resolved_at IS NOT NULL

            UNION ALL
            SELECT tsl.created_at AS timestamp,
                   'trust_score_updated' AS event_type,
                   CONCAT('trust score delta ', tsl.score_change::text, ' for ', tsl.user_id::text) AS description,
                   tsl.user_id::text AS actor
            FROM trust_score_logs tsl
            INNER JOIN impact_reviews ir ON ir.id = tsl.event_id
            WHERE ir.proposal_id = $1 AND tsl.event_type = 'impact_review_completed'
        "#,
        vec![proposal_id.to_string().into()],
    ))
    .all(db)
    .await?;

    events.sort_by_key(|event| event.timestamp);
    Ok(events)
}

pub async fn get_decision_analytics(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<DecisionAnalyticsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;

    if let Some(project_id) = query.project_id {
        ensure_project_visible(&state, project_id, user_id).await?;
    } else if !is_system_admin(&state.db, user_id).await? {
        return Err(ApiError::Forbidden(
            "admin access required for global analytics".to_string(),
        ));
    }

    if let (Some(start_at), Some(end_at)) = (query.start_at, query.end_at) && start_at > end_at {
        return Err(ApiError::BadRequest("start_at must be <= end_at".to_string()));
    }

    let (filter_sql, values) = filter_clause(query.project_id, query.start_at, query.end_at);

    let overview_sql = format!(
        r#"
            WITH filtered_proposals AS (
                SELECT p.id, p.proposal_type::text AS proposal_type, p.created_at, p.domains, wi.project_id
                FROM proposals p
                INNER JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                GROUP BY p.id, p.proposal_type, p.created_at, p.domains, wi.project_id
            )
            SELECT
                COUNT(*)::bigint AS total_decisions,
                COUNT(*) FILTER (WHERE d.result = 'approved')::bigint AS approved_count,
                COUNT(*) FILTER (WHERE d.result = 'rejected')::bigint AS rejected_count,
                COUNT(*) FILTER (WHERE d.result = 'vetoed')::bigint AS vetoed_count,
                AVG(EXTRACT(EPOCH FROM (d.decided_at - fp.created_at)) / 3600.0)::double precision AS avg_cycle_hours
            FROM decisions d
            INNER JOIN filtered_proposals fp ON fp.id = d.proposal_id
            {filter_sql}
        "#
    );

    let overview = DecisionOverviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        overview_sql,
        values.clone(),
    ))
    .one(&state.db)
    .await?
    .unwrap_or(DecisionOverviewRow {
        total_decisions: 0,
        approved_count: 0,
        rejected_count: 0,
        vetoed_count: 0,
        avg_cycle_hours: None,
    });

    let by_type_sql = format!(
        r#"
            WITH filtered_proposals AS (
                SELECT p.id, p.proposal_type::text AS proposal_type, p.created_at, p.domains, wi.project_id
                FROM proposals p
                INNER JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                GROUP BY p.id, p.proposal_type, p.created_at, p.domains, wi.project_id
            )
            SELECT
                fp.proposal_type,
                COUNT(*)::bigint AS total_decisions,
                COUNT(*) FILTER (WHERE d.result = 'approved')::bigint AS approved_count,
                COUNT(*) FILTER (WHERE d.result = 'rejected')::bigint AS rejected_count,
                COUNT(*) FILTER (WHERE d.result = 'vetoed')::bigint AS vetoed_count,
                AVG(EXTRACT(EPOCH FROM (d.decided_at - fp.created_at)) / 3600.0)::double precision AS avg_cycle_hours
            FROM decisions d
            INNER JOIN filtered_proposals fp ON fp.id = d.proposal_id
            {filter_sql}
            GROUP BY fp.proposal_type
            ORDER BY total_decisions DESC, fp.proposal_type ASC
        "#
    );

    let by_type = DecisionTypeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        by_type_sql,
        values.clone(),
    ))
    .all(&state.db)
    .await?;

    let by_domain_sql = format!(
        r#"
            WITH filtered_proposals AS (
                SELECT p.id, p.proposal_type::text AS proposal_type, p.created_at, p.domains, wi.project_id
                FROM proposals p
                INNER JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                GROUP BY p.id, p.proposal_type, p.created_at, p.domains, wi.project_id
            ),
            expanded AS (
                SELECT
                    d.id AS decision_id,
                    d.result::text AS result,
                    d.decided_at,
                    fp.created_at,
                    COALESCE(NULLIF(trim(domain_elem), ''), 'global') AS domain
                FROM decisions d
                INNER JOIN filtered_proposals fp ON fp.id = d.proposal_id
                LEFT JOIN LATERAL jsonb_array_elements_text(
                    CASE
                        WHEN jsonb_typeof(fp.domains::jsonb) = 'array' THEN fp.domains::jsonb
                        ELSE '[]'::jsonb
                    END
                ) domain_list(domain_elem) ON TRUE
                {filter_sql}
            )
            SELECT
                domain,
                COUNT(*)::bigint AS total_decisions,
                COUNT(*) FILTER (WHERE result = 'approved')::bigint AS approved_count,
                COUNT(*) FILTER (WHERE result = 'rejected')::bigint AS rejected_count,
                COUNT(*) FILTER (WHERE result = 'vetoed')::bigint AS vetoed_count,
                AVG(EXTRACT(EPOCH FROM (decided_at - created_at)) / 3600.0)::double precision AS avg_cycle_hours
            FROM expanded
            GROUP BY domain
            ORDER BY total_decisions DESC, domain ASC
        "#
    );

    let by_domain = DecisionDomainRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        by_domain_sql,
        values,
    ))
    .all(&state.db)
    .await?;

    let pass_rate = if overview.total_decisions > 0 {
        (overview.approved_count as f64) / (overview.total_decisions as f64)
    } else {
        0.0
    };

    Ok(ApiResponse::success(json!({
        "project_id": query.project_id,
        "time_range": {
            "start_at": query.start_at.map(|v| v.to_rfc3339()),
            "end_at": query.end_at.map(|v| v.to_rfc3339())
        },
        "overview": {
            "total_decisions": overview.total_decisions,
            "approved_count": overview.approved_count,
            "rejected_count": overview.rejected_count,
            "vetoed_count": overview.vetoed_count,
            "pass_rate": pass_rate,
            "avg_cycle_hours": overview.avg_cycle_hours
        },
        "by_type": by_type.into_iter().map(|row| {
            let rate = if row.total_decisions > 0 {
                (row.approved_count as f64) / (row.total_decisions as f64)
            } else {
                0.0
            };
            json!({
                "proposal_type": row.proposal_type,
                "total_decisions": row.total_decisions,
                "approved_count": row.approved_count,
                "rejected_count": row.rejected_count,
                "vetoed_count": row.vetoed_count,
                "pass_rate": rate,
                "avg_cycle_hours": row.avg_cycle_hours
            })
        }).collect::<Vec<Value>>(),
        "by_domain": by_domain.into_iter().map(|row| {
            let rate = if row.total_decisions > 0 {
                (row.approved_count as f64) / (row.total_decisions as f64)
            } else {
                0.0
            };
            json!({
                "domain": row.domain,
                "total_decisions": row.total_decisions,
                "approved_count": row.approved_count,
                "rejected_count": row.rejected_count,
                "vetoed_count": row.vetoed_count,
                "pass_rate": rate,
                "avg_cycle_hours": row.avg_cycle_hours
            })
        }).collect::<Vec<Value>>()
    })))
}

pub async fn create_project_audit_report(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<GenerateAuditReportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let allowed = is_project_admin_or_owner(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden("admin or owner required".to_string()));
    }

    let (period_start, period_end) = resolve_period(req.period_start, req.period_end)?;
    let start_at = Utc.from_utc_datetime(
        &period_start
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ApiError::BadRequest("invalid period_start".to_string()))?,
    );
    let end_at = Utc.from_utc_datetime(
        &period_end
            .and_hms_opt(23, 59, 59)
            .ok_or_else(|| ApiError::BadRequest("invalid period_end".to_string()))?,
    );

    let summary = DecisionOverviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            WITH decision_scope AS (
                SELECT DISTINCT
                    d.id,
                    d.result,
                    d.decided_at,
                    p.created_at
                FROM decisions d
                INNER JOIN proposals p ON p.id = d.proposal_id
                INNER JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                WHERE wi.project_id = $1
                  AND d.decided_at >= $2
                  AND d.decided_at <= $3
            )
            SELECT
                COUNT(*)::bigint AS total_decisions,
                COUNT(*) FILTER (WHERE ds.result = 'approved')::bigint AS approved_count,
                COUNT(*) FILTER (WHERE ds.result = 'rejected')::bigint AS rejected_count,
                COUNT(*) FILTER (WHERE ds.result = 'vetoed')::bigint AS vetoed_count,
                AVG(EXTRACT(EPOCH FROM (ds.decided_at - ds.created_at)) / 3600.0)::double precision AS avg_cycle_hours
            FROM decision_scope ds
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .one(&state.db)
    .await?
    .unwrap_or(DecisionOverviewRow {
        total_decisions: 0,
        approved_count: 0,
        rejected_count: 0,
        vetoed_count: 0,
        avg_cycle_hours: None,
    });

    let rating = RatingAggRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                COUNT(*) FILTER (WHERE ir.status = 'completed' AND ir.rating IS NOT NULL)::bigint AS reviewed_proposals,
                AVG(
                    CASE ir.rating
                        WHEN 'S' THEN 1.0
                        WHEN 'A' THEN 0.9
                        WHEN 'B' THEN 0.75
                        WHEN 'C' THEN 0.5
                        WHEN 'F' THEN 0.2
                        ELSE NULL
                    END
                )::double precision AS avg_review_rating,
                json_build_object(
                    'S', COUNT(*) FILTER (WHERE ir.rating = 'S'),
                    'A', COUNT(*) FILTER (WHERE ir.rating = 'A'),
                    'B', COUNT(*) FILTER (WHERE ir.rating = 'B'),
                    'C', COUNT(*) FILTER (WHERE ir.rating = 'C'),
                    'F', COUNT(*) FILTER (WHERE ir.rating = 'F')
                ) AS rating_distribution
            FROM impact_reviews ir
            WHERE ir.project_id = $1
              AND ir.created_at >= $2
              AND ir.created_at <= $3
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .one(&state.db)
    .await?
    .unwrap_or(RatingAggRow {
        reviewed_proposals: 0,
        avg_review_rating: None,
        rating_distribution: json!({"S":0,"A":0,"B":0,"C":0,"F":0}),
    });

    let trust_distribution = Value::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT json_build_object(
                'observer', COUNT(*) FILTER (WHERE level = 'observer'),
                'advisor', COUNT(*) FILTER (WHERE level = 'advisor'),
                'voter', COUNT(*) FILTER (WHERE level = 'voter'),
                'vetoer', COUNT(*) FILTER (WHERE level = 'vetoer'),
                'autonomous', COUNT(*) FILTER (WHERE level = 'autonomous')
            ) AS value
            FROM trust_scores
            WHERE project_id = $1
        "#,
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .and_then(|v| v.get("value").cloned())
    .unwrap_or_else(|| json!({}));

    let veto_records = Value::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT json_agg(row_data) AS value
            FROM (
                SELECT json_build_object(
                    'id', ve.id,
                    'proposal_id', ve.proposal_id,
                    'vetoer_id', ve.vetoer_id,
                    'domain', ve.domain,
                    'status', ve.status::text,
                    'created_at', ve.created_at
                ) AS row_data
                FROM veto_events ve
                WHERE EXISTS (
                    SELECT 1
                    FROM proposal_issue_links pil
                    INNER JOIN work_items wi ON wi.id = pil.issue_id
                    WHERE pil.proposal_id = ve.proposal_id
                      AND wi.project_id = $1
                )
                  AND ve.created_at >= $2
                  AND ve.created_at <= $3
                ORDER BY ve.created_at DESC
                LIMIT 50
            ) t
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .one(&state.db)
    .await?
    .and_then(|v| v.get("value").cloned())
    .unwrap_or_else(|| json!([]));

    let domain_rows = DomainStatsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            WITH proposal_scope AS (
                SELECT DISTINCT p.id, p.domains, d.result::text AS decision_result
                FROM proposals p
                INNER JOIN decisions d ON d.proposal_id = p.id
                INNER JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                WHERE wi.project_id = $1
                  AND d.decided_at >= $2
                  AND d.decided_at <= $3
            ),
            exploded AS (
                SELECT
                    ps.id,
                    ps.decision_result,
                    COALESCE(NULLIF(trim(domain_elem), ''), 'global') AS domain
                FROM proposal_scope ps
                LEFT JOIN LATERAL jsonb_array_elements_text(
                    CASE
                        WHEN jsonb_typeof(ps.domains::jsonb) = 'array' THEN ps.domains::jsonb
                        ELSE '[]'::jsonb
                    END
                ) domains(domain_elem) ON TRUE
            ),
            review_score AS (
                SELECT
                    ir.proposal_id,
                    CASE ir.rating
                        WHEN 'S' THEN 1.0
                        WHEN 'A' THEN 0.9
                        WHEN 'B' THEN 0.75
                        WHEN 'C' THEN 0.5
                        WHEN 'F' THEN 0.2
                        ELSE NULL
                    END AS score
                FROM impact_reviews ir
                WHERE ir.project_id = $1
                  AND ir.created_at >= $2
                  AND ir.created_at <= $3
            )
            SELECT
                e.domain,
                COUNT(*)::bigint AS total_decisions,
                COUNT(*) FILTER (WHERE e.decision_result = 'approved')::bigint AS approved_count,
                COUNT(*) FILTER (WHERE e.decision_result = 'rejected')::bigint AS rejected_count,
                COUNT(*) FILTER (WHERE e.decision_result = 'vetoed')::bigint AS vetoed_count,
                AVG(rs.score)::double precision AS avg_review_score
            FROM exploded e
            LEFT JOIN review_score rs ON rs.proposal_id = e.id
            GROUP BY e.domain
            ORDER BY total_decisions DESC, e.domain ASC
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .all(&state.db)
    .await?;

    let domain_stats = Value::Array(
        domain_rows
            .into_iter()
            .map(|row| {
                json!({
                    "domain": row.domain,
                    "total_decisions": row.total_decisions,
                    "approved_count": row.approved_count,
                    "rejected_count": row.rejected_count,
                    "vetoed_count": row.vetoed_count,
                    "avg_review_score": row.avg_review_score
                })
            })
            .collect(),
    );

    let ai_participation_stats = Value::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT json_build_object(
                'votes', COUNT(DISTINCT v.id) FILTER (WHERE v.voter_type = 'ai'),
                'comments', (
                    SELECT COUNT(DISTINCT pc.id)
                    FROM proposal_comments pc
                    WHERE EXISTS (
                        SELECT 1
                        FROM proposal_issue_links pil
                        INNER JOIN work_items wi ON wi.id = pil.issue_id
                        WHERE pil.proposal_id = pc.proposal_id
                          AND wi.project_id = $1
                    )
                      AND pc.author_type = 'ai'
                      AND pc.created_at >= $2
                      AND pc.created_at <= $3
                ),
                'active_ai_participants', (
                    SELECT COUNT(*)
                    FROM ai_participants ap
                    WHERE ap.project_id = $1 AND ap.is_active = true
                )
            ) AS value
            FROM votes v
            WHERE EXISTS (
                SELECT 1
                FROM proposal_issue_links pil
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                WHERE pil.proposal_id = v.proposal_id
                  AND wi.project_id = $1
            )
              AND v.voted_at >= $2
              AND v.voted_at <= $3
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .one(&state.db)
    .await?
    .and_then(|v| v.get("value").cloned())
    .unwrap_or_else(|| json!({}));

    let top_contributors = Value::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COALESCE(json_agg(row_data), '[]'::json) AS value
            FROM (
                SELECT json_build_object(
                    'user_id', tsl.user_id,
                    'events', COUNT(*)::bigint,
                    'score_delta_sum', SUM(tsl.score_change)::bigint
                ) AS row_data
                FROM trust_score_logs tsl
                WHERE tsl.project_id = $1
                  AND tsl.created_at >= $2
                  AND tsl.created_at <= $3
                GROUP BY tsl.user_id
                ORDER BY ABS(SUM(tsl.score_change)) DESC, COUNT(*) DESC
                LIMIT 20
            ) t
        "#,
        vec![project_id.into(), start_at.into(), end_at.into()],
    ))
    .one(&state.db)
    .await?
    .and_then(|v| v.get("value").cloned())
    .unwrap_or_else(|| json!([]));

    let report_id = format!(
        "AUDIT-{}-{:02}{:02}-{}",
        period_start.year(),
        period_start.month(),
        period_start.day(),
        &Uuid::new_v4().simple().to_string()[..8]
    );

    let key_insights = json!({
        "decision_stats": {
            "total_decisions": summary.total_decisions,
            "approved_count": summary.approved_count,
            "rejected_count": summary.rejected_count,
            "vetoed_count": summary.vetoed_count,
            "pass_rate": if summary.total_decisions > 0 {
                (summary.approved_count as f64) / (summary.total_decisions as f64)
            } else {
                0.0
            },
            "avg_cycle_hours": summary.avg_cycle_hours
        },
        "trust_distribution": trust_distribution,
        "veto_records": veto_records,
        "impact_review_summary": {
            "reviewed_proposals": rating.reviewed_proposals,
            "avg_review_rating": rating.avg_review_rating,
            "rating_distribution": rating.rating_distribution
        }
    });

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO decision_audit_reports (
                    id, project_id, period_start, period_end,
                    total_proposals, approved_proposals, rejected_proposals, vetoed_proposals,
                    reviewed_proposals, avg_review_rating, rating_distribution,
                    top_contributors, domain_stats, ai_participation_stats, key_insights,
                    generated_at, generated_by
                ) VALUES (
                    $1, $2, $3, $4,
                    $5, $6, $7, $8,
                    $9, $10, $11,
                    $12, $13, $14, $15,
                    $16, $17
                )
            "#,
            vec![
                report_id.clone().into(),
                project_id.into(),
                period_start.into(),
                period_end.into(),
                (summary.total_decisions as i32).into(),
                (summary.approved_count as i32).into(),
                (summary.rejected_count as i32).into(),
                (summary.vetoed_count as i32).into(),
                (rating.reviewed_proposals as i32).into(),
                rating.avg_review_rating.into(),
                rating.rating_distribution.into(),
                Some(top_contributors).into(),
                Some(domain_stats).into(),
                Some(ai_participation_stats).into(),
                Some(key_insights).into(),
                Utc::now().into(),
                user_id.to_string().into(),
            ],
        ))
        .await?;

    get_project_audit_report(
        State(state),
        Extension(claims),
        Path((project_id, report_id)),
    )
    .await
}

fn resolve_period(
    period_start: Option<NaiveDate>,
    period_end: Option<NaiveDate>,
) -> Result<(NaiveDate, NaiveDate), ApiError> {
    match (period_start, period_end) {
        (Some(start), Some(end)) => {
            if start > end {
                return Err(ApiError::BadRequest(
                    "period_start must be <= period_end".to_string(),
                ));
            }
            Ok((start, end))
        }
        (Some(start), None) => Ok((start, start + Duration::days(30))),
        (None, Some(end)) => Ok((end - Duration::days(30), end)),
        (None, None) => {
            let now = Utc::now().date_naive();
            let start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .ok_or_else(|| ApiError::BadRequest("invalid date".to_string()))?;
            let next_month = if now.month() == 12 {
                NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1)
            }
            .ok_or_else(|| ApiError::BadRequest("invalid date".to_string()))?;
            Ok((start, next_month - Duration::days(1)))
        }
    }
}

pub async fn list_project_audit_reports(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListAuditReportsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_project_visible(&state, project_id, user_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*)::bigint AS count
            FROM decision_audit_reports
            WHERE project_id = $1
        "#,
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    let items = AuditReportRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id, project_id, period_start, period_end,
                total_proposals, approved_proposals, rejected_proposals, vetoed_proposals,
                reviewed_proposals, avg_review_rating, rating_distribution,
                top_contributors, domain_stats, ai_participation_stats, key_insights,
                generated_at, generated_by
            FROM decision_audit_reports
            WHERE project_id = $1
            ORDER BY generated_at DESC
            LIMIT $2 OFFSET $3
        "#,
        vec![project_id.into(), per_page.into(), offset.into()],
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

pub async fn get_project_audit_report(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, report_id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_project_visible(&state, project_id, user_id).await?;

    let report = AuditReportRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id, project_id, period_start, period_end,
                total_proposals, approved_proposals, rejected_proposals, vetoed_proposals,
                reviewed_proposals, avg_review_rating, rating_distribution,
                top_contributors, domain_stats, ai_participation_stats, key_insights,
                generated_at, generated_by
            FROM decision_audit_reports
            WHERE project_id = $1 AND id = $2
        "#,
        vec![project_id.into(), report_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("audit report not found".to_string()))?;

    Ok(ApiResponse::success(report))
}

async fn get_ai_participant_project_id(
    db: &impl ConnectionTrait,
    ai_participant_id: &str,
) -> Result<Uuid, ApiError> {
    let row = ProjectIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_id FROM ai_participants WHERE id = $1",
        vec![ai_participant_id.to_string().into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("ai participant not found".to_string()))?;
    Ok(row.project_id)
}

pub async fn get_ai_review_feedback(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(review_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;

    let review_project = ProjectIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_id FROM impact_reviews WHERE id = $1",
        vec![review_id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;

    ensure_project_visible(&state, review_project.project_id, user_id).await?;

    let items = AiLearningRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                id,
                ai_participant_id,
                review_id,
                proposal_id,
                domain,
                review_rating::text AS review_rating,
                ai_vote_choice,
                ai_vote_reason,
                outcome_alignment,
                lesson_learned,
                will_change,
                follow_up_improvement,
                created_at
            FROM ai_learning_records
            WHERE review_id = $1
            ORDER BY created_at ASC
        "#,
        vec![review_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(json!({ "items": items })))
}

pub async fn get_ai_participant_learning(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(ai_participant_id): Path<String>,
    Query(query): Query<AiLearningQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let project_id = get_ai_participant_project_id(&state.db, &ai_participant_id).await?;
    ensure_project_visible(&state, project_id, user_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let mut where_parts = vec!["ai_participant_id = $1".to_string()];
    let mut values = vec![ai_participant_id.clone().into()];
    let mut idx = 2;
    if let Some(domain) = query.domain.as_deref() {
        let domain = domain.trim().to_ascii_lowercase();
        if !domain.is_empty() {
            where_parts.push(format!("domain = ${idx}"));
            values.push(domain.into());
            idx += 1;
        }
    }
    let where_sql = format!("WHERE {}", where_parts.join(" AND "));

    let count_sql = format!("SELECT COUNT(*)::bigint AS count FROM ai_learning_records {where_sql}");
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        count_sql,
        values.clone(),
    ))
    .one(&state.db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    let sql = format!(
        r#"
            SELECT
                id,
                ai_participant_id,
                review_id,
                proposal_id,
                domain,
                review_rating::text AS review_rating,
                ai_vote_choice,
                ai_vote_reason,
                outcome_alignment,
                lesson_learned,
                will_change,
                follow_up_improvement,
                created_at
            FROM ai_learning_records
            {where_sql}
            ORDER BY created_at DESC
            LIMIT ${idx} OFFSET ${}
        "#,
        idx + 1
    );
    values.push(per_page.into());
    values.push(offset.into());

    let items = AiLearningRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
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

pub async fn get_ai_participant_alignment_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(ai_participant_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let project_id = get_ai_participant_project_id(&state.db, &ai_participant_id).await?;
    ensure_project_visible(&state, project_id, user_id).await?;

    let stats = AlignmentStatsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            WITH ordered AS (
                SELECT
                    outcome_alignment,
                    created_at,
                    ROW_NUMBER() OVER (ORDER BY created_at DESC) AS rn
                FROM ai_learning_records
                WHERE ai_participant_id = $1
            )
            SELECT
                COUNT(*)::bigint AS total,
                COUNT(*) FILTER (WHERE outcome_alignment = 'aligned')::bigint AS aligned,
                COUNT(*) FILTER (WHERE outcome_alignment = 'misaligned')::bigint AS misaligned,
                COUNT(*) FILTER (WHERE outcome_alignment = 'neutral')::bigint AS neutral,
                COUNT(*) FILTER (WHERE rn <= 5)::bigint AS recent_total,
                COUNT(*) FILTER (WHERE rn <= 5 AND outcome_alignment = 'aligned')::bigint AS recent_aligned
            FROM ordered
        "#,
        vec![ai_participant_id.clone().into()],
    ))
    .one(&state.db)
    .await?
    .unwrap_or(AlignmentStatsRow {
        total: 0,
        aligned: 0,
        misaligned: 0,
        neutral: 0,
        recent_total: 0,
        recent_aligned: 0,
    });

    let overall_alignment_rate = if stats.total > 0 {
        (stats.aligned as f64) / (stats.total as f64)
    } else {
        0.0
    };
    let recent_alignment_rate = if stats.recent_total > 0 {
        (stats.recent_aligned as f64) / (stats.recent_total as f64)
    } else {
        0.0
    };

    Ok(ApiResponse::success(json!({
        "ai_participant_id": ai_participant_id,
        "total": stats.total,
        "aligned": stats.aligned,
        "misaligned": stats.misaligned,
        "neutral": stats.neutral,
        "overall_alignment_rate": overall_alignment_rate,
        "recent_alignment_rate": recent_alignment_rate,
        "improvement_trend": recent_alignment_rate - overall_alignment_rate
    })))
}
