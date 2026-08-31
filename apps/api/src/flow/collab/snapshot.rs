//! Snapshot advancement (gate 7 `minimal_snapshot_advancement_bounds_tail`): keeps
//! `collab_documents.snapshot`/`snapshot_seq` caught up to `head_seq` so a document's tail
//! (`collab_updates` rows with `seq > snapshot_seq`) and decoded bootstrap size (`snapshot` bytes
//! + tail bytes) stay under `limits-v1.md`'s frozen soft/hard triggers.
//!
//! `versions/v0.4-flow-alpha.md:29` is explicit about what this module does and does not do: it
//! *advances* the snapshot pointer forward — it never deletes a `collab_updates` row. Every
//! update this package has ever accepted for a document stays readable at its original `seq`
//! after any number of advancements; only `snapshot`/`snapshot_frontier`/`snapshot_seq` move.
//! Retention/compaction — deleting rows below some horizon — is out of scope before v0.8 ("旧
//! update 全部保留，v0.8 前不做 compaction/retention delete"), and this module contains no
//! `DELETE` statement anywhere. Do not confuse the two: "no compaction before v0.8" is a
//! statement about deletion, not about whether the snapshot pointer may move.
//!
//! Lock discipline (`ADR-0010`, `collab-protocol-v1.md`): [`build_candidate`] does every
//! expensive step — the [`bootstrap::load`] read, the CRDT decode/replay, the snapshot export,
//! and the round-trip validation decode — with no transaction open at all. [`commit_candidate`]
//! is the only function here that opens one, and it does nothing between `begin` and `commit` but
//! a `SELECT ... FOR UPDATE` boundary/head recheck and, if it still matches, one fixed `UPDATE` —
//! no engine call, no cache call, no network I/O, mirroring `write::run_locked_phase` exactly.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use collab_core::{CollabEngine, LoroCollabEngine};
use parking_lot::Mutex;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use crate::error::ApiError;

use super::bootstrap;
use super::limits::{
    BOOTSTRAP_DECODED_BYTES_MAX, DOCUMENT_LOCK_HOLD_MS_MAX, DOCUMENT_LOCK_WAIT_MS_MAX, MAX_REBASE_ATTEMPTS,
    SNAPSHOT_REBUILD_WALL_MS_SOFT_MAX, SNAPSHOT_TAIL_BYTES_HARD_MAX, SNAPSHOT_TAIL_BYTES_SOFT_MAX,
    SNAPSHOT_TAIL_UPDATES_HARD_MAX, SNAPSHOT_TAIL_UPDATES_SOFT_MAX,
};

/// A point-in-time read of one document's snapshot/tail shape. Never held across the trigger
/// decision or the (separately re-read, separately locked) advancement that may follow it.
#[derive(Debug, Clone, Copy)]
pub struct TailStats {
    pub snapshot_seq: i64,
    pub head_seq: i64,
    pub tail_updates: i64,
    pub tail_bytes: i64,
    pub snapshot_bytes: i64,
}

impl TailStats {
    /// `snapshot bytes + tail bytes` — the same quantity `flow::query::get_bootstrap` checks
    /// against `bootstrap_decoded_bytes_max`.
    #[must_use]
    pub const fn decoded_bootstrap_bytes(&self) -> i64 {
        self.snapshot_bytes.saturating_add(self.tail_bytes)
    }
}

/// Reads `document_id`'s current snapshot/tail shape with two plain, unlocked reads (a row read
/// on `collab_documents` and an indexed aggregate over `collab_updates`'s `(document_id, seq)`
/// primary key range) — never inside a transaction, never blocking a concurrent writer.
///
/// Returns `Ok(None)` when the document does not exist (callers treat that identically to "no
/// trigger", since the write path's own `read_observed_head` already owns reporting `not_found`).
pub async fn read_tail_stats(db: &DatabaseConnection, document_id: Uuid) -> Result<Option<TailStats>, ApiError> {
    #[derive(FromQueryResult)]
    struct DocRow {
        snapshot_seq: i64,
        head_seq: i64,
        snapshot_bytes: i64,
    }
    let Some(doc) = DocRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT snapshot_seq, head_seq, octet_length(snapshot)::bigint AS snapshot_bytes FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };

    #[derive(FromQueryResult)]
    struct TailRow {
        tail_updates: i64,
        tail_bytes: Option<i64>,
    }
    let tail = TailRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS tail_updates, sum(octet_length(bytes)) AS tail_bytes \
         FROM collab_updates WHERE document_id = $1 AND seq > $2",
        vec![document_id.into(), doc.snapshot_seq.into()],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)?; // `count(*)` always returns exactly one row, even over zero matches.

    Ok(Some(TailStats {
        snapshot_seq: doc.snapshot_seq,
        head_seq: doc.head_seq,
        tail_updates: tail.tail_updates,
        tail_bytes: tail.tail_bytes.unwrap_or(0),
        snapshot_bytes: doc.snapshot_bytes,
    }))
}

/// The advancement decision a [`TailStats`] reading (plus, optionally, the most recently measured
/// candidate-rebuild wall time for this document) resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Below every soft threshold; nothing to do.
    None,
    /// At or past a soft threshold (`snapshot_tail_updates_soft_max` / `_bytes_soft_max` /
    /// `snapshot_rebuild_wall_ms_p95_soft_max`): advancement should be scheduled, but the update
    /// that observed this need not wait for it.
    Soft,
    /// At or past a hard threshold (`snapshot_tail_updates_hard_max` / `_bytes_hard_max` /
    /// `bootstrap_decoded_bytes_max`): `limits-v1.md`'s
    /// "接受下一 update 前必须先成功推进 snapshot,不能继续扩大 tail" — the caller must not accept
    /// another update against this document until an advancement has actually succeeded.
    Hard,
}

/// Resolves a [`TailStats`] reading (`ADR`-frozen soft/hard triggers) to a [`Trigger`].
/// `last_rebuild_wall_ms` is the most recently measured [`Candidate::rebuild_wall_ms`] for this
/// document, if any — the third, latency-shaped soft trigger `limits-v1.md` freezes alongside the
/// two count/byte ones.
#[must_use]
pub fn evaluate(stats: &TailStats, last_rebuild_wall_ms: Option<u64>) -> Trigger {
    let hard_decoded_bytes_max = i64::try_from(BOOTSTRAP_DECODED_BYTES_MAX).unwrap_or(i64::MAX);
    if stats.tail_updates >= SNAPSHOT_TAIL_UPDATES_HARD_MAX
        || stats.tail_bytes >= SNAPSHOT_TAIL_BYTES_HARD_MAX
        || stats.decoded_bootstrap_bytes() >= hard_decoded_bytes_max
    {
        return Trigger::Hard;
    }
    if stats.tail_updates >= SNAPSHOT_TAIL_UPDATES_SOFT_MAX
        || stats.tail_bytes >= SNAPSHOT_TAIL_BYTES_SOFT_MAX
        || last_rebuild_wall_ms.is_some_and(|ms| ms >= SNAPSHOT_REBUILD_WALL_MS_SOFT_MAX)
    {
        return Trigger::Soft;
    }
    Trigger::None
}

/// A built, validated candidate snapshot for `document_id`'s head at the moment
/// [`build_candidate`] read it. Never itself written anywhere — only [`commit_candidate`] does
/// that, and only after re-confirming the boundary this candidate was built against still holds.
pub struct Candidate {
    pub head_seq: i64,
    pub head_frontier: Vec<u8>,
    pub snapshot_bytes: Vec<u8>,
    pub rebuild_wall_ms: u64,
}

/// Builds and validates a candidate snapshot for `document_id`'s *current* head, entirely outside
/// any transaction.
///
/// [`bootstrap::load`] already runs inside its own `REPEATABLE READ READ ONLY` transaction and
/// verifies seq continuity, frontier chaining, and per-update content hashes before returning.
/// This function then decodes that snapshot, replays its exact tail, and exports a fresh snapshot
/// at the same head — then, since "构建与校验" both belong outside the lock, immediately
/// re-decodes the exported bytes into a second, independent engine and compares its `frontier()`
/// and semantic hash back against the replayed one, so a candidate that fails to round-trip is
/// never handed to [`commit_candidate`].
///
/// Returns `Ok(None)` when there is nothing to advance (`snapshot_seq == head_seq` already).
///
/// # Errors
/// `ApiError::Conflict("resync_required")` propagated from [`bootstrap::load`] on a corrupted
/// tail (already fail-closed and alerted by that loader). `ApiError::Internal` if a decode/replay
/// step that should be infallible for previously-accepted, hash-verified bytes somehow fails, or
/// if the round-trip validation detects a mismatch — in either case nothing is written; the
/// caller sees this exactly like any other failed advancement attempt.
pub async fn build_candidate(db: &DatabaseConnection, document_id: Uuid) -> Result<Option<Candidate>, ApiError> {
    let start = Instant::now();
    let boot = bootstrap::load(db, document_id).await?;
    if boot.snapshot_seq >= boot.head_seq {
        return Ok(None);
    }

    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| {
        tracing::error!(error = %err, %document_id, "snapshot advancement: base snapshot failed to decode");
        ApiError::Internal
    })?;
    for tail in &boot.tail_updates {
        engine.import_update(&tail.bytes).map_err(|err| {
            tracing::error!(error = %err, %document_id, "snapshot advancement: tail update failed to re-apply");
            ApiError::Internal
        })?;
    }
    if engine.frontier().as_bytes() != boot.head_frontier.as_slice() {
        tracing::error!(
            %document_id,
            "snapshot advancement: replayed frontier does not match the bootstrap loader's verified head_frontier"
        );
        return Err(ApiError::Internal);
    }
    let expected_hash = engine
        .semantic_snapshot()
        .map_err(|err| {
            tracing::error!(error = %err, %document_id, "snapshot advancement: semantic_snapshot failed on the replayed engine");
            ApiError::Internal
        })?
        .semantic_hash();

    let snapshot_bytes = engine.export_snapshot().map_err(|err| {
        tracing::error!(error = %err, %document_id, "snapshot advancement: export_snapshot failed");
        ApiError::Internal
    })?;

    // Round-trip validation: decode the exported candidate bytes into a fresh, independent engine
    // and confirm it reproduces the exact same frontier and semantic content before this
    // candidate is ever offered to `commit_candidate`.
    let reloaded = LoroCollabEngine::load(&snapshot_bytes).map_err(|err| {
        tracing::error!(error = %err, %document_id, "snapshot advancement: candidate snapshot failed to round-trip decode");
        ApiError::Internal
    })?;
    if reloaded.frontier().as_bytes() != boot.head_frontier.as_slice() {
        tracing::error!(%document_id, "snapshot advancement: candidate snapshot round-trip frontier mismatch");
        return Err(ApiError::Internal);
    }
    let reloaded_hash = reloaded
        .semantic_snapshot()
        .map_err(|err| {
            tracing::error!(error = %err, %document_id, "snapshot advancement: semantic_snapshot failed on the round-trip engine");
            ApiError::Internal
        })?
        .semantic_hash();
    if reloaded_hash != expected_hash {
        tracing::error!(%document_id, "snapshot advancement: candidate snapshot round-trip semantic hash mismatch");
        return Err(ApiError::Internal);
    }

    #[allow(clippy::cast_possible_truncation)]
    let rebuild_wall_ms = start.elapsed().as_millis() as u64;

    Ok(Some(Candidate {
        head_seq: boot.head_seq,
        head_frontier: boot.head_frontier,
        snapshot_bytes,
        rebuild_wall_ms,
    }))
}

/// What [`commit_candidate`] actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The boundary this candidate was built against still held; `snapshot_seq` is now
    /// `candidate.head_seq`.
    Advanced,
    /// Another advancement (this instance or another) already moved `snapshot_seq` to or past
    /// this candidate's `head_seq` first. Not an error — nothing to do.
    AlreadyAdvanced,
    /// `head_seq`/`head_frontier` moved since this candidate was built (a concurrent write
    /// committed). The candidate is stale; the caller must rebuild outside the lock and retry.
    HeadMismatch,
}

/// The short, fixed transaction: `SELECT ... FOR UPDATE` the document row, recheck the boundary
/// this candidate was built against, and — only if it still matches — one parameterized `UPDATE`.
/// No engine call, no cache call, no network I/O, no `.await` on anything but the database itself,
/// mirroring `write::run_locked_phase`'s own discipline and reusing its exact lock-timeout
/// budgets (`document_lock_wait_ms_max` / `document_lock_hold_ms_max`).
pub async fn commit_candidate(
    db: &DatabaseConnection,
    document_id: Uuid,
    candidate: &Candidate,
) -> Result<CommitOutcome, ApiError> {
    let tx = db.begin().await?;
    tx.execute_unprepared(&format!("SET LOCAL lock_timeout = '{DOCUMENT_LOCK_WAIT_MS_MAX}ms'"))
        .await?;
    tx.execute_unprepared(&format!(
        "SET LOCAL statement_timeout = '{DOCUMENT_LOCK_HOLD_MS_MAX}ms'"
    ))
    .await?;

    #[derive(FromQueryResult)]
    struct LockedRow {
        snapshot_seq: i64,
        head_seq: i64,
        head_frontier: Vec<u8>,
    }
    let Some(locked) = LockedRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT snapshot_seq, head_seq, head_frontier FROM collab_documents WHERE id = $1 FOR UPDATE",
        vec![document_id.into()],
    ))
    .one(&tx)
    .await?
    else {
        let _ = tx.rollback().await;
        return Err(ApiError::NotFound("document not found".to_string()));
    };

    if locked.snapshot_seq >= candidate.head_seq {
        let _ = tx.rollback().await;
        return Ok(CommitOutcome::AlreadyAdvanced);
    }
    if locked.head_seq != candidate.head_seq || locked.head_frontier != candidate.head_frontier {
        let _ = tx.rollback().await;
        return Ok(CommitOutcome::HeadMismatch);
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE collab_documents
            SET snapshot = $2, snapshot_frontier = $3, snapshot_seq = $4, updated_at = now()
            WHERE id = $1
        ",
        vec![
            document_id.into(),
            candidate.snapshot_bytes.clone().into(),
            candidate.head_frontier.clone().into(),
            candidate.head_seq.into(),
        ],
    ))
    .await?;

    tx.commit().await?;
    Ok(CommitOutcome::Advanced)
}

/// What [`advance`] actually did, for logging/tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvanceOutcome {
    Advanced,
    /// `snapshot_seq` already equalled `head_seq` (or another attempt won the race) — nothing to
    /// do, not an error.
    NothingToAdvance,
    /// [`MAX_REBASE_ATTEMPTS`] candidate rebuild/commit round trips all lost the boundary/head
    /// race to a concurrent write. The caller decides what "contended" means for it (a hard-
    /// boundary write rejects itself; a background sweep just logs and lets the next trigger try
    /// again).
    Contended,
}

/// Builds a candidate outside the lock, tries to commit it, and — only on [`CommitOutcome::HeadMismatch`]
/// (a concurrent write raced ahead of the candidate this attempt built) — rebuilds outside the
/// lock again, bounded by [`MAX_REBASE_ATTEMPTS`] (`document_prepare_rebase_attempts_max`, the
/// same "head mismatch 后在锁外重建...三次仍竞争则临时退避" budget the regular write path uses for
/// its own bounded rebase).
///
/// # Errors
/// Propagates [`build_candidate`]'s and [`commit_candidate`]'s database/decode failure modes.
pub async fn advance(
    db: &DatabaseConnection,
    document_id: Uuid,
    advancer: &SnapshotAdvancer,
) -> Result<AdvanceOutcome, ApiError> {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let Some(candidate) = build_candidate(db, document_id).await? else {
            return Ok(AdvanceOutcome::NothingToAdvance);
        };
        advancer.record_rebuild_wall_ms(document_id, candidate.rebuild_wall_ms);

        match commit_candidate(db, document_id, &candidate).await? {
            CommitOutcome::Advanced => return Ok(AdvanceOutcome::Advanced),
            CommitOutcome::AlreadyAdvanced => return Ok(AdvanceOutcome::NothingToAdvance),
            CommitOutcome::HeadMismatch => {
                if attempts >= MAX_REBASE_ATTEMPTS {
                    return Ok(AdvanceOutcome::Contended);
                }
            }
        }
    }
}

/// Process-wide (one per [`super::runtime::CollabRuntime`]) bookkeeping for snapshot advancement:
/// which documents currently have a background advancement in flight (deduplicating a burst of
/// soft-trigger writes into a single background attempt per document) and the most recently
/// measured [`Candidate::rebuild_wall_ms`] per document, feeding [`evaluate`]'s third soft
/// trigger.
///
/// `Clone` (cheap: two `Arc` bumps) so [`spawn_background`] can hand a spawned `'static` task its
/// own owned handle without requiring every caller of [`super::write::accept_update`] to prove
/// its own `&SnapshotAdvancer` borrow is `'static` — production callers reach this type through
/// [`super::runtime::CollabRuntime`], which *is* `'static`, but this module does not need to
/// assume that to stay correct, and this crate's own `database_tests` construct a plain,
/// non-`'static` local instance. `parking_lot::Mutex` inside, never held across an `.await` —
/// every method here is a short synchronous map operation.
#[derive(Default, Clone)]
pub struct SnapshotAdvancer {
    in_flight: Arc<Mutex<HashSet<Uuid>>>,
    last_rebuild_wall_ms: Arc<Mutex<HashMap<Uuid, u64>>>,
}

impl SnapshotAdvancer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn last_rebuild_wall_ms(&self, document_id: Uuid) -> Option<u64> {
        self.last_rebuild_wall_ms.lock().get(&document_id).copied()
    }

    fn record_rebuild_wall_ms(&self, document_id: Uuid, wall_ms: u64) {
        self.last_rebuild_wall_ms.lock().insert(document_id, wall_ms);
    }

    /// `true` if this call is the one that gets to run a background advancement for
    /// `document_id` (no other one is in flight); `false` if one already is, in which case the
    /// caller must not spawn a second one.
    #[must_use]
    pub fn try_start(&self, document_id: Uuid) -> bool {
        self.in_flight.lock().insert(document_id)
    }

    fn finish(&self, document_id: Uuid) {
        self.in_flight.lock().remove(&document_id);
    }

    #[must_use]
    pub fn is_in_flight(&self, document_id: Uuid) -> bool {
        self.in_flight.lock().contains(&document_id)
    }
}

/// Spawns a best-effort, non-blocking background advancement for `document_id`, deduplicated
/// against any already in flight. Takes `db` by value (a cheap connection-pool handle clone the
/// caller already owns) and clones `advancer` (an `Arc` bump) so the spawned `'static` task owns
/// everything it touches — the write path itself never calls this; only its callers
/// (`flow::collab::session`, `flow::command`) do, right after a successful [`super::write::accept_update`]
/// whose [`super::write::Accepted::should_advance_snapshot`] came back `true`.
pub fn spawn_background(advancer: &SnapshotAdvancer, db: DatabaseConnection, document_id: Uuid) {
    if !advancer.try_start(document_id) {
        return;
    }
    let advancer = advancer.clone();
    tokio::spawn(async move {
        match advance(&db, document_id, &advancer).await {
            Ok(AdvanceOutcome::Advanced) => {
                tracing::info!(%document_id, "snapshot advancement: background advancement committed");
            }
            Ok(AdvanceOutcome::NothingToAdvance) => {}
            Ok(AdvanceOutcome::Contended) => {
                tracing::warn!(%document_id, "snapshot advancement: background advancement stayed contended after all bounded attempts");
            }
            Err(err) => {
                tracing::error!(error = %err, %document_id, "snapshot advancement: background advancement failed");
            }
        }
        advancer.finish(document_id);
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::super::egress::{EgressSequencer, SeqDecision};
    use super::super::limits::{
        BOOTSTRAP_DECODED_BYTES_MAX, SNAPSHOT_TAIL_BYTES_HARD_MAX, SNAPSHOT_TAIL_BYTES_SOFT_MAX,
        SNAPSHOT_TAIL_UPDATES_HARD_MAX, SNAPSHOT_TAIL_UPDATES_SOFT_MAX,
    };
    use super::{SnapshotAdvancer, TailStats, Trigger, evaluate};
    use uuid::Uuid;

    fn stats(snapshot_seq: i64, head_seq: i64, tail_updates: i64, tail_bytes: i64, snapshot_bytes: i64) -> TailStats {
        TailStats {
            snapshot_seq,
            head_seq,
            tail_updates,
            tail_bytes,
            snapshot_bytes,
        }
    }

    #[test]
    fn frozen_limits_v1_numbers_are_exactly_what_this_module_enforces() {
        assert_eq!(SNAPSHOT_TAIL_UPDATES_SOFT_MAX, 256);
        assert_eq!(SNAPSHOT_TAIL_BYTES_SOFT_MAX, 1_048_576);
        assert_eq!(SNAPSHOT_TAIL_UPDATES_HARD_MAX, 1_024);
        assert_eq!(SNAPSHOT_TAIL_BYTES_HARD_MAX, 4_194_304);
        assert_eq!(BOOTSTRAP_DECODED_BYTES_MAX, 8_388_608);
    }

    #[test]
    fn below_every_threshold_is_none() {
        assert_eq!(evaluate(&stats(0, 10, 10, 1024, 100), None), Trigger::None);
    }

    #[test]
    fn exactly_at_the_soft_update_count_is_soft_not_hard() {
        assert_eq!(
            evaluate(
                &stats(0, SNAPSHOT_TAIL_UPDATES_SOFT_MAX, SNAPSHOT_TAIL_UPDATES_SOFT_MAX, 0, 0),
                None
            ),
            Trigger::Soft
        );
    }

    #[test]
    fn one_below_the_soft_update_count_is_none() {
        assert_eq!(
            evaluate(
                &stats(
                    0,
                    SNAPSHOT_TAIL_UPDATES_SOFT_MAX - 1,
                    SNAPSHOT_TAIL_UPDATES_SOFT_MAX - 1,
                    0,
                    0
                ),
                None
            ),
            Trigger::None
        );
    }

    #[test]
    fn exactly_at_the_soft_byte_count_is_soft() {
        assert_eq!(
            evaluate(&stats(0, 1, 1, SNAPSHOT_TAIL_BYTES_SOFT_MAX, 0), None),
            Trigger::Soft
        );
    }

    #[test]
    fn a_slow_rebuild_alone_triggers_soft_even_with_a_tiny_tail() {
        assert_eq!(evaluate(&stats(0, 1, 1, 1, 1), Some(100)), Trigger::Soft);
        assert_eq!(evaluate(&stats(0, 1, 1, 1, 1), Some(99)), Trigger::None);
    }

    #[test]
    fn exactly_at_the_hard_update_count_is_hard_not_soft() {
        assert_eq!(
            evaluate(
                &stats(0, SNAPSHOT_TAIL_UPDATES_HARD_MAX, SNAPSHOT_TAIL_UPDATES_HARD_MAX, 0, 0),
                None
            ),
            Trigger::Hard
        );
    }

    #[test]
    fn one_below_the_hard_update_count_with_soft_bytes_is_soft() {
        assert_eq!(
            evaluate(
                &stats(
                    0,
                    SNAPSHOT_TAIL_UPDATES_HARD_MAX - 1,
                    SNAPSHOT_TAIL_UPDATES_HARD_MAX - 1,
                    0,
                    0
                ),
                None
            ),
            Trigger::Soft
        );
    }

    #[test]
    fn exactly_at_the_hard_byte_count_is_hard() {
        assert_eq!(
            evaluate(&stats(0, 1, 1, SNAPSHOT_TAIL_BYTES_HARD_MAX, 0), None),
            Trigger::Hard
        );
    }

    #[test]
    fn exactly_at_the_decoded_bootstrap_hard_ceiling_via_snapshot_plus_tail_is_hard() {
        let half = i64::try_from(BOOTSTRAP_DECODED_BYTES_MAX / 2).expect("fits");
        assert_eq!(evaluate(&stats(0, 1, 1, half, half), None), Trigger::Hard);
    }

    #[test]
    fn decoded_bootstrap_bytes_sums_snapshot_and_tail() {
        assert_eq!(stats(0, 1, 1, 300, 700).decoded_bootstrap_bytes(), 1000);
    }

    #[test]
    fn snapshot_advancer_dedupes_concurrent_starts_for_the_same_document() {
        let advancer = SnapshotAdvancer::new();
        let id = Uuid::new_v4();
        assert!(advancer.try_start(id), "first start wins");
        assert!(
            !advancer.try_start(id),
            "a second start while in flight must be refused"
        );
        assert!(advancer.is_in_flight(id));
    }

    #[test]
    fn snapshot_advancer_records_and_reads_back_rebuild_wall_ms() {
        let advancer = SnapshotAdvancer::new();
        let id = Uuid::new_v4();
        assert_eq!(advancer.last_rebuild_wall_ms(id), None);
        advancer.record_rebuild_wall_ms(id, 42);
        assert_eq!(advancer.last_rebuild_wall_ms(id), Some(42));
    }

    /// Gate 10 `accepted_egress_seq_monotonic_and_gap_resync`: duplicate and out-of-order
    /// accepted notices are filtered at the per-subscription sequencer. A complete persisted
    /// backfill advances through the revealing notice only after the missing range is supplied.
    #[test]
    fn accepted_egress_is_strictly_monotonic_across_duplicates_and_reordering() {
        let mut sequencer = EgressSequencer::after_snapshot(40);
        assert_eq!(sequencer.evaluate(41), SeqDecision::InOrder);
        assert_eq!(sequencer.evaluate(41), SeqDecision::Duplicate);
        assert_eq!(
            sequencer.evaluate(44),
            SeqDecision::Gap {
                missing_from: 42,
                missing_to: 43,
            }
        );
        assert_eq!(sequencer.next_expected_seq(), 42);
        sequencer.resolve_gap(44);
        assert_eq!(sequencer.evaluate(45), SeqDecision::InOrder);
    }

    /// Gate 10 `accepted_egress_seq_monotonic_and_gap_resync`: when persisted receipt backfill
    /// cannot close a gap, resync freezes this subscription. No revealing or later accepted can
    /// cross the gap and become a save acknowledgement before a new snapshot creates a fresh
    /// sequencer.
    #[test]
    fn accepted_egress_gap_resync_never_advances_saved_state_across_the_gap() {
        let mut sequencer = EgressSequencer::after_snapshot(7);
        assert_eq!(
            sequencer.evaluate(10),
            SeqDecision::Gap {
                missing_from: 8,
                missing_to: 9,
            }
        );
        sequencer.give_up_and_resync();
        assert_eq!(sequencer.next_expected_seq(), 8);
        assert_eq!(sequencer.evaluate(10), SeqDecision::ResyncPending);
        assert_eq!(sequencer.evaluate(11), SeqDecision::ResyncPending);
    }
}

// ---- Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Gate 7 (`minimal_snapshot_advancement_bounds_tail`) and gate 8 (`snapshot_tail_restart_recovery`)
// evidence, plus one concurrency-safety test paving toward gate 9. Mirrors the scratch-database
// convention `write.rs::database_tests` already uses: own throwaway database per run, migrated
// from `migrations/*.sql` on disk, dropped on the way out.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use collab_core::{CollabEngine, LoroCollabEngine};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use uuid::Uuid;

    use super::{AdvanceOutcome, SnapshotAdvancer, Trigger, advance, evaluate, read_tail_stats};
    use crate::events::{BusinessEventInput, insert_business_event};
    use crate::flow::collab::authz;
    use crate::flow::collab::bootstrap;
    use crate::flow::collab::cache::WarmCache;
    use crate::flow::collab::coordinator::DocumentCoordinator;
    use crate::flow::collab::write::{self, AcceptOutcome, UpdateRequest};
    use crate::flow::command::{CreateObjectInput, create_object};

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_collab_snapshot_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "collab-snapshot-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("collab-snapshot-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
        }
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@collab.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collab snapshot test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid) -> (Uuid, Uuid) {
        let accepted = create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "Snapshot Advancement Test Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object creation succeeds");
        (accepted.object.id, accepted.object.document_id)
    }

    #[derive(FromQueryResult)]
    struct DocRow {
        snapshot_seq: i64,
        head_seq: i64,
        snapshot: Vec<u8>,
    }

    async fn read_doc(state: &AppState, document_id: Uuid) -> DocRow {
        DocRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot_seq, head_seq, snapshot FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists")
    }

    async fn total_update_rows(state: &AppState, document_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
    }

    /// Writes one real update through the exact production write path (`write::accept_update`),
    /// mutating `engine`'s title so every write produces distinct `bytes`/`content_hash`. Panics on
    /// anything but `Accepted` — every caller of this helper controls exactly when a rejection is
    /// expected and asserts on the raw `accept_update` call directly in that case instead.
    #[allow(clippy::too_many_arguments)]
    async fn write_one_update(
        state: &AppState,
        cache: &WarmCache,
        coordinator: &DocumentCoordinator,
        advancer: &SnapshotAdvancer,
        engine: &mut LoroCollabEngine,
        document_id: Uuid,
        workspace_id: Uuid,
        actor_id: Uuid,
        label: &str,
    ) -> write::Accepted {
        let base_frontier = engine.frontier();
        engine.set_title(label).expect("set_title succeeds");
        let bytes = engine.export_from(&base_frontier).expect("export succeeds");
        let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
        // A throwaway registry: this helper's callers assert on the returned `Accepted` value and
        // on database state, never on WebSocket fan-out, so there is nothing for a real session to
        // observe here.
        let registry = crate::flow::collab::registry::SessionRegistry::new();
        let outcome = write::accept_update(
            &state.db,
            cache,
            coordinator,
            &registry,
            advancer,
            10,
            None,
            UpdateRequest {
                document_id,
                update_id: Uuid::new_v4(),
                bytes,
                idempotency_key: None,
                event_idempotency_key: None,
                origin_client_id: Some("snapshot-test".to_string()),
                message: None,
                actor_id,
                workspace_id,
                checked_epoch,
                expected_frontier: None,
            },
        )
        .await
        .expect("accept_update does not hit a hard database error");
        match outcome {
            AcceptOutcome::Accepted(accepted) => accepted,
            AcceptOutcome::Rejected(rejected) => panic!("expected Accepted, got {rejected:?}"),
        }
    }

    /// Bulk-seeds `count` bootstrap-verifiable tail rows directly (one shared `business_events`
    /// row reused as every fabricated row's FK target, one transaction for all the
    /// `collab_updates` inserts) so the hard-boundary test can exercise the real, frozen
    /// `limits-v1.md` threshold (1,024) without paying 1,024 sequential `accept_update` round
    /// trips. Every row's `content_hash`/frontier chain is real (computed from a real engine
    /// mutation + `export_from`), so `bootstrap::load`'s integrity verification accepts them
    /// exactly like it would accept rows `accept_update` itself produced.
    async fn seed_tail_updates_directly(
        state: &AppState,
        engine: &mut LoroCollabEngine,
        document_id: Uuid,
        workspace_id: Uuid,
        actor_id: Uuid,
        starting_seq: i64,
        count: i64,
    ) {
        let event_id = insert_business_event(
            &state.db,
            BusinessEventInput {
                workspace_id,
                project_id: None,
                event_type: "flow.content.accepted".to_string(),
                aggregate_type: "flow_document".to_string(),
                aggregate_id: document_id.to_string(),
                actor_id: Some(actor_id),
                source: serde_json::json!({ "surface": "system" }),
                payload: serde_json::json!({}),
                metadata: serde_json::json!({}),
                correlation_id: None,
                causation_id: None,
                idempotency_key: None,
            },
        )
        .await
        .expect("shared seed event inserts");

        let tx = state.db.begin().await.expect("seed transaction begins");
        let mut total_bytes: i64 = 0;
        for i in 0..count {
            let seq = starting_seq + i + 1;
            let before_frontier = engine.frontier().as_bytes().to_vec();
            engine.set_title(&format!("seed-{seq}")).expect("set_title succeeds");
            let bytes = engine
                .export_from(&collab_core::Frontier::from_bytes(before_frontier.clone()))
                .expect("export succeeds");
            let after_frontier = engine.frontier().as_bytes().to_vec();
            let content_hash_hex = bootstrap::content_hash(&bytes);
            #[allow(clippy::cast_possible_wrap)]
            {
                total_bytes += bytes.len() as i64;
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    INSERT INTO collab_updates
                        (document_id, seq, update_id, content_hash, before_frontier, after_frontier,
                         bytes, actor_id, origin_surface, projection_seq, event_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'system', $2, $9)
                ",
                vec![
                    document_id.into(),
                    seq.into(),
                    Uuid::new_v4().into(),
                    content_hash_hex.into(),
                    before_frontier.into(),
                    after_frontier.into(),
                    bytes.into(),
                    actor_id.into(),
                    event_id.into(),
                ],
            ))
            .await
            .unwrap_or_else(|err| panic!("seed insert for seq {seq} failed: {err}"));
        }
        let new_head_seq = starting_seq + count;
        let new_head_frontier = engine.frontier().as_bytes().to_vec();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE collab_documents
                SET head_seq = $2, head_frontier = $3,
                    byte_count = byte_count + $4, update_count = update_count + $5
                WHERE id = $1
            ",
            vec![
                document_id.into(),
                new_head_seq.into(),
                new_head_frontier.into(),
                total_bytes.into(),
                count.into(),
            ],
        ))
        .await
        .expect("head advance after seeding succeeds");
        tx.commit().await.expect("seed transaction commits");
    }

    /// Gate 7 `minimal_snapshot_advancement_bounds_tail`, soft-trigger half: writes real updates
    /// one at a time and asserts `should_advance_snapshot` flips from `false` to `true` at the
    /// *exact* frozen boundary (`limits-v1.md`'s exact-accepted/one-past convention, applied to a
    /// trigger flag instead of a rejection), then drives the advancement it recommended and proves
    /// three things with real queries: `snapshot_seq` actually moved, the tail is actually short
    /// afterward, and not one prior `collab_updates` row was deleted.
    #[tokio::test]
    async fn soft_trigger_advances_snapshot_shortens_the_tail_and_keeps_every_old_update() {
        let scratch = scratch_or_skip!("soft-trigger");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let doc = read_doc(&state, document_id).await;
        let mut engine = LoroCollabEngine::load(&doc.snapshot).expect("loads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let advancer = SnapshotAdvancer::new();

        let soft_max = usize::try_from(super::super::limits::SNAPSHOT_TAIL_UPDATES_SOFT_MAX).expect("fits");
        let mut last_flag = true;
        for i in 0..soft_max {
            let accepted = write_one_update(
                &state,
                &cache,
                &coordinator,
                &advancer,
                &mut engine,
                document_id,
                workspace_id,
                owner_id,
                &format!("soft-{i}"),
            )
            .await;
            assert!(
                !accepted.should_advance_snapshot,
                "write #{} (tail was {i} before it) must not have crossed the soft trigger yet",
                i + 1
            );
            last_flag = accepted.should_advance_snapshot;
        }
        assert!(!last_flag);

        // This next write observes a tail of exactly `soft_max` updates before it is applied --
        // the exact boundary `evaluate` treats as `Trigger::Soft`.
        let boundary_accepted = write_one_update(
            &state,
            &cache,
            &coordinator,
            &advancer,
            &mut engine,
            document_id,
            workspace_id,
            owner_id,
            "soft-boundary",
        )
        .await;
        assert!(
            boundary_accepted.should_advance_snapshot,
            "the write observing exactly {soft_max} prior tail updates must recommend advancement"
        );

        let before_advance_rows = total_update_rows(&state, document_id).await;
        assert_eq!(before_advance_rows, i64::try_from(soft_max + 1).expect("fits"));

        let outcome = advance(&state.db, document_id, &advancer)
            .await
            .expect("advance does not hit a hard database error");
        assert_eq!(outcome, AdvanceOutcome::Advanced);

        let doc_after = read_doc(&state, document_id).await;
        assert_eq!(
            doc_after.snapshot_seq, doc_after.head_seq,
            "snapshot_seq must have caught up to head_seq"
        );
        assert_eq!(
            doc_after.head_seq,
            i64::try_from(soft_max + 1).expect("fits"),
            "advancement must not itself change head_seq"
        );

        let tail_after = read_tail_stats(&state.db, document_id)
            .await
            .expect("tail stats read runs")
            .expect("document exists");
        assert_eq!(
            tail_after.tail_updates, 0,
            "the tail must be empty right after advancement"
        );
        assert_eq!(evaluate(&tail_after, None), Trigger::None);

        // The decisive assertion: not one prior `collab_updates` row was deleted by advancement.
        let after_advance_rows = total_update_rows(&state, document_id).await;
        assert_eq!(
            after_advance_rows, before_advance_rows,
            "snapshot advancement must never delete a collab_updates row"
        );

        // The system keeps working after an advancement: one more write lands at head_seq+1 with
        // a fresh, short tail built on top of the new snapshot.
        let after = write_one_update(
            &state,
            &cache,
            &coordinator,
            &advancer,
            &mut engine,
            document_id,
            workspace_id,
            owner_id,
            "after-advance",
        )
        .await;
        assert_eq!(after.head_seq, doc_after.head_seq + 1);
        let final_tail = read_tail_stats(&state.db, document_id)
            .await
            .expect("tail stats read runs")
            .expect("document exists");
        assert_eq!(final_tail.tail_updates, 1);
        assert_eq!(
            total_update_rows(&state, document_id).await,
            after_advance_rows + 1,
            "still nothing deleted"
        );

        scratch.drop_self().await;
    }

    /// Gate 7 `minimal_snapshot_advancement_bounds_tail`, hard-boundary half:
    /// `limits-v1.md`'s "接受下一 update 前必须先成功推进 snapshot,不能继续扩大 tail" — seeds a
    /// tail already sitting exactly at the frozen `snapshot_tail_updates_hard_max` (1,024), then
    /// proves the *next* `accept_update` call forces a synchronous checkpoint (this write blocks
    /// on a real `advance()`, not a mocked one) before it is allowed to add to the tail: the write
    /// still succeeds, but `snapshot_seq` has moved to the pre-write head by the time it commits,
    /// so the tail right after is exactly one row, not 1,025.
    #[tokio::test]
    async fn hard_boundary_forces_a_checkpoint_before_the_next_update_is_accepted() {
        let scratch = scratch_or_skip!("hard-boundary");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let doc = read_doc(&state, document_id).await;
        let mut engine = LoroCollabEngine::load(&doc.snapshot).expect("loads");

        let hard_max = super::super::limits::SNAPSHOT_TAIL_UPDATES_HARD_MAX;
        seed_tail_updates_directly(&state, &mut engine, document_id, workspace_id, owner_id, 0, hard_max).await;

        let seeded_stats = read_tail_stats(&state.db, document_id)
            .await
            .expect("tail stats read runs")
            .expect("document exists");
        assert_eq!(seeded_stats.tail_updates, hard_max);
        assert_eq!(evaluate(&seeded_stats, None), Trigger::Hard);

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let advancer = SnapshotAdvancer::new();

        let accepted = write_one_update(
            &state,
            &cache,
            &coordinator,
            &advancer,
            &mut engine,
            document_id,
            workspace_id,
            owner_id,
            "past-hard-boundary",
        )
        .await;
        assert_eq!(accepted.head_seq, hard_max + 1);

        let doc_after = read_doc(&state, document_id).await;
        assert_eq!(
            doc_after.snapshot_seq, hard_max,
            "the forced checkpoint must have advanced snapshot_seq to the pre-write head before this write was allowed through"
        );

        let tail_after = read_tail_stats(&state.db, document_id)
            .await
            .expect("tail stats read runs")
            .expect("document exists");
        assert_eq!(
            tail_after.tail_updates, 1,
            "the tail after a forced checkpoint must hold only the update that just committed, not 1,025"
        );

        assert_eq!(
            total_update_rows(&state, document_id).await,
            hard_max + 1,
            "the forced checkpoint must not have deleted any of the seeded rows"
        );

        scratch.drop_self().await;
    }

    /// Gate 8 `snapshot_tail_restart_recovery`: writes real updates, forces a snapshot
    /// advancement mid-stream (so recovery must combine an *advanced* snapshot with a
    /// *non-empty* tail, not just replay from `snapshot_seq=0`), computes the semantic hash,
    /// then simulates an API restart -- a brand-new `DatabaseConnection` (a real second TCP
    /// connection, not a clone/handle to the first) and brand-new, empty `WarmCache`/
    /// `DocumentCoordinator`/`SnapshotAdvancer` instances, exactly what a fresh process has -- and
    /// proves the semantic hash recomputed from nothing but the persisted snapshot+tail is
    /// byte-for-byte identical.
    #[tokio::test]
    async fn restart_recovery_reproduces_the_exact_same_semantic_hash_from_snapshot_plus_tail() {
        let scratch = scratch_or_skip!("restart-recovery");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let doc = read_doc(&state, document_id).await;
        let mut engine = LoroCollabEngine::load(&doc.snapshot).expect("loads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let advancer = SnapshotAdvancer::new();

        for i in 0..5 {
            write_one_update(
                &state,
                &cache,
                &coordinator,
                &advancer,
                &mut engine,
                document_id,
                workspace_id,
                owner_id,
                &format!("pre-advance-{i}"),
            )
            .await;
        }

        let advance_outcome = advance(&state.db, document_id, &advancer)
            .await
            .expect("advance does not hit a hard database error");
        assert_eq!(advance_outcome, AdvanceOutcome::Advanced);
        let doc_after_advance = read_doc(&state, document_id).await;
        assert_eq!(
            doc_after_advance.snapshot_seq, doc_after_advance.head_seq,
            "the mid-stream advancement must have caught the snapshot up before more writes land"
        );

        for i in 0..3 {
            write_one_update(
                &state,
                &cache,
                &coordinator,
                &advancer,
                &mut engine,
                document_id,
                workspace_id,
                owner_id,
                &format!("post-advance-{i}"),
            )
            .await;
        }

        // "Before restart": load through the loader every surface (REST bootstrap, WS snapshot)
        // shares, and hash the merged, engine-independent semantic state -- not the raw snapshot
        // bytes, which `versions/v0.4-flow-alpha.md:95,97` is explicit does not have to be
        // byte-identical across a cache hit/miss/eviction or a restart, only semantically so.
        let boot_before = bootstrap::load(&state.db, document_id)
            .await
            .expect("bootstrap loads before the simulated restart");
        let mut engine_before = LoroCollabEngine::load(&boot_before.snapshot).expect("loads");
        for tail in &boot_before.tail_updates {
            engine_before.import_update(&tail.bytes).expect("tail replays");
        }
        let hash_before = engine_before
            .semantic_snapshot()
            .expect("semantic_snapshot succeeds")
            .semantic_hash();
        let frontier_before = engine_before.frontier().as_bytes().to_vec();

        // ---- simulate an API restart ----
        // A genuinely independent connection (`Database::connect` again, not `state.db.clone()`)
        // and brand-new, empty process-local caches: nothing carries over except what is in
        // PostgreSQL.
        let (prefix, _) = scratch
            .admin_url
            .rsplit_once('/')
            .expect("admin url has a scheme/host prefix");
        let restarted_url = format!("{prefix}/{}", scratch.name);
        let restarted_db = Database::connect(&restarted_url)
            .await
            .expect("a fresh connection reconnects after the simulated restart");
        let _restarted_cache = WarmCache::new();
        let _restarted_coordinator = DocumentCoordinator::new();
        let _restarted_advancer = SnapshotAdvancer::new();

        let boot_after = bootstrap::load(&restarted_db, document_id)
            .await
            .expect("bootstrap loads after the simulated restart");
        let mut engine_after = LoroCollabEngine::load(&boot_after.snapshot).expect("loads");
        for tail in &boot_after.tail_updates {
            engine_after.import_update(&tail.bytes).expect("tail replays");
        }
        let hash_after = engine_after
            .semantic_snapshot()
            .expect("semantic_snapshot succeeds")
            .semantic_hash();
        let frontier_after = engine_after.frontier().as_bytes().to_vec();

        assert_eq!(
            hash_before, hash_after,
            "restart recovery must reproduce the exact same semantic hash, character for character"
        );
        assert_eq!(
            frontier_before, frontier_after,
            "restart recovery must also reproduce the exact same frontier"
        );
        assert_eq!(boot_before.head_seq, boot_after.head_seq);
        assert_eq!(boot_before.snapshot_seq, boot_after.snapshot_seq);

        drop(restarted_db);
        scratch.drop_self().await;
    }

    /// Gate 9 groundwork: a real concurrent race between snapshot advancement and the normal
    /// write path, on two independent connections, repeated several rounds to make actual overlap
    /// likely rather than merely possible. Not the full gate 9 fixture (`gate-commands.md`'s
    /// REST/WS 10-client load harness is out of this task's scope), but a genuine proof that
    /// advancement racing a writer neither loses a write nor corrupts the tail: every update
    /// this test issues is still present and verifiable afterward, and `snapshot_seq` never
    /// exceeds `head_seq` (the same invariant `collab_documents_seq_check` enforces in the
    /// schema, checked here from the application side too).
    #[tokio::test]
    async fn advancement_stays_correct_when_racing_a_concurrent_write() {
        let scratch = scratch_or_skip!("advance-race");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;

        let doc = read_doc(&state, document_id).await;
        let mut engine = LoroCollabEngine::load(&doc.snapshot).expect("loads");

        let cache = WarmCache::new();
        let coordinator = DocumentCoordinator::new();
        let advancer = SnapshotAdvancer::new();

        // Build up an initial tail so the first few advancement attempts have real work to do.
        for i in 0..30 {
            write_one_update(
                &state,
                &cache,
                &coordinator,
                &advancer,
                &mut engine,
                document_id,
                workspace_id,
                owner_id,
                &format!("initial-{i}"),
            )
            .await;
        }
        let rows_before_racing = total_update_rows(&state, document_id).await;

        let (prefix, _) = scratch
            .admin_url
            .rsplit_once('/')
            .expect("admin url has a scheme/host prefix");
        let racing_url = format!("{prefix}/{}", scratch.name);

        let rounds = 10;
        for round in 0..rounds {
            let db_for_advance = Database::connect(&racing_url)
                .await
                .expect("a second connection for the advancing task");
            let advancer_for_advance = advancer.clone();
            let advance_task =
                tokio::spawn(async move { advance(&db_for_advance, document_id, &advancer_for_advance).await });

            let db_for_write = Database::connect(&racing_url)
                .await
                .expect("a third connection for the writing task");
            let state_for_write = state_for(db_for_write);
            let write_label = format!("racing-{round}");
            let base_frontier = engine.frontier();
            engine.set_title(&write_label).expect("set_title succeeds");
            let bytes = engine.export_from(&base_frontier).expect("export succeeds");
            let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
            let write_task = tokio::spawn({
                let cache = WarmCache::new();
                let coordinator = DocumentCoordinator::new();
                let registry = crate::flow::collab::registry::SessionRegistry::new();
                let advancer_for_write = advancer.clone();
                async move {
                    write::accept_update(
                        &state_for_write.db,
                        &cache,
                        &coordinator,
                        &registry,
                        &advancer_for_write,
                        10,
                        None,
                        UpdateRequest {
                            document_id,
                            update_id: Uuid::new_v4(),
                            bytes,
                            idempotency_key: None,
                            event_idempotency_key: None,
                            origin_client_id: Some("race-writer".to_string()),
                            message: None,
                            actor_id: owner_id,
                            workspace_id,
                            checked_epoch,
                            expected_frontier: None,
                        },
                    )
                    .await
                }
            });

            let (advance_result, write_result) = tokio::join!(advance_task, write_task);
            let _ = advance_result.expect("advance task joins");
            let outcome = write_result
                .expect("write task joins")
                .expect("accept_update does not hit a hard database error");
            match outcome {
                AcceptOutcome::Accepted(_) => {}
                AcceptOutcome::Rejected(rejected) => {
                    panic!("round {round}: the racing write must still be accepted, got {rejected:?}")
                }
            }

            let doc_now = read_doc(&state, document_id).await;
            assert!(
                doc_now.snapshot_seq <= doc_now.head_seq,
                "round {round}: snapshot_seq must never exceed head_seq, even mid-race"
            );
        }

        assert_eq!(
            total_update_rows(&state, document_id).await,
            rows_before_racing + rounds,
            "every racing write must be present exactly once -- none lost, none duplicated"
        );

        // The tail is still verifiable end to end: seq continuity, frontier chaining, and every
        // content_hash all check out, or `bootstrap::load` would fail closed with
        // `resync_required` instead of returning.
        let boot = bootstrap::load(&state.db, document_id)
            .await
            .expect("the tail must still pass bootstrap's integrity verification after racing");
        let mut verify_engine = LoroCollabEngine::load(&boot.snapshot).expect("loads");
        for tail in &boot.tail_updates {
            verify_engine.import_update(&tail.bytes).expect("tail replays");
        }
        assert_eq!(verify_engine.frontier().as_bytes(), boot.head_frontier.as_slice());

        scratch.drop_self().await;
    }
}
