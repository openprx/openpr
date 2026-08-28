//! Real end-to-end convergence cases for the `loro` candidate, driven entirely through the
//! candidate-agnostic `collab-shared` corpus runner (this file contains no engine-specific
//! merge/hash logic of its own — only fixtures and assertions).

use collab_loro_spike::LoroCollabEngine;
use collab_shared::corpus::{
    apply_batch_checked, apply_operation_checked, capture_update_chunks, find_lost_edit, run_corrupt_update_rejected,
    run_duplicate_import_idempotent, run_out_of_order_import, run_partial_batch_import, run_snapshot_boundary_updates,
    run_snapshot_tail_rebuild, run_two_replica_merge, seed_unchecked_nodes, tune_update_to_exact_bytes,
};
use collab_shared::fixture::{
    ancestor_delete_descendant_move_case, concurrent_block_move_case, concurrent_reorder_case, offline_edit_log,
    out_of_order_source_ops, same_key_nested_container_creation_case, same_text_range_edit_case, snapshot_tail_split,
    unicode_concurrent_edit_case,
};
use collab_shared::limits::DocumentLimits;
use collab_shared::rng::SplitMix64;
use collab_shared::{CollabEngine, CorpusEngine, NodeId, NodeKind, Operation};

#[test]
fn same_text_range_edit_converges() {
    let case = same_text_range_edit_case(1001);
    let outcome = run_two_replica_merge::<LoroCollabEngine>(1, 2, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed on well-formed ops");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );

    // Both concurrent insertions must actually be present after the real text-CRDT merge (this
    // is a real assertion on merged content, not just "the hashes match").
    let block = outcome
        .snapshot_a
        .nodes
        .values()
        .find(|node| !node.text.is_empty())
        .expect("the edited block must exist in the merged snapshot");
    assert!(block.text.contains("[A-INSERTED]"), "text = {:?}", block.text);
    assert!(block.text.contains("[B-INSERTED]"), "text = {:?}", block.text);
    assert!(block.text.starts_with("01234"), "text = {:?}", block.text);
}

#[test]
fn concurrent_block_move_has_no_cycle_and_a_single_deterministic_parent() {
    let case = concurrent_block_move_case(2002);
    let outcome = run_two_replica_merge::<LoroCollabEngine>(3, 4, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed on well-formed ops");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );
    assert!(!outcome.snapshot_a.has_cycle(), "merged tree must not contain a cycle");

    let child = outcome
        .snapshot_a
        .nodes
        .iter()
        .find(|(id, _)| id.starts_with("child-"))
        .map(|(_, node)| node)
        .expect("the concurrently-moved child must still exist (not lost)");
    assert!(!child.deleted);
    assert!(child.parent.is_some(), "child must have exactly one resolved parent");
}

#[test]
fn ancestor_delete_and_descendant_move_do_not_lose_the_moved_subtree() {
    let case = ancestor_delete_descendant_move_case(3003);
    let outcome = run_two_replica_merge::<LoroCollabEngine>(5, 6, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed on well-formed ops");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );

    let moved_child_id = outcome
        .snapshot_a
        .nodes
        .keys()
        .find(|id| id.starts_with("moved-child-"))
        .cloned()
        .expect("moved-child must exist");
    assert!(
        outcome.snapshot_a.is_reachable(&moved_child_id),
        "a node moved out from under a concurrently-deleted ancestor must remain reachable"
    );

    let stayed_child_id = outcome
        .snapshot_a
        .nodes
        .keys()
        .find(|id| id.starts_with("stayed-child-"))
        .cloned()
        .expect("stayed-child must exist");
    assert!(
        !outcome.snapshot_a.is_reachable(&stayed_child_id),
        "a node left under a deleted ancestor is expected to become unreachable"
    );
}

#[test]
fn duplicate_import_is_idempotent() {
    let ops = collab_shared::fixture::page_ops(4004, 12);
    let outcome = run_duplicate_import_idempotent::<LoroCollabEngine>(7, 8, &ops)
        .expect("source ops and snapshot export/import must succeed");

    assert_ne!(
        outcome.hash_before, outcome.hash_after_first_import,
        "the first import must actually change the empty sink replica"
    );
    assert!(
        outcome.idempotent(),
        "re-importing the same snapshot changed the semantic hash: first={} duplicate={}",
        outcome.hash_after_first_import,
        outcome.hash_after_duplicate_import
    );
}

#[test]
fn corrupt_update_is_rejected_and_head_is_unchanged() {
    let ops = collab_shared::fixture::page_ops(5005, 8);
    let corrupt_bytes = vec![0xFFu8; 64];
    let outcome = run_corrupt_update_rejected::<LoroCollabEngine>(9, &ops, &corrupt_bytes)
        .expect("building the source replica must succeed even though the import attempt will fail");

    assert!(
        outcome.rejected,
        "importing 64 bytes of 0xFF must be rejected as invalid loro update bytes"
    );
    assert!(
        outcome.head_unchanged(),
        "a rejected import must not mutate state: before={} after={}",
        outcome.hash_before,
        outcome.hash_after_rejected_import
    );
}

#[test]
fn truncated_snapshot_is_rejected_and_head_is_unchanged() {
    let ops = collab_shared::fixture::page_ops(6006, 8);
    // A real snapshot, truncated mid-stream: still exercises "rejected, not silently accepted".
    let mut source = LoroCollabEngine::new_empty(10);
    for op in &ops {
        source.apply_operation(op).expect("well-formed op must apply");
    }
    let full_snapshot = collab_shared::CollabEngine::export_snapshot(&source).expect("export must succeed");
    let half = full_snapshot.len() / 2;
    let truncated = full_snapshot
        .get(..half)
        .expect("a length that is half of the snapshot's own length is always in bounds");

    let outcome = run_corrupt_update_rejected::<LoroCollabEngine>(11, &ops, truncated)
        .expect("building the source replica must succeed even though the import attempt will fail");

    assert!(
        outcome.rejected,
        "a truncated snapshot must be rejected, not silently accepted"
    );
    assert!(outcome.head_unchanged());
}

// --- New this round: corpus category expansion -------------------------------------------------

#[test]
fn concurrent_reorder_keeps_all_siblings_and_converges() {
    let case = concurrent_reorder_case(7007);
    let outcome = run_two_replica_merge::<LoroCollabEngine>(12, 13, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed on well-formed reorder ops");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );
    assert!(!outcome.snapshot_a.has_cycle());

    let live_fields = outcome
        .snapshot_a
        .nodes
        .values()
        .filter(|node| !node.deleted && node.kind == NodeKind::CollectionField)
        .count();
    // 1 parent + 5 siblings = 6 live CollectionField nodes; the concurrent reorder must not lose
    // or duplicate any of them.
    assert_eq!(live_fields, 6, "concurrent reorder lost or duplicated a sibling");
}

/// "同 key nested container creation": both replicas create a node under the identical logical id
/// with different content. This test documents the real, observed loro-adapter behavior rather
/// than an invariant assumed in advance.
///
/// Real finding: Loro itself keeps both physical tree nodes after merge -- no CRDT-level loss,
/// since `TreeID` embeds the creating peer id, so the two concurrent creates are never the same
/// engine-native node. But this adapter's logical-id bookkeeping
/// (`rebuild_id_cache`/`semantic_snapshot` in `src/engine.rs`) assumes a logical id maps to at
/// most one physical node: `semantic_snapshot`'s `snapshot.nodes.insert(logical_id, ...)` runs
/// once per physical tree node and silently overwrites on a repeated key, so one side's create
/// becomes unrepresentable in the semantic view. This is deterministic (both replicas'
/// independent `semantic_snapshot()` calls agree on the same winner, hence `converged()` below
/// still holds), but it is a genuine adapter-level content-visibility limitation, not merely "one
/// deterministic winner picked at the CRDT layer" the way `concurrent_block_move_case` is.
/// Reported here rather than fixed -- fixing the adapter's id-collision handling is outside this
/// round's scope.
#[test]
fn same_key_nested_container_creation_collapses_to_one_visible_node() {
    let case = same_key_nested_container_creation_case(8008);
    let shared_key = case
        .replica_a
        .first()
        .expect("same_key_nested_container_creation_case must produce a non-empty replica_a")
        .target()
        .clone();
    let outcome = run_two_replica_merge::<LoroCollabEngine>(14, 15, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed: both concurrent creates are individually well-formed");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );
    assert_eq!(
        outcome.snapshot_a.nodes.len(),
        1,
        "the loro adapter's logical-id bookkeeping collapses same-key concurrent creates to one \
         visible node in the semantic snapshot (see doc comment above)"
    );
    let node = outcome
        .snapshot_a
        .nodes
        .get(&shared_key)
        .expect("the shared key must resolve to the one surviving node");
    assert!(
        node.properties.get("name") == Some(&"from-a".to_string())
            || node.properties.get("name") == Some(&"from-b".to_string())
    );
}

#[test]
fn out_of_order_import_converges_regardless_of_arrival_order() {
    let ops = out_of_order_source_ops(9009, 20);
    let (source_hash, chunks) =
        capture_update_chunks::<LoroCollabEngine>(16, &ops).expect("capturing update chunks must succeed");

    let mut rng = SplitMix64::new(0x2331 ^ 0xABCD_1234);
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    for i in (1..order.len()).rev() {
        // Shuffle index bound is a loop counter over `chunks.len()` (a small op count),
        // always far below u32::MAX.
        let bound = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let j = rng.next_below(bound) as usize;
        order.swap(i, j);
    }
    assert_ne!(
        order,
        (0..chunks.len()).collect::<Vec<_>>(),
        "the shuffle must actually reorder the chunks"
    );

    let outcome = run_out_of_order_import::<LoroCollabEngine>(17, &source_hash, &chunks, &order)
        .expect("out-of-order import must succeed once every chunk has arrived");
    assert!(
        outcome.converged(),
        "source={} sink={}",
        outcome.source_hash,
        outcome.sink_hash_after_all
    );
}

#[test]
fn partial_batch_import_stays_valid_then_converges_once_completed() {
    let ops = out_of_order_source_ops(10_010, 20);
    let (source_hash, chunks) =
        capture_update_chunks::<LoroCollabEngine>(18, &ops).expect("capturing update chunks must succeed");
    let half = chunks.len() / 2;

    let outcome = run_partial_batch_import::<LoroCollabEngine>(19, &source_hash, &chunks, half)
        .expect("partial batch import must succeed");

    assert!(
        !outcome.partial_has_cycle,
        "a partial delivery of independent node creates must never form a cycle"
    );
    assert!(
        outcome.converged(),
        "source={} sink={}",
        outcome.source_hash,
        outcome.sink_hash_after_remaining
    );
}

#[test]
// `replica_a_ops`/`replica_b_ops` intentionally share every character but one -- that is the
// naming's whole point (the two sides of the same offline-reconnect scenario).
#[allow(clippy::similar_names)]
fn offline_reconnect_loses_no_accepted_edit() {
    let replica_a_ops = offline_edit_log(11_011, "a", 15);
    let replica_b_ops = offline_edit_log(22_022, "b", 15);

    let outcome = run_two_replica_merge::<LoroCollabEngine>(20, 21, &[], &replica_a_ops, &replica_b_ops)
        .expect("merge must succeed on well-formed offline batches");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );
    if let Some(lost) = find_lost_edit(&outcome.snapshot_a, &replica_a_ops) {
        panic!("replica A's offline edit was lost after reconnect: {lost:?}");
    }
    if let Some(lost) = find_lost_edit(&outcome.snapshot_a, &replica_b_ops) {
        panic!("replica B's offline edit was lost after reconnect: {lost:?}");
    }
}

#[test]
fn snapshot_tail_rebuild_hash_matches_full_replay() {
    let (initial_ops, tail_ops) = snapshot_tail_split(12_012, 20, 15);
    let outcome = run_snapshot_tail_rebuild::<LoroCollabEngine>(23, &initial_ops, &tail_ops)
        .expect("snapshot+tail rebuild must succeed");

    assert!(
        outcome.all_hashes_match(),
        "continuous={} independent={} rebuilt={}",
        outcome.continuous_replay_hash,
        outcome.independent_full_replay_hash,
        outcome.snapshot_tail_rebuild_hash
    );
}

#[test]
fn snapshot_boundary_update_is_not_lost_or_duplicated() {
    let (initial_ops, tail_ops) = snapshot_tail_split(13_013, 20, 15);
    let outcome = run_snapshot_boundary_updates::<LoroCollabEngine>(24, &initial_ops, &tail_ops)
        .expect("snapshot boundary handling must succeed");

    assert!(
        outcome.duplicate_pre_boundary_import_was_noop,
        "re-delivering an update the snapshot already covers must not change state"
    );
    assert!(
        outcome.converged(),
        "reference={} final={}",
        outcome.reference_hash,
        outcome.final_hash
    );
}

#[test]
fn unicode_concurrent_edit_converges_and_preserves_content() {
    let case = unicode_concurrent_edit_case(14_014);
    let outcome = run_two_replica_merge::<LoroCollabEngine>(25, 26, &case.shared, &case.replica_a, &case.replica_b)
        .expect("merge must succeed on well-formed unicode text ops");

    assert!(
        outcome.converged(),
        "replica hashes diverged: a={} b={}",
        outcome.replica_a_hash,
        outcome.replica_b_hash
    );

    let block = outcome
        .snapshot_a
        .nodes
        .values()
        .find(|node| !node.text.is_empty())
        .expect("the edited block must exist in the merged snapshot");
    assert!(block.text.contains("中文日本語のテスト"), "text = {:?}", block.text);
    assert!(block.text.contains("🎉🎊👨‍👩‍👧‍👦"), "text = {:?}", block.text);
    assert!(block.text.contains("e\u{0301}n\u{0303}"), "text = {:?}", block.text);
}

// --- New this round: numeric boundary fixtures (limits-v1.md) ----------------------------------

#[test]
fn boundary_update_bytes_exact_accepted_plus_one_rejected() {
    let target = 65_536usize;
    let exact_bytes = tune_update_to_exact_bytes::<LoroCollabEngine>(27, target)
        .expect("must be able to tune a real loro snapshot to exactly 65,536 bytes");
    assert_eq!(exact_bytes.len(), target);

    let mut sink = LoroCollabEngine::new_empty(28);
    let diff = sink
        .import_update(&exact_bytes)
        .expect("an exactly-65,536-byte real update must be accepted");
    assert!(diff.changed);

    let mut plus_one_bytes = exact_bytes;
    plus_one_bytes.push(0u8);
    assert_eq!(plus_one_bytes.len(), target + 1);

    let mut rejectee = LoroCollabEngine::new_empty(29);
    let head_before = rejectee
        .semantic_snapshot()
        .expect("snapshot must succeed")
        .semantic_hash();
    let frontier_before = rejectee.frontier();

    let result = rejectee.import_update(&plus_one_bytes);
    let err = result.expect_err("a 65,537-byte update must be rejected before decode is even attempted");
    assert_eq!(err.limit_kind(), Some("update_bytes"));

    let head_after = rejectee
        .semantic_snapshot()
        .expect("snapshot must succeed")
        .semantic_hash();
    let frontier_after = rejectee.frontier();
    assert_eq!(head_before, head_after, "a rejected update must not mutate state");
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_tree_depth_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();
    let mut engine = LoroCollabEngine::new_empty(30);

    // Unchecked chain build: depth-chain-0 (depth 0, root) .. depth-chain-31 (depth 31).
    let mut parent: Option<NodeId> = None;
    for i in 0..32u32 {
        let id = NodeId::from(format!("depth-chain-{i}"));
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: parent.clone(),
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .expect("unchecked chain build must succeed");
        parent = Some(id);
    }

    // depth-chain-32, parented under depth-chain-31 (depth 31), reaches depth 32: exact boundary.
    let exact_id = NodeId::from("depth-chain-32");
    apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: exact_id.clone(),
            parent,
            index: 0,
            kind: NodeKind::NavigatorNode,
        },
    )
    .expect("depth exactly at tree_depth_max=32 must be accepted");

    let head_before = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine.frontier();

    let result = apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: NodeId::from("depth-chain-33"),
            parent: Some(exact_id),
            index: 0,
            kind: NodeKind::NavigatorNode,
        },
    );
    let err = result.expect_err("depth 33 exceeds tree_depth_max=32");
    assert_eq!(err.limit_kind(), Some("tree_depth"));

    let head_after = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine.frontier();
    assert_eq!(head_before, head_after);
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_container_count_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();
    let mut engine = LoroCollabEngine::new_empty(31);
    seed_unchecked_nodes(&mut engine, NodeKind::NavigatorNode, None, "nav-bulk", 9_999)
        .expect("unchecked bulk seed of 9,999 navigator nodes must succeed");

    apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: NodeId::from("nav-boundary-exact"),
            parent: None,
            index: 9_999,
            kind: NodeKind::NavigatorNode,
        },
    )
    .expect("exactly 10,000 navigator nodes must be accepted");

    let head_before = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine.frontier();

    let result = apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: NodeId::from("nav-boundary-plus-one"),
            parent: None,
            index: 10_000,
            kind: NodeKind::NavigatorNode,
        },
    );
    let err = result.expect_err("the 10,001st navigator node must be rejected");
    assert_eq!(err.limit_kind(), Some("container_count"));

    let head_after = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine.frontier();
    assert_eq!(head_before, head_after);
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_document_block_count_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();
    let mut engine = LoroCollabEngine::new_empty(32);
    seed_unchecked_nodes(&mut engine, NodeKind::Block, None, "blk-bulk", 9_999)
        .expect("unchecked bulk seed of 9,999 blocks must succeed");

    apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: NodeId::from("blk-boundary-exact"),
            parent: None,
            index: 9_999,
            kind: NodeKind::Block,
        },
    )
    .expect("exactly 10,000 blocks must be accepted");

    let head_before = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine.frontier();

    let result = apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::CreateNode {
            id: NodeId::from("blk-boundary-plus-one"),
            parent: None,
            index: 10_000,
            kind: NodeKind::Block,
        },
    );
    let err = result.expect_err("the 10,001st block must be rejected");
    assert_eq!(err.limit_kind(), Some("document_block_count"));

    let head_after = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine.frontier();
    assert_eq!(head_before, head_after);
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_text_block_chars_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();
    let mut engine = LoroCollabEngine::new_empty(33);
    let block_id = NodeId::from("text-boundary-block");
    engine
        .apply_operation(&Operation::CreateNode {
            id: block_id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create must succeed");
    let filler: String = "x".repeat(99_999);
    engine
        .apply_operation(&Operation::InsertText {
            id: block_id.clone(),
            index: 0,
            text: filler,
        })
        .expect("bulk unchecked text insert must succeed");

    apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::InsertText {
            id: block_id.clone(),
            index: 0,
            text: "x".to_string(),
        },
    )
    .expect("reaching exactly 100,000 chars in one block must be accepted");

    let head_before = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine.frontier();

    let result = apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::InsertText {
            id: block_id,
            index: 0,
            text: "x".to_string(),
        },
    );
    let err = result.expect_err("the 100,001st char in one block must be rejected");
    assert_eq!(err.limit_kind(), Some("text_block_chars"));

    let head_after = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine.frontier();
    assert_eq!(head_before, head_after);
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_document_text_chars_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();
    let mut engine = LoroCollabEngine::new_empty(34);

    // 9 blocks at the per-block ceiling (100,000 chars each) plus one block one char short of
    // it: 9 * 100,000 + 99,999 = 999,999 document chars, all unchecked bulk.
    for i in 0..9u32 {
        let id = NodeId::from(format!("doc-text-block-{i}"));
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: i,
                kind: NodeKind::Block,
            })
            .expect("create must succeed");
        let filler: String = "x".repeat(100_000);
        engine
            .apply_operation(&Operation::InsertText {
                id,
                index: 0,
                text: filler,
            })
            .expect("bulk insert must succeed");
    }
    let last_block = NodeId::from("doc-text-block-9");
    engine
        .apply_operation(&Operation::CreateNode {
            id: last_block.clone(),
            parent: None,
            index: 9,
            kind: NodeKind::Block,
        })
        .expect("create must succeed");
    let almost_full: String = "x".repeat(99_999);
    engine
        .apply_operation(&Operation::InsertText {
            id: last_block.clone(),
            index: 0,
            text: almost_full,
        })
        .expect("bulk insert must succeed");

    // Exact boundary: the 1,000,000th document char, added to the one block not yet at its own
    // per-block ceiling either (so this can only be gated by `document_text_chars`).
    apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::InsertText {
            id: last_block,
            index: 0,
            text: "x".to_string(),
        },
    )
    .expect("reaching exactly 1,000,000 document chars must be accepted");

    // A brand new, nearly-empty block: the per-block ceiling cannot be what rejects the next
    // insert into it, only the document-wide ceiling can.
    let overflow_block = NodeId::from("doc-text-overflow-block");
    engine
        .apply_operation(&Operation::CreateNode {
            id: overflow_block.clone(),
            parent: None,
            index: 10,
            kind: NodeKind::Block,
        })
        .expect("create must succeed");

    let head_before = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine.frontier();

    let result = apply_operation_checked(
        &mut engine,
        &limits,
        &Operation::InsertText {
            id: overflow_block,
            index: 0,
            text: "x".to_string(),
        },
    );
    let err = result.expect_err("the 1,000,001st document char must be rejected");
    assert_eq!(err.limit_kind(), Some("document_text_chars"));

    let head_after = engine.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine.frontier();
    assert_eq!(head_before, head_after);
    assert_eq!(frontier_before, frontier_after);
}

#[test]
fn boundary_semantic_patch_operations_exact_accepted_plus_one_rejected() {
    let limits = DocumentLimits::default();

    let ops_100: Vec<Operation> = (0..100u32)
        .map(|i| Operation::CreateNode {
            id: NodeId::from(format!("patch-100-{i}")),
            parent: None,
            index: i,
            kind: NodeKind::NavigatorNode,
        })
        .collect();
    let mut engine_100 = LoroCollabEngine::new_empty(35);
    apply_batch_checked(&mut engine_100, &limits, &ops_100).expect("a 100-operation batch must be accepted in full");
    assert_eq!(engine_100.semantic_snapshot().expect("snapshot").nodes.len(), 100);

    let ops_101: Vec<Operation> = (0..101u32)
        .map(|i| Operation::CreateNode {
            id: NodeId::from(format!("patch-101-{i}")),
            parent: None,
            index: i,
            kind: NodeKind::NavigatorNode,
        })
        .collect();
    let mut engine_101 = LoroCollabEngine::new_empty(36);
    let head_before = engine_101.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_before = engine_101.frontier();

    let result = apply_batch_checked(&mut engine_101, &limits, &ops_101);
    let err = result.expect_err("a 101-operation batch must be rejected in full");
    assert_eq!(err.limit_kind(), Some("semantic_patch_operations"));

    let head_after = engine_101.semantic_snapshot().expect("snapshot").semantic_hash();
    let frontier_after = engine_101.frontier();
    assert_eq!(
        head_before, head_after,
        "a rejected batch must apply none of its operations"
    );
    assert_eq!(frontier_before, frontier_after);
    assert_eq!(
        engine_101.semantic_snapshot().expect("snapshot").nodes.len(),
        0,
        "zero of the 101 operations may have been applied"
    );
}

// `websocket_frame_bytes` is intentionally not covered here: see `collab_shared::limits`'s module
// docs for why a real transport-frame boundary cannot be tested at this layer without fabricating
// a frame codec that does not otherwise exist in this spike.
