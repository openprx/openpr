use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_learning_records")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub ai_participant_id: String,
    pub review_id: String,
    pub proposal_id: String,
    pub domain: String,
    pub review_rating: super::impact_review::ReviewRating,
    pub ai_vote_choice: Option<String>,
    pub ai_vote_reason: Option<String>,
    pub outcome_alignment: String,
    pub lesson_learned: Option<String>,
    pub will_change: Option<String>,
    pub follow_up_improvement: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::ai_participant::Entity",
        from = "Column::AiParticipantId",
        to = "super::ai_participant::Column::Id"
    )]
    AiParticipant,
    #[sea_orm(
        belongs_to = "super::impact_review::Entity",
        from = "Column::ReviewId",
        to = "super::impact_review::Column::Id"
    )]
    Review,
    #[sea_orm(
        belongs_to = "super::proposal::Entity",
        from = "Column::ProposalId",
        to = "super::proposal::Column::Id"
    )]
    Proposal,
}

impl Related<super::ai_participant::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AiParticipant.def()
    }
}

impl Related<super::impact_review::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Review.def()
    }
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
