use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::operation::{NodeId, NodeKind};

/// One node's fully-merged, engine-independent state. Both adapters export their internal engine
/// state into this shape so a semantic hash can be compared across candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNode {
    pub parent: Option<NodeId>,
    pub order_key: String,
    pub kind: NodeKind,
    pub text: String,
    pub properties: BTreeMap<String, String>,
    pub deleted: bool,
}

/// The canonical, engine-independent snapshot of a merged document.
///
/// `nodes` is a `BTreeMap` (not `HashMap`) specifically so that `serde_json` serialization is
/// key-order-deterministic without depending on the `preserve_order` feature flag: this is what
/// makes `semantic_hash` reproducible run to run and comparable byte-for-byte between the loro
/// and yrs-yjs candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SemanticSnapshot {
    #[serde(default)]
    pub nodes: BTreeMap<NodeId, SemanticNode>,
}

/// One node whose engine-independent state differs between two accepted document sequences.
///
/// Both sides use [`SemanticNode`], deliberately keeping engine update bytes, version-vector
/// entries, and peer ids outside the public diff vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNodeChange {
    pub before: SemanticNode,
    pub after: SemanticNode,
}

/// Deterministic semantic delta between two [`SemanticSnapshot`]s.
///
/// Logical [`NodeId`] values are caller/application ids, not CRDT peer ids. `BTreeMap` keeps the
/// JSON representation stable while the value types make it impossible for an engine's binary
/// update representation to leak through this structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SemanticDiff {
    pub added: BTreeMap<NodeId, SemanticNode>,
    pub removed: BTreeMap<NodeId, SemanticNode>,
    pub changed: BTreeMap<NodeId, SemanticNodeChange>,
}

impl SemanticSnapshot {
    /// Computes an engine-independent semantic delta from `self` to `after`.
    #[must_use]
    pub fn diff(&self, after: &Self) -> SemanticDiff {
        let mut diff = SemanticDiff::default();
        for (id, before_node) in &self.nodes {
            match after.nodes.get(id) {
                None => {
                    diff.removed.insert(id.clone(), before_node.clone());
                }
                Some(after_node) if after_node != before_node => {
                    diff.changed.insert(
                        id.clone(),
                        SemanticNodeChange {
                            before: before_node.clone(),
                            after: after_node.clone(),
                        },
                    );
                }
                Some(_) => {}
            }
        }
        for (id, after_node) in &after.nodes {
            if !self.nodes.contains_key(id) {
                diff.added.insert(id.clone(), after_node.clone());
            }
        }
        diff
    }

    /// Canonical JSON bytes: sorted map keys, no insertion-order dependence.
    ///
    /// # Errors
    /// Returns an error only if the snapshot somehow contains non-finite floats or similar
    /// values `serde_json` refuses to encode; `SemanticNode`'s fields never do, so in practice
    /// this always succeeds, but the caller still gets a `Result` rather than an `unwrap`.
    pub fn canonical_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// SHA-256 of the canonical JSON, as a lowercase hex string (64 chars, matching the
    /// `sha256` pattern in both result schemas).
    #[must_use]
    pub fn semantic_hash(&self) -> String {
        // Canonical JSON encoding of a `BTreeMap`-backed struct cannot fail in practice (see
        // `canonical_json`'s doc comment); an encode failure here would mean serde_json itself
        // regressed, not a caller error, so we hash an empty-document sentinel bytes on the
        // (unreachable in practice) error path rather than unwrap/expect.
        let bytes = self.canonical_json().unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    /// A node is reachable when it is not itself tombstoned and every ancestor up to the root is
    /// also not tombstoned. Matches the corpus invariant "ancestor delete does not implicitly
    /// lose a subtree that was moved out from under it": a node moved under a live parent becomes
    /// reachable again regardless of its old, now-deleted, parent.
    #[must_use]
    pub fn is_reachable(&self, id: &NodeId) -> bool {
        let mut current = id.clone();
        let mut hops = 0usize;
        loop {
            let Some(node) = self.nodes.get(&current) else {
                return false;
            };
            if node.deleted {
                return false;
            }
            match &node.parent {
                None => return true,
                Some(parent_id) => {
                    current = parent_id.clone();
                }
            }
            hops += 1;
            if hops > self.nodes.len() + 1 {
                // A cycle would otherwise loop forever; treat it as unreachable/invalid rather
                // than hanging. Adapters are responsible for never letting a cycle form (see the
                // `concurrent_block_move` invariant test), so this is a defensive fallback only.
                return false;
            }
        }
    }

    /// Returns `true` if following any node's parent chain (through the whole map, deleted or
    /// not) ever revisits a node already on the path, i.e. the raw parent-pointer graph has a
    /// cycle. Used directly by invariant tests, independent of tombstone state.
    #[must_use]
    pub fn has_cycle(&self) -> bool {
        for start in self.nodes.keys() {
            let mut current = start.clone();
            let mut seen = std::collections::HashSet::new();
            loop {
                if !seen.insert(current.clone()) {
                    return true;
                }
                match self.nodes.get(&current).and_then(|node| node.parent.clone()) {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<&str>, order_key: &str) -> SemanticNode {
        SemanticNode {
            parent: parent.map(NodeId::from),
            order_key: order_key.to_string(),
            kind: NodeKind::Block,
            text: String::new(),
            properties: BTreeMap::new(),
            deleted: false,
        }
    }

    #[test]
    fn identical_snapshots_hash_identically_regardless_of_insertion_order() {
        let mut a = SemanticSnapshot::default();
        a.nodes.insert(NodeId::from("b"), node(None, "b"));
        a.nodes.insert(NodeId::from("a"), node(None, "a"));

        let mut b = SemanticSnapshot::default();
        b.nodes.insert(NodeId::from("a"), node(None, "a"));
        b.nodes.insert(NodeId::from("b"), node(None, "b"));

        assert_eq!(a.semantic_hash(), b.semantic_hash());
    }

    #[test]
    fn differing_snapshots_hash_differently() {
        let mut a = SemanticSnapshot::default();
        a.nodes.insert(NodeId::from("a"), node(None, "a"));

        let mut b = SemanticSnapshot::default();
        b.nodes.insert(NodeId::from("a"), node(Some("root"), "a"));

        assert_ne!(a.semantic_hash(), b.semantic_hash());
    }

    #[test]
    fn detects_a_two_node_cycle() {
        let mut snap = SemanticSnapshot::default();
        snap.nodes.insert(NodeId::from("x"), node(Some("y"), "a"));
        snap.nodes.insert(NodeId::from("y"), node(Some("x"), "a"));
        assert!(snap.has_cycle());
    }

    #[test]
    fn no_cycle_in_a_normal_tree() {
        let mut snap = SemanticSnapshot::default();
        snap.nodes.insert(NodeId::from("root"), node(None, "a"));
        snap.nodes.insert(NodeId::from("child"), node(Some("root"), "a"));
        assert!(!snap.has_cycle());
    }

    #[test]
    fn node_under_deleted_ancestor_is_unreachable_but_moved_out_node_is_reachable() {
        let mut snap = SemanticSnapshot::default();
        let mut deleted_root = node(None, "a");
        deleted_root.deleted = true;
        snap.nodes.insert(NodeId::from("deleted-root"), deleted_root);
        snap.nodes
            .insert(NodeId::from("still-under"), node(Some("deleted-root"), "a"));
        snap.nodes.insert(NodeId::from("live-root"), node(None, "b"));
        snap.nodes
            .insert(NodeId::from("moved-out"), node(Some("live-root"), "a"));

        assert!(!snap.is_reachable(&NodeId::from("still-under")));
        assert!(snap.is_reachable(&NodeId::from("moved-out")));
    }

    #[test]
    fn semantic_diff_reports_added_removed_and_changed_nodes_without_engine_identifiers() {
        let mut before = SemanticSnapshot::default();
        before.nodes.insert(NodeId::from("removed"), node(None, "a"));
        before.nodes.insert(NodeId::from("changed"), node(None, "b"));

        let mut after = SemanticSnapshot::default();
        let mut changed = node(None, "b");
        changed.text = "new text".to_string();
        after.nodes.insert(NodeId::from("changed"), changed);
        after.nodes.insert(NodeId::from("added"), node(None, "c"));

        let diff = before.diff(&after);
        assert_eq!(diff.added.keys().map(AsRef::as_ref).collect::<Vec<_>>(), ["added"]);
        assert_eq!(diff.removed.keys().map(AsRef::as_ref).collect::<Vec<_>>(), ["removed"]);
        assert_eq!(diff.changed.keys().map(AsRef::as_ref).collect::<Vec<_>>(), ["changed"]);

        let wire = serde_json::to_value(diff).expect("semantic diff serializes");
        assert!(wire.get("bytes").is_none());
        assert!(wire.get("peer_id").is_none());
        assert!(!wire.to_string().contains("peer_id"));
    }
}
