use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "decision_audit_reports")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub project_id: Uuid,
    pub period_start: Date,
    pub period_end: Date,
    pub total_proposals: i32,
    pub approved_proposals: i32,
    pub rejected_proposals: i32,
    pub vetoed_proposals: i32,
    pub reviewed_proposals: i32,
    pub avg_review_rating: Option<f64>,
    pub rating_distribution: Json,
    pub top_contributors: Option<Json>,
    pub domain_stats: Option<Json>,
    pub ai_participation_stats: Option<Json>,
    pub key_insights: Option<Json>,
    pub generated_at: DateTimeWithTimeZone,
    pub generated_by: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
