use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "decisions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub proposal_id: String,
    pub result: DecisionResult,
    pub approval_rate: Option<f64>,
    pub total_votes: i32,
    pub yes_votes: i32,
    pub no_votes: i32,
    pub abstain_votes: i32,
    pub weighted_yes: Option<f64>,
    pub weighted_no: Option<f64>,
    pub weighted_approval_rate: Option<f64>,
    pub is_weighted: bool,
    pub veto_event_id: Option<i64>,
    pub decided_at: DateTimeWithTimeZone,
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
        belongs_to = "super::veto_event::Entity",
        from = "Column::VetoEventId",
        to = "super::veto_event::Column::Id"
    )]
    VetoEvent,
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl Related<super::veto_event::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::VetoEvent.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "decision_result")]
pub enum DecisionResult {
    #[sea_orm(string_value = "approved")]
    Approved,
    #[sea_orm(string_value = "rejected")]
    Rejected,
    #[sea_orm(string_value = "vetoed")]
    Vetoed,
}
