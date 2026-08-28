//! Real `yrs` engine adapter implementing the shared [`collab_shared::CollabEngine`] /
//! [`collab_shared::CorpusEngine`] contracts.
//!
//! Unlike Loro, yrs has no native moveable-tree container: the closest primitives are `Map`,
//! `Array`, `Text` and `XmlFragment`. This adapter therefore *builds* a tree on top of `Map`:
//! every logical node is an entry in a top-level `nodes: Map<node_id, Map>` container, and each
//! node's own nested map holds a `parent` LWW register (a plain node-id string, or absent for a
//! root), an `order` LWW register (a fractional-index string from [`collab_shared::order`]), a
//! `deleted` tombstone flag, a `kind` tag, `prop:<key>` property registers, and a nested `text`
//! [`yrs::TextRef`] for rich text.
//!
//! Two concurrency properties that Loro's tree gets "for free" from its CRDT algorithm have to be
//! engineered explicitly here:
//!
//! - **No lost subtree on ancestor delete + descendant move**: this falls out for free from the
//!   data model above, because `deleted` is a per-node flag, not a recursive operation — a node
//!   moved out from under a deleted ancestor is reachable again purely because its own `parent`
//!   register points elsewhere. See [`collab_shared::SemanticSnapshot::is_reachable`].
//! - **No cycles from concurrent moves**: each node's `parent` register is an *independent* LWW
//!   cell, so two concurrent moves that would jointly form a cycle (A's parent set to B on one
//!   replica, B's parent set to A on another) each resolve locally as an ordinary LWW write —
//!   nothing in yrs itself notices the cycle. [`break_cycles`] is a deterministic, read-only,
//!   post-merge pass run inside `semantic_snapshot` that detects any such cycle and severs it
//!   (reparents the lexicographically-smallest node in the cycle to the root). Because it is a
//!   pure function of the (already-converged) merged state, both replicas compute the identical
//!   resolution without writing anything back into the CRDT.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use collab_shared::{
    CollabEngine, CollabError, CorpusEngine, Diff, Frontier, InputLimits, NodeId, NodeKind, Operation, SemanticNode,
    SemanticSnapshot,
};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, GetString, Map, MapPrelim, MapRef, Out, ReadTxn, StateVector, Text, TextPrelim, Transact, Update, WriteTxn,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMetadata {
    pub candidate: &'static str,
    pub rust_engine: &'static str,
    pub rust_engine_version: &'static str,
}

#[must_use]
pub const fn metadata() -> EngineMetadata {
    EngineMetadata {
        candidate: "yrs-yjs",
        rust_engine: "yrs",
        rust_engine_version: "0.27.3",
    }
}

const NODES_MAP: &str = "nodes";
const FIELD_PARENT: &str = "parent";
const FIELD_ORDER: &str = "order";
const FIELD_KIND: &str = "kind";
const FIELD_DELETED: &str = "deleted";
const FIELD_TEXT: &str = "text";
const PROPERTY_PREFIX: &str = "prop:";

pub struct YrsCollabEngine {
    doc: Doc,
    nodes: MapRef,
    /// Short, replica-distinguishing token derived from this replica's own `client_id`, used only
    /// to disambiguate fractional-index collisions (see `collab_shared::order`). Never exposed
    /// outside this file.
    replica_token: String,
    /// Incrementally-maintained `parent_key -> sorted (order_key, node_id)` index, purely a local
    /// read cache (never synced, never part of the CRDT state) that exists so `CreateNode`/
    /// `MoveNode` can find "this parent's children, in order" in O(children) instead of scanning
    /// every node in the document. yrs has no native tree/children-index primitive (unlike Loro's
    /// `LoroTree`, which returns a parent's children directly from the engine); without this
    /// index, `siblings_sorted` previously scanned the entire flat `nodes` map on every single
    /// `CreateNode`/`MoveNode`, which is O(n) per op and therefore O(n^2) (empirically closer to
    /// n^2.1-n^2.3, likely from the per-call sort) over a bootstrap of n sequential ops -- see
    /// `spikes/collab-shared`'s benchmark delivery report for the measured blowup this fixes.
    /// This is a real, disclosed implementation cost specific to adapting yrs onto a tree shape it
    /// does not natively provide, not a performance property of yrs the library itself.
    ///
    /// Kept consistent with the real (possibly remote-merged) CRDT state via two different paths:
    /// incrementally, by `CreateNode`/`MoveNode` themselves when they run locally; and by a full
    /// [`Self::rebuild_children_index`] scan after `load`/`import_update`, since remote-origin ops
    /// are decoded straight into the yrs `Doc` and never pass through this adapter's own
    /// `apply_operation` match arms at all (mirroring `LoroCollabEngine::rebuild_id_cache`'s same
    /// reasoning on the Loro side).
    children_index: HashMap<String, BTreeSet<(String, NodeId)>>,
}

fn empty_map_prelim() -> MapPrelim {
    std::iter::empty::<(String, String)>().collect::<MapPrelim>()
}

const fn kind_to_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Block => "block",
        NodeKind::CollectionField => "collection_field",
        NodeKind::CollectionView => "collection_view",
        NodeKind::RecordProperty => "record_property",
        NodeKind::NavigatorNode => "navigator_node",
    }
}

fn str_to_kind(value: &str) -> NodeKind {
    match value {
        "collection_field" => NodeKind::CollectionField,
        "collection_view" => NodeKind::CollectionView,
        "record_property" => NodeKind::RecordProperty,
        "navigator_node" => NodeKind::NavigatorNode,
        _ => NodeKind::Block,
    }
}

fn as_string(out: &Out) -> Option<String> {
    match out {
        Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

const fn as_bool(out: &Out) -> Option<bool> {
    match out {
        Out::Any(yrs::Any::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Computes a fresh `parent_key -> sorted (order_key, node_id)` index by scanning every node in
/// `nodes` once. A free function (not a `&self`/`&mut self` method) so callers can hold an
/// immutable read on `nodes` and `txn` (both borrowed from the engine's fields) while computing
/// the result, then separately assign it into `self.children_index` with a plain field write --
/// avoiding a `&mut self` method call that the borrow checker would otherwise see as conflicting
/// with the live `Transaction` borrowed from `self.doc`. Called after `load` (fresh reconstruction
/// from a snapshot) and after every state-changing `import_update`: those are the only two points
/// where remote-origin ops (decoded straight into the yrs `Doc`, never passing through this
/// adapter's own `CreateNode`/`MoveNode` `apply_operation` arms) could leave the incrementally-
/// maintained index out of sync with the real merged CRDT state -- mirroring
/// `LoroCollabEngine::rebuild_id_cache`'s same reasoning on the Loro side. O(n) once per
/// `load`/`import_update` call is the intended trade-off: `children_index` exists precisely so
/// the far more frequent per-operation `CreateNode`/`MoveNode` path costs O(children) instead of
/// an O(n) scan on every single local op.
fn compute_children_index<T: ReadTxn>(nodes: &MapRef, txn: &T) -> HashMap<String, BTreeSet<(String, NodeId)>> {
    let mut index: HashMap<String, BTreeSet<(String, NodeId)>> = HashMap::new();
    for (key, value) in nodes.iter(txn) {
        let Out::YMap(node_map) = value else { continue };
        let parent = node_map
            .get(txn, FIELD_PARENT)
            .as_ref()
            .and_then(as_string)
            .unwrap_or_default();
        let order = node_map
            .get(txn, FIELD_ORDER)
            .as_ref()
            .and_then(as_string)
            .unwrap_or_default();
        index.entry(parent).or_default().insert((order, NodeId::from(key)));
    }
    index
}

impl YrsCollabEngine {
    fn node_map<T: ReadTxn>(&self, txn: &T, id: &str) -> Option<MapRef> {
        match self.nodes.get(txn, id)? {
            Out::YMap(map) => Some(map),
            _ => None,
        }
    }

    fn require_node_map<T: ReadTxn>(&self, txn: &T, id: &NodeId) -> Result<MapRef, CollabError> {
        self.node_map(txn, id)
            .ok_or_else(|| CollabError::UnknownNode { id: id.to_string() })
    }

    fn field_string<T: ReadTxn>(txn: &T, node_map: &MapRef, field: &str) -> Option<String> {
        node_map.get(txn, field).as_ref().and_then(as_string)
    }

    /// Sorted `(node_id, order_key)` pairs for every node currently indexed under `parent` (an
    /// empty string encodes "no parent" / root, matching how `CreateNode`/`MoveNode` write it).
    /// Reads `children_index` -- O(children) -- rather than scanning every node in the document;
    /// see that field's own doc comment for why this distinction matters at scale.
    fn siblings_sorted(&self, parent: &str) -> Vec<(NodeId, String)> {
        let Some(siblings) = self.children_index.get(parent) else {
            return Vec::new();
        };
        siblings.iter().map(|(order, id)| (id.clone(), order.clone())).collect()
    }

    fn order_key_for_position(&self, parent: &str, index: u32, exclude: Option<&str>) -> String {
        let mut siblings = self.siblings_sorted(parent);
        if let Some(exclude_id) = exclude {
            siblings.retain(|(id, _)| id.as_ref() != exclude_id);
        }
        let position = (index as usize).min(siblings.len());
        let lower = if position == 0 {
            None
        } else {
            siblings.get(position - 1).map(|(_, o)| o.as_str())
        };
        let upper = siblings.get(position).map(|(_, o)| o.as_str());
        let base = collab_shared::order::between(lower, upper);
        collab_shared::order::with_replica_tiebreak(&base, &self.replica_token)
    }

    fn parent_key(parent: Option<&NodeId>) -> String {
        parent.map_or_else(String::new, std::string::ToString::to_string)
    }
}

impl CollabEngine for YrsCollabEngine {
    type Error = CollabError;

    fn load(snapshot: &[u8]) -> Result<Self, Self::Error> {
        InputLimits::default().validate_snapshot(snapshot)?;
        let doc = Doc::new();
        let nodes = {
            let mut txn = doc
                .try_transact_mut()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            let nodes = txn.get_or_insert_map(NODES_MAP);
            let update = Update::decode_v1(snapshot).map_err(|e| CollabError::DecodeFailed {
                input: "snapshot",
                reason: e.to_string(),
            })?;
            txn.apply_update(update).map_err(|e| CollabError::DecodeFailed {
                input: "snapshot",
                reason: e.to_string(),
            })?;
            nodes
        };
        let replica_token = format!("{}", doc.client_id());
        let mut engine = Self {
            doc,
            nodes,
            replica_token,
            children_index: HashMap::new(),
        };
        {
            let txn = engine
                .doc
                .try_transact()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            engine.children_index = compute_children_index(&engine.nodes, &txn);
        }
        Ok(engine)
    }

    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error> {
        InputLimits::default().validate_update(update)?;
        // Decoding happens *before* any write-transaction is opened, so a malformed update can
        // never partially mutate state: this whole method is a no-op on the `Err` path.
        let decoded = Update::decode_v1(update).map_err(|e| CollabError::DecodeFailed {
            input: "update",
            reason: e.to_string(),
        })?;
        let before = {
            let txn = self
                .doc
                .try_transact()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            txn.state_vector()
        };
        {
            let mut txn = self
                .doc
                .try_transact_mut()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            txn.apply_update(decoded).map_err(|e| CollabError::DecodeFailed {
                input: "update",
                reason: e.to_string(),
            })?;
        }
        let after = {
            let txn = self
                .doc
                .try_transact()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            txn.state_vector()
        };
        let changed = before != after;
        if changed {
            // Remote-origin ops just applied above were decoded straight into `self.doc` and
            // never passed through this adapter's own `CreateNode`/`MoveNode` `apply_operation`
            // arms, so `children_index`'s incremental maintenance never saw them -- rebuild it
            // from the (now-merged) state. Skipped when `changed` is false (a pure duplicate/
            // already-known update) since the index is already correct in that case.
            let txn = self
                .doc
                .try_transact()
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
            self.children_index = compute_children_index(&self.nodes, &txn);
        }
        Ok(Diff { changed })
    }

    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error> {
        let txn = self
            .doc
            .try_transact()
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
        Ok(txn.encode_state_as_update_v1(&StateVector::default()))
    }

    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error> {
        let state_vector = if frontier.is_empty() {
            StateVector::default()
        } else {
            StateVector::decode_v1(frontier.as_bytes()).map_err(|e| CollabError::DecodeFailed {
                input: "frontier",
                reason: e.to_string(),
            })?
        };
        let txn = self
            .doc
            .try_transact()
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
        Ok(txn.encode_state_as_update_v1(&state_vector))
    }

    fn frontier(&self) -> Frontier {
        // A read-only transaction can only fail to acquire while a write transaction from this
        // same replica is concurrently open, which never happens across the boundary of a public
        // `CollabEngine` method call; fall back to an empty (start-of-time) frontier rather than
        // panicking if that invariant is ever violated.
        let Ok(txn) = self.doc.try_transact() else {
            return Frontier::from_bytes(Vec::new());
        };
        Frontier::from_bytes(txn.state_vector().encode_v1())
    }
}

impl CorpusEngine for YrsCollabEngine {
    fn new_empty(replica_seed: u64) -> Self {
        let doc = Doc::with_client_id(replica_seed.wrapping_add(1).max(1));
        let nodes = doc.get_or_insert_map(NODES_MAP);
        let replica_token = format!("{}", doc.client_id());
        Self {
            doc,
            nodes,
            replica_token,
            children_index: HashMap::new(),
        }
    }

    fn apply_operation(&mut self, operation: &Operation) -> Result<(), Self::Error> {
        match operation {
            Operation::CreateNode {
                id,
                parent,
                index,
                kind,
            } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                if self.node_map(&txn, id).is_some() {
                    return Err(CollabError::DuplicateNode { id: id.to_string() });
                }
                let parent_key = Self::parent_key(parent.as_ref());
                if !parent_key.is_empty() && self.node_map(&txn, &parent_key).is_none() {
                    return Err(CollabError::UnknownNode { id: parent_key });
                }
                let order_key = self.order_key_for_position(&parent_key, *index, None);
                self.children_index
                    .entry(parent_key.clone())
                    .or_default()
                    .insert((order_key.clone(), id.clone()));

                let node_map: MapRef = self.nodes.insert(&mut txn, id.as_ref(), empty_map_prelim());
                node_map.insert(&mut txn, FIELD_PARENT, parent_key);
                node_map.insert(&mut txn, FIELD_ORDER, order_key);
                node_map.insert(&mut txn, FIELD_KIND, kind_to_str(*kind));
                node_map.insert(&mut txn, FIELD_DELETED, false);
                let _text: yrs::TextRef = node_map.insert(&mut txn, FIELD_TEXT, TextPrelim::new(""));
                drop(txn);
                Ok(())
            }
            Operation::MoveNode { id, new_parent, index } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                let node_map = self.require_node_map(&txn, id)?;
                // Read this node's *current* parent/order before any mutation below, so its old
                // `children_index` entry (indexed under its old parent, not the new one) can be
                // removed -- otherwise a moved node would be double-counted as a sibling of both
                // its old and new parent on every subsequent `order_key_for_position` call.
                let old_parent_key = Self::field_string(&txn, &node_map, FIELD_PARENT).unwrap_or_default();
                let old_order_key = Self::field_string(&txn, &node_map, FIELD_ORDER).unwrap_or_default();
                let parent_key = Self::parent_key(new_parent.as_ref());
                if !parent_key.is_empty() && self.node_map(&txn, &parent_key).is_none() {
                    return Err(CollabError::UnknownNode { id: parent_key });
                }
                let order_key = self.order_key_for_position(&parent_key, *index, Some(id.as_ref()));
                if let Some(old_siblings) = self.children_index.get_mut(&old_parent_key) {
                    old_siblings.remove(&(old_order_key, id.clone()));
                }
                self.children_index
                    .entry(parent_key.clone())
                    .or_default()
                    .insert((order_key.clone(), id.clone()));
                node_map.insert(&mut txn, FIELD_PARENT, parent_key);
                node_map.insert(&mut txn, FIELD_ORDER, order_key);
                drop(txn);
                Ok(())
            }
            Operation::DeleteNode { id } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                let node_map = self.require_node_map(&txn, id)?;
                node_map.insert(&mut txn, FIELD_DELETED, true);
                drop(txn);
                Ok(())
            }
            Operation::InsertText { id, index, text } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                let node_map = self.require_node_map(&txn, id)?;
                let Some(Out::YText(text_ref)) = node_map.get(&txn, FIELD_TEXT) else {
                    return Err(CollabError::OperationFailed {
                        reason: format!("node {id} has no text container"),
                    });
                };
                text_ref.insert(&mut txn, *index, text);
                drop(txn);
                Ok(())
            }
            Operation::DeleteText { id, index, len } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                let node_map = self.require_node_map(&txn, id)?;
                let Some(Out::YText(text_ref)) = node_map.get(&txn, FIELD_TEXT) else {
                    return Err(CollabError::OperationFailed {
                        reason: format!("node {id} has no text container"),
                    });
                };
                let current_len = text_ref.len(&txn);
                let position = (*index).min(current_len);
                let clamped_len = (*len).min(current_len.saturating_sub(position));
                text_ref.remove_range(&mut txn, position, clamped_len);
                drop(txn);
                Ok(())
            }
            Operation::SetProperty { id, key, value } => {
                let mut txn = self
                    .doc
                    .try_transact_mut()
                    .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
                let node_map = self.require_node_map(&txn, id)?;
                let full_key = format!("{PROPERTY_PREFIX}{key}");
                node_map.insert(&mut txn, full_key.as_str(), value.as_str());
                drop(txn);
                Ok(())
            }
        }
    }

    fn semantic_snapshot(&self) -> Result<SemanticSnapshot, Self::Error> {
        let txn = self
            .doc
            .try_transact()
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;

        let mut snapshot = SemanticSnapshot::default();
        for (key, value) in self.nodes.iter(&txn) {
            let Out::YMap(node_map) = value else { continue };
            let id = NodeId::from(key);
            let parent_raw = Self::field_string(&txn, &node_map, FIELD_PARENT).unwrap_or_default();
            let parent = if parent_raw.is_empty() {
                None
            } else {
                Some(NodeId::from(parent_raw))
            };
            let order_key = Self::field_string(&txn, &node_map, FIELD_ORDER).unwrap_or_default();
            let kind = Self::field_string(&txn, &node_map, FIELD_KIND).map_or(NodeKind::Block, |s| str_to_kind(&s));
            let deleted = node_map
                .get(&txn, FIELD_DELETED)
                .as_ref()
                .and_then(as_bool)
                .unwrap_or(false);
            let text = match node_map.get(&txn, FIELD_TEXT) {
                Some(Out::YText(text_ref)) => text_ref.get_string(&txn),
                _ => String::new(),
            };

            let mut properties = BTreeMap::new();
            for (field_key, field_value) in node_map.iter(&txn) {
                if let Some(prop_name) = field_key.strip_prefix(PROPERTY_PREFIX)
                    && let Some(value) = as_string(&field_value)
                {
                    properties.insert(prop_name.to_string(), value);
                }
            }

            snapshot.nodes.insert(
                id,
                SemanticNode {
                    parent,
                    order_key,
                    kind,
                    text,
                    properties,
                    deleted,
                },
            );
        }
        drop(txn);

        Ok(break_cycles(snapshot))
    }
}

/// Deterministic, read-only cycle resolution over a raw parent-pointer projection (see the module
/// doc comment for why this is necessary for a `Map`-emulated tree). Repeatedly finds a cycle and
/// detaches its lexicographically-smallest member to the root, until no cycle remains.
fn break_cycles(mut snapshot: SemanticSnapshot) -> SemanticSnapshot {
    while let Some(id) = find_cycle_member(&snapshot) {
        if let Some(node) = snapshot.nodes.get_mut(&id) {
            node.parent = None;
        }
    }
    snapshot
}

fn find_cycle_member(snapshot: &SemanticSnapshot) -> Option<NodeId> {
    let mut globally_cleared: HashSet<NodeId> = HashSet::new();
    for start in snapshot.nodes.keys() {
        if globally_cleared.contains(start) {
            continue;
        }
        let mut path: Vec<NodeId> = Vec::new();
        let mut on_path: HashSet<NodeId> = HashSet::new();
        let mut current = start.clone();
        loop {
            if on_path.contains(&current) {
                // `on_path`/`path` are always updated in lockstep (every push below pairs with an
                // insert above), so `on_path.contains(&current)` guarantees `current` is present
                // in `path` and `position` always returns `Some`; `.get()` still avoids a
                // panicking index expression rather than trusting that invariant at this call
                // site too.
                let cycle_start = path.iter().position(|id| *id == current).unwrap_or(0);
                let cycle_members = path.get(cycle_start..).unwrap_or(&path);
                return cycle_members.iter().min().cloned();
            }
            on_path.insert(current.clone());
            path.push(current.clone());
            match snapshot.nodes.get(&current).and_then(|node| node.parent.clone()) {
                Some(parent) if snapshot.nodes.contains_key(&parent) => current = parent,
                _ => break,
            }
        }
        globally_cleared.extend(path);
    }
    None
}
