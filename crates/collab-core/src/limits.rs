//! Structural limit checks mirroring the frozen `limit_kind` values from `contracts/limits-v1.md`.
//!
//! These checks can be evaluated purely from an in-memory [`SemanticSnapshot`] and a proposed
//! [`Operation`] (or operation batch), *before* the operation ever reaches an engine adapter.
//!
//! This is a validation layer the v0.3 Rust spike does not otherwise have: there is no HTTP/WS
//! server here, so `limits-v1.md`'s `document_lock_hold_ms_max`-style persistence-path budgets
//! and the transport-level `websocket_frame_bytes_max` frame envelope genuinely do not exist at
//! this layer (see the module docs on `websocket_frame_bytes` below for why that one specific
//! `limit_kind` is intentionally *not* covered here). `update_bytes_max` already has real
//! production enforcement inside each adapter's `import_update` (see
//! [`crate::error::InputLimits`]); everything else in this module is new — added so the
//! convergence corpus can exercise `limit_kind`-precise accept/reject boundaries for the
//! remaining checkable `limits-v1.md` ceilings ahead of the real v0.4 server-side enforcement.
//!
//! `websocket_frame_bytes_max` is deliberately **not** implemented here: `limits-v1.md` defines
//! it as a transport-frame envelope size, checked "before decode" by a WebSocket frame codec that
//! does not exist anywhere in `spikes/collab-*` (no transport layer exists in this spike at all).
//! Reusing the `update_bytes` byte-length gate under a different `limit_kind` label would not be
//! testing the frame codec — it would be testing the same code twice under a false name. That
//! boundary pair is left unimplemented and reported as such; it belongs to the v0.4 transport
//! work package.

use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::SemanticSnapshot;

/// The subset of `limits-v1.md`'s frozen v0.4 `FlowLimitsV1` ceilings this module can check
/// purely from operation/semantic-snapshot shape.
///
/// Field names and values match the wire schema in `contracts/limits-v1.md` exactly
/// (`update_bytes_max`, `tree_depth_max`, `container_count_max`, `document_block_count_max`,
/// `text_block_chars_max`, `document_text_chars_max`, `semantic_patch_operations_max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentLimits {
    pub update_bytes_max: usize,
    pub tree_depth_max: usize,
    pub container_count_max: usize,
    pub document_block_count_max: usize,
    pub text_block_chars_max: usize,
    pub document_text_chars_max: usize,
    pub semantic_patch_operations_max: usize,
}

impl Default for DocumentLimits {
    fn default() -> Self {
        Self {
            update_bytes_max: 65_536,
            tree_depth_max: 32,
            container_count_max: 10_000,
            document_block_count_max: 10_000,
            text_block_chars_max: 100_000,
            document_text_chars_max: 1_000_000,
            semantic_patch_operations_max: 100,
        }
    }
}

/// A single limit-exceeded outcome, carrying the frozen `limit_kind` string verbatim from
/// `limits-v1.md`'s table (never abbreviated, pluralized, or renamed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitViolation {
    pub limit_kind: &'static str,
    pub limit: u64,
    pub observed: u64,
}

/// Depth of `id` counted as "number of ancestors", i.e. a root node (no parent) has depth 0 and
/// its direct child has depth 1. Matches how `tree_depth_max` is framed in `limits-v1.md`
/// ("recursive verification and stack has a hard bound") — the *new* node being created is what
/// gets compared against the limit, so callers pass the depth of the *new* node's parent and this
/// helper reports that parent's own depth.
fn depth_of(snapshot: &SemanticSnapshot, id: &NodeId) -> usize {
    let mut current = id.clone();
    let mut depth = 0usize;
    let mut hops = 0usize;
    while let Some(parent) = snapshot.nodes.get(&current).and_then(|node| node.parent.clone()) {
        depth += 1;
        current = parent;
        hops += 1;
        if hops > snapshot.nodes.len() + 1 {
            // A cycle would otherwise loop forever; the semantic layer's own cycle handling
            // (loro rejects at merge time, the yrs adapter's `break_cycles`) is responsible for
            // never letting one reach here. Treat it as "as deep as the whole document" rather
            // than hang, which is a safe (over-, not under-, restrictive) fallback.
            return snapshot.nodes.len();
        }
    }
    depth
}

fn live_count(snapshot: &SemanticSnapshot, kind: NodeKind) -> usize {
    snapshot
        .nodes
        .values()
        .filter(|node| !node.deleted && node.kind == kind)
        .count()
}

fn document_text_chars(snapshot: &SemanticSnapshot) -> usize {
    snapshot
        .nodes
        .values()
        .filter(|node| !node.deleted)
        .map(|node| node.text.chars().count())
        .sum()
}

/// Checks one about-to-be-applied operation against `limits`, given the snapshot of state
/// *before* the operation is applied.
///
/// Returns `Ok(())` when the operation would stay within every limit this module can check. Never
/// mutates anything and never calls into an engine — callers are responsible for actually applying
/// the operation afterwards (see [`crate::corpus::apply_operation_checked`]).
///
/// `container_count` is checked against newly-created [`NodeKind::NavigatorNode`]s (the corpus's
/// Navigator fixed model is the one explicitly sized to the 10k `container_count_max` ceiling in
/// `limits-v1.md`'s rationale column); `document_block_count` is checked against newly-created
/// [`NodeKind::Block`]s (the Page fixed model, likewise sized to the 10k node corpus). Both are
/// real, independent counters — creating a `Block` never counts against `container_count` and
/// vice versa.
pub fn check_operation(
    snapshot: &SemanticSnapshot,
    operation: &Operation,
    limits: &DocumentLimits,
) -> Result<(), LimitViolation> {
    match operation {
        Operation::CreateNode { parent, kind, .. } => {
            if let Some(parent_id) = parent {
                let new_depth = depth_of(snapshot, parent_id) + 1;
                if new_depth > limits.tree_depth_max {
                    return Err(LimitViolation {
                        limit_kind: "tree_depth",
                        limit: limits.tree_depth_max as u64,
                        observed: new_depth as u64,
                    });
                }
            }
            match kind {
                NodeKind::Block => {
                    let observed = live_count(snapshot, NodeKind::Block) + 1;
                    if observed > limits.document_block_count_max {
                        return Err(LimitViolation {
                            limit_kind: "document_block_count",
                            limit: limits.document_block_count_max as u64,
                            observed: observed as u64,
                        });
                    }
                }
                NodeKind::NavigatorNode => {
                    let observed = live_count(snapshot, NodeKind::NavigatorNode) + 1;
                    if observed > limits.container_count_max {
                        return Err(LimitViolation {
                            limit_kind: "container_count",
                            limit: limits.container_count_max as u64,
                            observed: observed as u64,
                        });
                    }
                }
                NodeKind::CollectionField | NodeKind::CollectionView | NodeKind::RecordProperty => {}
            }
            Ok(())
        }
        Operation::InsertText { id, text, .. } => {
            let current_block_len = snapshot.nodes.get(id).map_or(0, |node| node.text.chars().count());
            let added = text.chars().count();
            let new_block_len = current_block_len + added;
            if new_block_len > limits.text_block_chars_max {
                return Err(LimitViolation {
                    limit_kind: "text_block_chars",
                    limit: limits.text_block_chars_max as u64,
                    observed: new_block_len as u64,
                });
            }
            let new_document_len = document_text_chars(snapshot) + added;
            if new_document_len > limits.document_text_chars_max {
                return Err(LimitViolation {
                    limit_kind: "document_text_chars",
                    limit: limits.document_text_chars_max as u64,
                    observed: new_document_len as u64,
                });
            }
            Ok(())
        }
        Operation::MoveNode { .. }
        | Operation::DeleteNode { .. }
        | Operation::DeleteText { .. }
        | Operation::SetProperty { .. } => Ok(()),
    }
}

/// Checks a whole semantic-patch-shaped operation batch's *count* before any operation in it is
/// applied.
///
/// Matches `semantic_patch_operations_max`'s "reject the whole patch atomically, never a partial
/// prefix" semantics.
pub const fn check_operation_batch_count(op_count: usize, limits: &DocumentLimits) -> Result<(), LimitViolation> {
    if op_count > limits.semantic_patch_operations_max {
        return Err(LimitViolation {
            limit_kind: "semantic_patch_operations",
            limit: limits.semantic_patch_operations_max as u64,
            observed: op_count as u64,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::SemanticNode;
    use std::collections::BTreeMap;

    fn node(parent: Option<&str>, kind: NodeKind, text: &str) -> SemanticNode {
        SemanticNode {
            parent: parent.map(NodeId::from),
            order_key: "0".to_string(),
            kind,
            text: text.to_string(),
            properties: BTreeMap::new(),
            deleted: false,
        }
    }

    #[test]
    fn depth_of_root_is_zero_and_child_is_one() {
        let mut snapshot = SemanticSnapshot::default();
        snapshot
            .nodes
            .insert(NodeId::from("root"), node(None, NodeKind::NavigatorNode, ""));
        snapshot
            .nodes
            .insert(NodeId::from("child"), node(Some("root"), NodeKind::NavigatorNode, ""));
        assert_eq!(depth_of(&snapshot, &NodeId::from("root")), 0);
        assert_eq!(depth_of(&snapshot, &NodeId::from("child")), 1);
    }

    #[test]
    fn create_node_at_exact_depth_is_accepted_one_past_is_rejected() {
        let limits = DocumentLimits {
            tree_depth_max: 2,
            ..DocumentLimits::default()
        };
        let mut snapshot = SemanticSnapshot::default();
        snapshot
            .nodes
            .insert(NodeId::from("root"), node(None, NodeKind::NavigatorNode, ""));
        snapshot
            .nodes
            .insert(NodeId::from("depth1"), node(Some("root"), NodeKind::NavigatorNode, ""));

        // Creating under "depth1" reaches depth 2: exactly at the limit, must be accepted.
        let create_at_limit = Operation::CreateNode {
            id: NodeId::from("depth2"),
            parent: Some(NodeId::from("depth1")),
            index: 0,
            kind: NodeKind::NavigatorNode,
        };
        assert!(check_operation(&snapshot, &create_at_limit, &limits).is_ok());

        snapshot.nodes.insert(
            NodeId::from("depth2"),
            node(Some("depth1"), NodeKind::NavigatorNode, ""),
        );
        let create_over_limit = Operation::CreateNode {
            id: NodeId::from("depth3"),
            parent: Some(NodeId::from("depth2")),
            index: 0,
            kind: NodeKind::NavigatorNode,
        };
        let violation = check_operation(&snapshot, &create_over_limit, &limits).expect_err("depth 3 exceeds max 2");
        assert_eq!(violation.limit_kind, "tree_depth");
        assert_eq!(violation.limit, 2);
        assert_eq!(violation.observed, 3);
    }

    #[test]
    fn container_count_and_document_block_count_are_independent_counters() {
        let limits = DocumentLimits {
            container_count_max: 1,
            document_block_count_max: 1,
            ..DocumentLimits::default()
        };
        let mut snapshot = SemanticSnapshot::default();
        snapshot
            .nodes
            .insert(NodeId::from("nav-1"), node(None, NodeKind::NavigatorNode, ""));

        let create_block = Operation::CreateNode {
            id: NodeId::from("blk-1"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        };
        assert!(
            check_operation(&snapshot, &create_block, &limits).is_ok(),
            "one existing navigator node must not count against document_block_count"
        );

        let create_second_nav = Operation::CreateNode {
            id: NodeId::from("nav-2"),
            parent: None,
            index: 0,
            kind: NodeKind::NavigatorNode,
        };
        let violation =
            check_operation(&snapshot, &create_second_nav, &limits).expect_err("second navigator node exceeds max 1");
        assert_eq!(violation.limit_kind, "container_count");
    }

    #[test]
    fn text_block_chars_checked_before_document_text_chars() {
        let limits = DocumentLimits {
            text_block_chars_max: 3,
            document_text_chars_max: 1000,
            ..DocumentLimits::default()
        };
        let mut snapshot = SemanticSnapshot::default();
        snapshot
            .nodes
            .insert(NodeId::from("blk"), node(None, NodeKind::Block, "ab"));
        let insert = Operation::InsertText {
            id: NodeId::from("blk"),
            index: 2,
            text: "cd".to_string(),
        };
        let violation = check_operation(&snapshot, &insert, &limits).expect_err("2 + 2 = 4 exceeds block max 3");
        assert_eq!(violation.limit_kind, "text_block_chars");
        assert_eq!(violation.observed, 4);
    }

    #[test]
    fn document_text_chars_rejects_even_when_the_target_block_is_small() {
        let limits = DocumentLimits {
            text_block_chars_max: 1000,
            document_text_chars_max: 5,
            ..DocumentLimits::default()
        };
        let mut snapshot = SemanticSnapshot::default();
        snapshot
            .nodes
            .insert(NodeId::from("blk-a"), node(None, NodeKind::Block, "abcd"));
        snapshot
            .nodes
            .insert(NodeId::from("blk-b"), node(None, NodeKind::Block, ""));
        let insert = Operation::InsertText {
            id: NodeId::from("blk-b"),
            index: 0,
            text: "xy".to_string(),
        };
        let violation =
            check_operation(&snapshot, &insert, &limits).expect_err("document total 4 + 2 = 6 exceeds max 5");
        assert_eq!(violation.limit_kind, "document_text_chars");
        assert_eq!(violation.observed, 6);
    }

    #[test]
    fn batch_count_exact_accepted_plus_one_rejected() {
        let limits = DocumentLimits {
            semantic_patch_operations_max: 2,
            ..DocumentLimits::default()
        };
        assert!(check_operation_batch_count(2, &limits).is_ok());
        let violation = check_operation_batch_count(3, &limits).expect_err("3 exceeds max 2");
        assert_eq!(violation.limit_kind, "semantic_patch_operations");
        assert_eq!(violation.limit, 2);
        assert_eq!(violation.observed, 3);
    }
}
