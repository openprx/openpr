use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "appeals")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub log_id: i64,
    pub appellant_id: Uuid,
    pub reason: String,
    pub evidence: Option<Json>,
    pub status: AppealStatus,
    pub reviewer_id: Option<Uuid>,
    pub review_note: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub resolved_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::trust_score_log::Entity",
        from = "Column::LogId",
        to = "super::trust_score_log::Column::Id"
    )]
    TrustScoreLog,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::AppellantId",
        to = "super::user::Column::Id"
    )]
    Appellant,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::ReviewerId",
        to = "super::user::Column::Id"
    )]
    Reviewer,
}

impl Related<super::trust_score_log::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TrustScoreLog.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Appellant.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Copy, Clone, Debug, EnumIter, DeriveActiveEnum, PartialEq, Eq, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "appeal_status")]
pub enum AppealStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "accepted")]
    Accepted,
    #[sea_orm(string_value = "rejected")]
    Rejected,
}
