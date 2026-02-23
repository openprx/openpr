use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "proposal_issue_links")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub proposal_id: String,
    pub issue_id: Uuid,
    pub created_at: DateTimeWithTimeZone,
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
        belongs_to = "super::work_item::Entity",
        from = "Column::IssueId",
        to = "super::work_item::Column::Id"
    )]
    Issue,
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
