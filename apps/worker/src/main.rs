use clap::Parser;
use platform::{app::connect_db, config::AppConfig, logging};
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
    project_id: Uuid,
    ai_participant_id: Uuid,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let cfg = AppConfig::from_env("worker", "0.0.0.0:8081")?;
    logging::init("worker");

    let db = connect_db(&cfg.database_url).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    tracing::info!(concurrency = args.concurrency, app = %cfg.app_name, "worker started");

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            result = process_pending_tasks(&db, &client, args.concurrency) => {
                if let Err(err) = result {
                    tracing::warn!(error = %err, "task polling failed");
                }
            }
        }

        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }

    Ok(())
}

async fn process_pending_tasks(
    db: &sea_orm::DatabaseConnection,
    client: &reqwest::Client,
    concurrency: usize,
) -> anyhow::Result<()> {
    let limit = (concurrency.max(1) as i64) * 10;
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

async fn pickup_pending_tasks(
    db: &sea_orm::DatabaseConnection,
    limit: i64,
) -> anyhow::Result<Vec<AiTaskDispatchRow>> {
    let tasks = AiTaskDispatchRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
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
                t.project_id,
                t.ai_participant_id,
                t.task_type,
                t.reference_type,
                t.reference_id,
                t.payload,
                t.attempts,
                t.max_attempts,
                t.priority
        "#,
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
        r#"
            SELECT w.id AS webhook_id, w.url
            FROM webhooks w
            INNER JOIN projects p ON p.workspace_id = w.workspace_id
            WHERE p.id = $1
              AND w.bot_user_id = $2
              AND w.active = true
            ORDER BY w.updated_at DESC
            LIMIT 1
        "#,
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
        "task_type": task.task_type,
        "reference_type": task.reference_type,
        "reference_id": task.reference_id.map(|v| v.to_string()),
        "payload": task.payload,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
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
        let next_retry_at = now + chrono::Duration::seconds((task.attempts.max(1) * 30) as i64);
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE ai_tasks
                SET status = 'pending',
                    error_message = $2,
                    next_retry_at = $3,
                    updated_at = $4
                WHERE id = $1
            "#,
            vec![
                task.id.into(),
                error.clone().into(),
                next_retry_at.into(),
                now.into(),
            ],
        ))
        .await?;

        insert_task_event(
            db,
            task.id,
            "retried",
            json!({
                "error_message": error,
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "next_retry_at": next_retry_at.to_rfc3339(),
            }),
        )
        .await?;
    } else {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                UPDATE ai_tasks
                SET status = 'failed',
                    error_message = $2,
                    completed_at = $3,
                    updated_at = $3
                WHERE id = $1
            "#,
            vec![task.id.into(), error.clone().into(), now.into()],
        ))
        .await?;

        insert_task_event(
            db,
            task.id,
            "failed",
            json!({
                "error_message": error,
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
            }),
        )
        .await?;
    }

    Ok(())
}

async fn insert_task_event(
    db: &sea_orm::DatabaseConnection,
    task_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            INSERT INTO ai_task_events (id, task_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, $5)
        "#,
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
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
