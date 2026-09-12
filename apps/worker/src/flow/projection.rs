//! Worker-owned projection rebuild jobs.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
struct Job {
    id: Uuid,
    scope_id: Uuid,
    dry_run: bool,
    expected_head_seq: Option<i64>,
}

#[derive(Debug, Default)]
pub struct TickReport {
    pub completed: u64,
    pub failed: u64,
}

pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<TickReport> {
    let mut report = TickReport::default();
    for _ in 0..requested_batch_size.clamp(1, 100) {
        let job = Job::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE flow_operation_runs SET status = 'running' \
             WHERE id = (SELECT id FROM flow_operation_runs \
               WHERE operation = 'rebuild_projection' AND status = 'planned' \
               ORDER BY created_at, id FOR UPDATE SKIP LOCKED LIMIT 1) \
             RETURNING id, scope_id, dry_run, expected_head_seq"
                .to_string(),
        ))
        .one(db)
        .await?;
        let Some(job) = job else { break };

        let object_id: Option<Uuid> = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT object_id FROM collab_documents WHERE id = $1",
                vec![job.scope_id.into()],
            ))
            .await?
            .and_then(|row| row.try_get("", "object_id").ok());
        let outcome = if let Some(object_id) = object_id {
            api::flow::maintenance::rebuild_projection(db, object_id, job.expected_head_seq, !job.dry_run)
                .await
                .map(|result| serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({})))
        } else {
            Err(api::error::ApiError::NotFound("collab document not found".to_string()))
        };
        match outcome {
            Ok(result) => {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_operation_runs SET status = 'completed', result_redacted = $2, \
                     finished_at = now() WHERE id = $1",
                    vec![job.id.into(), result.into()],
                ))
                .await?;
                report.completed += 1;
            }
            Err(error) => {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_operation_runs SET status = 'failed', \
                     result_redacted = jsonb_build_object('reason', 'projection_rebuild_failed'), \
                     finished_at = now() WHERE id = $1",
                    vec![job.id.into()],
                ))
                .await?;
                tracing::warn!(job_id = %job.id, %error, "flow projection rebuild job failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}
