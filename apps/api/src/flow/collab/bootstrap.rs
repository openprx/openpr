//! The consistency loader `collab-protocol-v1.md` requires the WebSocket `snapshot` frame (and,
//! were it in scope, `GET .../bootstrap`) to share: a single `PostgreSQL` `REPEATABLE READ READ
//! ONLY` transaction reads `collab_documents` first, then the exact `(snapshot_seq,head_seq]`
//! tail, verifies seq continuity and frontier/hash chaining, and fails the whole call closed on
//! any gap or corruption rather than returning a partial tail.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use sea_orm::{AccessMode, DatabaseConnection, FromQueryResult, IsolationLevel, Statement, TransactionTrait};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::limits;
use crate::error::ApiError;
use crate::flow::repository::{self, IntegrityRecordInput};

#[derive(Debug)]
pub struct TailUpdateRow {
    pub seq: i64,
    pub update_id: Uuid,
    pub bytes: Vec<u8>,
    pub before_frontier: Vec<u8>,
    pub after_frontier: Vec<u8>,
}

#[derive(Debug)]
pub struct BootstrapResult {
    pub document_id: Uuid,
    pub engine: String,
    pub format_version: String,
    pub snapshot_seq: i64,
    pub head_seq: i64,
    pub snapshot: Vec<u8>,
    pub tail_updates: Vec<TailUpdateRow>,
    pub head_frontier: Vec<u8>,
}

pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// `bootstrap_decoded_bytes_max` (`limits-v1.md`): "snapshot+tail 上限约占 v0.3 256 MiB browser
/// peak budget的 3%；超出要求 compaction/resync". Checked against `snapshot.len() + sum(tail
/// update bytes)` -- [`limits::BOOTSTRAP_DECODED_BYTES_MAX`]'s own doc comment names this exact
/// formula as the single definition of "decoded bytes" for this ceiling.
///
/// # Errors
/// `ApiError::limit_exceeded` with `limit_kind = "bootstrap_decoded_bytes"` when `observed`
/// exceeds the fixed ceiling (`error-mapping-v1.md`: `limit_exceeded` -> 400, not
/// `resync_required` -- a document merely being large is not a corrupted tail).
pub fn check_bootstrap_decoded_bytes(observed: u64) -> Result<(), ApiError> {
    if observed > limits::BOOTSTRAP_DECODED_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "limit_exceeded: bootstrap_decoded_bytes",
            "bootstrap_decoded_bytes",
            Some(json!(limits::BOOTSTRAP_DECODED_BYTES_MAX)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(())
}

/// The exact byte length base64 (`base64::engine::general_purpose::STANDARD`, the encoding every
/// bootstrap wire response in this package uses -- `session.rs`'s `Frame::Snapshot` and
/// `query.rs::get_bootstrap`'s `Bootstrap` both encode the identical fields this function sums)
/// produces for `n` raw bytes: `4 * ceil(n / 3)`, with no padding-dependent branching needed since
/// that formula already accounts for it.
const fn base64_len(n: usize) -> u64 {
    (n as u64).div_ceil(3).saturating_mul(4)
}

/// A real, purely length-derived worst-case upper bound on the serialized (base64 + JSON
/// envelope) bytes a bootstrap response built from `snapshot`/`tail_updates`/`head_frontier` would
/// produce -- summing the deterministic base64 expansion of every byte field the wire response
/// actually carries, plus a conservative fixed per-record JSON-structure allowance
/// (`BOOTSTRAP_TAIL_UPDATE_JSON_OVERHEAD_BYTES`) so this stays a real second constraint even when
/// many small tail updates keep `bootstrap_decoded_bytes` low but multiply per-record envelope
/// overhead.
fn estimated_response_bytes(snapshot_len: usize, tail_updates: &[TailUpdateRow], head_frontier_len: usize) -> u64 {
    /// Generous on purpose (JSON keys, punctuation, `seq`/`update_id` fields per tail-update
    /// record): this only needs to keep `bootstrap_response_bytes_max` meaningful, not reproduce
    /// `serde_json`'s exact byte count.
    const BOOTSTRAP_TAIL_UPDATE_JSON_OVERHEAD_BYTES: u64 = 256;

    let mut total = base64_len(snapshot_len).saturating_add(base64_len(head_frontier_len));
    for update in tail_updates {
        total = total
            .saturating_add(base64_len(update.bytes.len()))
            .saturating_add(base64_len(update.before_frontier.len()))
            .saturating_add(base64_len(update.after_frontier.len()))
            .saturating_add(BOOTSTRAP_TAIL_UPDATE_JSON_OVERHEAD_BYTES);
    }
    total
}

/// `bootstrap_response_bytes_max` (`limits-v1.md`): "覆盖 8 MiB binary 的 base64 4/3 膨胀和 JSON
/// envelope，仍远低于全局 200 MiB body ceiling".
///
/// # Errors
/// `ApiError::limit_exceeded` with `limit_kind = "bootstrap_response_bytes"` when `observed`
/// exceeds the fixed ceiling.
pub fn check_bootstrap_response_bytes(observed: u64) -> Result<(), ApiError> {
    if observed > limits::BOOTSTRAP_RESPONSE_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "limit_exceeded: bootstrap_response_bytes",
            "bootstrap_response_bytes",
            Some(json!(limits::BOOTSTRAP_RESPONSE_BYTES_MAX)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(())
}

/// Loads a document's snapshot + exact tail inside one MVCC snapshot, and verifies it before
/// returning.
///
/// # Errors
/// `ApiError::Conflict` carrying `"resync_required"` when seq continuity, frontier chaining, or a
/// `collab_updates.content_hash` does not check out (`collab-protocol-v1.md`: "任一 gap/corruption
/// 必须整次 fail closed 为 `resync_required/integrity` alert") — each such failure also writes one
/// `flow_integrity_records` row (`ADR-0013` §4) before returning. `NotFound` when the document
/// does not exist. Propagates a database failure otherwise.
pub async fn load(db: &DatabaseConnection, document_id: Uuid) -> Result<BootstrapResult, ApiError> {
    let tx = db
        .begin_with_config(Some(IsolationLevel::RepeatableRead), Some(AccessMode::ReadOnly))
        .await?;

    #[derive(FromQueryResult)]
    struct DocRow {
        workspace_id: Uuid,
        engine: String,
        format_version: String,
        snapshot: Vec<u8>,
        snapshot_frontier: Vec<u8>,
        snapshot_seq: i64,
        head_frontier: Vec<u8>,
        head_seq: i64,
    }
    let doc = DocRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT fo.workspace_id, cd.engine, cd.format_version, cd.snapshot, cd.snapshot_frontier, \
                cd.snapshot_seq, cd.head_frontier, cd.head_seq \
         FROM collab_documents cd JOIN flow_objects fo ON fo.id = cd.object_id \
         WHERE cd.id = $1",
        vec![document_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("document not found".to_string()))?;
    let workspace_id = doc.workspace_id;

    #[derive(FromQueryResult)]
    struct UpdateRow {
        seq: i64,
        update_id: Uuid,
        bytes: Vec<u8>,
        before_frontier: Vec<u8>,
        after_frontier: Vec<u8>,
        content_hash: String,
    }
    let rows = UpdateRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT seq, update_id, bytes, before_frontier, after_frontier, content_hash \
         FROM collab_updates WHERE document_id = $1 AND seq > $2 AND seq <= $3 ORDER BY seq ASC",
        vec![document_id.into(), doc.snapshot_seq.into(), doc.head_seq.into()],
    ))
    .all(&tx)
    .await?;

    // Read-only; nothing to persist, but ends the MVCC snapshot cleanly rather than relying on
    // drop-triggered rollback.
    tx.commit().await?;

    let expected_len = doc.head_seq.saturating_sub(doc.snapshot_seq);
    let expected_len_usize = usize::try_from(expected_len).unwrap_or(usize::MAX);
    if rows.len() != expected_len_usize {
        return Err(resync_required(
            db,
            workspace_id,
            document_id,
            "tail length does not match snapshot_seq..head_seq",
        )
        .await);
    }

    let mut running_frontier = doc.snapshot_frontier.clone();
    let mut tail_updates = Vec::with_capacity(rows.len());
    for (expected_seq, row) in (doc.snapshot_seq + 1..).zip(rows) {
        if row.seq != expected_seq {
            return Err(resync_required(db, workspace_id, document_id, "tail has a seq gap or duplicate").await);
        }
        if row.before_frontier != running_frontier {
            return Err(resync_required(db, workspace_id, document_id, "tail frontier chain is broken").await);
        }
        if content_hash(&row.bytes) != row.content_hash {
            return Err(resync_required(
                db,
                workspace_id,
                document_id,
                "tail update content_hash does not match its bytes",
            )
            .await);
        }
        running_frontier.clone_from(&row.after_frontier);
        tail_updates.push(TailUpdateRow {
            seq: row.seq,
            update_id: row.update_id,
            bytes: row.bytes,
            before_frontier: row.before_frontier,
            after_frontier: row.after_frontier,
        });
    }
    if running_frontier != doc.head_frontier {
        return Err(resync_required(db, workspace_id, document_id, "tail does not chain to head_frontier").await);
    }

    // `bootstrap_decoded_bytes_max` / `bootstrap_response_bytes_max` (`limits-v1.md`): checked
    // here, inside the one loader every caller (WS `snapshot`, REST `Bootstrap`, and every
    // internal re-hydration path) shares, so a document too large to safely bootstrap fails closed
    // for all of them alike rather than only for whichever caller happened to add its own check.
    let decoded_bytes = (doc.snapshot.len() as u64)
        .saturating_add(tail_updates.iter().map(|update| update.bytes.len() as u64).sum::<u64>());
    check_bootstrap_decoded_bytes(decoded_bytes)?;
    let response_bytes = estimated_response_bytes(doc.snapshot.len(), &tail_updates, doc.head_frontier.len());
    check_bootstrap_response_bytes(response_bytes)?;

    Ok(BootstrapResult {
        document_id,
        engine: doc.engine,
        format_version: doc.format_version,
        snapshot_seq: doc.snapshot_seq,
        head_seq: doc.head_seq,
        snapshot: doc.snapshot,
        tail_updates,
        head_frontier: doc.head_frontier,
    })
}

/// Fails the bootstrap call closed and records the drift in `flow_integrity_records`
/// (`ADR-0013` §4) — the read-only `REPEATABLE READ` transaction has already committed by the
/// time any caller reaches this (see `load`'s own commit above), so this insert runs on `db`
/// directly, not on the transaction that discovered the corruption. `details_redacted` carries
/// only the logical reason string, never tail bytes or CRDT content.
async fn resync_required(db: &DatabaseConnection, workspace_id: Uuid, document_id: Uuid, reason: &str) -> ApiError {
    tracing::error!(
        reason,
        %document_id,
        "collab bootstrap loader: integrity check failed, failing closed"
    );
    if let Err(err) = repository::insert_integrity_record(
        db,
        IntegrityRecordInput {
            workspace_id,
            kind: "collab_tail_integrity_violation",
            subject_kind: "collab_document",
            subject_id: &document_id.to_string(),
            detected_by: "flow.collab.bootstrap_loader",
            details_redacted: json!({ "reason": reason }),
        },
    )
    .await
    {
        tracing::error!(error = %err, "failed to record integrity alert for a resync_required fail-closed bootstrap");
    }
    ApiError::Conflict("resync_required".to_string())
}

/// One `collab_updates` row shaped for [`fetch_update_range`]'s egress-gap backfill use: every
/// field a synthesized `update`+`accepted` frame pair needs, read from already-persisted receipt
/// metadata rather than recomputed (`collab-protocol-v1.md` "禁止 ... 重新 apply update 拼
/// receipt").
pub struct EgressBackfillRow {
    pub seq: i64,
    pub update_id: Uuid,
    pub bytes: Vec<u8>,
    pub before_frontier: Vec<u8>,
    pub after_frontier: Vec<u8>,
    pub projection_seq: i64,
    pub event_id: Uuid,
    pub origin_client_id: Option<String>,
}

/// Reads the `[from_seq, to_seq]` slice of `collab_updates` a per-session
/// `egress::EgressSequencer` gap needs to backfill.
///
/// A plain, unlocked, non-transactional read: this runs on a live connection well after the
/// commit(s) that produced these rows, needs no MVCC snapshot consistency with anything else (each
/// row is independently a finished, immutable fact), and must never itself block a concurrent
/// writer the way `bootstrap::load`'s `REPEATABLE READ` transaction briefly can.
///
/// Returns fewer rows than the requested range when part of it no longer exists (compacted away,
/// or otherwise missing) — the caller, never this function, decides that a short read means "give
/// up and resync" (`egress::EgressSequencer::give_up_and_resync`) rather than forwarding a partial
/// catch-up.
///
/// # Errors
/// A database failure.
pub async fn fetch_update_range(
    db: &DatabaseConnection,
    document_id: Uuid,
    from_seq: i64,
    to_seq: i64,
) -> Result<Vec<EgressBackfillRow>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        seq: i64,
        update_id: Uuid,
        bytes: Vec<u8>,
        before_frontier: Vec<u8>,
        after_frontier: Vec<u8>,
        projection_seq: i64,
        event_id: Uuid,
        origin_client_id: Option<String>,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT seq, update_id, bytes, before_frontier, after_frontier, projection_seq, event_id, \
                origin_client_id \
         FROM collab_updates WHERE document_id = $1 AND seq >= $2 AND seq <= $3 ORDER BY seq ASC",
        vec![document_id.into(), from_seq.into(), to_seq.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| EgressBackfillRow {
            seq: row.seq,
            update_id: row.update_id,
            bytes: row.bytes,
            before_frontier: row.before_frontier,
            after_frontier: row.after_frontier,
            projection_seq: row.projection_seq,
            event_id: row.event_id,
            origin_client_id: row.origin_client_id,
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::content_hash;

    #[test]
    fn content_hash_is_stable_and_input_sensitive() {
        let a = content_hash(b"hello");
        let b = content_hash(b"hello");
        let c = content_hash(b"world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64, "sha-256 hex digest is 64 chars");
    }
}
