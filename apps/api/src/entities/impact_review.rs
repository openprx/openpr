use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "impact_reviews")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub proposal_id: String,
    pub project_id: Uuid,
    pub status: ReviewStatus,
    pub rating: Option<ReviewRating>,
    pub metrics: Option<Json>,
    pub goal_achievements: Option<Json>,
    pub achievements: Option<String>,
    pub lessons: Option<String>,
    pub reviewer_id: Option<Uuid>,
    pub is_auto_triggered: bool,
    pub data_sources: Option<Json>,
    pub trust_score_applied: bool,
    pub scheduled_at: Option<DateTimeWithTimeZone>,
    pub conducted_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "review_status")]
pub enum ReviewStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "collecting")]
    Collecting,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "skipped")]
    Skipped,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "review_rating")]
pub enum ReviewRating {
    #[sea_orm(string_value = "S")]
    S,
    #[sea_orm(string_value = "A")]
    A,
    #[sea_orm(string_value = "B")]
    B,
    #[sea_orm(string_value = "C")]
    C,
    #[sea_orm(string_value = "F")]
    F,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::proposal::Entity",
        from = "Column::ProposalId",
        to = "super::proposal::Column::Id"
    )]
    Proposal,
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::ReviewerId",
        to = "super::user::Column::Id"
    )]
    Reviewer,
    #[sea_orm(has_many = "super::impact_metric::Entity")]
    Metrics,
    #[sea_orm(has_many = "super::review_participant::Entity")]
    Participants,
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Reviewer.def()
    }
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::impact_metric::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Metrics.def()
    }
}

impl Related<super::review_participant::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Participants.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
