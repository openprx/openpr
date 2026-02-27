use chrono::{Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde_json::Value;
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    services::governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Observer,
    Advisor,
    Voter,
    Vetoer,
    Autonomous,
}

impl TrustLevel {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Advisor => "advisor",
            Self::Voter => "voter",
            Self::Vetoer => "vetoer",
            Self::Autonomous => "autonomous",
        }
    }

    pub fn from_score(score: i32) -> Self {
        match score {
            i32::MIN..=49 => Self::Observer,
            50..=99 => Self::Advisor,
            100..=199 => Self::Voter,
            200..=299 => Self::Vetoer,
            _ => Self::Autonomous,
        }
    }

    pub fn vote_weight_for_score(score: i32) -> f64 {
        let weight = 1.0 + (score as f64 - 100.0) / 200.0;
        weight.clamp(0.5, 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustEventType {
    ProposalApproved,
    ProposalRejected,
    InactivityPenalty,
    AppealAccepted,
    ImpactReviewCompleted,
}

impl TrustEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProposalApproved => "proposal_approved",
            Self::ProposalRejected => "proposal_rejected",
            Self::InactivityPenalty => "inactivity_penalty",
            Self::AppealAccepted => "appeal_accepted",
            Self::ImpactReviewCompleted => "impact_review_completed",
        }
    }
}

#[derive(Debug, Clone, FromQueryResult)]
struct TrustScoreRow {
    id: i64,
    score: i32,
    level: String,
    consecutive_rejections: i32,
}

#[derive(Debug, Clone, FromQueryResult)]
struct CountRow {
    count: i64,
}

pub struct TrustScoreService {
    db: DatabaseConnection,
}

impl TrustScoreService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn proposal_score_delta(is_approved: bool) -> i32 {
        if is_approved { 2 } else { -1 }
    }

    pub async fn apply_proposal_result(
        &self,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        proposal_id: &str,
        is_approved: bool,
        domains: &[String],
    ) -> Result<(), ApiError> {
        let tx = self.db.begin().await?;
        self.apply_proposal_result_with_conn(
            &tx,
            user_id,
            user_type,
            project_id,
            proposal_id,
            is_approved,
            domains,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn apply_proposal_result_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        proposal_id: &str,
        is_approved: bool,
        domains: &[String],
    ) -> Result<(), ApiError> {
        let event_type = if is_approved {
            TrustEventType::ProposalApproved
        } else {
            TrustEventType::ProposalRejected
        };
        let delta = Self::proposal_score_delta(is_approved);
        let reason = if is_approved {
            format!("proposal {proposal_id} approved")
        } else {
            format!("proposal {proposal_id} rejected")
        };

        self.apply_change(
            db,
            user_id,
            user_type,
            project_id,
            "global",
            delta,
            event_type,
            proposal_id,
            &reason,
        )
        .await?;

        let mut seen = HashSet::new();
        for domain in domains {
            let normalized = domain.trim().to_ascii_lowercase();
            if normalized.is_empty() || normalized == "global" || normalized.len() > 50 {
                continue;
            }
            if !seen.insert(normalized.clone()) {
                continue;
            }
            self.apply_change(
                db,
                user_id,
                user_type,
                project_id,
                &normalized,
                delta,
                event_type,
                proposal_id,
                &reason,
            )
            .await?;
        }

        Ok(())
    }

    pub async fn apply_inactivity_decay(
        &self,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        months: i32,
        event_id: &str,
    ) -> Result<(), ApiError> {
        if months <= 0 {
            return Ok(());
        }

        let delta = -(2 * months);
        let reason = format!("{months} months inactivity penalty");
        let tx = self.db.begin().await?;
        self.apply_change(
            &tx,
            user_id,
            user_type,
            project_id,
            "global",
            delta,
            TrustEventType::InactivityPenalty,
            event_id,
            &reason,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn apply_manual_adjustment_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        domain: &str,
        delta: i32,
        event_type: TrustEventType,
        event_id: &str,
        reason: &str,
    ) -> Result<(), ApiError> {
        let normalized = normalize_domain_key(domain);
        let target_domain = if normalized.is_empty() {
            "global"
        } else {
            normalized.as_str()
        };

        self.apply_change(
            db,
            user_id,
            user_type,
            project_id,
            target_domain,
            delta,
            event_type,
            event_id,
            reason,
        )
        .await
    }

    pub async fn apply_impact_review_delta_with_conn<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        domain: &str,
        event_id: &str,
        delta: i32,
        reason: &str,
    ) -> Result<(), ApiError> {
        let normalized = normalize_domain_key(domain);
        let target_domain = if normalized.is_empty() {
            "global"
        } else {
            normalized.as_str()
        };

        self.apply_change(
            db,
            user_id,
            user_type,
            project_id,
            target_domain,
            delta,
            TrustEventType::ImpactReviewCompleted,
            event_id,
            reason,
        )
        .await
    }

    async fn apply_change<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        domain: &str,
        delta: i32,
        event_type: TrustEventType,
        event_id: &str,
        reason: &str,
    ) -> Result<(), ApiError> {
        let already_applied = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    SELECT 1
                    FROM trust_score_logs
                    WHERE user_id = $1
                      AND project_id = $2
                      AND domain = $3
                      AND event_type = $4
                      AND event_id = $5
                    LIMIT 1
                "#,
                vec![
                    user_id.into(),
                    project_id.into(),
                    domain.to_string().into(),
                    event_type.as_str().into(),
                    event_id.to_string().into(),
                ],
            ))
            .await?
            .is_some();
        if already_applied {
            return Ok(());
        }

        let current = self
            .get_or_create_score(db, user_id, user_type, project_id, domain)
            .await?;

        let already_applied_after_lock = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    SELECT 1
                    FROM trust_score_logs
                    WHERE user_id = $1
                      AND project_id = $2
                      AND domain = $3
                      AND event_type = $4
                      AND event_id = $5
                    LIMIT 1
                "#,
                vec![
                    user_id.into(),
                    project_id.into(),
                    domain.to_string().into(),
                    event_type.as_str().into(),
                    event_id.to_string().into(),
                ],
            ))
            .await?
            .is_some();
        if already_applied_after_lock {
            return Ok(());
        }

        let old_score = current.score;
        let old_level = current.level;
        let old_rejections = current.consecutive_rejections;
        let new_score = (old_score + delta).max(0);
        let new_level = TrustLevel::from_score(new_score);
        let new_weight = TrustLevel::vote_weight_for_score(new_score);
        let new_rejections = if event_type == TrustEventType::ProposalRejected {
            old_rejections + 1
        } else if event_type == TrustEventType::ProposalApproved {
            0
        } else {
            old_rejections
        };
        let cooldown_until = if new_rejections >= 3 {
            Some(Utc::now() + Duration::days(7))
        } else {
            None
        };
        let cooldown_until_str = cooldown_until.map(|v| v.to_rfc3339());

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE trust_scores
                SET score = $2,
                    level = $3::trust_level,
                    vote_weight = $4,
                    consecutive_rejections = $5,
                    cooldown_until = $6::timestamptz,
                    updated_at = $7
                WHERE id = $1
            "#,
            vec![
                current.id.into(),
                new_score.into(),
                new_level.as_db_str().into(),
                new_weight.into(),
                new_rejections.into(),
                cooldown_until_str.into(),
                Utc::now().into(),
            ],
        ))
        .await?;

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO trust_score_logs (
                    user_id, project_id, domain, event_type, event_id, score_change, old_score,
                    new_score, old_level, new_level, reason, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::trust_level, $10::trust_level, $11, $12)
            "#,
            vec![
                user_id.into(),
                project_id.into(),
                domain.to_string().into(),
                event_type.as_str().into(),
                event_id.to_string().into(),
                delta.into(),
                old_score.into(),
                new_score.into(),
                old_level.clone().into(),
                new_level.as_db_str().into(),
                reason.to_string().into(),
                Utc::now().into(),
            ],
        ))
        .await?;

        write_governance_audit_log(
            db,
            GovernanceAuditLogInput {
                project_id,
                actor_id: None,
                action: "trust_score.changed".to_string(),
                resource_type: "trust_score".to_string(),
                resource_id: Some(format!("{user_id}:{project_id}:{domain}")),
                old_value: Some(serde_json::json!({
                    "score": old_score,
                    "level": old_level,
                    "consecutive_rejections": old_rejections,
                })),
                new_value: Some(serde_json::json!({
                    "score": new_score,
                    "level": new_level.as_db_str(),
                    "vote_weight": new_weight,
                    "consecutive_rejections": new_rejections,
                    "cooldown_until": cooldown_until.map(|v| v.to_rfc3339()),
                })),
                metadata: Some(serde_json::json!({
                    "event_type": event_type.as_str(),
                    "event_id": event_id,
                    "delta": delta,
                    "reason": reason,
                })),
            },
        )
        .await?;

        self.sync_vetoer_rights(db, user_id, project_id, domain, new_level)
            .await?;
        Ok(())
    }

    async fn get_or_create_score<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        user_type: ParticipantType,
        project_id: Uuid,
        domain: &str,
    ) -> Result<TrustScoreRow, ApiError> {
        let existing = TrustScoreRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, score, level::text AS level, consecutive_rejections
                FROM trust_scores
                WHERE user_id = $1 AND project_id = $2 AND domain = $3
                FOR UPDATE
            "#,
            vec![user_id.into(), project_id.into(), domain.to_string().into()],
        ))
        .one(db)
        .await?;

        if let Some(row) = existing {
            return Ok(row);
        }

        let user_type = match user_type {
            ParticipantType::Ai => "ai",
            ParticipantType::Human => "human",
        };

        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO trust_scores (
                    user_id, user_type, project_id, domain, score, level, vote_weight,
                    consecutive_rejections, cooldown_until, updated_at
                ) VALUES ($1, $2::participant_type, $3, $4, 100, 'voter', 1.0, 0, NULL, $5)
                ON CONFLICT (user_id, project_id, domain) DO NOTHING
            "#,
            vec![
                user_id.into(),
                user_type.into(),
                project_id.into(),
                domain.to_string().into(),
                Utc::now().into(),
            ],
        ))
        .await?;

        let row = TrustScoreRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT id, score, level::text AS level, consecutive_rejections
                FROM trust_scores
                WHERE user_id = $1 AND project_id = $2 AND domain = $3
                FOR UPDATE
            "#,
            vec![user_id.into(), project_id.into(), domain.to_string().into()],
        ))
        .one(db)
        .await?
        .ok_or_else(|| ApiError::Internal)?;

        Ok(row)
    }

    async fn sync_vetoer_rights<C: ConnectionTrait>(
        &self,
        db: &C,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
        level: TrustLevel,
    ) -> Result<(), ApiError> {
        if matches!(level, TrustLevel::Vetoer | TrustLevel::Autonomous) {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO vetoers (user_id, project_id, domain, granted_by, granted_at)
                    VALUES ($1, $2, $3, 'trust_score', $4)
                    ON CONFLICT (user_id, project_id, domain) DO NOTHING
                "#,
                vec![
                    user_id.into(),
                    project_id.into(),
                    domain.to_string().into(),
                    Utc::now().into(),
                ],
            ))
            .await?;
        } else {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM vetoers WHERE user_id = $1 AND project_id = $2 AND domain = $3",
                vec![user_id.into(), project_id.into(), domain.to_string().into()],
            ))
            .await?;
        }

        Ok(())
    }
}

pub fn parse_domains(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn parse_participant_type(value: &str) -> ParticipantType {
    if value == "ai" {
        ParticipantType::Ai
    } else {
        ParticipantType::Human
    }
}

pub fn normalize_domain_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch
            } else if ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

pub fn scoped_domain_id(project_id: Uuid, key: &str) -> String {
    let normalized = normalize_domain_key(key);
    let short = project_id.simple().to_string();
    format!("{}-{}", &short[..8], normalized)
}

pub fn split_domain_key(domain_id: &str) -> &str {
    domain_id
        .split_once('-')
        .map(|(_, right)| right)
        .unwrap_or(domain_id)
}

pub async fn is_project_member(
    db: &DatabaseConnection,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = $1 AND wm.user_id = $2
                LIMIT 1
            "#,
            vec![project_id.into(), user_id.into()],
        ))
        .await?
        .is_some();
    Ok(exists)
}

pub async fn is_project_admin_or_owner(
    db: &DatabaseConnection,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    let count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*)::bigint AS count
            FROM projects p
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE p.id = $1
              AND wm.user_id = $2
              AND wm.role IN ('owner', 'admin')
        "#,
        vec![project_id.into(), user_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.count)
    .unwrap_or(0);

    Ok(count > 0)
}

pub async fn is_system_admin(db: &DatabaseConnection, user_id: Uuid) -> Result<bool, ApiError> {
    let is_admin = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 1
                FROM users
                WHERE id = $1
                  AND is_active = true
                  AND lower(trim(role)) = 'admin'
                LIMIT 1
            "#,
            vec![user_id.into()],
        ))
        .await?
        .is_some();

    Ok(is_admin)
}
