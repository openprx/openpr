use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "impact_metrics")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub review_id: String,
    pub metric_key: String,
    pub metric_label: Option<String>,
    pub target_value: Option<f64>,
    pub actual_value: Option<f64>,
    pub unit: Option<String>,
    pub achievement_rate: Option<f64>,
    pub metadata: Option<Json>,
    pub recorded_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::impact_review::Entity",
        from = "Column::ReviewId",
        to = "super::impact_review::Column::Id"
    )]
    Review,
}

impl Related<super::impact_review::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Review.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
