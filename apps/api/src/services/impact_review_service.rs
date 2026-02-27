use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DbBackend, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder, Statement,
    TransactionTrait,
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    entities::{
        ai_learning_record, governance_config,
        impact_review::{self, ReviewRating, ReviewStatus},
        proposal, review_participant,
    },
    error::ApiError,
    services::trust_score_service::{
        TrustScoreService, normalize_domain_key, parse_domains, parse_participant_type,
    },
};

pub struct ImpactReviewService {
    db: DatabaseConnection,
    trust_svc: TrustScoreService,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewSummary {
    pub review_id: String,
    pub participants_count: i64,
    pub feedback_submitted_count: i64,
    pub trust_delta_total: i64,
    pub trust_delta_avg: f64,
}

#[derive(Debug, Clone, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Clone, FromQueryResult)]
struct SummaryRow {
    participants_count: i64,
    feedback_submitted_count: i64,
    trust_delta_total: i64,
    trust_delta_avg: f64,
}

#[derive(Debug, Clone, FromQueryResult)]
struct TimestampRow {
    value: chrono::DateTime<chrono::Utc>,
}

impl ImpactReviewService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            trust_svc: TrustScoreService::new(db.clone()),
            db,
        }
    }

    fn generate_review_id() -> String {
        format!("REV-{}", &Uuid::new_v4().simple().to_string()[..10])
    }

    pub async fn get_by_proposal(
        &self,
        proposal_id: &str,
    ) -> Result<Option<impact_review::Model>, ApiError> {
        impact_review::Entity::find()
            .filter(impact_review::Column::ProposalId.eq(proposal_id))
            .one(&self.db)
            .await
            .map_err(Into::into)
    }

    pub async fn get_participants(
        &self,
        review_id: &str,
    ) -> Result<Vec<review_participant::Model>, ApiError> {
        review_participant::Entity::find()
            .filter(review_participant::Column::ReviewId.eq(review_id))
            .all(&self.db)
            .await
            .map_err(Into::into)
    }

    pub async fn summarize(&self, review_id: &str) -> Result<ReviewSummary, ApiError> {
        let row = SummaryRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    COUNT(*)::bigint AS participants_count,
                    COUNT(*) FILTER (WHERE feedback_submitted = true)::bigint AS feedback_submitted_count,
                    COALESCE(SUM(trust_score_change), 0)::bigint AS trust_delta_total,
                    COALESCE(AVG(COALESCE(trust_score_change, 0)), 0)::double precision AS trust_delta_avg
                FROM review_participants
                WHERE review_id = $1
            "#,
            vec![review_id.to_string().into()],
        ))
        .one(&self.db)
        .await?
        .unwrap_or(SummaryRow {
            participants_count: 0,
            feedback_submitted_count: 0,
            trust_delta_total: 0,
            trust_delta_avg: 0.0,
        });

        Ok(ReviewSummary {
            review_id: review_id.to_string(),
            participants_count: row.participants_count,
            feedback_submitted_count: row.feedback_submitted_count,
            trust_delta_total: row.trust_delta_total,
            trust_delta_avg: row.trust_delta_avg,
        })
    }

    pub async fn schedule_review(
        &self,
        proposal_id: &str,
    ) -> Result<impact_review::Model, ApiError> {
        let tx = self.db.begin().await?;
        let review = self
            .schedule_review_with_conn(&tx, proposal_id, true)
            .await?;
        tx.commit().await?;
        Ok(review)
    }

    pub async fn create_review(
        &self,
        proposal_id: &str,
        reviewer_id: Option<Uuid>,
        scheduled_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<impact_review::Model, ApiError> {
        let tx = self.db.begin().await?;
        let mut review = self
            .schedule_review_with_conn(&tx, proposal_id, false)
            .await?;
        if reviewer_id.is_some() || scheduled_at.is_some() {
            let mut active: impact_review::ActiveModel = review.clone().into();
            if let Some(reviewer_id) = reviewer_id {
                active.reviewer_id = Set(Some(reviewer_id));
            }
            if let Some(scheduled_at) = scheduled_at {
                active.scheduled_at = Set(Some(scheduled_at.into()));
            }
            review = active.update(&tx).await?;
        }
        tx.commit().await?;
        Ok(review)
    }

    pub async fn get_project_id_for_review(&self, review_id: &str) -> Result<Uuid, ApiError> {
        let review = impact_review::Entity::find_by_id(review_id)
            .one(&self.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;
        Ok(review.project_id)
    }

    pub async fn get_project_id_for_proposal(&self, proposal_id: &str) -> Result<Uuid, ApiError> {
        let proposal = proposal::Entity::find_by_id(proposal_id)
            .one(&self.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;
        resolve_project_id_for_proposal(&self.db, proposal_id, &proposal.author_id).await
    }

    pub async fn schedule_review_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        proposal_id: &str,
        is_auto_triggered: bool,
    ) -> Result<impact_review::Model, ApiError> {
        let existing = impact_review::Entity::find()
            .filter(impact_review::Column::ProposalId.eq(proposal_id))
            .one(db)
            .await?;
        if let Some(existing) = existing {
            return Ok(existing);
        }

        let proposal = proposal::Entity::find_by_id(proposal_id)
            .one(db)
            .await?
            .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;

        if proposal.status != proposal::ProposalStatus::Approved {
            return Err(ApiError::BadRequest(
                "impact review can only be created for approved proposal".to_string(),
            ));
        }

        let scheduled_at = Some(
            self.compute_scheduled_at_with_conn(db, &proposal.id, proposal.voting_ended_at)
                .await?
                .into(),
        );

        let review = impact_review::ActiveModel {
            id: Set(Self::generate_review_id()),
            proposal_id: Set(proposal.id.clone()),
            project_id: Set(
                resolve_project_id_for_proposal(db, &proposal.id, &proposal.author_id).await?,
            ),
            status: Set(ReviewStatus::Pending),
            is_auto_triggered: Set(is_auto_triggered),
            scheduled_at: Set(scheduled_at),
            created_at: Set(Utc::now().into()),
            ..Default::default()
        }
        .insert(db)
        .await?;

        self.populate_participants(db, &review.id, &proposal.id)
            .await?;
        Ok(review)
    }

    async fn compute_scheduled_at_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        proposal_id: &str,
        voting_ended_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<chrono::DateTime<chrono::Utc>, ApiError> {
        #[derive(Debug, Clone, FromQueryResult)]
        struct ProjectIdRow {
            project_id: Uuid,
        }

        let project = ProjectIdRow::find_by_statement(Statement::from_sql_and_values(
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

        let auto_review_days = if let Some(project) = project {
            governance_config::Entity::find()
                .filter(governance_config::Column::ProjectId.eq(project.project_id))
                .one(db)
                .await?
                .map(|cfg| cfg.auto_review_days.max(1))
                .unwrap_or(30)
        } else {
            30
        };

        let from_voting = voting_ended_at
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(Utc::now)
            + Duration::days(auto_review_days as i64);
        let all_closed_at = TimestampRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT MAX(wi.updated_at) AS value
                FROM proposal_issue_links pil
                INNER JOIN work_items wi ON wi.id = pil.issue_id
                WHERE pil.proposal_id = $1
                GROUP BY pil.proposal_id
                HAVING COUNT(*) > 0
                   AND COUNT(*) FILTER (WHERE wi.state <> 'done') = 0
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .one(db)
        .await?;

        let from_issues = all_closed_at.map(|row| row.value + Duration::days(7));
        Ok(from_issues
            .map(|time| time.min(from_voting))
            .unwrap_or(from_voting))
    }

    async fn populate_participants<C: ConnectionTrait>(
        &self,
        db: &C,
        review_id: &str,
        proposal_id: &str,
    ) -> Result<(), ApiError> {
        #[derive(Debug, FromQueryResult)]
        struct ProposalActorRow {
            author_id: String,
        }
        #[derive(Debug, FromQueryResult)]
        struct VoteActorRow {
            voter_id: String,
            choice: String,
        }
        #[derive(Debug, FromQueryResult)]
        struct VetoActorRow {
            vetoer_id: String,
            status: String,
        }

        let proposal = ProposalActorRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT author_id FROM proposals WHERE id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .one(db)
        .await?
        .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO review_participants (
                    review_id, user_id, role, feedback_submitted, exercised_veto, veto_overturned
                ) VALUES ($1, $2, 'proposer', false, false, false)
                ON CONFLICT (review_id, user_id) DO NOTHING
            "#,
            vec![review_id.to_string().into(), proposal.author_id.into()],
        ))
        .await?;

        let votes = VoteActorRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT voter_id, choice::text AS choice
                FROM votes
                WHERE proposal_id = $1
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .all(db)
        .await?;

        for vote in votes {
            let role = match vote.choice.as_str() {
                "yes" => "voter_yes",
                "no" => "voter_no",
                _ => "voter_abstain",
            };

            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO review_participants (
                        review_id, user_id, role, vote_choice, feedback_submitted, exercised_veto, veto_overturned
                    ) VALUES ($1, $2, $3, $4, false, false, false)
                    ON CONFLICT (review_id, user_id)
                    DO UPDATE SET role = EXCLUDED.role, vote_choice = EXCLUDED.vote_choice
                "#,
                vec![
                    review_id.to_string().into(),
                    vote.voter_id.into(),
                    role.into(),
                    Some(vote.choice).into(),
                ],
            ))
            .await?;
        }

        let vetoes = VetoActorRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT vetoer_id::text AS vetoer_id, status::text AS status
                FROM veto_events
                WHERE proposal_id = $1
                ORDER BY created_at DESC
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .all(db)
        .await?;

        for veto in vetoes {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO review_participants (
                        review_id, user_id, role, exercised_veto, veto_overturned, feedback_submitted
                    ) VALUES ($1, $2, 'vetoer', true, $3, false)
                    ON CONFLICT (review_id, user_id)
                    DO UPDATE SET
                        role = 'vetoer',
                        exercised_veto = true,
                        veto_overturned = EXCLUDED.veto_overturned
                "#,
                vec![
                    review_id.to_string().into(),
                    veto.vetoer_id.into(),
                    (veto.status == "overturned").into(),
                ],
            ))
            .await?;
        }

        Ok(())
    }

    pub async fn upsert_participant(
        &self,
        review_id: &str,
        user_id: String,
        role: &str,
        vote_choice: Option<String>,
        exercised_veto: bool,
        veto_overturned: bool,
    ) -> Result<review_participant::Model, ApiError> {
        review_participant::Entity::find()
            .filter(review_participant::Column::ReviewId.eq(review_id))
            .one(&self.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("review not found".to_string()))?;

        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            return Err(ApiError::BadRequest("user_id is required".to_string()));
        }

        let role = role.trim().to_string();
        if role.is_empty() {
            return Err(ApiError::BadRequest("role is required".to_string()));
        }

        let normalized_choice = vote_choice
            .as_deref()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());
        if let Some(choice) = normalized_choice.as_deref()
            && !matches!(choice, "yes" | "no" | "abstain")
        {
            return Err(ApiError::BadRequest("invalid vote_choice".to_string()));
        }

        let model = review_participant::Entity::find()
            .filter(review_participant::Column::ReviewId.eq(review_id))
            .filter(review_participant::Column::UserId.eq(user_id.clone()))
            .one(&self.db)
            .await?;

        let upserted = if let Some(model) = model {
            let mut active: review_participant::ActiveModel = model.into();
            active.role = Set(role);
            active.vote_choice = Set(normalized_choice);
            active.exercised_veto = Set(exercised_veto);
            active.veto_overturned = Set(veto_overturned);
            active.update(&self.db).await?
        } else {
            review_participant::ActiveModel {
                review_id: Set(review_id.to_string()),
                user_id: Set(user_id),
                role: Set(role),
                vote_choice: Set(normalized_choice),
                exercised_veto: Set(exercised_veto),
                veto_overturned: Set(veto_overturned),
                feedback_submitted: Set(false),
                ..Default::default()
            }
            .insert(&self.db)
            .await?
        };

        Ok(upserted)
    }

    pub async fn update_participant_feedback(
        &self,
        review_id: &str,
        user_id: String,
        feedback_submitted: Option<bool>,
        feedback_content: Option<String>,
        role: Option<String>,
        vote_choice: Option<String>,
        exercised_veto: Option<bool>,
        veto_overturned: Option<bool>,
    ) -> Result<review_participant::Model, ApiError> {
        let user_id = user_id.trim().to_string();
        let model = review_participant::Entity::find()
            .filter(review_participant::Column::ReviewId.eq(review_id))
            .filter(review_participant::Column::UserId.eq(user_id))
            .one(&self.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("participant not found".to_string()))?;

        let mut active: review_participant::ActiveModel = model.into();
        if let Some(v) = feedback_submitted {
            active.feedback_submitted = Set(v);
        }
        if let Some(v) = feedback_content {
            let trimmed = v.trim().to_string();
            active.feedback_content = if trimmed.is_empty() {
                Set(None)
            } else {
                Set(Some(trimmed))
            };
        }
        if let Some(v) = role {
            let trimmed = v.trim().to_string();
            if trimmed.is_empty() {
                return Err(ApiError::BadRequest("role cannot be empty".to_string()));
            }
            active.role = Set(trimmed);
        }
        if let Some(v) = vote_choice {
            let normalized = v.trim().to_ascii_lowercase();
            if !matches!(normalized.as_str(), "yes" | "no" | "abstain") {
                return Err(ApiError::BadRequest("invalid vote_choice".to_string()));
            }
            active.vote_choice = Set(Some(normalized));
        }
        if let Some(v) = exercised_veto {
            active.exercised_veto = Set(v);
        }
        if let Some(v) = veto_overturned {
            active.veto_overturned = Set(v);
        }

        active.update(&self.db).await.map_err(Into::into)
    }

    pub async fn remove_participant(
        &self,
        review_id: &str,
        user_id: String,
    ) -> Result<(), ApiError> {
        let user_id = user_id.trim().to_string();
        let deleted = review_participant::Entity::delete_many()
            .filter(review_participant::Column::ReviewId.eq(review_id))
            .filter(review_participant::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await?;

        if deleted.rows_affected == 0 {
            return Err(ApiError::NotFound("participant not found".to_string()));
        }
        Ok(())
    }

    pub async fn delete_review(&self, proposal_id: &str) -> Result<(), ApiError> {
        let review = impact_review::Entity::find()
            .filter(impact_review::Column::ProposalId.eq(proposal_id))
            .one(&self.db)
            .await?
            .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;

        if review.trust_score_applied {
            return Err(ApiError::Conflict(
                "review trust score changes are already applied".to_string(),
            ));
        }

        impact_review::Entity::delete_by_id(review.id)
            .exec(&self.db)
            .await?;

        Ok(())
    }

    pub async fn complete_review(
        &self,
        proposal_id: &str,
        reviewer_id: Uuid,
        rating: Option<ReviewRating>,
        goal_achievements: Option<Value>,
        achievements: Option<String>,
        lessons: Option<String>,
        metrics: Option<Value>,
        status: Option<ReviewStatus>,
        data_sources: Option<Value>,
    ) -> Result<impact_review::Model, ApiError> {
        let tx = self.db.begin().await?;

        #[derive(Debug, FromQueryResult)]
        struct ReviewLockRow {
            id: String,
        }
        let locked = ReviewLockRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id
                FROM impact_reviews
                WHERE proposal_id = $1
                FOR UPDATE
            "#,
            vec![proposal_id.to_string().into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;

        let review = impact_review::Entity::find_by_id(locked.id)
            .one(&tx)
            .await?
            .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;

        let mut active: impact_review::ActiveModel = review.clone().into();
        if let Some(status) = status {
            active.status = Set(status);
        }
        if let Some(rating) = rating {
            active.rating = Set(Some(rating));
            active.status = Set(ReviewStatus::Completed);
            active.conducted_at = Set(Some(Utc::now().into()));
        }
        if let Some(goal_achievements) = goal_achievements {
            active.goal_achievements = Set(Some(goal_achievements));
        }
        if let Some(achievements) = achievements {
            active.achievements = Set(Some(achievements));
        }
        if let Some(lessons) = lessons {
            active.lessons = Set(Some(lessons));
        }
        if let Some(metrics) = metrics {
            active.metrics = Set(Some(metrics));
        }
        if let Some(data_sources) = data_sources.clone() {
            active.data_sources = Set(Some(data_sources));
        }

        let final_status = match active.status.clone() {
            sea_orm::ActiveValue::Set(v) => v,
            _ => review.status,
        };
        let final_rating = match active.rating.clone() {
            sea_orm::ActiveValue::Set(v) => v,
            _ => review.rating,
        };
        if final_status == ReviewStatus::Completed && final_rating.is_none() {
            return Err(ApiError::BadRequest(
                "completed review must include rating".to_string(),
            ));
        }
        if final_rating == Some(ReviewRating::F) {
            let mut ds = data_sources
                .or_else(|| review.data_sources.clone())
                .unwrap_or_else(|| json!({}));
            if !ds.is_object() {
                ds = json!({});
            }
            ds["repair_suggestion_required"] = json!(true);
            active.data_sources = Set(Some(ds));
        }

        active.reviewer_id = Set(Some(reviewer_id));

        let updated = active.update(&tx).await?;

        if updated.status == ReviewStatus::Completed
            && updated.rating.is_some()
            && !updated.trust_score_applied
        {
            self.apply_trust_score_updates_with_conn(&tx, &updated)
                .await?;
            self.write_ai_learning_records_with_conn(&tx, &updated)
                .await?;
            let mut mark_applied: impact_review::ActiveModel = updated.clone().into();
            mark_applied.trust_score_applied = Set(true);
            let updated = mark_applied.update(&tx).await?;
            tx.commit().await?;
            return Ok(updated);
        }

        tx.commit().await?;
        Ok(updated)
    }

    async fn apply_trust_score_updates_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        review: &impact_review::Model,
    ) -> Result<(), ApiError> {
        let rating = review
            .rating
            .ok_or_else(|| ApiError::BadRequest("review rating is required".to_string()))?;

        #[derive(Debug, FromQueryResult)]
        struct ProposalCtxRow {
            author_id: String,
            author_type: String,
            domains: Value,
        }

        let proposal = ProposalCtxRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT author_id, author_type::text AS author_type, domains
                FROM proposals
                WHERE id = $1
            "#,
            vec![review.proposal_id.clone().into()],
        ))
        .one(db)
        .await?
        .ok_or_else(|| ApiError::NotFound("proposal not found".to_string()))?;

        let participants = review_participant::Entity::find()
            .filter(review_participant::Column::ReviewId.eq(&review.id))
            .all(db)
            .await?;

        let domains = parse_domains(&proposal.domains);
        let primary_domain = domains
            .into_iter()
            .map(|d| normalize_domain_key(&d))
            .find(|d| !d.is_empty() && d != "global")
            .unwrap_or_else(|| "global".to_string());

        for participant in participants {
            let delta = compute_participant_delta(
                rating,
                participant.role.as_str(),
                participant.vote_choice.as_deref(),
                participant.exercised_veto,
                participant.veto_overturned,
            );

            if delta == 0 {
                continue;
            }

            let participant_type = infer_participant_type(
                db,
                &review.proposal_id,
                &proposal.author_id,
                &proposal.author_type,
                &participant,
            )
            .await?;

            let Ok(participant_uuid) = Uuid::parse_str(&participant.user_id) else {
                tracing::debug!(
                    participant_id = %participant.user_id,
                    review_id = %review.id,
                    "skip trust score update for non-uuid participant"
                );
                continue;
            };

            self.trust_svc
                .apply_impact_review_delta_with_conn(
                    db,
                    participant_uuid,
                    participant_type,
                    review.project_id,
                    &primary_domain,
                    &review.id,
                    delta,
                    &format!(
                        "impact review {} rating {:?}, role {}",
                        review.id, rating, participant.role
                    ),
                )
                .await?;

            let mut participant_active: review_participant::ActiveModel = participant.into();
            participant_active.trust_score_change = Set(Some(delta));
            participant_active.update(db).await?;
        }

        Ok(())
    }

    async fn write_ai_learning_records_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        review: &impact_review::Model,
    ) -> Result<(), ApiError> {
        let Some(rating) = review.rating else {
            return Ok(());
        };

        #[derive(Debug, Clone, FromQueryResult)]
        struct AiLearningCandidateRow {
            ai_participant_id: String,
            domain: String,
            vote_choice: Option<String>,
            vote_reason: Option<String>,
        }

        let rows = AiLearningCandidateRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    rp.user_id AS ai_participant_id,
                    COALESCE(NULLIF(trim(v.domain), ''), 'global') AS domain,
                    rp.vote_choice,
                    v.reason AS vote_reason
                FROM review_participants rp
                INNER JOIN ai_participants ap
                        ON ap.id = rp.user_id AND ap.project_id = $1
                LEFT JOIN votes v
                       ON v.proposal_id = $2 AND v.voter_id = rp.user_id
                WHERE rp.review_id = $3
            "#,
            vec![
                review.project_id.into(),
                review.proposal_id.clone().into(),
                review.id.clone().into(),
            ],
        ))
        .all(db)
        .await?;

        for row in rows {
            let alignment = compute_outcome_alignment(rating, row.vote_choice.as_deref());
            let follow_up = if rating == ReviewRating::F {
                Some("required".to_string())
            } else {
                None
            };

            ai_learning_record::ActiveModel {
                ai_participant_id: Set(row.ai_participant_id),
                review_id: Set(review.id.clone()),
                proposal_id: Set(review.proposal_id.clone()),
                domain: Set(normalize_domain_key(&row.domain)),
                review_rating: Set(rating),
                ai_vote_choice: Set(row.vote_choice),
                ai_vote_reason: Set(row.vote_reason),
                outcome_alignment: Set(alignment.to_string()),
                lesson_learned: Set(None),
                will_change: Set(None),
                follow_up_improvement: Set(follow_up),
                created_at: Set(Utc::now().into()),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }

        Ok(())
    }

    pub async fn list_reviews(
        &self,
        project_id: Option<Uuid>,
        status: Option<ReviewStatus>,
        rating: Option<ReviewRating>,
        page: i64,
        per_page: i64,
    ) -> Result<(Vec<impact_review::Model>, i64), ApiError> {
        let mut query = impact_review::Entity::find();
        if let Some(project_id) = project_id {
            query = query.filter(impact_review::Column::ProjectId.eq(project_id));
        }
        if let Some(status) = status {
            query = query.filter(impact_review::Column::Status.eq(status));
        }
        if let Some(rating) = rating {
            query = query.filter(impact_review::Column::Rating.eq(rating));
        }

        let paginator = query
            .order_by_desc(impact_review::Column::CreatedAt)
            .paginate(&self.db, per_page as u64);
        let total = paginator.num_items().await? as i64;
        let items = paginator.fetch_page((page - 1) as u64).await?;

        Ok((items, total))
    }
}

fn compute_participant_delta(
    rating: ReviewRating,
    role: &str,
    vote_choice: Option<&str>,
    exercised_veto: bool,
    veto_overturned: bool,
) -> i32 {
    let base = match rating {
        ReviewRating::S => 5,
        ReviewRating::A => 3,
        ReviewRating::B => 1,
        ReviewRating::C => -1,
        ReviewRating::F => -3,
    };

    let positive_outcome = matches!(rating, ReviewRating::S | ReviewRating::A | ReviewRating::B);

    let mut bonus = 0;
    if role == "proposer" {
        bonus = if positive_outcome { 1 } else { -2 };
    }

    if let Some(choice) = vote_choice {
        let normalized = choice.to_ascii_lowercase();
        if normalized == "yes" && matches!(rating, ReviewRating::S | ReviewRating::A) {
            bonus += 1;
        }
        if normalized == "no" && rating == ReviewRating::F {
            bonus += 2;
        }
        if normalized == "no" && matches!(rating, ReviewRating::S | ReviewRating::A) {
            bonus -= 1;
        }
    }

    if exercised_veto {
        if veto_overturned {
            bonus -= 1;
        } else if !positive_outcome {
            bonus += 1;
        }
    }

    base + bonus
}

async fn infer_participant_type<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
    proposal_author_id: &str,
    proposal_author_type: &str,
    participant: &review_participant::Model,
) -> Result<crate::entities::trust_score::ParticipantType, ApiError> {
    if proposal_author_id == participant.user_id.as_str() {
        return Ok(parse_participant_type(proposal_author_type));
    }

    #[derive(Debug, FromQueryResult)]
    struct VoteTypeRow {
        voter_type: String,
    }

    let row = VoteTypeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT voter_type::text AS voter_type
            FROM votes
            WHERE proposal_id = $1 AND voter_id = $2
            LIMIT 1
        "#,
        vec![
            proposal_id.to_string().into(),
            participant.user_id.clone().into(),
        ],
    ))
    .one(db)
    .await?;

    Ok(row
        .map(|r| parse_participant_type(&r.voter_type))
        .unwrap_or(crate::entities::trust_score::ParticipantType::Human))
}

fn compute_outcome_alignment(rating: ReviewRating, vote_choice: Option<&str>) -> &'static str {
    let outcome_positive = matches!(rating, ReviewRating::S | ReviewRating::A | ReviewRating::B);
    match vote_choice.map(|v| v.to_ascii_lowercase()) {
        Some(choice) if choice == "abstain" => "neutral",
        Some(choice) if choice == "yes" && outcome_positive => "aligned",
        Some(choice) if choice == "no" && !outcome_positive => "aligned",
        Some(_) => "misaligned",
        None => "neutral",
    }
}

async fn resolve_project_id_for_proposal<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
    author_id: &str,
) -> Result<Uuid, ApiError> {
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

    if let Some(project) = direct {
        return Ok(project.project_id);
    }

    let author_id = Uuid::parse_str(author_id)
        .map_err(|_| ApiError::BadRequest("proposal author_id is not uuid".to_string()))?;

    let fallback = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.id AS project_id
            FROM projects p
            INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id
            WHERE wm.user_id = $1
            ORDER BY p.created_at DESC
            LIMIT 1
        "#,
        vec![author_id.into()],
    ))
    .one(db)
    .await?;

    fallback
        .map(|r| r.project_id)
        .ok_or_else(|| ApiError::BadRequest("proposal project not found".to_string()))
}
