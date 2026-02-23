use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_participants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub project_id: Uuid,
    pub name: String,
    pub model: String,
    pub provider: String,
    pub api_endpoint: Option<String>,
    pub capabilities: Json,
    pub domain_overrides: Option<Json>,
    pub max_domain_level: String,
    pub can_veto_human_consensus: bool,
    pub reason_min_length: i32,
    pub is_active: bool,
    pub registered_by: Uuid,
    pub last_active_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::RegisteredBy",
        to = "super::user::Column::Id"
    )]
    RegisteredByUser,
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RegisteredByUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
