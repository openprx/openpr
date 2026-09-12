//! Worker-owned v0.8 compaction scheduler. Hot collaboration never enters this module: it scans
//! persisted document facts and invokes the same exact-head compaction service as admin surfaces.

use api::flow::collab::{compaction, limits, snapshot};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

const MAX_BATCH_SIZE: usize = 100;

#[derive(Debug, FromQueryResult)]
struct CandidateRow {
    document_id: Uuid,
    head_seq: i64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    pub examined: u64,
    pub compacted: u64,
    pub retained_for_acks: u64,
    pub failed: u64,
}

/// Scans a bounded set selected by real persisted update count/bytes, then re-evaluates the same
/// production thresholds immediately before compaction. The scheduler never invents a separate
/// threshold and never force-resyncs a client; only an explicit admin execution may do that.
pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<TickReport> {
    let limit = requested_batch_size.clamp(1, MAX_BATCH_SIZE);
    let rows = CandidateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT cd.id AS document_id, cd.head_seq \
           FROM collab_documents cd \
          WHERE cd.head_seq > cd.compaction_boundary_seq \
            AND (cd.head_seq - cd.snapshot_seq >= $1 OR \
                 COALESCE((SELECT sum(octet_length(cu.bytes)) FROM collab_updates cu \
                            WHERE cu.document_id = cd.id AND cu.seq > cd.snapshot_seq), 0) >= $2) \
          ORDER BY cd.updated_at ASC, cd.id ASC LIMIT $3",
        vec![
            limits::SNAPSHOT_TAIL_UPDATES_SOFT_MAX.into(),
            limits::SNAPSHOT_TAIL_BYTES_SOFT_MAX.into(),
            i64::try_from(limit).unwrap_or(i64::MAX).into(),
        ],
    ))
    .all(db)
    .await?;

    let mut report = TickReport::default();
    for row in rows {
        report.examined += 1;
        let Some(stats) = snapshot::read_tail_stats(db, row.document_id).await? else {
            report.failed += 1;
            continue;
        };
        if snapshot::evaluate(&stats, None) == snapshot::Trigger::None {
            continue;
        }
        match compaction::compact(db, row.document_id, row.head_seq, false).await {
            Ok(result) => {
                report.compacted += 1;
                if result.disposition == compaction::HistoryDisposition::RetainForLaggingClients {
                    report.retained_for_acks += 1;
                }
            }
            Err(error) => {
                report.failed += 1;
                tracing::warn!(document_id = %row.document_id, %error, "flow compaction tick failed");
            }
        }
    }
    Ok(report)
}
