use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "feedback_loop_links")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub source_review_id: String,
    pub derived_proposal_id: String,
    pub link_type: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::impact_review::Entity",
        from = "Column::SourceReviewId",
        to = "super::impact_review::Column::Id"
    )]
    SourceReview,
    #[sea_orm(
        belongs_to = "super::proposal::Entity",
        from = "Column::DerivedProposalId",
        to = "super::proposal::Column::Id"
    )]
    DerivedProposal,
}

impl Related<super::impact_review::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SourceReview.def()
    }
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DerivedProposal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
