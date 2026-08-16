use std::path::PathBuf;

use api::config::RuntimeConfig;
use api::outbound::validate_outbound_url;
use clap::Parser;
use platform::{
    app::{AppState, connect_db},
    config::{AppConfig, OpenPrConfig},
    logging,
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

/// Operation-log retention runs once per day; the first run happens at worker startup.
const OPERATION_LOG_CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_hours(24);

#[derive(Debug, Parser)]
struct WorkerArgs {
    /// Path of the configuration file. Defaults to `config/openpr.toml` relative to the working
    /// directory. The worker reads no environment variables, so this file is its only input.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
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
}

/// Publishes the configuration file to the API library this binary links, and hands back the
/// installed value.
///
/// The worker is a second process, so it owns its own copy of the API library's process global and
/// has to install into it exactly like `api::main` does. It used to skip this, which left every
/// code path the worker reaches through that library running on `RuntimeConfig::fallback`: local
/// object storage under `./uploads` and an empty outbound allowlist. The
/// form export, form import and attachment packaging jobs the polling loop runs are API library
/// code that reads object storage that way, so an operator who pointed `[storage] dir` somewhere
/// else — or switched the backend to S3 — got a worker writing to a different place than the API
/// read from, with nothing on either side reporting a problem.
///
/// The installed value is returned rather than left for the caller to fetch so that startup cannot
/// run jobs against a configuration it forgot to publish.
fn install_runtime_config(config: &OpenPrConfig) -> anyhow::Result<&'static RuntimeConfig> {
    // A second call is an error, not a no-op, and a worker that cannot publish its configuration
    // must fail here rather than poll with the fallback settings.
    api::config::install(config).map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(api::config::runtime())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let config = OpenPrConfig::load(args.config.as_deref())?;
    let operation_log_retention_days = i32::try_from(config.audit.operation_log_retention_days)
        .map_err(|_| anyhow::anyhow!("audit.operation_log_retention_days exceeds the worker range"))?;
    // Installed before the logger, as in `api::main`: a configuration this process cannot publish
    // aborts startup instead of leaving the jobs it runs on the fallback settings.
    let runtime = install_runtime_config(&config)?;
    // Initialised only once the file parsed, so a configuration error is reported by the process
    // exit rather than swallowed by a subscriber the file was supposed to describe.
    logging::init(&config.logging, "worker")?;
    // from_config first: it reports a missing database url and signing key together, so the
    // operator fixes both in one pass instead of being walked through them one at a time.
    let cfg = AppConfig::from_config(&config, "worker", "0.0.0.0:8081")?;
    let db = connect_db(&config.database_runtime()?).await?;
    let state = AppState {
        cfg: cfg.clone(),
        db: db.clone(),
    };
    // Redirects are disabled so a validated public endpoint cannot bounce the request into the
    // internal network after the target checks have already passed.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    // The storage settings are logged because the form jobs below write through them and there is
    // otherwise no way to tell from outside which of the two processes reads which directory.
    // Only the backend name and the local root are logged; the S3 section carries credentials.
    tracing::info!(
        concurrency = args.concurrency,
        app = %cfg.app_name,
        storage_backend = ?runtime.storage.backend,
        storage_dir = %runtime.storage.dir.display(),
        "worker started"
    );

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut next_operation_log_cleanup = tokio::time::Instant::now();

    loop {
        if tokio::time::Instant::now() >= next_operation_log_cleanup {
            match cleanup_bot_operation_logs(&db, operation_log_retention_days).await {
                Ok(deleted) => tracing::info!(
                    deleted,
                    retention_days = operation_log_retention_days,
                    "bot operation log retention completed"
                ),
                Err(error) => tracing::warn!(error = %error, "bot operation log retention failed"),
            }
            next_operation_log_cleanup = tokio::time::Instant::now() + OPERATION_LOG_CLEANUP_INTERVAL;
        }

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

        if let Err(err) =
            api::routes::form::process_pending_form_jobs_from_worker(state.clone(), args.concurrency).await
        {
            tracing::warn!(error = %err, "form job polling failed");
        }

        // Proposal settlement lives here rather than on the API read path, where an unauthorized
        // `GET` used to open a write transaction. The tick is idempotent and guarded by a
        // per-proposal advisory lock, so several worker replicas may run it concurrently.
        if let Err(err) = api::routes::proposal::governance_tick(&state).await {
            tracing::warn!(error = %err, "governance polling failed");
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

async fn cleanup_bot_operation_logs(db: &sea_orm::DatabaseConnection, retention_days: i32) -> anyhow::Result<u64> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM bot_operation_logs WHERE created_at < now() - make_interval(days => $1::int)",
            vec![retention_days.into()],
        ))
        .await?;
    Ok(result.rows_affected())
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
                w.url
            FROM webhooks w
            INNER JOIN projects p ON p.workspace_id = w.workspace_id
            WHERE p.id = $1
              AND w.bot_user_id = $2
              AND w.active = true
            ORDER BY w.updated_at DESC
            LIMIT 1
        ",
        vec![task.project_id.into(), task.ai_participant_id.into()],
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
        "trigger_kind": trigger_kind_for_task(&task.task_type),
    });

    let target = validate_outbound_url(&webhook.url)
        .await
        .map_err(|err| anyhow::anyhow!("webhook {} url rejected: {err}", webhook.webhook_id))?;

    let response = client.post(target).json(&body).send().await?;
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

fn trigger_kind_for_task(task_type: &str) -> &'static str {
    match task_type {
        "issue_assigned" => "assigned",
        "vote_requested" => "proposal_vote",
        "review_requested" | "comment_requested" => "mention",
        _ => "manual",
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
