use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

pub struct BusinessEventInput {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub actor_id: Option<Uuid>,
    pub source: Value,
    pub payload: Value,
    pub metadata: Value,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
}

pub async fn insert_business_event<C>(db: &C, input: BusinessEventInput) -> Result<Uuid, ApiError>
where
    C: ConnectionTrait,
{
    let event_id = Uuid::new_v4();

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO business_events (
                id, workspace_id, project_id, event_type, aggregate_type, aggregate_id,
                actor_id, source, payload, metadata, correlation_id, causation_id,
                idempotency_key, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
        ",
        vec![
            event_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.into(),
            input.aggregate_type.into(),
            input.aggregate_id.into(),
            input.actor_id.into(),
            input.source.into(),
            input.payload.into(),
            input.metadata.into(),
            input.correlation_id.into(),
            input.causation_id.into(),
            input.idempotency_key.into(),
        ],
    ))
    .await?;

    Ok(event_id)
}
