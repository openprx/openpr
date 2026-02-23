use chrono::{Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    services::impact_review_service::ImpactReviewService,
    services::permission_service::PermissionService,
    services::trust_score_service::normalize_domain_key,
};

#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct VetoEventRow {
    pub id: i64,
    pub proposal_id: String,
    pub vetoer_id: Uuid,
    pub domain: String,
    pub reason: String,
    pub status: String,
    pub escalation_started_at: Option<chrono::DateTime<Utc>>,
    pub escalation_result: Option<String>,
    pub escalation_votes: Option<Value>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ProposalStateRow {
    status: String,
    domains: Value,
}

#[derive(Debug, FromQueryResult)]
struct VetoerCountRow {
    count: i64,
}

#[derive(Debug, FromQueryResult)]
struct ProposalTimingRow {
    voting_started_at: Option<chrono::DateTime<Utc>>,
    voting_ended_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct HumanVoteConsensusRow {
    total_human_votes: i64,
    distinct_choices: i64,
}

pub struct VetoService {
    db: DatabaseConnection,
    permission_service: PermissionService,
}

impl VetoService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            permission_service: PermissionService::new(db.clone()),
            db,
        }
    }

    pub async fn exercise_veto(
        &self,
        proposal_id: &str,
        vetoer_id: Uuid,
        reason: &str,
        domain: Option<&str>,
        voter_type: ParticipantType,
    ) -> Result<VetoEventRow, ApiError> {
        let tx = self.db.begin().await?;

        let proposal = ProposalStateRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status::text AS status, domains FROM proposals WHERE id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;

        if proposal.status != "open" && proposal.status != "voting" {
            return Err(ApiError::BadRequest(
                "proposal must be open or voting to veto".to_string(),
            ));
        }

        if reason.trim().chars().count() < 100 {
            return Err(ApiError::BadRequest(
                "veto reason must be at least 100 characters".to_string(),
            ));
        }

        let normalized_domain = select_veto_domain(&proposal.domains, domain);
        let project_id = resolve_project_id_for_proposal(&tx, proposal_id).await?;
        let Some(project_id) = project_id else {
            return Err(ApiError::BadRequest("proposal project not found".to_string()));
        };

        let can_veto = self
            .permission_service
            .can_veto(vetoer_id, project_id, &normalized_domain, voter_type)
            .await?;
        if !can_veto {
            return Err(ApiError::Forbidden(
                "no veto permission in this domain".to_string(),
            ));
        }

        let has_vetoer_cache = self
            .check_veto_eligibility(&tx, vetoer_id, project_id, &normalized_domain)
            .await?;
        if !has_vetoer_cache {
            return Err(ApiError::Forbidden("vetoer record not found".to_string()));
        }

        if voter_type == ParticipantType::Ai {
            let can_veto_human_consensus = self
                .permission_service
                .ai_can_veto_human_consensus(vetoer_id, project_id)
                .await?;
            if !can_veto_human_consensus
                && self.has_human_vote_consensus(&tx, proposal_id).await?
            {
                return Err(ApiError::Forbidden(
                    "ai veto is blocked because all human votes are in consensus".to_string(),
                ));
            }
        }

        let inserted = VetoEventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO veto_events (
                    proposal_id, vetoer_id, domain, reason, status, created_at
                ) VALUES ($1, $2, $3, $4, 'active'::veto_status, $5)
                RETURNING id, proposal_id, vetoer_id, domain, reason,
                          status::text AS status, escalation_started_at,
                          escalation_result, escalation_votes, created_at
            "#,
            vec![
                proposal_id.to_string().into(),
                vetoer_id.into(),
                normalized_domain.clone().into(),
                reason.trim().to_string().into(),
                Utc::now().into(),
            ],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE proposals SET status = 'vetoed'::proposal_status WHERE id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .await?;

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO decisions (
                    id, proposal_id, result, total_votes, yes_votes, no_votes, abstain_votes,
                    weighted_yes, weighted_no, weighted_approval_rate, is_weighted, veto_event_id, decided_at
                ) VALUES ($1, $2, 'vetoed'::decision_result, 0, 0, 0, 0, 0, 0, NULL, true, $3, $4)
                ON CONFLICT (proposal_id) DO UPDATE
                SET result = 'vetoed'::decision_result,
                    veto_event_id = EXCLUDED.veto_event_id,
                    decided_at = EXCLUDED.decided_at
            "#,
            vec![
                format!("DEC-{}", Uuid::new_v4()).into(),
                proposal_id.to_string().into(),
                inserted.id.into(),
                Utc::now().into(),
            ],
        ))
        .await?;

        tx.commit().await?;
        Ok(inserted)
    }

    pub async fn start_escalation(
        &self,
        proposal_id: &str,
        initiator_id: Uuid,
    ) -> Result<VetoEventRow, ApiError> {
        let tx = self.db.begin().await?;
        let veto = self.get_active_veto_by_proposal_with_conn(&tx, proposal_id).await?;
        let Some(veto) = veto else {
            return Err(ApiError::NotFound("active veto not found".to_string()));
        };

        let proposal_author = proposal_author_with_conn(&tx, proposal_id).await?;
        let Some(author_id) = proposal_author else {
            return Err(ApiError::NotFound("proposal author not found".to_string()));
        };
        if author_id != initiator_id {
            return Err(ApiError::Forbidden(
                "only proposal author can start escalation".to_string(),
            ));
        }

        if veto.status != "active" {
            return Err(ApiError::BadRequest(
                "only active veto can be escalated".to_string(),
            ));
        }

        if veto.escalation_started_at.is_some() {
            return Err(ApiError::Conflict("escalation already started".to_string()));
        }
        if Utc::now() > veto.created_at + Duration::hours(48) {
            return Err(ApiError::BadRequest(
                "escalation window has expired (48h after veto)".to_string(),
            ));
        }

        let updated = VetoEventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE veto_events
                SET escalation_started_at = $2
                WHERE id = $1
                RETURNING id, proposal_id, vetoer_id, domain, reason,
                          status::text AS status, escalation_started_at,
                          escalation_result, escalation_votes, created_at
            "#,
            vec![veto.id.into(), Utc::now().into()],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;

        tx.commit().await?;
        Ok(updated)
    }

    pub async fn cast_escalation_vote(
        &self,
        proposal_id: &str,
        voter_id: Uuid,
        overturn: bool,
    ) -> Result<VetoEventRow, ApiError> {
        let tx = self.db.begin().await?;

        let veto = self
            .get_active_veto_by_proposal_with_conn(&tx, proposal_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("active veto not found".to_string()))?;

        if veto.escalation_started_at.is_none() {
            return Err(ApiError::BadRequest(
                "escalation has not started".to_string(),
            ));
        }
        let escalation_started_at = veto.escalation_started_at.ok_or(ApiError::Internal)?;
        if Utc::now() > escalation_started_at + Duration::hours(48) {
            return Err(ApiError::BadRequest(
                "escalation voting window has expired (48h after escalation start)".to_string(),
            ));
        }

        let project_id = resolve_project_id_for_proposal(&tx, proposal_id).await?;
        let Some(project_id) = project_id else {
            return Err(ApiError::BadRequest("proposal project not found".to_string()));
        };

        let is_vetoer = self
            .check_veto_eligibility(&tx, voter_id, project_id, &veto.domain)
            .await?;
        if !is_vetoer {
            return Err(ApiError::Forbidden("vetoer required".to_string()));
        }

        let mut votes_json = veto
            .escalation_votes
            .clone()
            .unwrap_or_else(|| json!({ "ballots": {}, "overturned": 0, "upheld": 0 }));

        let ballots = votes_json
            .get_mut("ballots")
            .and_then(Value::as_object_mut)
            .ok_or(ApiError::Internal)?;
        ballots.insert(voter_id.to_string(), Value::Bool(overturn));

        let overturn_count = ballots.values().filter(|v| v.as_bool() == Some(true)).count() as i64;
        let uphold_count = ballots.values().filter(|v| v.as_bool() == Some(false)).count() as i64;
        votes_json["overturned"] = Value::from(overturn_count);
        votes_json["upheld"] = Value::from(uphold_count);

        let total_vetoers = self.count_domain_vetoers(&tx, project_id, &veto.domain).await?;
        let threshold = ((total_vetoers as f64) * (2.0 / 3.0)).ceil() as i64;

        let mut status = veto.status.clone();
        let mut escalation_result: Option<String> = None;
        if overturn_count >= threshold && total_vetoers > 0 {
            status = "overturned".to_string();
            escalation_result = Some("overturned".to_string());
            let resumed_status = proposal_status_after_veto_release(&tx, proposal_id).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE proposals SET status = $2::proposal_status WHERE id = $1",
                vec![proposal_id.to_string().into(), resumed_status.into()],
            ))
            .await?;
            sync_decision_after_veto_release(&tx, proposal_id).await?;
            schedule_review_if_decision_approved_with_conn(&self.db, &tx, proposal_id).await?;
        } else if (overturn_count + uphold_count) >= total_vetoers && total_vetoers > 0 {
            status = "upheld".to_string();
            escalation_result = Some("upheld".to_string());
        }

        let updated = VetoEventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE veto_events
                SET status = $2::veto_status,
                    escalation_result = $3,
                    escalation_votes = $4
                WHERE id = $1
                RETURNING id, proposal_id, vetoer_id, domain, reason,
                          status::text AS status, escalation_started_at,
                          escalation_result, escalation_votes, created_at
            "#,
            vec![veto.id.into(), status.into(), escalation_result.into(), votes_json.into()],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;

        tx.commit().await?;
        Ok(updated)
    }

    pub async fn withdraw_veto(
        &self,
        proposal_id: &str,
        requester_id: Uuid,
    ) -> Result<VetoEventRow, ApiError> {
        let tx = self.db.begin().await?;
        let veto = self
            .get_active_veto_by_proposal_with_conn(&tx, proposal_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("active veto not found".to_string()))?;

        if veto.status != "active" {
            return Err(ApiError::BadRequest(
                "only active veto can be withdrawn".to_string(),
            ));
        }
        if veto.vetoer_id != requester_id {
            return Err(ApiError::Forbidden(
                "only original vetoer can withdraw veto".to_string(),
            ));
        }

        let timing = proposal_timing_by_id(&tx, proposal_id).await?;
        if timing
            .voting_ended_at
            .map(|ended_at| ended_at <= Utc::now())
            .unwrap_or(false)
        {
            return Err(ApiError::BadRequest(
                "voting period has ended, veto cannot be withdrawn".to_string(),
            ));
        }

        let resumed_status = proposal_status_after_veto_release(&tx, proposal_id).await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE proposals SET status = $2::proposal_status WHERE id = $1",
            vec![proposal_id.to_string().into(), resumed_status.into()],
        ))
        .await?;
        sync_decision_after_veto_release(&tx, proposal_id).await?;
        schedule_review_if_decision_approved_with_conn(&self.db, &tx, proposal_id).await?;

        let updated = VetoEventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE veto_events
                SET status = 'withdrawn'::veto_status,
                    escalation_result = 'withdrawn'
                WHERE id = $1
                RETURNING id, proposal_id, vetoer_id, domain, reason,
                          status::text AS status, escalation_started_at,
                          escalation_result, escalation_votes, created_at
            "#,
            vec![veto.id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;

        tx.commit().await?;
        Ok(updated)
    }

    pub async fn get_active_veto_by_proposal(
        &self,
        proposal_id: &str,
    ) -> Result<Option<VetoEventRow>, ApiError> {
        self.get_active_veto_by_proposal_with_conn(&self.db, proposal_id)
            .await
    }

    async fn get_active_veto_by_proposal_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        proposal_id: &str,
    ) -> Result<Option<VetoEventRow>, ApiError> {
        VetoEventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, proposal_id, vetoer_id, domain, reason,
                       status::text AS status, escalation_started_at,
                       escalation_result, escalation_votes, created_at
                FROM veto_events
                WHERE proposal_id = $1
                ORDER BY created_at DESC
                LIMIT 1
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .one(db)
        .await
        .map_err(Into::into)
    }

    pub async fn check_veto_eligibility<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
    ) -> Result<bool, ApiError> {
        let count = VetoerCountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT COUNT(*)::bigint AS count
                FROM vetoers
                WHERE user_id = $1
                  AND project_id = $2
                  AND domain = $3
            "#,
            vec![user_id.into(), project_id.into(), domain.to_string().into()],
        ))
        .one(db)
        .await?
        .map(|row| row.count)
        .unwrap_or(0);

        Ok(count > 0)
    }

    async fn count_domain_vetoers<C: ConnectionTrait>(
        &self,
        db: &C,
        project_id: Uuid,
        domain: &str,
    ) -> Result<i64, ApiError> {
        let count = VetoerCountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT COUNT(*)::bigint AS count
                FROM vetoers
                WHERE project_id = $1
                  AND domain = $2
            "#,
            vec![project_id.into(), domain.to_string().into()],
        ))
        .one(db)
        .await?
        .map(|row| row.count)
        .unwrap_or(0);

        Ok(count)
    }

    async fn has_human_vote_consensus<C: ConnectionTrait>(
        &self,
        db: &C,
        proposal_id: &str,
    ) -> Result<bool, ApiError> {
        let row = HumanVoteConsensusRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    COUNT(*)::bigint AS total_human_votes,
                    COUNT(DISTINCT choice)::bigint AS distinct_choices
                FROM votes
                WHERE proposal_id = $1
                  AND voter_type = 'human'::author_type
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .one(db)
        .await?;

        let consensus = row
            .map(|r| r.total_human_votes > 0 && r.distinct_choices == 1)
            .unwrap_or(false);
        Ok(consensus)
    }
}

async fn proposal_author_with_conn<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ProposalAuthorRow {
        author_id: String,
    }

    let row = ProposalAuthorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT author_id FROM proposals WHERE id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .one(db)
    .await?;

    Ok(row.and_then(|item| Uuid::parse_str(&item.author_id).ok()))
}

fn select_veto_domain(domains: &Value, requested_domain: Option<&str>) -> String {
    if let Some(domain) = requested_domain {
        let normalized = normalize_domain_key(domain);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    if let Some(first_domain) = domains
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item.as_str()))
    {
        let normalized = normalize_domain_key(first_domain);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    "global".to_string()
}

async fn resolve_project_id_for_proposal<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ProjectRow {
        project_id: Uuid,
    }

    let direct = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id
            FROM proposal_issue_links pil
            INNER JOIN work_items wi ON wi.id = pil.issue_id
            WHERE pil.proposal_id = $1
            LIMIT 1
        "#,
        vec![proposal_id.to_string().into()],
    ))
    .one(db)
    .await?;

    if let Some(row) = direct {
        return Ok(Some(row.project_id));
    }

    #[derive(Debug, FromQueryResult)]
    struct AuthorRow {
        author_id: String,
    }

    let author = AuthorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT author_id FROM proposals WHERE id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .one(db)
    .await?;

    let Some(author) = author else {
        return Ok(None);
    };

    let Ok(author_id) = Uuid::parse_str(&author.author_id) else {
        return Ok(None);
    };

    // Risk note: this fallback infers project by author membership.
    // It is only safe when exactly one project matches; otherwise we return None to avoid misrouting.
    let fallback = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.id AS project_id
            FROM projects p
            INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id
            WHERE wm.user_id = $1
            ORDER BY p.created_at DESC
            LIMIT 2
        "#,
        vec![author_id.into()],
    ))
    .all(db)
    .await?;

    if fallback.len() == 1 {
        Ok(Some(fallback[0].project_id))
    } else {
        Ok(None)
    }
}

async fn proposal_timing_by_id<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<ProposalTimingRow, ApiError> {
    ProposalTimingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT voting_started_at, voting_ended_at
            FROM proposals
            WHERE id = $1
        "#,
        vec![proposal_id.to_string().into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))
}

async fn proposal_status_after_veto_release<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<String, ApiError> {
    let timing = proposal_timing_by_id(db, proposal_id).await?;
    let now = Utc::now();
    let in_voting_window = timing.voting_started_at.is_some()
        && timing
            .voting_ended_at
            .map(|voting_ended_at| voting_ended_at > now)
            .unwrap_or(true);
    if in_voting_window {
        Ok("voting".to_string())
    } else {
        Ok("open".to_string())
    }
}

async fn sync_decision_after_veto_release<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            UPDATE decisions d
            SET result = CASE
                    WHEN tally.weighted_yes > tally.weighted_no THEN 'approved'::decision_result
                    ELSE 'rejected'::decision_result
                END,
                total_votes = tally.total_votes,
                yes_votes = tally.yes_votes,
                no_votes = tally.no_votes,
                abstain_votes = tally.abstain_votes,
                weighted_yes = tally.weighted_yes,
                weighted_no = tally.weighted_no,
                weighted_approval_rate = CASE
                    WHEN (tally.weighted_yes + tally.weighted_no) > 0
                    THEN tally.weighted_yes / (tally.weighted_yes + tally.weighted_no)
                    ELSE 0
                END,
                veto_event_id = NULL,
                decided_at = $2
            FROM (
                SELECT
                    COUNT(*)::integer AS total_votes,
                    COUNT(*) FILTER (WHERE choice = 'yes')::integer AS yes_votes,
                    COUNT(*) FILTER (WHERE choice = 'no')::integer AS no_votes,
                    COUNT(*) FILTER (WHERE choice = 'abstain')::integer AS abstain_votes,
                    COALESCE(SUM(CASE WHEN choice = 'yes' THEN weight ELSE 0 END), 0) AS weighted_yes,
                    COALESCE(SUM(CASE WHEN choice = 'no' THEN weight ELSE 0 END), 0) AS weighted_no
                FROM votes
                WHERE proposal_id = $1
            ) AS tally
            WHERE d.proposal_id = $1
        "#,
        vec![proposal_id.to_string().into(), Utc::now().into()],
    ))
    .await?;
    Ok(())
}

async fn schedule_review_if_decision_approved_with_conn<C: ConnectionTrait>(
    raw_db: &DatabaseConnection,
    db: &C,
    proposal_id: &str,
) -> Result<(), ApiError> {
    let approved = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM decisions WHERE proposal_id = $1 AND result = 'approved'::decision_result",
            vec![proposal_id.to_string().into()],
        ))
        .await?
        .is_some();

    if approved {
        let svc = ImpactReviewService::new(raw_db.clone());
        svc.schedule_review_with_conn(db, proposal_id, true).await?;
    }
    Ok(())
}
