use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use super::trust_score::TrustLevel;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "trust_score_logs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    pub project_id: Uuid,
    pub domain: String,
    pub event_type: String,
    pub event_id: String,
    pub score_change: i32,
    pub old_score: i32,
    pub new_score: i32,
    pub old_level: TrustLevel,
    pub new_level: TrustLevel,
    pub reason: String,
    pub is_appealed: bool,
    pub appeal_result: Option<String>,
    pub created_at: DateTimeWithTimeZone,
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
    #[sea_orm(has_many = "super::appeal::Entity")]
    Appeals,
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

impl Related<super::appeal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Appeals.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
