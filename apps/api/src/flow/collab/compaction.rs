//! v0.8 document compaction.
//!
//! A candidate is rebuilt and round-trip verified outside a transaction by [`super::snapshot`].
//! The short transaction below then locks the authoritative document row, rechecks the exact
//! head, persists snapshot/checksum/boundary, and either retains old updates for a lagging client
//! or marks those clients for resync before pruning.  The `PostgreSQL` head is never derived from a
//! fan-out message and is never modified by compaction.

use collab_core::CollabEngine as _;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::{bootstrap, snapshot};
use crate::error::ApiError;

/// Ack rows older than this are not active compaction blockers. Reconnecting clients still fail
/// closed to a full snapshot because resume refuses any `known_seq < snapshot_seq`.
pub const ACTIVE_ACK_WINDOW_SECONDS: i64 = 120;

#[derive(FromQueryResult)]
struct FrontierRow {
    frontier: Vec<u8>,
}

#[derive(FromQueryResult)]
struct LockedDocument {
    head_seq: i64,
    head_frontier: Vec<u8>,
}

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDisposition {
    Prune,
    RetainForLaggingClients,
    ForceResyncAndPrune,
}

#[must_use]
pub const fn history_disposition(lagging_clients: u64, force_resync: bool) -> HistoryDisposition {
    if lagging_clients == 0 {
        HistoryDisposition::Prune
    } else if force_resync {
        HistoryDisposition::ForceResyncAndPrune
    } else {
        HistoryDisposition::RetainForLaggingClients
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub document_id: Uuid,
    pub head_seq: i64,
    pub head_frontier: Vec<u8>,
    pub semantic_hash: String,
    pub snapshot_checksum: String,
    pub deleted_updates: u64,
    pub lagging_clients: u64,
    pub forced_resync_clients: u64,
    pub disposition: HistoryDisposition,
}

/// Registers the start of a live client view before its opening frames are sent.  Seq zero is
/// intentionally conservative: until a verified ack arrives this lease blocks history pruning.
pub async fn begin_client_view(db: &DatabaseConnection, document_id: Uuid, client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() {
        return Err(ApiError::BadRequest("client_id is required".to_string()));
    }
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_collab_client_acks \
             (document_id, client_id, ack_seq, ack_frontier, resync_required, resync_reason, last_seen_at) \
         VALUES ($1, $2, 0, ''::bytea, false, NULL, now()) \
         ON CONFLICT (document_id, client_id) DO UPDATE \
           SET ack_seq = 0, ack_frontier = ''::bytea, resync_required = false, \
               resync_reason = NULL, last_seen_at = now()",
        vec![document_id.into(), client_id.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// Persists a verified, monotonic client acknowledgement for cross-instance compaction safety.
///
/// A claimed frontier is checked against `PostgreSQL` at that exact seq; a client cannot make old
/// updates eligible for deletion by merely sending a large seq with invented bytes.
pub async fn record_client_ack(
    db: &DatabaseConnection,
    document_id: Uuid,
    client_id: &str,
    seq: i64,
    frontier: &[u8],
) -> Result<bool, ApiError> {
    if client_id.trim().is_empty() || seq < 0 {
        return Ok(false);
    }
    let expected = FrontierRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT frontier FROM ( \
           SELECT head_frontier AS frontier FROM collab_documents WHERE id = $1 AND head_seq = $2 \
           UNION ALL \
           SELECT snapshot_frontier AS frontier FROM collab_documents WHERE id = $1 AND snapshot_seq = $2 \
           UNION ALL \
           SELECT after_frontier AS frontier FROM collab_updates WHERE document_id = $1 AND seq = $2 \
         ) acknowledged LIMIT 1",
        vec![document_id.into(), seq.into()],
    ))
    .one(db)
    .await?;
    if expected.is_none_or(|row| row.frontier != frontier) {
        return Ok(false);
    }

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_collab_client_acks \
             (document_id, client_id, ack_seq, ack_frontier, last_seen_at) \
         VALUES ($1, $2, $3, $4, now()) \
         ON CONFLICT (document_id, client_id) DO UPDATE \
           SET ack_seq = EXCLUDED.ack_seq, ack_frontier = EXCLUDED.ack_frontier, last_seen_at = now() \
         WHERE flow_collab_client_acks.ack_seq < EXCLUDED.ack_seq \
           AND NOT flow_collab_client_acks.resync_required",
        vec![
            document_id.into(),
            client_id.to_string().into(),
            seq.into(),
            frontier.to_vec().into(),
        ],
    ))
    .await?;
    Ok(true)
}

/// Compacts one exact document head.
///
/// `expected_head_seq` is mandatory on execute surfaces. A mismatch returns `stale_frontier`
/// before any mutation. `force_resync=false` never deletes history needed by a recently seen ack;
/// `true` first persists the resync obligation in the same transaction that prunes it.
pub async fn compact(
    db: &DatabaseConnection,
    document_id: Uuid,
    expected_head_seq: i64,
    force_resync: bool,
) -> Result<CompactionResult, ApiError> {
    let boot = bootstrap::load(db, document_id).await?;
    if boot.head_seq != expected_head_seq {
        return Err(ApiError::Conflict("stale_frontier".to_string()));
    }

    let candidate = if boot.snapshot_seq < boot.head_seq {
        snapshot::build_candidate(db, document_id)
            .await?
            .ok_or(ApiError::Internal)?
    } else {
        let engine = collab_core::LoroCollabEngine::load(&boot.snapshot).map_err(|error| {
            tracing::error!(%document_id, %error, "compaction: current snapshot failed to decode");
            ApiError::Internal
        })?;
        let semantic_hash = engine
            .semantic_snapshot()
            .map_err(|error| {
                tracing::error!(%document_id, %error, "compaction: current snapshot semantic read failed");
                ApiError::Internal
            })?
            .semantic_hash();
        snapshot::Candidate {
            head_seq: boot.head_seq,
            head_frontier: boot.head_frontier,
            snapshot_checksum: bootstrap::content_hash(&boot.snapshot),
            semantic_hash,
            snapshot_bytes: boot.snapshot,
            rebuild_wall_ms: 0,
        }
    };

    if bootstrap::content_hash(&candidate.snapshot_bytes) != candidate.snapshot_checksum {
        return Err(ApiError::Internal);
    }

    let tx = db.begin().await?;
    let locked = LockedDocument::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT head_seq, head_frontier FROM collab_documents WHERE id = $1 FOR UPDATE",
        vec![document_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("document not found".to_string()))?;
    if locked.head_seq != expected_head_seq
        || locked.head_seq != candidate.head_seq
        || locked.head_frontier != candidate.head_frontier
    {
        tx.rollback().await?;
        return Err(ApiError::Conflict("stale_frontier".to_string()));
    }

    let lagging = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::bigint AS count FROM flow_collab_client_acks \
         WHERE document_id = $1 AND NOT resync_required AND ack_seq < $2 \
           AND last_seen_at >= now() - make_interval(secs => $3::double precision)",
        vec![
            document_id.into(),
            candidate.head_seq.into(),
            ACTIVE_ACK_WINDOW_SECONDS.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?;
    let lagging_clients = u64::try_from(lagging.count).unwrap_or(0);
    let disposition = history_disposition(lagging_clients, force_resync);

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE collab_documents SET snapshot = $2, snapshot_frontier = $3, snapshot_seq = $4, \
         snapshot_checksum = $5, compaction_boundary_seq = $4, compaction_boundary_frontier = $3, \
         compaction_generation = compaction_generation + 1, last_compacted_at = now(), updated_at = now() \
         WHERE id = $1",
        vec![
            document_id.into(),
            candidate.snapshot_bytes.clone().into(),
            candidate.head_frontier.clone().into(),
            candidate.head_seq.into(),
            candidate.snapshot_checksum.clone().into(),
        ],
    ))
    .await?;

    let forced_resync_clients = if disposition == HistoryDisposition::ForceResyncAndPrune {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_collab_client_acks SET resync_required = true, \
             resync_reason = 'compaction_boundary', last_seen_at = now() \
             WHERE document_id = $1 AND NOT resync_required AND ack_seq < $2 \
               AND last_seen_at >= now() - make_interval(secs => $3::double precision)",
            vec![
                document_id.into(),
                candidate.head_seq.into(),
                ACTIVE_ACK_WINDOW_SECONDS.into(),
            ],
        ))
        .await?
        .rows_affected()
    } else {
        0
    };

    let deleted_updates = if disposition == HistoryDisposition::RetainForLaggingClients {
        0
    } else {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM collab_updates WHERE document_id = $1 AND seq <= $2",
            vec![document_id.into(), candidate.head_seq.into()],
        ))
        .await?
        .rows_affected()
    };
    tx.commit().await?;

    Ok(CompactionResult {
        document_id,
        head_seq: candidate.head_seq,
        head_frontier: candidate.head_frontier,
        semantic_hash: candidate.semantic_hash,
        snapshot_checksum: candidate.snapshot_checksum,
        deleted_updates,
        lagging_clients,
        forced_resync_clients,
        disposition,
    })
}

#[cfg(test)]
mod tests {
    use super::{HistoryDisposition, history_disposition};

    #[test]
    fn no_lagging_client_prunes() {
        assert_eq!(history_disposition(0, false), HistoryDisposition::Prune);
        assert_eq!(history_disposition(0, true), HistoryDisposition::Prune);
    }

    #[test]
    fn lagging_client_retains_history_without_force() {
        assert_eq!(
            history_disposition(1, false),
            HistoryDisposition::RetainForLaggingClients
        );
    }

    #[test]
    fn force_resync_is_not_a_blanket_error_tolerance() {
        assert_eq!(history_disposition(1, true), HistoryDisposition::ForceResyncAndPrune);
        assert_ne!(
            history_disposition(1, false),
            HistoryDisposition::ForceResyncAndPrune,
            "a missing force flag must not silently swallow a lagging-client safety violation"
        );
    }
}
