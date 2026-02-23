use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "webhooks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub url: String,
    #[serde(skip_serializing)]
    pub secret: String,
    pub events: Json,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub last_triggered_at: Option<DateTimeWithTimeZone>,
    pub metadata: Option<Json>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::workspace::Entity",
        from = "Column::WorkspaceId",
        to = "super::workspace::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Workspace,
}

impl Related<super::workspace::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Workspace.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

// Webhook delivery record (not using SeaORM entity for now)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub request_headers: Option<serde_json::Value>,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub delivered_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    pub retry_count: i32,
    pub next_retry_at: Option<DateTimeWithTimeZone>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: Option<String>,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
    pub active: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub bot_user_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub secret: Option<String>,
    pub events: Option<Vec<String>>,
    pub active: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub bot_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub last_triggered_at: Option<DateTimeWithTimeZone>,
}

impl From<Model> for WebhookResponse {
    fn from(model: Model) -> Self {
        let events = serde_json::from_value(model.events).unwrap_or_default();
        Self {
            id: model.id,
            workspace_id: model.workspace_id,
            name: model.name,
            url: model.url,
            events,
            active: model.active,
            bot_user_id: model.bot_user_id,
            created_by: model.created_by,
            created_at: model.created_at,
            updated_at: model.updated_at,
            last_triggered_at: model.last_triggered_at,
        }
    }
}

// Available webhook events
pub const WEBHOOK_EVENTS: &[&str] = &[
    "issue.created",
    "issue.updated",
    "issue.assigned",
    "issue.deleted",
    "issue.state_changed",
    "comment.created",
    "comment.updated",
    "comment.deleted",
    "label.added",
    "label.removed",
    "sprint.started",
    "sprint.completed",
    "ai.task_completed",
    "ai.task_failed",
];
