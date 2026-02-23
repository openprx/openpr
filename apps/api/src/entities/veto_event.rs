use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "veto_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub proposal_id: String,
    pub vetoer_id: Uuid,
    pub domain: String,
    pub reason: String,
    pub status: VetoStatus,
    pub escalation_started_at: Option<DateTimeWithTimeZone>,
    pub escalation_result: Option<String>,
    pub escalation_votes: Option<Json>,
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
        belongs_to = "super::user::Entity",
        from = "Column::VetoerId",
        to = "super::user::Column::Id"
    )]
    VetoerUser,
    #[sea_orm(has_many = "super::decision::Entity")]
    Decisions,
}

impl Related<super::proposal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Proposal.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::VetoerUser.def()
    }
}

impl Related<super::decision::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Decisions.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "veto_status")]
pub enum VetoStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "overturned")]
    Overturned,
    #[sea_orm(string_value = "upheld")]
    Upheld,
    #[sea_orm(string_value = "withdrawn")]
    Withdrawn,
}
