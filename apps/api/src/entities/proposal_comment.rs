use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use super::proposal::AuthorType;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "proposal_comments")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub proposal_id: String,
    pub author_id: String,
    pub author_type: AuthorType,
    pub comment_type: String,
    pub content: String,
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
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
