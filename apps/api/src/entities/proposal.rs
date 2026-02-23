use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "proposals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub title: String,
    pub proposal_type: ProposalType,
    pub status: ProposalStatus,
    pub author_id: String,
    pub author_type: AuthorType,
    pub content: String,
    pub domains: Json,
    pub voting_rule: VotingRule,
    pub cycle_template: CycleTemplate,
    pub template_id: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub submitted_at: Option<DateTimeWithTimeZone>,
    pub voting_started_at: Option<DateTimeWithTimeZone>,
    pub voting_ended_at: Option<DateTimeWithTimeZone>,
    pub archived_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::vote::Entity")]
    Votes,
    #[sea_orm(has_many = "super::proposal_comment::Entity")]
    Comments,
    #[sea_orm(has_many = "super::proposal_issue_link::Entity")]
    IssueLinks,
    #[sea_orm(has_one = "super::decision::Entity")]
    Decision,
}

impl Related<super::vote::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Votes.def()
    }
}

impl Related<super::proposal_comment::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Comments.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "proposal_type")]
pub enum ProposalType {
    #[sea_orm(string_value = "feature")]
    Feature,
    #[sea_orm(string_value = "architecture")]
    Architecture,
    #[sea_orm(string_value = "priority")]
    Priority,
    #[sea_orm(string_value = "resource")]
    Resource,
    #[sea_orm(string_value = "governance")]
    Governance,
    #[sea_orm(string_value = "bugfix")]
    Bugfix,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "proposal_status")]
pub enum ProposalStatus {
    #[sea_orm(string_value = "draft")]
    Draft,
    #[sea_orm(string_value = "open")]
    Open,
    #[sea_orm(string_value = "voting")]
    Voting,
    #[sea_orm(string_value = "approved")]
    Approved,
    #[sea_orm(string_value = "rejected")]
    Rejected,
    #[sea_orm(string_value = "vetoed")]
    Vetoed,
    #[sea_orm(string_value = "archived")]
    Archived,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "author_type")]
pub enum AuthorType {
    #[sea_orm(string_value = "human")]
    Human,
    #[sea_orm(string_value = "ai")]
    Ai,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "voting_rule")]
pub enum VotingRule {
    #[sea_orm(string_value = "simple_majority")]
    SimpleMajority,
    #[sea_orm(string_value = "absolute_majority")]
    AbsoluteMajority,
    #[sea_orm(string_value = "consensus")]
    Consensus,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "cycle_template")]
pub enum CycleTemplate {
    #[sea_orm(string_value = "rapid")]
    Rapid,
    #[sea_orm(string_value = "fast")]
    Fast,
    #[sea_orm(string_value = "standard")]
    Standard,
    #[sea_orm(string_value = "critical")]
    Critical,
}

impl CycleTemplate {
    pub fn discussion_hours(self) -> i64 {
        match self {
            Self::Rapid => 1,
            Self::Fast => 24,
            Self::Standard => 72,
            Self::Critical => 168,
        }
    }

    pub fn voting_hours(self) -> i64 {
        match self {
            Self::Rapid => 1,
            Self::Fast => 24,
            Self::Standard => 48,
            Self::Critical => 72,
        }
    }
}
