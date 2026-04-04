use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    services::trust_score_service::{TrustLevel, normalize_domain_key},
};

#[derive(Debug, FromQueryResult)]
struct TrustScoreValueRow {
    score: i32,
}

#[derive(Debug, FromQueryResult)]
struct AiParticipantCapRow {
    max_domain_level: String,
    domain_overrides: Option<Value>,
    can_veto_human_consensus: bool,
    is_active: bool,
    reason_min_length: i32,
}

pub struct PermissionService {
    db: DatabaseConnection,
}

impl PermissionService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn get_effective_level(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
        user_type: ParticipantType,
    ) -> Result<TrustLevel, ApiError> {
        let normalized_domain = normalize_domain_key(domain);
        let domain_key = if normalized_domain.is_empty() {
            "global".to_string()
        } else {
            normalized_domain
        };

        let score_row = TrustScoreValueRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT score
                FROM trust_scores
                WHERE user_id = $1
                  AND project_id = $2
                  AND user_type = $3::participant_type
                  AND domain = $4
                LIMIT 1
            ",
            vec![
                user_id.into(),
                project_id.into(),
                participant_type_str(user_type).into(),
                domain_key.clone().into(),
            ],
        ))
        .one(&self.db)
        .await?;

        let domain_level = TrustLevel::from_score(score_row.map(|row| row.score).unwrap_or(100));

        if user_type != ParticipantType::Ai {
            return Ok(domain_level);
        }

        let ai_row = AiParticipantCapRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT max_domain_level, domain_overrides, can_veto_human_consensus, is_active, reason_min_length
                FROM ai_participants
                WHERE id = $1
                  AND project_id = $2
                LIMIT 1
            ",
            vec![user_id.to_string().into(), project_id.into()],
        ))
        .one(&self.db)
        .await?;

        let Some(ai) = ai_row else {
            return Ok(TrustLevel::Observer);
        };

        if !ai.is_active {
            return Ok(TrustLevel::Observer);
        }

        let ai_max = ai_max_level_for_domain(&ai, &domain_key);
        Ok(min_level(domain_level, ai_max))
    }

    pub async fn can_vote(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
        user_type: ParticipantType,
    ) -> Result<bool, ApiError> {
        let level = self.get_effective_level(user_id, project_id, domain, user_type).await?;
        Ok(matches!(
            level,
            TrustLevel::Voter | TrustLevel::Vetoer | TrustLevel::Autonomous
        ))
    }

    pub async fn can_comment(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
        user_type: ParticipantType,
    ) -> Result<bool, ApiError> {
        let level = self.get_effective_level(user_id, project_id, domain, user_type).await?;
        Ok(matches!(
            level,
            TrustLevel::Advisor | TrustLevel::Voter | TrustLevel::Vetoer | TrustLevel::Autonomous
        ))
    }

    pub async fn can_veto(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        domain: &str,
        user_type: ParticipantType,
    ) -> Result<bool, ApiError> {
        let level = self.get_effective_level(user_id, project_id, domain, user_type).await?;
        Ok(matches!(level, TrustLevel::Vetoer | TrustLevel::Autonomous))
    }

    pub async fn ai_reason_min_length(&self, user_id: Uuid, project_id: Uuid) -> Result<Option<i32>, ApiError> {
        let ai_row = AiParticipantCapRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT max_domain_level, domain_overrides, can_veto_human_consensus, is_active, reason_min_length
                FROM ai_participants
                WHERE id = $1
                  AND project_id = $2
                LIMIT 1
            ",
            vec![user_id.to_string().into(), project_id.into()],
        ))
        .one(&self.db)
        .await?;

        Ok(ai_row.filter(|ai| ai.is_active).map(|ai| ai.reason_min_length))
    }

    pub async fn ai_can_veto_human_consensus(&self, user_id: Uuid, project_id: Uuid) -> Result<bool, ApiError> {
        let ai_row = AiParticipantCapRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT max_domain_level, domain_overrides, can_veto_human_consensus, is_active, reason_min_length
                FROM ai_participants
                WHERE id = $1
                  AND project_id = $2
                LIMIT 1
            ",
            vec![user_id.to_string().into(), project_id.into()],
        ))
        .one(&self.db)
        .await?;

        Ok(ai_row
            .filter(|ai| ai.is_active)
            .map(|ai| ai.can_veto_human_consensus)
            .unwrap_or(false))
    }
}

fn participant_type_str(value: ParticipantType) -> &'static str {
    match value {
        ParticipantType::Ai => "ai",
        ParticipantType::Human => "human",
    }
}

fn ai_max_level_for_domain(ai: &AiParticipantCapRow, domain: &str) -> TrustLevel {
    if domain == "governance" {
        return TrustLevel::Observer;
    }

    if let Some(overrides) = &ai.domain_overrides
        && let Some(level) = overrides.get(domain).and_then(Value::as_str)
    {
        return parse_level(level);
    }

    parse_level(&ai.max_domain_level)
}

fn parse_level(level: &str) -> TrustLevel {
    match level {
        "observer" => TrustLevel::Observer,
        "advisor" => TrustLevel::Advisor,
        "voter" => TrustLevel::Voter,
        "vetoer" => TrustLevel::Vetoer,
        "autonomous" => TrustLevel::Autonomous,
        _ => TrustLevel::Observer,
    }
}

fn min_level(left: TrustLevel, right: TrustLevel) -> TrustLevel {
    if level_rank(left) <= level_rank(right) {
        left
    } else {
        right
    }
}

fn level_rank(level: TrustLevel) -> i32 {
    match level {
        TrustLevel::Observer => 0,
        TrustLevel::Advisor => 1,
        TrustLevel::Voter => 2,
        TrustLevel::Vetoer => 3,
        TrustLevel::Autonomous => 4,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{entities::trust_score::ParticipantType, services::trust_score_service::TrustLevel};

    // ── parse_level ───────────────────────────────────────────────────────────

    #[test]
    fn parse_level_observer() {
        assert_eq!(parse_level("observer"), TrustLevel::Observer);
    }

    #[test]
    fn parse_level_advisor() {
        assert_eq!(parse_level("advisor"), TrustLevel::Advisor);
    }

    #[test]
    fn parse_level_voter() {
        assert_eq!(parse_level("voter"), TrustLevel::Voter);
    }

    #[test]
    fn parse_level_vetoer() {
        assert_eq!(parse_level("vetoer"), TrustLevel::Vetoer);
    }

    #[test]
    fn parse_level_autonomous() {
        assert_eq!(parse_level("autonomous"), TrustLevel::Autonomous);
    }

    #[test]
    fn parse_level_unknown_fallback_observer() {
        assert_eq!(parse_level("unknown"), TrustLevel::Observer);
        assert_eq!(parse_level(""), TrustLevel::Observer);
        assert_eq!(parse_level("VOTER"), TrustLevel::Observer);
    }

    // ── level_rank ────────────────────────────────────────────────────────────

    #[test]
    fn level_rank_observer_is_zero() {
        assert_eq!(level_rank(TrustLevel::Observer), 0);
    }

    #[test]
    fn level_rank_advisor_is_one() {
        assert_eq!(level_rank(TrustLevel::Advisor), 1);
    }

    #[test]
    fn level_rank_voter_is_two() {
        assert_eq!(level_rank(TrustLevel::Voter), 2);
    }

    #[test]
    fn level_rank_vetoer_is_three() {
        assert_eq!(level_rank(TrustLevel::Vetoer), 3);
    }

    #[test]
    fn level_rank_autonomous_is_four() {
        assert_eq!(level_rank(TrustLevel::Autonomous), 4);
    }

    #[test]
    fn level_rank_strictly_increasing() {
        assert!(level_rank(TrustLevel::Observer) < level_rank(TrustLevel::Advisor));
        assert!(level_rank(TrustLevel::Advisor) < level_rank(TrustLevel::Voter));
        assert!(level_rank(TrustLevel::Voter) < level_rank(TrustLevel::Vetoer));
        assert!(level_rank(TrustLevel::Vetoer) < level_rank(TrustLevel::Autonomous));
    }

    // ── min_level ─────────────────────────────────────────────────────────────

    #[test]
    fn min_level_returns_lower_when_left_is_lower() {
        assert_eq!(min_level(TrustLevel::Observer, TrustLevel::Autonomous), TrustLevel::Observer);
    }

    #[test]
    fn min_level_returns_lower_when_right_is_lower() {
        assert_eq!(min_level(TrustLevel::Autonomous, TrustLevel::Advisor), TrustLevel::Advisor);
    }

    #[test]
    fn min_level_same_returns_that_level() {
        assert_eq!(min_level(TrustLevel::Voter, TrustLevel::Voter), TrustLevel::Voter);
    }

    #[test]
    fn min_level_observer_beats_all() {
        for level in [
            TrustLevel::Advisor,
            TrustLevel::Voter,
            TrustLevel::Vetoer,
            TrustLevel::Autonomous,
        ] {
            assert_eq!(min_level(TrustLevel::Observer, level), TrustLevel::Observer);
            assert_eq!(min_level(level, TrustLevel::Observer), TrustLevel::Observer);
        }
    }

    // ── participant_type_str ──────────────────────────────────────────────────

    #[test]
    fn participant_type_str_ai() {
        assert_eq!(participant_type_str(ParticipantType::Ai), "ai");
    }

    #[test]
    fn participant_type_str_human() {
        assert_eq!(participant_type_str(ParticipantType::Human), "human");
    }
}
