use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "review_participants")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub review_id: String,
    pub user_id: String,
    pub role: String,
    pub vote_choice: Option<String>,
    pub exercised_veto: bool,
    pub veto_overturned: bool,
    pub feedback_submitted: bool,
    pub feedback_content: Option<String>,
    pub trust_score_change: Option<i32>,
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
