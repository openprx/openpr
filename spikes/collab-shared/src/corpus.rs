//! Candidate-agnostic corpus runner.
//!
//! Takes any [`CorpusEngine`] implementation and drives it through the same operation logs and
//! sync sequence regardless of which candidate it is — this is the single source of truth
//! referenced by work package 3b: neither `collab-loro` nor `collab-yrs-yjs` may hold their own
//! copy of this logic.

use crate::engine::CorpusEngine;
use crate::error::CollabError;
use crate::limits::{DocumentLimits, check_operation, check_operation_batch_count};
use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::SemanticSnapshot;

/// Outcome of running a shared prefix plus two divergent branches on two replicas and then
/// syncing them bidirectionally to convergence.
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub replica_a_hash: String,
    pub replica_b_hash: String,
    pub snapshot_a: SemanticSnapshot,
    pub snapshot_b: SemanticSnapshot,
}

impl MergeOutcome {
    #[must_use]
    pub fn converged(&self) -> bool {
        self.replica_a_hash == self.replica_b_hash
    }
}

/// Applies `ops` in order to `engine`, returning on the first failure.
pub fn apply_all<E: CorpusEngine>(engine: &mut E, ops: &[Operation]) -> Result<(), E::Error> {
    for op in ops {
        engine.apply_operation(op)?;
    }
    Ok(())
}

/// Runs a two-replica concurrent-edit-then-merge case.
///
/// Replica A starts empty and applies `shared`, establishing the common ancestor state; replica B
/// is then *forked* from replica A's exported snapshot (a real `export_snapshot`/`load` round trip
/// — not an independent replay of `shared`, which would mint two unrelated sets of engine-native
/// node identities for what the fixture intends to be the same nodes). Both replicas then apply
/// their own divergent branch (`replica_a_ops` / `replica_b_ops`) independently, sync
/// bidirectionally via real `export_from`/`import_update` calls, and their semantic hashes are
/// compared.
///
/// `replica_a_seed` seeds replica A's (and therefore, transitively, the shared ancestor state's)
/// engine identity. `replica_b_seed` is accepted for API symmetry/documentation of intent, but
/// [`CollabEngine::load`] — matching the frozen v0.3 contract shape — does not take a seed, so
/// replica B's post-fork identity comes from the adapter's own `load` behavior.
// `replica_a_ops`/`replica_b_ops` intentionally share every character but one: that is the whole
// point of the naming (they are the same kind of value for the two sides of the merge).
#[allow(clippy::similar_names)]
pub fn run_two_replica_merge<E: CorpusEngine>(
    replica_a_seed: u64,
    _replica_b_seed: u64,
    shared: &[Operation],
    replica_a_ops: &[Operation],
    replica_b_ops: &[Operation],
) -> Result<MergeOutcome, E::Error> {
    let mut replica_a = E::new_empty(replica_a_seed);
    apply_all(&mut replica_a, shared)?;

    let shared_snapshot = replica_a.export_snapshot()?;
    let mut replica_b = E::load(&shared_snapshot)?;

    apply_all(&mut replica_a, replica_a_ops)?;
    apply_all(&mut replica_b, replica_b_ops)?;

    // Two-way delta sync: each side only sends what the other is missing, using the real
    // frontier/export_from/import_update path from the CollabEngine contract.
    let frontier_a = replica_a.frontier();
    let frontier_b = replica_b.frontier();

    let update_for_b = replica_a.export_from(&frontier_b)?;
    let update_for_a = replica_b.export_from(&frontier_a)?;

    replica_b.import_update(&update_for_b)?;
    replica_a.import_update(&update_for_a)?;

    let snapshot_a = replica_a.semantic_snapshot()?;
    let snapshot_b = replica_b.semantic_snapshot()?;

    Ok(MergeOutcome {
        replica_a_hash: snapshot_a.semantic_hash(),
        replica_b_hash: snapshot_b.semantic_hash(),
        snapshot_a,
        snapshot_b,
    })
}

/// Outcome of a duplicate-import idempotency check.
#[derive(Debug, Clone)]
pub struct IdempotencyOutcome {
    pub hash_before: String,
    pub hash_after_first_import: String,
    pub hash_after_duplicate_import: String,
}

impl IdempotencyOutcome {
    #[must_use]
    pub fn idempotent(&self) -> bool {
        self.hash_after_first_import == self.hash_after_duplicate_import
    }
}

/// Builds a source replica from `source_ops`, exports an update for a fresh empty replica,
/// imports it twice, and reports whether the second (duplicate) import changed anything.
pub fn run_duplicate_import_idempotent<E: CorpusEngine>(
    source_seed: u64,
    sink_seed: u64,
    source_ops: &[Operation],
) -> Result<IdempotencyOutcome, E::Error> {
    let mut source = E::new_empty(source_seed);
    apply_all(&mut source, source_ops)?;
    let snapshot_bytes = source.export_snapshot()?;

    let mut sink = E::new_empty(sink_seed);
    let hash_before = sink.semantic_snapshot()?.semantic_hash();

    sink.import_update(&snapshot_bytes)?;
    let hash_after_first_import = sink.semantic_snapshot()?.semantic_hash();

    sink.import_update(&snapshot_bytes)?;
    let hash_after_duplicate_import = sink.semantic_snapshot()?.semantic_hash();

    Ok(IdempotencyOutcome {
        hash_before,
        hash_after_first_import,
        hash_after_duplicate_import,
    })
}

/// Outcome of a corrupt/truncated update rejection check.
#[derive(Debug, Clone)]
pub struct CorruptRejectionOutcome {
    pub hash_before: String,
    pub hash_after_rejected_import: String,
    pub rejected: bool,
}

impl CorruptRejectionOutcome {
    #[must_use]
    pub fn head_unchanged(&self) -> bool {
        self.hash_before == self.hash_after_rejected_import
    }
}

/// Builds a replica from `source_ops`, then attempts to import `corrupt_bytes`.
///
/// The corpus invariant is: the import must be rejected with a typed error, and the replica's
/// semantic hash (standing in for head/frontier state) must be byte-identical before and after
/// the failed attempt.
pub fn run_corrupt_update_rejected<E: CorpusEngine>(
    seed: u64,
    source_ops: &[Operation],
    corrupt_bytes: &[u8],
) -> Result<CorruptRejectionOutcome, E::Error> {
    let mut replica = E::new_empty(seed);
    apply_all(&mut replica, source_ops)?;
    let hash_before = replica.semantic_snapshot()?.semantic_hash();

    let rejected = replica.import_update(corrupt_bytes).is_err();

    let hash_after_rejected_import = replica.semantic_snapshot()?.semantic_hash();

    Ok(CorruptRejectionOutcome {
        hash_before,
        hash_after_rejected_import,
        rejected,
    })
}

// --- Structural limit enforcement (`limits-v1.md`) --------------------------------------------

/// Applies `operation` only if it stays within every structural limit [`check_operation`] can
/// evaluate from the engine's *current* semantic snapshot.
///
/// On rejection the engine is never touched — no `apply_operation` call happens — so its state,
/// and therefore its `frontier()`, is left byte-for-byte unchanged, matching `limits-v1.md`'s
/// "content/shape overruns do not mutate state" requirement.
pub fn apply_operation_checked<E: CorpusEngine<Error = CollabError>>(
    engine: &mut E,
    limits: &DocumentLimits,
    operation: &Operation,
) -> Result<(), CollabError> {
    let before = engine.semantic_snapshot()?;
    check_operation(&before, operation, limits)?;
    engine.apply_operation(operation)
}

/// Applies a whole operation batch only if its *count* stays within
/// `semantic_patch_operations_max`.
///
/// On rejection none of `operations` is applied (atomic reject, not a partial prefix — matches
/// the corpus's `duplicate_out_of_order_partial_batch` category's "rejected intent, not silently
/// truncated" framing). Each individual operation is still checked against the other structural
/// limits as it is applied.
pub fn apply_batch_checked<E: CorpusEngine<Error = CollabError>>(
    engine: &mut E,
    limits: &DocumentLimits,
    operations: &[Operation],
) -> Result<(), CollabError> {
    check_operation_batch_count(operations.len(), limits)?;
    for operation in operations {
        apply_operation_checked(engine, limits, operation)?;
    }
    Ok(())
}

/// Builds a real, single-block document's incremental `export_from(<empty frontier>)` update whose
/// byte length is exactly `target_bytes`.
///
/// This is the same shape of byte blob `import_update` receives for an ordinary delta, as opposed
/// to a full-history `export_snapshot()` bootstrap blob.
///
/// The filler text is a deterministic pseudo-random ASCII stream (via [`crate::rng::SplitMix64`],
/// fixed seed, taken as a prefix so length `n+1`'s stream extends length `n`'s): both engines'
/// wire encodings compress a *repeating* filler almost arbitrarily (empirically, a 23-letter
/// cycling pattern collapsed a 65,536-character block down to ~1.2 KB for one candidate — nowhere
/// near enough range to hit an exact byte target), so incompressible content is required for the
/// exported size to actually track the filler length. Binary search then finds a filler length
/// whose export size lands exactly on `target_bytes`; a small linear window around the search
/// result absorbs any single-byte parity/rounding step in the underlying encoder.
///
/// Used only by the `update_bytes` boundary fixture: this exercises real production
/// `import_update`/`export_from` code, not a synthetic byte buffer, so the "exact" boundary case
/// is a genuine accept, not merely a length check.
///
/// # Errors
/// Returns [`CollabError::OperationFailed`] if no filler length in the search window produces
/// exactly `target_bytes` (e.g. because the underlying encoder's size-per-character is not stable
/// in this range for a given engine) — callers must report this honestly rather than accept a
/// near-miss.
pub fn tune_update_to_exact_bytes<E: CorpusEngine<Error = CollabError>>(
    seed: u64,
    target_bytes: usize,
) -> Result<Vec<u8>, CollabError> {
    fn filler(char_len: usize) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        // ALPHABET's length (62) fits comfortably in a u32 regardless of target pointer width.
        #[allow(clippy::cast_possible_truncation)]
        let alphabet_len = ALPHABET.len() as u32;
        // A fixed-seed, deterministic pseudo-random ASCII stream — incompressible enough that
        // export size tracks filler length closely, and a pure function of `char_len` alone (a
        // fresh RNG is reseeded every call and only the first `char_len` characters are taken),
        // which is what the binary search below requires.
        let mut rng = crate::rng::SplitMix64::new(0x7A7A_5FED);
        (0..char_len)
            .map(|_| {
                let idx = rng.next_below(alphabet_len) as usize;
                ALPHABET.get(idx).copied().unwrap_or(b'a') as char
            })
            .collect()
    }

    let block_id = NodeId::from("boundary-block");
    let empty_frontier = crate::frontier::Frontier::from_bytes(Vec::new());
    let build = |char_len: usize| -> Result<Vec<u8>, CollabError> {
        let mut engine = E::new_empty(seed);
        engine.apply_operation(&Operation::CreateNode {
            id: block_id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })?;
        if char_len > 0 {
            engine.apply_operation(&Operation::InsertText {
                id: block_id.clone(),
                index: 0,
                text: filler(char_len),
            })?;
        }
        engine.export_from(&empty_frontier)
    };

    // Binary search for the smallest filler length whose export size is >= target_bytes. Export
    // size is monotonic non-decreasing in filler length in this range for both candidates.
    let mut low = 0usize;
    let mut high = target_bytes;
    while low < high {
        let mid = low + (high - low) / 2;
        let size = build(mid)?.len();
        if size < target_bytes {
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    let window_start = low.saturating_sub(16);
    for candidate in window_start..=(low + 16) {
        let bytes = build(candidate)?;
        if bytes.len() == target_bytes {
            return Ok(bytes);
        }
    }

    Err(CollabError::OperationFailed {
        reason: format!("could not tune a real update to exactly {target_bytes} bytes near filler length {low}"),
    })
}

/// Builds `count` live nodes of `kind` under `parent` via raw (unchecked) `apply_operation` calls.
///
/// Used to cheaply construct the "one under the limit" bulk state for a `container_count`/
/// `document_block_count` boundary fixture without paying the O(n) `semantic_snapshot()` cost of
/// [`apply_operation_checked`] on every one of `count` calls, which would make a 10,000-node
/// boundary fixture quadratic. The boundary node itself is still applied through
/// [`apply_operation_checked`] by the caller, so the actual accept/reject decision under test is
/// always real-checked.
pub fn seed_unchecked_nodes<E: CorpusEngine<Error = CollabError>>(
    engine: &mut E,
    kind: NodeKind,
    parent: Option<&NodeId>,
    prefix: &str,
    count: u32,
) -> Result<Vec<NodeId>, CollabError> {
    let mut ids = Vec::with_capacity(count as usize);
    for i in 0..count {
        let id = NodeId::from(format!("{prefix}-{i}"));
        engine.apply_operation(&Operation::CreateNode {
            id: id.clone(),
            parent: parent.cloned(),
            index: i,
            kind,
        })?;
        ids.push(id);
    }
    Ok(ids)
}

// --- Out-of-order / partial-batch import -------------------------------------------------------

/// Applies `ops` to a fresh replica one at a time, capturing one incremental `export_from` update
/// per operation.
///
/// The resulting chunks can be replayed to a sink in any order (out-of-order) or only partially
/// (partial batch) while still exercising the real adapter's `export_from`/`import_update` path
/// rather than a synthetic split.
pub fn capture_update_chunks<E: CorpusEngine<Error = CollabError>>(
    seed: u64,
    ops: &[Operation],
) -> Result<(String, Vec<Vec<u8>>), CollabError> {
    let mut source = E::new_empty(seed);
    let mut chunks = Vec::with_capacity(ops.len());
    let mut previous_frontier = source.frontier();
    for op in ops {
        source.apply_operation(op)?;
        chunks.push(source.export_from(&previous_frontier)?);
        previous_frontier = source.frontier();
    }
    let final_hash = source.semantic_snapshot()?.semantic_hash();
    Ok((final_hash, chunks))
}

/// Outcome of applying update chunks to a sink in an order other than the one they were produced
/// in.
#[derive(Debug, Clone)]
pub struct OutOfOrderOutcome {
    pub source_hash: String,
    pub sink_hash_after_all: String,
}

impl OutOfOrderOutcome {
    #[must_use]
    pub fn converged(&self) -> bool {
        self.source_hash == self.sink_hash_after_all
    }
}

/// Applies `chunks` to a fresh sink replica in the order given by `apply_order` (expected to be a
/// permutation of `0..chunks.len()`), regardless of the order the source produced them in.
///
/// The out-of-order convergence invariant is: once every chunk has been applied, the sink's
/// semantic hash must match the source's, no matter what order they arrived in.
pub fn run_out_of_order_import<E: CorpusEngine<Error = CollabError>>(
    sink_seed: u64,
    source_hash: &str,
    chunks: &[Vec<u8>],
    apply_order: &[usize],
) -> Result<OutOfOrderOutcome, CollabError> {
    let mut sink = E::new_empty(sink_seed);
    for &index in apply_order {
        let chunk = chunks.get(index).ok_or_else(|| CollabError::OperationFailed {
            reason: format!("apply_order index {index} is out of range for {} chunks", chunks.len()),
        })?;
        sink.import_update(chunk)?;
    }
    let sink_hash_after_all = sink.semantic_snapshot()?.semantic_hash();
    Ok(OutOfOrderOutcome {
        source_hash: source_hash.to_string(),
        sink_hash_after_all,
    })
}

/// Outcome of delivering only a prefix of update chunks before the rest arrive later.
#[derive(Debug, Clone)]
pub struct PartialBatchOutcome {
    pub partial_has_cycle: bool,
    pub source_hash: String,
    pub sink_hash_after_remaining: String,
}

impl PartialBatchOutcome {
    #[must_use]
    pub fn converged(&self) -> bool {
        self.source_hash == self.sink_hash_after_remaining
    }
}

/// Applies only the first `first_half_len` chunks to a fresh sink (a batch that only makes it
/// halfway before, e.g., a connection drop).
///
/// Records whether the resulting partial state is still locally well-formed (no cycle), then
/// applies the remaining chunks and checks the sink converges to the same hash as a replica that
/// received the full log continuously.
///
/// # Errors
/// Returns [`CollabError::OperationFailed`] if `first_half_len` exceeds `chunks.len()`.
pub fn run_partial_batch_import<E: CorpusEngine<Error = CollabError>>(
    sink_seed: u64,
    source_hash: &str,
    chunks: &[Vec<u8>],
    first_half_len: usize,
) -> Result<PartialBatchOutcome, CollabError> {
    let first_half = chunks
        .get(..first_half_len)
        .ok_or_else(|| CollabError::OperationFailed {
            reason: format!("first_half_len {first_half_len} exceeds {} chunks", chunks.len()),
        })?;
    let second_half = chunks.get(first_half_len..).unwrap_or(&[]);

    let mut sink = E::new_empty(sink_seed);
    for chunk in first_half {
        sink.import_update(chunk)?;
    }
    let partial_has_cycle = sink.semantic_snapshot()?.has_cycle();

    for chunk in second_half {
        sink.import_update(chunk)?;
    }
    let sink_hash_after_remaining = sink.semantic_snapshot()?.semantic_hash();

    Ok(PartialBatchOutcome {
        partial_has_cycle,
        source_hash: source_hash.to_string(),
        sink_hash_after_remaining,
    })
}

// --- Offline reconnect: accepted-edit-loss check -----------------------------------------------

/// Verifies that every `CreateNode`/`InsertText` in `ops` left an observable, still-present trace
/// in `snapshot`.
///
/// This is the concrete form of "accepted edit 零丢失" this corpus checks, strictly stronger than
/// only comparing the two replicas' hashes (which alone would not catch both replicas losing the
/// *same* edit, since they would still agree with each other). Returns the first operation whose
/// effect is missing, if any.
#[must_use]
pub fn find_lost_edit<'a>(snapshot: &SemanticSnapshot, ops: &'a [Operation]) -> Option<&'a Operation> {
    for op in ops {
        match op {
            Operation::CreateNode { id, .. } => {
                if !snapshot.nodes.contains_key(id) {
                    return Some(op);
                }
            }
            Operation::InsertText { id, text, .. } => {
                let Some(node) = snapshot.nodes.get(id) else {
                    return Some(op);
                };
                if !node.text.contains(text.as_str()) {
                    return Some(op);
                }
            }
            Operation::MoveNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::DeleteText { .. }
            | Operation::SetProperty { .. } => {}
        }
    }
    None
}

// --- Snapshot + tail rebuild ---------------------------------------------------------------

/// Outcome of rebuilding a document from `snapshot + tail update` and comparing it against two
/// independent references.
#[derive(Debug, Clone)]
pub struct SnapshotTailOutcome {
    pub continuous_replay_hash: String,
    pub independent_full_replay_hash: String,
    pub snapshot_tail_rebuild_hash: String,
}

impl SnapshotTailOutcome {
    #[must_use]
    pub fn all_hashes_match(&self) -> bool {
        self.continuous_replay_hash == self.independent_full_replay_hash
            && self.continuous_replay_hash == self.snapshot_tail_rebuild_hash
    }
}

/// Builds a `source` replica that applies `initial_ops`, exports a snapshot at that point,
/// continues applying `tail_ops` on the very same replica.
///
/// It also separately exports the tail as an incremental update from the snapshot's frontier.
/// `continuous_replay_hash` is what "replaying the whole log in one document" produces. A *fresh*
/// replica is then rebuilt purely from `snapshot + tail update` (`load` + one `import_update`,
/// never touching `initial_ops`/`tail_ops` directly), and its hash is compared both against the
/// continuous replay and against a second, fully independent replica that applies `initial_ops`
/// then `tail_ops` from scratch. Targets the `snapshot_tail_rebuild_hash` hard gate: the
/// shared-runner-only "hash always matches its own history" property is not what this proves —
/// the fresh rebuilt replica never sees the original ops at all, only the snapshot bytes and the
/// tail update bytes.
pub fn run_snapshot_tail_rebuild<E: CorpusEngine<Error = CollabError>>(
    seed: u64,
    initial_ops: &[Operation],
    tail_ops: &[Operation],
) -> Result<SnapshotTailOutcome, CollabError> {
    let mut source = E::new_empty(seed);
    apply_all(&mut source, initial_ops)?;
    let snapshot_bytes = source.export_snapshot()?;
    let frontier_at_snapshot = source.frontier();

    apply_all(&mut source, tail_ops)?;
    let continuous_replay_hash = source.semantic_snapshot()?.semantic_hash();
    let tail_update = source.export_from(&frontier_at_snapshot)?;

    let mut rebuilt = E::load(&snapshot_bytes)?;
    rebuilt.import_update(&tail_update)?;
    let snapshot_tail_rebuild_hash = rebuilt.semantic_snapshot()?.semantic_hash();

    let mut independent = E::new_empty(seed);
    apply_all(&mut independent, initial_ops)?;
    apply_all(&mut independent, tail_ops)?;
    let independent_full_replay_hash = independent.semantic_snapshot()?.semantic_hash();

    Ok(SnapshotTailOutcome {
        continuous_replay_hash,
        independent_full_replay_hash,
        snapshot_tail_rebuild_hash,
    })
}

// --- Snapshot boundary updates ------------------------------------------------------------------

/// Outcome of delivering updates that straddle a snapshot boundary.
#[derive(Debug, Clone)]
pub struct SnapshotBoundaryOutcome {
    pub reference_hash: String,
    pub duplicate_pre_boundary_import_was_noop: bool,
    pub final_hash: String,
}

impl SnapshotBoundaryOutcome {
    #[must_use]
    pub fn converged(&self) -> bool {
        self.reference_hash == self.final_hash
    }
}

/// Exercises updates that straddle a snapshot boundary.
///
/// A `pre_boundary_update` (already fully covered by the snapshot) re-delivered to a
/// snapshot-loaded replica must be a no-op, and a `tail_update` covering ops *after* the boundary
/// must still apply correctly exactly once even though it arrives alongside a redundant
/// pre-boundary re-delivery — "跨快照边界的 update 不丢不重".
pub fn run_snapshot_boundary_updates<E: CorpusEngine<Error = CollabError>>(
    seed: u64,
    initial_ops: &[Operation],
    tail_ops: &[Operation],
) -> Result<SnapshotBoundaryOutcome, CollabError> {
    let mut source = E::new_empty(seed);
    let empty_frontier = source.frontier();
    apply_all(&mut source, initial_ops)?;
    let frontier_at_boundary = source.frontier();
    let pre_boundary_update = source.export_from(&empty_frontier)?;
    let snapshot_bytes = source.export_snapshot()?;

    apply_all(&mut source, tail_ops)?;
    let reference_hash = source.semantic_snapshot()?.semantic_hash();
    let tail_update = source.export_from(&frontier_at_boundary)?;

    let mut rebuilt = E::load(&snapshot_bytes)?;
    let hash_before_redundant = rebuilt.semantic_snapshot()?.semantic_hash();
    // Re-deliver an update the snapshot already fully covers: must be a no-op.
    rebuilt.import_update(&pre_boundary_update)?;
    let after_pre_boundary_replay_hash = rebuilt.semantic_snapshot()?.semantic_hash();
    let duplicate_pre_boundary_import_was_noop = hash_before_redundant == after_pre_boundary_replay_hash;

    rebuilt.import_update(&tail_update)?;
    let final_hash = rebuilt.semantic_snapshot()?.semantic_hash();

    Ok(SnapshotBoundaryOutcome {
        reference_hash,
        duplicate_pre_boundary_import_was_noop,
        final_hash,
    })
}
