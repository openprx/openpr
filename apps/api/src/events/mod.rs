use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
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
    let outbox_id = Uuid::new_v4();
    let envelope = json!({
        "version": "openpr.event.v1",
        "event_id": event_id,
        "event_type": input.event_type,
        "workspace_id": input.workspace_id,
        "project_id": input.project_id,
        "aggregate": {
            "type": input.aggregate_type,
            "id": input.aggregate_id
        },
        "actor_id": input.actor_id,
        "source": input.source,
        "payload": input.payload,
        "metadata": input.metadata,
        "correlation_id": input.correlation_id,
        "causation_id": input.causation_id
    });
    let headers = json!({
        "schema": "openpr.event.v1",
        "idempotency_key": input.idempotency_key,
        "correlation_id": input.correlation_id,
        "causation_id": input.causation_id
    });

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
            input.event_type.clone().into(),
            input.aggregate_type.clone().into(),
            input.aggregate_id.clone().into(),
            input.actor_id.into(),
            input.source.into(),
            input.payload.into(),
            input.metadata.into(),
            input.correlation_id.into(),
            input.causation_id.into(),
            input.idempotency_key.clone().into(),
        ],
    ))
    .await?;

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_outbox (
                id, business_event_id, workspace_id, project_id, event_type,
                aggregate_type, aggregate_id, payload, headers, status,
                attempts, max_attempts, available_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', 0, 10, now(), now(), now())
        ",
        vec![
            outbox_id.into(),
            event_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.into(),
            input.aggregate_type.into(),
            input.aggregate_id.into(),
            envelope.into(),
            headers.into(),
        ],
    ))
    .await?;

    Ok(event_id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn event_envelope_keeps_decimal_payload_as_string() {
        let event_id = Uuid::new_v4();
        let envelope = json!({
            "version": "openpr.event.v1",
            "event_id": event_id,
            "event_type": "form.record.created",
            "payload": {
                "values": {
                    "total_amount": {
                        "type": "amount",
                        "decimal": "0.30",
                        "currency": "CNY",
                        "scale": 2
                    }
                }
            }
        });

        let decimal = envelope
            .get("payload")
            .and_then(|payload| payload.get("values"))
            .and_then(|values| values.get("total_amount"))
            .and_then(|amount| amount.get("decimal"))
            .and_then(serde_json::Value::as_str);

        assert_eq!(decimal, Some("0.30"));
    }
}
