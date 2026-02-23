use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "trust_scores")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub user_type: ParticipantType,
    pub project_id: Uuid,
    pub domain: String,
    pub score: i32,
    pub level: TrustLevel,
    pub vote_weight: f64,
    pub consecutive_rejections: i32,
    pub cooldown_until: Option<DateTimeWithTimeZone>,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "trust_level")]
pub enum TrustLevel {
    #[sea_orm(string_value = "observer")]
    Observer,
    #[sea_orm(string_value = "advisor")]
    Advisor,
    #[sea_orm(string_value = "voter")]
    Voter,
    #[sea_orm(string_value = "vetoer")]
    Vetoer,
    #[sea_orm(string_value = "autonomous")]
    Autonomous,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "participant_type")]
pub enum ParticipantType {
    #[sea_orm(string_value = "human")]
    Human,
    #[sea_orm(string_value = "ai")]
    Ai,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
