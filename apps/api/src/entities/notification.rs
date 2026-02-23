use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "notifications")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    #[serde(rename = "type")]
    #[sea_orm(column_name = "type")]
    pub notification_type: String,
    pub title: String,
    pub content: String,
    pub link: Option<String>,
    pub related_issue_id: Option<Uuid>,
    pub related_comment_id: Option<Uuid>,
    pub related_project_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: DateTimeWithTimeZone,
    pub read_at: Option<DateTimeWithTimeZone>,
    pub metadata: Option<Json>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationResponse {
    pub id: Uuid,
    pub user_id: Uuid,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub content: String,
    pub link: Option<String>,
    pub related_issue_id: Option<Uuid>,
    pub related_comment_id: Option<Uuid>,
    pub related_project_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: DateTimeWithTimeZone,
    pub read_at: Option<DateTimeWithTimeZone>,
}

impl From<Model> for NotificationResponse {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            user_id: model.user_id,
            notification_type: model.notification_type,
            title: model.title,
            content: model.content,
            link: model.link,
            related_issue_id: model.related_issue_id,
            related_comment_id: model.related_comment_id,
            related_project_id: model.related_project_id,
            is_read: model.is_read,
            created_at: model.created_at,
            read_at: model.read_at,
        }
    }
}

// Notification types
pub const NOTIFICATION_TYPE_MENTION: &str = "mention";
pub const NOTIFICATION_TYPE_ASSIGNMENT: &str = "assignment";
pub const NOTIFICATION_TYPE_COMMENT_REPLY: &str = "comment_reply";
pub const NOTIFICATION_TYPE_ISSUE_UPDATE: &str = "issue_update";
pub const NOTIFICATION_TYPE_PROJECT_UPDATE: &str = "project_update";
