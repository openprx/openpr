//! Deterministic fixture generation for the v0.3 convergence corpus's fixed models
//! (Page / Collection / Record / Navigator), plus a few named builders for the corpus's
//! required concurrency cases.
//!
//! Every generator is a pure function of its seed: same seed, same `Vec<Operation>`, forever.

use crate::operation::{NodeId, NodeKind, Operation};
use crate::rng::SplitMix64;

fn node_id(prefix: &str, rng: &mut SplitMix64) -> NodeId {
    NodeId::from(format!("{prefix}-{}", rng.token(10)))
}

/// Page fixed model: a nested block tree (paragraph/heading/list/code) with rich text content in
/// every block.
///
/// `op_count` is the number of `CreateNode` block operations; each block also gets a handful of
/// `InsertText` operations, so the returned log is longer than `op_count`.
#[must_use]
pub fn page_ops(seed: u64, op_count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let kinds = [
        "paragraph text with ordinary words",
        "# Heading",
        "- list item",
        "```code line```",
    ];
    let mut ops = Vec::with_capacity(op_count as usize * 2);
    let mut ancestry: Vec<NodeId> = Vec::new();
    for i in 0..op_count {
        let id = node_id("blk", &mut rng);
        let parent = if ancestry.is_empty() || rng.next_below(3) == 0 {
            None
        } else {
            let idx = rng.choose_index(ancestry.len()).unwrap_or(0);
            ancestry.get(idx).cloned()
        };
        let index = rng.next_below(i.saturating_add(1).min(1000));
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent,
            index,
            kind: NodeKind::Block,
        });
        let text_variant = rng
            .choose_index(kinds.len())
            .and_then(|idx| kinds.get(idx))
            .copied()
            .unwrap_or("paragraph text with ordinary words");
        ops.push(Operation::InsertText {
            id: id.clone(),
            index: 0,
            text: text_variant.to_string(),
        });
        if ancestry.len() < 64 {
            ancestry.push(id);
        }
    }
    ops
}

/// Collection fixed model: `field_count` fields and `view_count` views, each an ordered child of
/// a synthetic fields/views root, followed by a reorder pass (moves within the same parent).
#[must_use]
pub fn collection_ops(seed: u64, field_count: u32, view_count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let fields_root = node_id("fields-root", &mut rng);
    let views_root = node_id("views-root", &mut rng);
    let mut ops = vec![
        Operation::CreateNode {
            id: fields_root.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::CollectionField,
        },
        Operation::CreateNode {
            id: views_root.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::CollectionView,
        },
    ];

    let mut field_ids = Vec::with_capacity(field_count as usize);
    for i in 0..field_count {
        let id = node_id("field", &mut rng);
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent: Some(fields_root.clone()),
            index: i,
            kind: NodeKind::CollectionField,
        });
        ops.push(Operation::SetProperty {
            id: id.clone(),
            key: "name".to_string(),
            value: format!("field_{i}"),
        });
        field_ids.push(id);
    }

    let mut view_ids = Vec::with_capacity(view_count as usize);
    for i in 0..view_count {
        let id = node_id("view", &mut rng);
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent: Some(views_root.clone()),
            index: i,
            kind: NodeKind::CollectionView,
        });
        view_ids.push(id);
    }

    // Reorder pass: move a handful of fields/views to a freshly rolled index within their parent.
    for id in field_ids.iter().take((field_count / 4) as usize) {
        let new_index = rng.next_below(field_count.max(1));
        ops.push(Operation::MoveNode {
            id: id.clone(),
            new_parent: Some(fields_root.clone()),
            index: new_index,
        });
    }
    for id in view_ids.iter().take((view_count / 4) as usize) {
        let new_index = rng.next_below(view_count.max(1));
        ops.push(Operation::MoveNode {
            id: id.clone(),
            new_parent: Some(views_root.clone()),
            index: new_index,
        });
    }

    ops
}

/// Record fixed model: `property_count` scalar/relation properties plus an optional body block.
#[must_use]
pub fn record_ops(seed: u64, property_count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let record_id = node_id("record", &mut rng);
    let mut ops = vec![Operation::CreateNode {
        id: record_id.clone(),
        parent: None,
        index: 0,
        kind: NodeKind::RecordProperty,
    }];
    for i in 0..property_count {
        let is_relation = rng.next_below(5) == 0;
        let value = if is_relation {
            format!("rel:{}", rng.token(8))
        } else {
            format!("value_{i}")
        };
        ops.push(Operation::SetProperty {
            id: record_id.clone(),
            key: format!("prop_{i}"),
            value,
        });
    }
    ops.push(Operation::InsertText {
        id: record_id,
        index: 0,
        text: "record body text".to_string(),
    });
    ops
}

/// Navigator fixed model: `node_count` nodes across several levels, with cross-level moves and one
/// ancestor delete near the end of the log.
#[must_use]
pub fn navigator_ops(seed: u64, node_count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let mut ops = Vec::with_capacity(node_count as usize + 8);
    let mut all_ids: Vec<NodeId> = Vec::new();
    for i in 0..node_count {
        let id = node_id("nav", &mut rng);
        let parent = if all_ids.is_empty() || rng.next_below(4) == 0 {
            None
        } else {
            let idx = rng.choose_index(all_ids.len()).unwrap_or(0);
            all_ids.get(idx).cloned()
        };
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent,
            index: i,
            kind: NodeKind::NavigatorNode,
        });
        all_ids.push(id);
    }
    // Cross-level moves: relocate a handful of nodes to a different, later-created ancestor.
    // `all_ids.len()` is bounded by `node_count`, well under any realistic fixture size, so the
    // usize -> u32 conversion below is exact for every size this generator is actually asked for.
    #[allow(clippy::cast_possible_truncation)]
    let all_ids_len_u32 = all_ids.len() as u32;
    let move_count = (node_count / 5).max(1).min(all_ids_len_u32);
    for _ in 0..move_count {
        let Some(target_idx) = rng.choose_index(all_ids.len()) else {
            break;
        };
        let Some(new_parent_idx) = rng.choose_index(all_ids.len()) else {
            break;
        };
        if target_idx == new_parent_idx {
            continue;
        }
        let (Some(target_id), Some(new_parent_id)) =
            (all_ids.get(target_idx).cloned(), all_ids.get(new_parent_idx).cloned())
        else {
            continue;
        };
        ops.push(Operation::MoveNode {
            id: target_id,
            new_parent: Some(new_parent_id),
            index: 0,
        });
    }
    // Ancestor delete near the end: delete a mid-log node whose descendants (if any moved out
    // earlier) must remain reachable.
    if let Some(idx) = all_ids.first() {
        ops.push(Operation::DeleteNode { id: idx.clone() });
    }
    ops
}

/// One named concurrency case: `shared` is applied to both replicas first (establishing a common
/// ancestor state).
///
/// `replica_a`/`replica_b` are then applied independently before the corpus runner syncs them
/// bidirectionally.
#[derive(Debug, Clone)]
pub struct ConcurrentCase {
    pub shared: Vec<Operation>,
    pub replica_a: Vec<Operation>,
    pub replica_b: Vec<Operation>,
}

/// "两端同时编辑同一 text range": both replicas insert distinct text at the same index of the
/// same block after a shared setup, exercising each engine's real text-CRDT merge.
#[must_use]
pub fn same_text_range_edit_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let block = node_id("range-block", &mut rng);
    ConcurrentCase {
        shared: vec![
            Operation::CreateNode {
                id: block.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            },
            Operation::InsertText {
                id: block.clone(),
                index: 0,
                text: "0123456789".to_string(),
            },
        ],
        replica_a: vec![Operation::InsertText {
            id: block.clone(),
            index: 5,
            text: "[A-INSERTED]".to_string(),
        }],
        replica_b: vec![Operation::InsertText {
            id: block,
            index: 5,
            text: "[B-INSERTED]".to_string(),
        }],
    }
}

/// "同一 block 移到不同 parent": both replicas concurrently move the same child under a
/// different candidate parent.
///
/// The merge must pick exactly one winner deterministically and must not create a cycle or lose
/// the node.
#[must_use]
pub fn concurrent_block_move_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let parent_a = node_id("parent-a", &mut rng);
    let parent_b = node_id("parent-b", &mut rng);
    let child = node_id("child", &mut rng);
    ConcurrentCase {
        shared: vec![
            Operation::CreateNode {
                id: parent_a.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            },
            Operation::CreateNode {
                id: parent_b.clone(),
                parent: None,
                index: 1,
                kind: NodeKind::Block,
            },
            Operation::CreateNode {
                id: child.clone(),
                parent: Some(parent_a.clone()),
                index: 0,
                kind: NodeKind::Block,
            },
        ],
        replica_a: vec![Operation::MoveNode {
            id: child.clone(),
            new_parent: Some(parent_a),
            index: 0,
        }],
        replica_b: vec![Operation::MoveNode {
            id: child,
            new_parent: Some(parent_b),
            index: 0,
        }],
    }
}

/// "ancestor delete 与 descendant move": replica A deletes the ancestor while replica B
/// concurrently moves the descendant out from under it.
///
/// After merge the descendant must remain reachable (not lost), while a sibling descendant that
/// was *not* moved out is allowed to become unreachable along with its deleted ancestor.
#[must_use]
pub fn ancestor_delete_descendant_move_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let ancestor = node_id("ancestor", &mut rng);
    let safe_parent = node_id("safe-parent", &mut rng);
    let moved_child = node_id("moved-child", &mut rng);
    let stayed_child = node_id("stayed-child", &mut rng);
    ConcurrentCase {
        shared: vec![
            Operation::CreateNode {
                id: ancestor.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::NavigatorNode,
            },
            Operation::CreateNode {
                id: safe_parent.clone(),
                parent: None,
                index: 1,
                kind: NodeKind::NavigatorNode,
            },
            Operation::CreateNode {
                id: moved_child.clone(),
                parent: Some(ancestor.clone()),
                index: 0,
                kind: NodeKind::NavigatorNode,
            },
            Operation::CreateNode {
                id: stayed_child,
                parent: Some(ancestor.clone()),
                index: 1,
                kind: NodeKind::NavigatorNode,
            },
        ],
        replica_a: vec![Operation::DeleteNode { id: ancestor }],
        replica_b: vec![Operation::MoveNode {
            id: moved_child,
            new_parent: Some(safe_parent),
            index: 0,
        }],
    }
}

/// "同一 field/view/block reorder 并发": five siblings (standing in for Collection fields/views or
/// Page blocks) are created under one parent.
///
/// Both replicas then concurrently reorder the *same* pair of siblings to conflicting target
/// positions. After merge every sibling must still exist under the same parent (no sibling lost
/// or duplicated), and the merge must pick one deterministic order. The corpus's reorder
/// invariant is identical regardless of which `NodeKind` is used.
#[must_use]
pub fn concurrent_reorder_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let parent = node_id("reorder-root", &mut rng);
    let siblings: Vec<NodeId> = (0..5).map(|_| node_id("sibling", &mut rng)).collect();
    let mut shared = vec![Operation::CreateNode {
        id: parent.clone(),
        parent: None,
        index: 0,
        kind: NodeKind::CollectionField,
    }];
    for (i, id) in siblings.iter().enumerate() {
        // `siblings` always has exactly 5 elements (built from a fixed `0..5` range above), so
        // this index is always tiny and the usize -> u32 conversion is exact.
        #[allow(clippy::cast_possible_truncation)]
        let index = i as u32;
        shared.push(Operation::CreateNode {
            id: id.clone(),
            parent: Some(parent.clone()),
            index,
            kind: NodeKind::CollectionField,
        });
    }
    let first_sibling = siblings
        .first()
        .cloned()
        .unwrap_or_else(|| NodeId::from("unreachable-sibling"));
    let last_sibling = siblings
        .last()
        .cloned()
        .unwrap_or_else(|| NodeId::from("unreachable-sibling"));
    ConcurrentCase {
        shared,
        // Replica A moves the first sibling to the end.
        replica_a: vec![Operation::MoveNode {
            id: first_sibling,
            new_parent: Some(parent.clone()),
            index: 4,
        }],
        // Replica B concurrently moves the last sibling to the front.
        replica_b: vec![Operation::MoveNode {
            id: last_sibling,
            new_parent: Some(parent),
            index: 0,
        }],
    }
}

/// "同 key nested container creation": both replicas independently create a node under the same
/// logical id, with deliberately different content.
///
/// The id is never seen in `shared`, so neither replica's local duplicate check fires. This
/// exercises what each engine's real merge does when two concurrent creates target the same
/// logical identity. Neither engine's shared `Operation` vocabulary defines a winner ahead of
/// time — this case's assertions are about what each real engine actually does (see the corpus
/// test), not an invariant assumed in advance.
#[must_use]
pub fn same_key_nested_container_creation_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let shared_key = node_id("shared-key", &mut rng);
    ConcurrentCase {
        shared: Vec::new(),
        replica_a: vec![
            Operation::CreateNode {
                id: shared_key.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::CollectionField,
            },
            Operation::SetProperty {
                id: shared_key.clone(),
                key: "name".to_string(),
                value: "from-a".to_string(),
            },
        ],
        replica_b: vec![
            Operation::CreateNode {
                id: shared_key.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::CollectionView,
            },
            Operation::SetProperty {
                id: shared_key,
                key: "name".to_string(),
                value: "from-b".to_string(),
            },
        ],
    }
}

/// Concurrent Unicode text insertion: CJK, emoji, and combining marks all land at the same text
/// position from two replicas concurrently.
///
/// The emoji include a family emoji joined with ZWJ sequences and other non-BMP astral-plane
/// codepoints (the Rust/UTF-8-side equivalent of what would require a UTF-16 surrogate pair in
/// the Web/JS-side engine); the combining marks are a base letter followed by a combining accent,
/// deliberately *not* pre-composed. The IME editing-layer interaction itself is out of scope for
/// the Rust CRDT corpus (see the delivery report); this case exercises only the text CRDT's
/// handling of the underlying Unicode content.
#[must_use]
pub fn unicode_concurrent_edit_case(seed: u64) -> ConcurrentCase {
    let mut rng = SplitMix64::new(seed);
    let block = node_id("unicode-block", &mut rng);
    ConcurrentCase {
        shared: vec![
            Operation::CreateNode {
                id: block.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            },
            Operation::InsertText {
                id: block.clone(),
                index: 0,
                // Chinese and Japanese base text.
                text: "中文日本語のテスト".to_string(),
            },
        ],
        replica_a: vec![Operation::InsertText {
            id: block.clone(),
            index: 0,
            // Emoji including a ZWJ-joined family sequence (non-BMP codepoints throughout).
            text: "🎉🎊👨‍👩‍👧‍👦".to_string(),
        }],
        replica_b: vec![Operation::InsertText {
            id: block,
            index: 0,
            // Decomposed combining marks: "e" + combining acute, "n" + combining tilde.
            text: "e\u{0301}n\u{0303}".to_string(),
        }],
    }
}

/// A batch of `count` distinct, independently-verifiable creates/edits, meant to be split into
/// two non-overlapping halves and applied to two replicas as "offline" edits before reconnecting.
///
/// Every id and every inserted text fragment is unique across the whole returned log so a
/// post-merge check can assert, for each individual operation, that its effect is actually
/// present — proving "accepted edit 零丢失" rather than only that the two replicas' hashes happen
/// to match (which alone would not rule out both replicas losing the *same* edit).
#[must_use]
pub fn offline_edit_log(seed: u64, prefix: &str, count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let mut ops = Vec::with_capacity(count as usize * 2);
    for i in 0..count {
        let id = NodeId::from(format!("{prefix}-{i}-{}", rng.token(6)));
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: i,
            kind: NodeKind::NavigatorNode,
        });
        ops.push(Operation::InsertText {
            id,
            index: 0,
            text: format!("offline-marker-{prefix}-{i}"),
        });
    }
    ops
}

/// Splits a page-shaped operation log into an `initial` prefix (applied before a snapshot is
/// taken) and a `tail` suffix.
///
/// The `tail` suffix is applied after, as incremental updates layered on top of that snapshot,
/// for the snapshot+tail-restore and snapshot-boundary corpus cases.
#[must_use]
pub fn snapshot_tail_split(seed: u64, initial_op_count: u32, tail_op_count: u32) -> (Vec<Operation>, Vec<Operation>) {
    let initial = page_ops(seed, initial_op_count);
    let tail = page_ops(seed.wrapping_add(0x5AFE_5AFE), tail_op_count);
    (initial, tail)
}

/// An out-of-order/partial-batch source log: `count` independent `CreateNode` + `InsertText`
/// pairs on disjoint nodes (no node is touched by more than one pair).
///
/// Shuffling or truncating the resulting per-operation update chunks never produces a
/// locally-invalid partial state (every prefix or permutation is still a set of well-formed,
/// independent node creations).
#[must_use]
pub fn out_of_order_source_ops(seed: u64, count: u32) -> Vec<Operation> {
    let mut rng = SplitMix64::new(seed);
    let mut ops = Vec::with_capacity(count as usize * 2);
    for i in 0..count {
        let id = NodeId::from(format!("ooo-{i}-{}", rng.token(6)));
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: i,
            kind: NodeKind::NavigatorNode,
        });
        ops.push(Operation::InsertText {
            id,
            index: 0,
            text: format!("chunk-{i}"),
        });
    }
    ops
}
