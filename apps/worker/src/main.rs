use clap::Parser;
use platform::{
    app::{AppState, connect_db},
    config::AppConfig,
    logging,
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Parser)]
struct WorkerArgs {
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
}

#[derive(Debug, Clone, FromQueryResult)]
struct AiTaskDispatchRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    ai_participant_id: Uuid,
    ai_participant_name: Option<String>,
    ai_participant_agent_type: Option<String>,
    task_type: String,
    reference_type: Option<String>,
    reference_id: Option<Uuid>,
    payload: serde_json::Value,
    attempts: i32,
    max_attempts: i32,
    priority: i32,
}

#[derive(Debug, FromQueryResult)]
struct BotWebhookRow {
    webhook_id: Uuid,
    url: String,
    connector_id: Option<Uuid>,
    invocation_id: Option<Uuid>,
}

#[derive(Debug, Clone, FromQueryResult)]
struct ConnectorInvocationDispatchRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    connector_id: Uuid,
    connector_kind: String,
    connector_name: String,
    endpoint: String,
    trigger_kind: String,
    trigger_ref_type: Option<String>,
    trigger_ref_id: Option<Uuid>,
    payload: serde_json::Value,
}

struct ConnectorDispatchOutcome {
    success: bool,
    result: serde_json::Value,
    error_message: Option<String>,
}

#[derive(Debug, Clone, FromQueryResult)]
struct EventOutboxDispatchRow {
    outbox_id: Uuid,
    business_event_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    payload: serde_json::Value,
}

#[derive(Debug, Clone, FromQueryResult)]
struct EventInboxProcessingRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    source_kind: String,
    source_id: Option<String>,
    idempotency_key: String,
    event_type: String,
    payload: serde_json::Value,
    attempts: i32,
}

#[derive(Debug, Clone, FromQueryResult)]
struct InvocationEventRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    target_agent_id: Option<Uuid>,
    source_task_id: Option<Uuid>,
    trigger_kind: String,
    trigger_ref_type: Option<String>,
    trigger_ref_id: Option<Uuid>,
    connector_id: Option<Uuid>,
    connector_kind: Option<String>,
    status: String,
    payload: serde_json::Value,
    result: Option<serde_json::Value>,
    error_message: Option<String>,
    audit_chain_id: Option<Uuid>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let cfg = AppConfig::from_env("worker", "0.0.0.0:8081")?;
    logging::init("worker");

    let db = connect_db(&cfg.database_url).await?;
    let state = AppState {
        cfg: cfg.clone(),
        db: db.clone(),
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    tracing::info!(concurrency = args.concurrency, app = %cfg.app_name, "worker started");

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            result = process_pending_tasks(&db, &client, args.concurrency) => {
                if let Err(err) = result {
                    tracing::warn!(error = %err, "task polling failed");
                }
            }
        }

        if let Err(err) = process_pending_event_outbox(&db, args.concurrency).await {
            tracing::warn!(error = %err, "event outbox polling failed");
        }

        if let Err(err) = process_received_event_inbox(&db, args.concurrency).await {
            tracing::warn!(error = %err, "event inbox polling failed");
        }

        if let Err(err) = process_pending_connector_invocations(&db, &client, args.concurrency).await {
            tracing::warn!(error = %err, "connector invocation polling failed");
        }

        if let Err(err) =
            api::routes::form::process_pending_form_jobs_from_worker(state.clone(), args.concurrency).await
        {
            tracing::warn!(error = %err, "form job polling failed");
        }

        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }

    Ok(())
}

async fn process_received_event_inbox(db: &sea_orm::DatabaseConnection, concurrency: usize) -> anyhow::Result<()> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let rows = pickup_received_event_inbox(db, limit).await?;

    for row in rows {
        match consume_event_inbox_row(db, &row).await {
            Ok(()) => {
                mark_event_inbox_processed(db, row.id).await?;
            }
            Err(err) => {
                tracing::warn!(
                    inbox_id = %row.id,
                    event_type = %row.event_type,
                    source_kind = %row.source_kind,
                    source_id = ?row.source_id,
                    attempts = row.attempts,
                    error = %err,
                    "event inbox consumption failed"
                );
                mark_event_inbox_failed(db, row.id, &err.to_string()).await?;
            }
        }
    }

    Ok(())
}

async fn pickup_received_event_inbox(
    db: &sea_orm::DatabaseConnection,
    limit: i64,
) -> anyhow::Result<Vec<EventInboxProcessingRow>> {
    let rows = EventInboxProcessingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT id
                FROM event_inbox
                WHERE status = 'received'
                ORDER BY received_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            ),
            updated AS (
                UPDATE event_inbox ei
                SET status = 'processing',
                    attempts = attempts + 1,
                    updated_at = now()
                FROM picked
                WHERE ei.id = picked.id
                RETURNING
                    ei.id,
                    ei.workspace_id,
                    ei.project_id,
                    ei.source_kind,
                    ei.source_id,
                    ei.idempotency_key,
                    ei.event_type,
                    ei.payload,
                    ei.attempts
            )
            SELECT * FROM updated
        ",
        vec![limit.into()],
    ))
    .all(db)
    .await?;

    Ok(rows)
}

async fn consume_event_inbox_row(
    db: &sea_orm::DatabaseConnection,
    row: &EventInboxProcessingRow,
) -> anyhow::Result<()> {
    match row.event_type.as_str() {
        "connector.delivery.received" | "connector.delivery.failed" => {
            apply_connector_delivery_inbox_receipt(db, row).await
        }
        event_type => anyhow::bail!(
            "unsupported event inbox type {event_type} for idempotency key {}",
            row.idempotency_key
        ),
    }
}

async fn apply_connector_delivery_inbox_receipt(
    db: &sea_orm::DatabaseConnection,
    row: &EventInboxProcessingRow,
) -> anyhow::Result<()> {
    let invocation_id = event_inbox_invocation_id(row)
        .ok_or_else(|| anyhow::anyhow!("event inbox row {} has no invocation_id", row.id))?;
    let previous = find_invocation_by_id(db, invocation_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("invocation {invocation_id} not found for inbox row {}", row.id))?;
    if previous.workspace_id != row.workspace_id {
        anyhow::bail!(
            "event inbox row {} workspace does not match invocation {invocation_id}",
            row.id
        );
    }
    if previous.project_id != row.project_id {
        anyhow::bail!(
            "event inbox row {} project does not match invocation {invocation_id}",
            row.id
        );
    }

    let receipt_status = receipt_status_for_inbox(row)?;
    let invocation_status = invocation_status_for_receipt(&receipt_status)?;
    let result_payload = if receipt_status == "failed" {
        json!({ "receipt": row.payload })
    } else {
        row.payload.clone()
    };
    let error_message = if receipt_status == "failed" {
        Some(
            row.payload
                .get("error_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("connector delivery failed")
                .to_string(),
        )
    } else {
        None
    };

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = CASE
                    WHEN status IN ('completed', 'failed', 'cancelled') THEN status
                    ELSE $2
                END,
                result = COALESCE($3, result),
                error_message = COALESCE($4, error_message),
                updated_at = now()
            WHERE id = $1
              AND workspace_id = $5
        ",
        vec![
            invocation_id.into(),
            invocation_status.to_string().into(),
            Some(result_payload).into(),
            error_message.into(),
            row.workspace_id.into(),
        ],
    ))
    .await?;

    if let Some(updated) = find_invocation_by_id(db, invocation_id).await?
        && previous.status != updated.status
    {
        insert_worker_invocation_business_event(
            db,
            &updated,
            invocation_event_type_for_status(&updated.status),
            Some(previous.status.as_str()),
            "event_inbox",
        )
        .await?;
    }

    Ok(())
}

async fn mark_event_inbox_processed(db: &sea_orm::DatabaseConnection, inbox_id: Uuid) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE event_inbox
            SET status = 'processed',
                last_error = NULL,
                processed_at = now(),
                updated_at = now()
            WHERE id = $1
        ",
        vec![inbox_id.into()],
    ))
    .await?;
    Ok(())
}

async fn mark_event_inbox_failed(db: &sea_orm::DatabaseConnection, inbox_id: Uuid, error: &str) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE event_inbox
            SET status = 'failed',
                last_error = left($2, 2000),
                updated_at = now()
            WHERE id = $1
        ",
        vec![inbox_id.into(), error.to_string().into()],
    ))
    .await?;
    Ok(())
}

fn event_inbox_invocation_id(row: &EventInboxProcessingRow) -> Option<Uuid> {
    row.payload
        .get("invocation_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn receipt_status_for_inbox(row: &EventInboxProcessingRow) -> anyhow::Result<String> {
    if let Some(status) = row.payload.get("receipt_status").and_then(serde_json::Value::as_str) {
        return normalize_receipt_status(status);
    }

    match row.event_type.as_str() {
        "connector.delivery.failed" => Ok("failed".to_string()),
        "connector.delivery.received" => Ok("completed".to_string()),
        event_type => anyhow::bail!("unsupported receipt event type {event_type}"),
    }
}

fn normalize_receipt_status(status: &str) -> anyhow::Result<String> {
    match status.trim() {
        "received" | "completed" | "failed" => Ok(status.trim().to_string()),
        _ => anyhow::bail!("invalid receipt status {status}"),
    }
}

fn invocation_status_for_receipt(receipt_status: &str) -> anyhow::Result<&'static str> {
    match receipt_status {
        "completed" => Ok("completed"),
        "failed" => Ok("failed"),
        "received" => Ok("running"),
        _ => anyhow::bail!("unsupported receipt status {receipt_status}"),
    }
}

async fn process_pending_event_outbox(db: &sea_orm::DatabaseConnection, concurrency: usize) -> anyhow::Result<()> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let events = pickup_pending_event_outbox(db, limit).await?;

    for event in events {
        match create_connector_invocations_for_event(db, &event).await {
            Ok(created_count) => {
                mark_event_outbox_dispatched(db, event.outbox_id, created_count).await?;
            }
            Err(err) => {
                tracing::warn!(
                    outbox_id = %event.outbox_id,
                    business_event_id = %event.business_event_id,
                    error = %err,
                    "event outbox dispatch failed"
                );
                mark_event_outbox_failed(db, event.outbox_id, &err.to_string()).await?;
            }
        }
    }

    Ok(())
}

async fn pickup_pending_event_outbox(
    db: &sea_orm::DatabaseConnection,
    limit: i64,
) -> anyhow::Result<Vec<EventOutboxDispatchRow>> {
    let events = EventOutboxDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT id
                FROM event_outbox
                WHERE status IN ('pending', 'failed')
                  AND attempts < max_attempts
                  AND available_at <= now()
                ORDER BY created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            ),
            updated AS (
                UPDATE event_outbox eo
                SET status = 'leased',
                    attempts = attempts + 1,
                    leased_until = now() + interval '60 seconds',
                    updated_at = now()
                FROM picked
                WHERE eo.id = picked.id
                RETURNING
                    eo.id AS outbox_id,
                    eo.business_event_id,
                    eo.workspace_id,
                    eo.project_id,
                    eo.event_type,
                    eo.aggregate_type,
                    eo.aggregate_id,
                    eo.payload
            )
            SELECT * FROM updated
        ",
        vec![limit.into()],
    ))
    .all(db)
    .await?;

    Ok(events)
}

async fn create_connector_invocations_for_event(
    db: &sea_orm::DatabaseConnection,
    event: &EventOutboxDispatchRow,
) -> anyhow::Result<u64> {
    let trigger_ref_id = Uuid::parse_str(&event.aggregate_id).ok();
    let payload = json!({
        "event": event.event_type,
        "business_event_id": event.business_event_id,
        "outbox_id": event.outbox_id,
        "envelope": event.payload
    });

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO agent_invocations (
                    id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                    trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                    status, payload, audit_chain_id, created_at, updated_at
                )
                SELECT
                    gen_random_uuid(),
                    c.workspace_id,
                    $2,
                    NULL,
                    NULL,
                    NULL,
                    'workflow',
                    $3,
                    $4,
                    c.id,
                    c.kind,
                    'pending',
                    $5,
                    $6,
                    now(),
                    now()
                FROM connectors c
                LEFT JOIN projects p ON p.id = $2::uuid
                WHERE c.workspace_id = $1
                  AND ($2::uuid IS NULL OR c.project_id IS NULL OR c.project_id = $2::uuid)
                  AND c.is_active = true
                  AND c.endpoint IS NOT NULL
                  AND c.kind IN ('webhook', 'rest', 'mcp', 'openprx_tunnel', 'print', 'device')
                  AND CASE
                    WHEN $7 LIKE 'invocation.%' THEN
                      c.capability_manifest ? 'events'
                      AND jsonb_typeof(c.capability_manifest->'events') = 'array'
                      AND (c.capability_manifest->'events') ? $7
                    WHEN NOT (c.capability_manifest ? 'events') THEN true
                    WHEN jsonb_typeof(c.capability_manifest->'events') <> 'array' THEN false
                    WHEN jsonb_array_length(c.capability_manifest->'events') = 0 THEN true
                    ELSE (c.capability_manifest->'events') ? $7
                  END
                  AND CASE
                    WHEN NOT (c.capability_manifest ? 'project_types') THEN true
                    WHEN jsonb_typeof(c.capability_manifest->'project_types') <> 'array' THEN false
                    WHEN jsonb_array_length(c.capability_manifest->'project_types') = 0 THEN true
                    ELSE (c.capability_manifest->'project_types') ? COALESCE(p.type_key, '')
                  END
                  AND CASE
                    WHEN NOT (c.capability_manifest ? 'form_keys') THEN true
                    WHEN jsonb_typeof(c.capability_manifest->'form_keys') <> 'array' THEN false
                    WHEN jsonb_array_length(c.capability_manifest->'form_keys') = 0 THEN true
                    ELSE (c.capability_manifest->'form_keys') ? COALESCE($5->'envelope'->'metadata'->>'form_key', '')
                  END
                  AND CASE
                    WHEN NOT (c.capability_manifest ? 'connector_kinds') THEN true
                    WHEN jsonb_typeof(c.capability_manifest->'connector_kinds') <> 'array' THEN false
                    WHEN jsonb_array_length(c.capability_manifest->'connector_kinds') = 0 THEN true
                    ELSE (c.capability_manifest->'connector_kinds') ? c.kind
                  END
                  AND NOT EXISTS (
                    SELECT 1
                    FROM agent_invocations ai
                    WHERE ai.audit_chain_id = $6
                      AND ai.connector_id = c.id
                  )
            ",
            vec![
                event.workspace_id.into(),
                event.project_id.into(),
                event.aggregate_type.clone().into(),
                trigger_ref_id.into(),
                payload.into(),
                event.business_event_id.into(),
                event.event_type.clone().into(),
            ],
        ))
        .await?;

    let created_count = result.rows_affected();
    let invocations = find_invocations_for_audit_chain(db, event.business_event_id).await?;
    for invocation in invocations {
        emit_invocation_event_if_missing(db, &invocation, "invocation.created", None, "event_outbox").await?;
    }

    Ok(created_count)
}

async fn mark_event_outbox_dispatched(
    db: &sea_orm::DatabaseConnection,
    outbox_id: Uuid,
    created_count: u64,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE event_outbox
            SET status = 'dispatched',
                leased_until = NULL,
                last_error = NULL,
                dispatched_at = now(),
                updated_at = now(),
                headers = headers || jsonb_build_object('connector_invocations_created', $2::bigint)
            WHERE id = $1
        ",
        vec![
            outbox_id.into(),
            i64::try_from(created_count).unwrap_or(i64::MAX).into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn mark_event_outbox_failed(
    db: &sea_orm::DatabaseConnection,
    outbox_id: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE event_outbox
            SET status = 'failed',
                leased_until = NULL,
                last_error = left($2, 2000),
                available_at = now() + make_interval(secs => LEAST(attempts * 30, 300)),
                updated_at = now()
            WHERE id = $1
        ",
        vec![outbox_id.into(), error.to_string().into()],
    ))
    .await?;
    Ok(())
}

async fn process_pending_connector_invocations(
    db: &sea_orm::DatabaseConnection,
    client: &reqwest::Client,
    concurrency: usize,
) -> anyhow::Result<()> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let invocations = pickup_pending_connector_invocations(db, limit).await?;

    for invocation in invocations {
        let outcome = dispatch_connector_invocation(client, &invocation).await;
        if outcome.success {
            update_connector_invocation_status(db, invocation.id, "dispatched", None, Some(outcome.result)).await?;
        } else {
            if let Some(error) = outcome.error_message.as_deref() {
                tracing::warn!(invocation_id = %invocation.id, error = %error, "connector invocation dispatch failed");
            }
            update_connector_invocation_status(
                db,
                invocation.id,
                "failed",
                outcome.error_message,
                Some(outcome.result),
            )
            .await?;
        }
    }

    Ok(())
}

async fn pickup_pending_connector_invocations(
    db: &sea_orm::DatabaseConnection,
    limit: i64,
) -> anyhow::Result<Vec<ConnectorInvocationDispatchRow>> {
    let invocations = ConnectorInvocationDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT
                    ai.id,
                    c.kind AS connector_kind,
                    c.name AS connector_name,
                    c.endpoint AS endpoint
                FROM agent_invocations ai
                INNER JOIN connectors c ON c.id = ai.connector_id
                WHERE ai.status = 'pending'
                  AND ai.source_task_id IS NULL
                  AND ai.connector_id IS NOT NULL
                  AND c.is_active = true
                  AND c.endpoint IS NOT NULL
                  AND c.kind IN ('webhook', 'rest', 'mcp', 'openprx_tunnel', 'print', 'device')
                ORDER BY ai.created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            ),
            updated AS (
                UPDATE agent_invocations ai
                SET status = 'running',
                    updated_at = now()
                FROM picked
                WHERE ai.id = picked.id
                RETURNING
                    ai.id,
                    ai.workspace_id,
                    ai.project_id,
                    ai.connector_id AS connector_id,
                    picked.connector_kind,
                    picked.connector_name,
                    picked.endpoint,
                    ai.trigger_kind,
                    ai.trigger_ref_type,
                    ai.trigger_ref_id,
                    ai.payload
            )
            SELECT * FROM updated
        ",
        vec![limit.into()],
    ))
    .all(db)
    .await?;

    for invocation in &invocations {
        if let Some(full_invocation) = find_invocation_by_id(db, invocation.id).await? {
            insert_worker_invocation_business_event(
                db,
                &full_invocation,
                "invocation.running",
                Some("pending"),
                "worker",
            )
            .await?;
        }
    }

    Ok(invocations)
}

async fn dispatch_connector_invocation(
    client: &reqwest::Client,
    invocation: &ConnectorInvocationDispatchRow,
) -> ConnectorDispatchOutcome {
    let body = connector_invocation_payload(invocation);
    let dispatched_at = chrono::Utc::now().to_rfc3339();
    match client.post(&invocation.endpoint).json(&body).send().await {
        Ok(response) => {
            let status_code = response.status().as_u16();
            let success = response.status().is_success();
            let response_body = truncate_diagnostic(response.text().await.unwrap_or_default(), 2_000);
            let error_message = if success {
                None
            } else {
                Some(format!(
                    "connector {} returned status {} body {}",
                    invocation.connector_id, status_code, response_body
                ))
            };
            ConnectorDispatchOutcome {
                success,
                result: connector_delivery_result(
                    invocation,
                    if success { "delivered" } else { "failed" },
                    Some(status_code),
                    Some(response_body),
                    error_message.as_deref(),
                    &dispatched_at,
                ),
                error_message,
            }
        }
        Err(err) => {
            let error_message = format!("connector {} delivery failed: {err}", invocation.connector_id);
            ConnectorDispatchOutcome {
                success: false,
                result: connector_delivery_result(
                    invocation,
                    "failed",
                    None,
                    None,
                    Some(&error_message),
                    &dispatched_at,
                ),
                error_message: Some(error_message),
            }
        }
    }
}

fn connector_delivery_result(
    invocation: &ConnectorInvocationDispatchRow,
    status: &str,
    status_code: Option<u16>,
    response_body: Option<String>,
    error_message: Option<&str>,
    dispatched_at: &str,
) -> serde_json::Value {
    json!({
        "connector_delivery": {
            "status": status,
            "endpoint": invocation.endpoint,
            "connector_id": invocation.connector_id.to_string(),
            "connector_kind": invocation.connector_kind,
            "connector_name": invocation.connector_name,
            "status_code": status_code,
            "response_body": response_body,
            "error_message": error_message,
            "dispatched_at": dispatched_at
        }
    })
}

fn truncate_diagnostic(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn connector_invocation_payload(invocation: &ConnectorInvocationDispatchRow) -> serde_json::Value {
    let mut body = json!({
        "event": invocation
            .payload
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("connector.invocation"),
        "invocation_id": invocation.id.to_string(),
        "workspace_id": invocation.workspace_id.to_string(),
        "project_id": invocation.project_id.to_string(),
        "connector_id": invocation.connector_id.to_string(),
        "connector_kind": invocation.connector_kind,
        "connector_name": invocation.connector_name,
        "trigger_kind": invocation.trigger_kind,
        "trigger_ref_type": invocation.trigger_ref_type,
        "trigger_ref_id": invocation.trigger_ref_id.map(|value| value.to_string()),
        "payload": invocation.payload,
    });

    if let Some(bot_context) = invocation.payload.get("bot_context")
        && let Some(object) = body.as_object_mut()
    {
        object.insert("bot_context".to_string(), bot_context.clone());
    }

    body
}

async fn update_connector_invocation_status(
    db: &sea_orm::DatabaseConnection,
    invocation_id: Uuid,
    status: &str,
    error_message: Option<String>,
    result: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let previous = find_invocation_by_id(db, invocation_id).await?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = $2,
                error_message = $3,
                result = COALESCE($4, result),
                updated_at = now()
            WHERE id = $1
              AND source_task_id IS NULL
              AND status NOT IN ('completed', 'failed', 'cancelled')
        ",
        vec![
            invocation_id.into(),
            status.to_string().into(),
            error_message.into(),
            result.into(),
        ],
    ))
    .await?;
    if let Some(updated) = find_invocation_by_id(db, invocation_id).await?
        && previous.as_ref().map(|invocation| invocation.status.as_str()) != Some(updated.status.as_str())
    {
        insert_worker_invocation_business_event(
            db,
            &updated,
            invocation_event_type_for_status(&updated.status),
            previous.as_ref().map(|invocation| invocation.status.as_str()),
            "worker",
        )
        .await?;
    }
    Ok(())
}

async fn process_pending_tasks(
    db: &sea_orm::DatabaseConnection,
    client: &reqwest::Client,
    concurrency: usize,
) -> anyhow::Result<()> {
    let limit = i64::try_from(concurrency.max(1)).unwrap_or(i64::MAX) * 10;
    let tasks = pickup_pending_tasks(db, limit).await?;

    if tasks.is_empty() {
        return Ok(());
    }

    for task in tasks {
        if let Err(err) = dispatch_task(db, client, &task).await {
            tracing::warn!(task_id = %task.id, error = %err, "dispatch failed");
            record_dispatch_failure(db, &task, err.to_string()).await?;
        }
    }

    Ok(())
}

async fn pickup_pending_tasks(db: &sea_orm::DatabaseConnection, limit: i64) -> anyhow::Result<Vec<AiTaskDispatchRow>> {
    let tasks = AiTaskDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            WITH picked AS (
                SELECT id
                FROM ai_tasks
                WHERE status = 'pending'
                  AND (next_retry_at IS NULL OR next_retry_at <= now())
                ORDER BY priority DESC, created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE ai_tasks t
            SET status = 'processing',
                attempts = attempts + 1,
                started_at = now(),
                next_retry_at = NULL,
                updated_at = now()
            FROM picked
            WHERE t.id = picked.id
            RETURNING
                t.id,
                (
                    SELECT workspace_id
                    FROM projects p
                    WHERE p.id = t.project_id
                ) AS workspace_id,
                t.project_id,
                t.ai_participant_id,
                (
                    SELECT name
                    FROM users u
                    WHERE u.id = t.ai_participant_id
                ) AS ai_participant_name,
                (
                    SELECT agent_type
                    FROM users u
                    WHERE u.id = t.ai_participant_id
                ) AS ai_participant_agent_type,
                t.task_type,
                t.reference_type,
                t.reference_id,
                t.payload,
                t.attempts,
                t.max_attempts,
                t.priority
        ",
        vec![limit.into()],
    ))
    .all(db)
    .await?;

    for task in &tasks {
        insert_task_event(
            db,
            task.id,
            "picked_up",
            json!({
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "priority": task.priority,
            }),
        )
        .await?;
        insert_worker_ai_task_business_event(
            db,
            task,
            "ai_task.picked_up",
            "pending",
            json!({
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "priority": task.priority,
            }),
        )
        .await?;
        update_invocation_for_task(db, task.id, "running", None, None).await?;
    }

    Ok(tasks)
}

async fn dispatch_task(
    db: &sea_orm::DatabaseConnection,
    client: &reqwest::Client,
    task: &AiTaskDispatchRow,
) -> anyhow::Result<()> {
    let webhook = BotWebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT
                w.id AS webhook_id,
                w.url,
                c.id AS connector_id,
                ai.id AS invocation_id
            FROM webhooks w
            INNER JOIN projects p ON p.workspace_id = w.workspace_id
            LEFT JOIN connectors c ON c.webhook_id = w.id
            LEFT JOIN agent_invocations ai ON ai.source_task_id = $3
            WHERE p.id = $1
              AND w.bot_user_id = $2
              AND w.active = true
            ORDER BY w.updated_at DESC
            LIMIT 1
        ",
        vec![task.project_id.into(), task.ai_participant_id.into(), task.id.into()],
    ))
    .one(db)
    .await?;

    let webhook = webhook.ok_or_else(|| {
        anyhow::anyhow!(
            "no active webhook found for bot {} in project {}",
            task.ai_participant_id,
            task.project_id
        )
    })?;

    let body = json!({
        "task_id": task.id.to_string(),
        "project_id": task.project_id.to_string(),
        "ai_participant_id": task.ai_participant_id.to_string(),
        "ai_participant_name": task.ai_participant_name.as_deref(),
        "ai_participant_agent_type": task.ai_participant_agent_type.as_deref(),
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id.map(|v| v.to_string()),
        "payload": task.payload,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "connector_id": webhook.connector_id.map(|v| v.to_string()),
        "invocation_id": webhook.invocation_id.map(|v| v.to_string()),
        "trigger_kind": trigger_kind_for_task(&task.task_type),
    });

    let response = client.post(&webhook.url).json(&body).send().await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "webhook {} returned status {} body {}",
            webhook.webhook_id,
            status,
            text
        ));
    }

    update_invocation_dispatch(db, task.id, webhook.connector_id).await?;

    Ok(())
}

async fn record_dispatch_failure(
    db: &sea_orm::DatabaseConnection,
    task: &AiTaskDispatchRow,
    error: String,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let should_retry = task.attempts < task.max_attempts;

    if should_retry {
        let next_retry_at = now + chrono::Duration::seconds(i64::from(task.attempts.max(1) * 30));
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE ai_tasks
                SET status = 'pending',
                    error_message = $2,
                    next_retry_at = $3,
                    updated_at = $4
                WHERE id = $1
            ",
            vec![task.id.into(), error.clone().into(), next_retry_at.into(), now.into()],
        ))
        .await?;

        insert_task_event(
            db,
            task.id,
            "retried",
            json!({
                "error_message": error.clone(),
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "next_retry_at": next_retry_at.to_rfc3339(),
            }),
        )
        .await?;
        insert_worker_ai_task_business_event(
            db,
            task,
            "ai_task.retried",
            "processing",
            json!({
                "error_message": error.clone(),
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "next_retry_at": next_retry_at.to_rfc3339(),
            }),
        )
        .await?;
        update_invocation_for_task(db, task.id, "pending", Some(error), None).await?;
    } else {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE ai_tasks
                SET status = 'failed',
                    error_message = $2,
                    completed_at = $3,
                    updated_at = $3
                WHERE id = $1
            ",
            vec![task.id.into(), error.clone().into(), now.into()],
        ))
        .await?;

        insert_task_event(
            db,
            task.id,
            "failed",
            json!({
                "error_message": error.clone(),
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
            }),
        )
        .await?;
        insert_worker_ai_task_business_event(
            db,
            task,
            "ai_task.failed",
            "processing",
            json!({
                "error_message": error.clone(),
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
            }),
        )
        .await?;
        update_invocation_for_task(db, task.id, "failed", Some(error), None).await?;
    }

    Ok(())
}

async fn update_invocation_dispatch(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    connector_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let previous = find_invocation_for_task(db, task_id).await?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = 'dispatched',
                connector_id = COALESCE($2, connector_id),
                connector_kind = CASE WHEN $2 IS NULL THEN connector_kind ELSE 'webhook' END,
                updated_at = now()
            WHERE source_task_id = $1
              AND status NOT IN ('completed', 'failed', 'cancelled')
        ",
        vec![task_id.into(), connector_id.into()],
    ))
    .await?;
    emit_invocation_event_for_task(
        db,
        task_id,
        "invocation.dispatched",
        previous.as_ref().map(|invocation| invocation.status.as_str()),
        "worker",
    )
    .await?;
    Ok(())
}

async fn update_invocation_for_task(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    status: &str,
    error_message: Option<String>,
    result: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let previous = find_invocation_for_task(db, task_id).await?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = $2,
                error_message = $3,
                result = COALESCE($4, result),
                updated_at = now()
            WHERE source_task_id = $1
              AND status NOT IN ('completed', 'failed', 'cancelled')
        ",
        vec![
            task_id.into(),
            status.to_string().into(),
            error_message.into(),
            result.into(),
        ],
    ))
    .await?;
    emit_invocation_event_for_task(
        db,
        task_id,
        invocation_event_type_for_status(status),
        previous.as_ref().map(|invocation| invocation.status.as_str()),
        "worker",
    )
    .await?;
    Ok(())
}

async fn find_invocation_for_task(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
) -> anyhow::Result<Option<InvocationEventRow>> {
    let invocation = InvocationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id
            FROM agent_invocations
            WHERE source_task_id = $1
        ",
        vec![task_id.into()],
    ))
    .one(db)
    .await?;
    Ok(invocation)
}

async fn find_invocation_by_id(
    db: &sea_orm::DatabaseConnection,
    invocation_id: Uuid,
) -> anyhow::Result<Option<InvocationEventRow>> {
    let invocation = InvocationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id
            FROM agent_invocations
            WHERE id = $1
        ",
        vec![invocation_id.into()],
    ))
    .one(db)
    .await?;
    Ok(invocation)
}

async fn find_invocations_for_audit_chain(
    db: &sea_orm::DatabaseConnection,
    audit_chain_id: Uuid,
) -> anyhow::Result<Vec<InvocationEventRow>> {
    let invocations = InvocationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id
            FROM agent_invocations
            WHERE audit_chain_id = $1
        ",
        vec![audit_chain_id.into()],
    ))
    .all(db)
    .await?;
    Ok(invocations)
}

async fn emit_invocation_event_for_task(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    event_type: &str,
    previous_status: Option<&str>,
    source_type: &str,
) -> anyhow::Result<()> {
    if let Some(invocation) = find_invocation_for_task(db, task_id).await?
        && previous_status != Some(invocation.status.as_str())
    {
        insert_worker_invocation_business_event(db, &invocation, event_type, previous_status, source_type).await?;
    }
    Ok(())
}

async fn emit_invocation_event_if_missing(
    db: &sea_orm::DatabaseConnection,
    invocation: &InvocationEventRow,
    event_type: &str,
    previous_status: Option<&str>,
    source_type: &str,
) -> anyhow::Result<()> {
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT 1
                FROM business_events
                WHERE event_type = $1
                  AND aggregate_type = 'invocation'
                  AND aggregate_id = $2
                LIMIT 1
            ",
            vec![event_type.to_string().into(), invocation.id.to_string().into()],
        ))
        .await?
        .is_some();

    if !exists {
        insert_worker_invocation_business_event(db, invocation, event_type, previous_status, source_type).await?;
    }
    Ok(())
}

async fn insert_worker_ai_task_business_event(
    db: &sea_orm::DatabaseConnection,
    task: &AiTaskDispatchRow,
    event_type: &str,
    previous_status: &str,
    event_payload: serde_json::Value,
) -> anyhow::Result<()> {
    let status = match event_type {
        "ai_task.retried" => "pending",
        "ai_task.failed" => "failed",
        _ => "processing",
    };
    let payload = json!({
        "task_id": task.id,
        "project_id": task.project_id,
        "ai_participant_id": task.ai_participant_id,
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id,
        "status": status,
        "previous_status": previous_status,
        "priority": task.priority,
        "payload": task.payload,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "event_payload": event_payload
    });
    let metadata = json!({
        "ai_task_id": task.id,
        "ai_participant_id": task.ai_participant_id,
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id,
        "status": status,
        "previous_status": previous_status,
        "worker_event": true
    });

    insert_business_event_and_outbox(
        db,
        WorkerBusinessEventInput {
            event_id: Uuid::new_v4(),
            outbox_id: Uuid::new_v4(),
            workspace_id: task.workspace_id,
            project_id: Some(task.project_id),
            event_type,
            aggregate_type: "ai_task",
            aggregate_id: task.id.to_string(),
            actor_id: None,
            source: json!({ "type": "worker" }),
            payload,
            metadata,
            correlation_id: Some(task.id),
            causation_id: task.reference_id,
        },
    )
    .await
}

async fn insert_worker_invocation_business_event(
    db: &sea_orm::DatabaseConnection,
    invocation: &InvocationEventRow,
    event_type: &str,
    previous_status: Option<&str>,
    source_type: &str,
) -> anyhow::Result<()> {
    let payload = json!({
        "invocation_id": invocation.id,
        "workspace_id": invocation.workspace_id,
        "project_id": invocation.project_id,
        "actor_id": invocation.actor_id,
        "target_agent_id": invocation.target_agent_id,
        "source_task_id": invocation.source_task_id,
        "trigger_kind": invocation.trigger_kind,
        "trigger_ref_type": invocation.trigger_ref_type,
        "trigger_ref_id": invocation.trigger_ref_id,
        "connector_id": invocation.connector_id,
        "connector_kind": invocation.connector_kind,
        "status": invocation.status,
        "previous_status": previous_status,
        "payload": invocation.payload,
        "result": invocation.result,
        "error_message": invocation.error_message,
        "audit_chain_id": invocation.audit_chain_id
    });
    let metadata = json!({
        "invocation_id": invocation.id,
        "source_task_id": invocation.source_task_id,
        "trigger_kind": invocation.trigger_kind,
        "connector_id": invocation.connector_id,
        "connector_kind": invocation.connector_kind,
        "previous_status": previous_status,
        "status": invocation.status,
        "worker_event": true
    });

    insert_business_event_and_outbox(
        db,
        WorkerBusinessEventInput {
            event_id: Uuid::new_v4(),
            outbox_id: Uuid::new_v4(),
            workspace_id: invocation.workspace_id,
            project_id: invocation.project_id,
            event_type,
            aggregate_type: "invocation",
            aggregate_id: invocation.id.to_string(),
            actor_id: None,
            source: json!({ "type": source_type }),
            payload,
            metadata,
            correlation_id: invocation.audit_chain_id,
            causation_id: invocation.source_task_id,
        },
    )
    .await
}

struct WorkerBusinessEventInput<'a> {
    event_id: Uuid,
    outbox_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    event_type: &'a str,
    aggregate_type: &'a str,
    aggregate_id: String,
    actor_id: Option<Uuid>,
    source: serde_json::Value,
    payload: serde_json::Value,
    metadata: serde_json::Value,
    correlation_id: Option<Uuid>,
    causation_id: Option<Uuid>,
}

async fn insert_business_event_and_outbox(
    db: &sea_orm::DatabaseConnection,
    input: WorkerBusinessEventInput<'_>,
) -> anyhow::Result<()> {
    let envelope = json!({
        "version": "openpr.event.v1",
        "event_id": input.event_id,
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
        "idempotency_key": null,
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NULL, now())
        ",
        vec![
            input.event_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.to_string().into(),
            input.aggregate_type.to_string().into(),
            input.aggregate_id.clone().into(),
            input.actor_id.into(),
            input.source.into(),
            input.payload.into(),
            input.metadata.into(),
            input.correlation_id.into(),
            input.causation_id.into(),
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
            input.outbox_id.into(),
            input.event_id.into(),
            input.workspace_id.into(),
            input.project_id.into(),
            input.event_type.to_string().into(),
            input.aggregate_type.to_string().into(),
            input.aggregate_id.into(),
            envelope.into(),
            headers.into(),
        ],
    ))
    .await?;
    Ok(())
}

fn invocation_event_type_for_status(status: &str) -> &'static str {
    match status {
        "running" => "invocation.running",
        "completed" => "invocation.completed",
        "failed" => "invocation.failed",
        "cancelled" => "invocation.cancelled",
        "dispatched" => "invocation.dispatched",
        _ => "invocation.status_changed",
    }
}

fn trigger_kind_for_task(task_type: &str) -> &'static str {
    match task_type {
        "issue_assigned" => "assigned",
        "vote_requested" => "proposal_vote",
        "review_requested" | "comment_requested" => "mention",
        _ => "manual",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EventInboxProcessingRow, event_inbox_invocation_id, invocation_status_for_receipt, receipt_status_for_inbox,
    };
    use serde_json::json;
    use uuid::Uuid;

    fn inbox_row(event_type: &str, payload: serde_json::Value) -> EventInboxProcessingRow {
        EventInboxProcessingRow {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Some(Uuid::new_v4()),
            source_kind: "connector.webhook".to_string(),
            source_id: Some(Uuid::new_v4().to_string()),
            idempotency_key: "form:invocation:completed".to_string(),
            event_type: event_type.to_string(),
            payload,
            attempts: 0,
        }
    }

    #[test]
    fn inbox_receipt_status_prefers_payload_status() {
        let row = inbox_row(
            "connector.delivery.received",
            json!({ "receipt_status": "received", "invocation_id": Uuid::new_v4() }),
        );

        assert!(matches!(receipt_status_for_inbox(&row).as_deref(), Ok("received")));
        assert!(matches!(invocation_status_for_receipt("received"), Ok("running")));
    }

    #[test]
    fn inbox_receipt_status_defaults_from_event_type() {
        let failed = inbox_row("connector.delivery.failed", json!({ "invocation_id": Uuid::new_v4() }));
        let completed = inbox_row(
            "connector.delivery.received",
            json!({ "invocation_id": Uuid::new_v4() }),
        );

        assert!(matches!(receipt_status_for_inbox(&failed).as_deref(), Ok("failed")));
        assert!(matches!(
            receipt_status_for_inbox(&completed).as_deref(),
            Ok("completed")
        ));
        assert!(matches!(invocation_status_for_receipt("failed"), Ok("failed")));
        assert!(matches!(invocation_status_for_receipt("completed"), Ok("completed")));
    }

    #[test]
    fn inbox_invocation_id_is_read_from_payload() {
        let invocation_id = Uuid::new_v4();
        let row = inbox_row(
            "connector.delivery.received",
            json!({ "receipt_status": "completed", "invocation_id": invocation_id }),
        );

        assert_eq!(event_inbox_invocation_id(&row), Some(invocation_id));
    }
}

async fn insert_task_event(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO ai_task_events (id, task_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, $5)
        ",
        vec![
            Uuid::new_v4().into(),
            task_id.into(),
            event_type.to_string().into(),
            payload.into(),
            chrono::Utc::now().into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    res = tokio::signal::ctrl_c() => {
                        if let Err(err) = res {
                            tracing::warn!(error = %err, "ctrl_c signal error");
                        }
                    },
                    _ = sigterm.recv() => {},
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to register SIGTERM handler, falling back to ctrl_c only");
                if let Err(err) = tokio::signal::ctrl_c().await {
                    tracing::warn!(error = %err, "ctrl_c signal error");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c signal error");
        }
    }
}
