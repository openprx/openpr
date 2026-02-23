use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use super::proposal::AuthorType;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "votes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub proposal_id: String,
    pub voter_id: String,
    pub voter_type: AuthorType,
    pub choice: VoteChoice,
    pub weight: f64,
    pub reason: Option<String>,
    pub voted_at: DateTimeWithTimeZone,
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

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "vote_choice")]
pub enum VoteChoice {
    #[sea_orm(string_value = "yes")]
    Yes,
    #[sea_orm(string_value = "no")]
    No,
    #[sea_orm(string_value = "abstain")]
    Abstain,
}
