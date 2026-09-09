//! The Loro adapter selected in v0.3, promoted from `spikes/collab-loro` to production.
//!
//! Every business-facing id in this module is a [`NodeId`] chosen by the caller (the Flow command
//! layer, or a fixture in this crate's own tests). Loro's own [`TreeID`] (which embeds a
//! [`loro::PeerID`]) never crosses out of this file: it is looked up from a purely local,
//! non-synced cache (`id_to_tree` / `tree_to_id`), rebuilt after every `load`/`import_update` by
//! reading back a `logical_id` field this adapter itself writes into each tree node's meta map.
//! That `logical_id` field is business data flowing *into* the engine (so this replica can find
//! "the node the fixture calls `blk-abc123`" again after a remote peer creates it) — it is not the
//! engine exposing its own identity outward.
//!
//! Beyond the `tree` container the spike adapter had, this production version also attaches a
//! root-level `meta` map for document-level fields. `ADR-0002` (`decisions/` in
//! `/opt/working/sylvode-flow`) freezes `meta.title` as the canonical source for a Page/Collection
//! title (`flow_object_projections.title` is only the rebuildable read replica), so the engine
//! needs a place to hold it that survives snapshot/update round trips exactly like tree content
//! does.

use std::collections::BTreeMap;
use std::collections::HashMap;

use loro::{
    Container, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer, VersionVector,
};

use crate::error::{CollabError, InputLimits};
use crate::frontier::Frontier;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::{SemanticNode, SemanticSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMetadata {
    pub candidate: &'static str,
    pub rust_engine: &'static str,
    pub rust_engine_version: &'static str,
}

#[must_use]
pub const fn metadata() -> EngineMetadata {
    EngineMetadata {
        candidate: "loro",
        rust_engine: "loro",
        rust_engine_version: "1.13.9",
    }
}

/// Summary of what an `import_update` call actually changed, without exposing any engine-internal
/// op/diff representation to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Diff {
    /// `false` when the update was a byte-for-byte or semantic no-op (e.g. a duplicate replay).
    pub changed: bool,
}

/// The frozen v0.3 sync contract every adapter implements:
///
/// ```ignore
/// trait CollabEngine {
///     fn load(snapshot: &[u8]) -> Result<Self>;
///     fn import_update(&mut self, update: &[u8]) -> Result<Diff>;
///     fn export_snapshot(&self) -> Result<Vec<u8>>;
///     fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>>;
///     fn frontier(&self) -> Frontier;
/// }
/// ```
///
/// Every method returns a typed [`CollabError`] (never a boxed/opaque error), every byte-slice
/// input is length-validated before it reaches the underlying engine's decoder (see
/// [`LoroCollabEngine`]'s use of [`InputLimits`]), and [`Frontier`] is an opaque byte wrapper so no
/// implementation can leak an engine-specific peer/client id through this trait.
pub trait CollabEngine: Sized {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Loads a document from a full snapshot previously produced by [`Self::export_snapshot`].
    fn load(snapshot: &[u8]) -> Result<Self, Self::Error>;

    /// Applies a remote update (full snapshot or incremental delta) produced by
    /// [`Self::export_snapshot`] or [`Self::export_from`]. Must be atomic: on error, local state
    /// (and therefore [`Self::frontier`]) is left byte-for-byte unchanged.
    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error>;

    /// Exports the full document state (history + current state) as a single portable blob.
    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error>;

    /// Exports only the changes this replica has that the given remote `frontier` does not.
    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error>;

    /// The current version marker for this replica, suitable for passing to a peer's
    /// [`Self::export_from`].
    fn frontier(&self) -> Frontier;
}

const TREE_CONTAINER: &str = "tree";
const META_CONTAINER: &str = "meta";
const META_TITLE: &str = "title";
const META_LOGICAL_ID: &str = "logical_id";
const META_KIND: &str = "kind";
const META_TEXT: &str = "text";
const META_PROPERTY_PREFIX: &str = "prop:";

pub struct LoroCollabEngine {
    doc: LoroDoc,
    tree: LoroTree,
    meta: LoroMap,
    id_to_tree: HashMap<NodeId, TreeID>,
    tree_to_id: HashMap<TreeID, NodeId>,
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

fn map_loro_error(err: &loro::LoroError, node_id: &str) -> CollabError {
    if matches!(err, loro::LoroError::TreeError(loro::LoroTreeError::CyclicMoveError)) {
        CollabError::CycleRejected {
            id: node_id.to_string(),
        }
    } else {
        CollabError::OperationFailed {
            reason: err.to_string(),
        }
    }
}

/// Zero-pads `position` to at least 8 digits (matching `format!("{position:08}")`'s exact output
/// for every non-negative value, including the unpadded case where `position` already has 8+
/// digits), without routing through `core::fmt`'s generic width/fill machinery
/// (`Formatter::pad_integral`).
///
/// `semantic_snapshot` calls this once per node, so its cost multiplies by document size; a
/// release-mode `perf` profile of the isolated-apply worker at the `container_count_max`/
/// `document_block_count_max` boundary (10,000 nodes) showed `Formatter::pad_integral` and the
/// `core::fmt::write` machinery it pulls in accounting for roughly a tenth of total samples,
/// almost entirely attributable to this one call site -- the `{:08}` width specifier routes
/// unsigned integer formatting through that generic, allocating-per-`write_str`-call path instead
/// of `usize::to_string`'s specialized fast path. Using `to_string` (fast path) plus a manual
/// left-pad keeps the identical output with meaningfully less per-call overhead.
fn zero_padded_order_key(position: usize) -> String {
    let digits = position.to_string();
    if digits.len() >= 8 {
        return digits;
    }
    let mut key = String::with_capacity(8);
    for _ in 0..(8 - digits.len()) {
        key.push('0');
    }
    key.push_str(&digits);
    key
}

fn read_string_value(value_or_container: &ValueOrContainer) -> Option<String> {
    match value_or_container {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.as_ref().to_string()),
        _ => None,
    }
}

impl LoroCollabEngine {
    fn attach_tree(doc: &LoroDoc) -> LoroTree {
        let tree = doc.get_tree(TREE_CONTAINER);
        // jitter=0 keeps sibling ordering fully deterministic for a given operation log; Loro
        // still resolves *concurrent* insert/move collisions correctly (this is exactly what the
        // moveable-tree CRDT is for).
        tree.enable_fractional_index(0);
        tree
    }

    fn attach_meta(doc: &LoroDoc) -> LoroMap {
        doc.get_map(META_CONTAINER)
    }

    fn resolve_parent(&self, parent: Option<&NodeId>) -> Result<TreeParentId, CollabError> {
        parent.map_or(Ok(TreeParentId::Root), |id| {
            self.id_to_tree
                .get(id)
                .copied()
                .map(TreeParentId::Node)
                .ok_or_else(|| CollabError::UnknownNode { id: id.to_string() })
        })
    }

    fn require_tree_id(&self, id: &NodeId) -> Result<TreeID, CollabError> {
        self.id_to_tree
            .get(id)
            .copied()
            .ok_or_else(|| CollabError::UnknownNode { id: id.to_string() })
    }

    /// Reads back every tree node's `logical_id` meta field to (re)populate the local id caches.
    /// Called after construction and after every successful `load`/`import_update`, since remote
    /// peers may have created nodes this replica has never locally indexed before.
    fn rebuild_id_cache(&mut self) -> Result<(), CollabError> {
        for tree_id in self.tree.nodes() {
            if self.tree_to_id.contains_key(&tree_id) {
                continue;
            }
            let meta = self
                .tree
                .get_meta(tree_id)
                .map_err(|e| map_loro_error(&e, "<rebuild>"))?;
            let Some(raw) = meta.get(META_LOGICAL_ID) else {
                continue;
            };
            if let Some(logical) = read_string_value(&raw) {
                let id = NodeId::from(logical);
                self.id_to_tree.insert(id.clone(), tree_id);
                self.tree_to_id.insert(tree_id, id);
            }
        }
        Ok(())
    }

    fn text_handler(&self, tree_id: TreeID) -> Result<LoroText, CollabError> {
        let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, "<text>"))?;
        meta.ensure_mergeable_text(META_TEXT)
            .map_err(|e| map_loro_error(&e, "<text>"))
    }

    /// A caller-supplied index is rolled independently of the actual sibling count at apply time
    /// (deliberately: a client's intended insert position can be stale by the time the op applies
    /// against the server's canonical head), so clamping to the valid range here — rather than
    /// requiring the caller to somehow predict it — mirrors realistic adapter behavior.
    fn clamp_index(&self, parent: TreeParentId, requested: u32) -> usize {
        let children_count = self.tree.children_num(parent).unwrap_or(0);
        (requested as usize).min(children_count)
    }

    /// A deep, independent copy of this engine (Loro's `LoroDoc::fork`, not `Clone`: `Clone` on a
    /// `LoroDoc` is a *reference* clone that shares the same underlying document, which would let
    /// a caller that mutates the copy also mutate `self`).
    ///
    /// Exists so a caller can apply a candidate update to an isolated working copy without
    /// touching the original — the shape the v0.4 collab server's warm cache needs (`ADR-0010`:
    /// hydrate/isolated-apply happen outside any lock and outside the shared cache entry; only a
    /// *successful, committed* write is allowed to replace it).
    ///
    /// # Errors
    /// Propagates [`Self::rebuild_id_cache`]'s failure mode, which cannot happen for a `self` that
    /// was itself produced by `new_empty`/`load`/`apply_operation`/`import_update` (i.e. every
    /// engine this crate can hand a caller), but is surfaced rather than assumed away.
    pub fn fork(&self) -> Result<Self, CollabError> {
        let doc = self.doc.fork();
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut engine = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
        };
        engine.rebuild_id_cache()?;
        Ok(engine)
    }

    /// Creates a fresh, empty document (no blocks, no title). `replica_seed` deterministically
    /// seeds the engine's internal peer id.
    #[must_use]
    pub fn new_empty(replica_seed: u64) -> Self {
        let doc = LoroDoc::new();
        // PeerID 0 is reserved by loro's internals for some sentinel comparisons; keep every
        // replica's seed in the non-zero range while staying deterministic.
        let peer_id = replica_seed.wrapping_add(1).max(1);
        // A brand-new, uncommitted doc always accepts a peer id change.
        let _ = doc.set_peer_id(peer_id);
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
        }
    }

    /// The document-level title (`ADR-0002`: `meta.title` is canonical; `flow_object_projections`
    /// only mirrors it). Empty when no title has ever been set.
    ///
    /// # Errors
    /// Only if the underlying engine reports a read failure on an attached map, which does not
    /// happen for a well-formed local document.
    pub fn title(&self) -> Result<String, CollabError> {
        Ok(self
            .meta
            .get(META_TITLE)
            .and_then(|value| read_string_value(&value))
            .unwrap_or_default())
    }

    /// Creates an independent read-only historical view at an opaque version-vector frontier.
    ///
    /// Loro full snapshots retain operation history, so this remains valid after the server has
    /// advanced its canonical snapshot pointer. The returned engine contains only history up to
    /// `frontier`; callers cannot accidentally expose Loro peer ids because the input and output
    /// boundary remains [`Frontier`] plus the semantic accessors on this type.
    ///
    /// # Errors
    /// `DecodeFailed` when `frontier` is not an encoded version vector, or `OperationFailed` when
    /// it is not represented by this document's retained history.
    pub fn fork_at_frontier(&self, frontier: &Frontier) -> Result<Self, CollabError> {
        let vv = if frontier.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(frontier.as_bytes()).map_err(|error| CollabError::DecodeFailed {
                input: "frontier",
                reason: error.to_string(),
            })?
        };
        let frontiers = self.doc.vv_to_frontiers(&vv);
        let doc = self
            .doc
            .fork_at(&frontiers)
            .map_err(|error| CollabError::OperationFailed {
                reason: error.to_string(),
            })?;
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut fork = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
        };
        fork.rebuild_id_cache()?;
        Ok(fork)
    }

    /// Sets the document-level title and commits the change.
    ///
    /// # Errors
    /// Propagates the underlying engine's map-write failure, if any.
    pub fn set_title(&mut self, title: &str) -> Result<(), CollabError> {
        self.meta
            .insert(META_TITLE, title)
            .map_err(|e| map_loro_error(&e, "<meta.title>"))?;
        self.doc.commit();
        Ok(())
    }

    /// Applies one locally-originated operation from the shared vocabulary.
    ///
    /// # Errors
    /// See [`CollabError`]'s variants for the possible rejection reasons (unknown node, duplicate
    /// node, cyclic move, or an underlying engine failure).
    pub fn apply_operation(&mut self, operation: &Operation) -> Result<(), CollabError> {
        match operation {
            Operation::CreateNode {
                id,
                parent,
                index,
                kind,
            } => {
                if self.id_to_tree.contains_key(id) {
                    return Err(CollabError::DuplicateNode { id: id.to_string() });
                }
                let parent_id = self.resolve_parent(parent.as_ref())?;
                let safe_index = self.clamp_index(parent_id, *index);
                let tree_id = self
                    .tree
                    .create_at(parent_id, safe_index)
                    .map_err(|e| map_loro_error(&e, id))?;
                let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, id))?;
                meta.insert(META_LOGICAL_ID, id.as_ref())
                    .map_err(|e| map_loro_error(&e, id))?;
                meta.insert(META_KIND, kind_to_str(*kind))
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                self.id_to_tree.insert(id.clone(), tree_id);
                self.tree_to_id.insert(tree_id, id.clone());
                Ok(())
            }
            Operation::MoveNode { id, new_parent, index } => {
                let tree_id = self.require_tree_id(id)?;
                let parent_id = self.resolve_parent(new_parent.as_ref())?;
                let safe_index = self.clamp_index(parent_id, *index);
                self.tree
                    .mov_to(tree_id, parent_id, safe_index)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::DeleteNode { id } => {
                let tree_id = self.require_tree_id(id)?;
                self.tree.delete(tree_id).map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::InsertText { id, index, text } => {
                let tree_id = self.require_tree_id(id)?;
                let handler = self.text_handler(tree_id)?;
                handler
                    .insert_utf8(*index as usize, text)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::DeleteText { id, index, len } => {
                let tree_id = self.require_tree_id(id)?;
                let handler = self.text_handler(tree_id)?;
                handler
                    .delete_utf8(*index as usize, *len as usize)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::SetProperty { id, key, value } => {
                let tree_id = self.require_tree_id(id)?;
                let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, id))?;
                let full_key = format!("{META_PROPERTY_PREFIX}{key}");
                meta.insert(&full_key, value.as_str())
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
        }
    }

    /// Exports the current merged tree state as the engine-independent [`SemanticSnapshot`].
    /// Document-level fields (`title`) are not part of this shape; read them via [`Self::title`].
    ///
    /// # Errors
    /// Propagates the underlying engine's read failure, if any.
    pub fn semantic_snapshot(&self) -> Result<SemanticSnapshot, CollabError> {
        let mut snapshot = SemanticSnapshot::default();
        // Every node's `order_key` needs its position among its parent's children. The
        // straightforward per-node `siblings.iter().position(...)` (this method's shape before
        // this fix) is O(sibling count) *per node*, so a parent with many children makes building
        // the whole snapshot O(n^2) in that group's size -- this package's isolated-apply
        // delivery report has the measured before/after (a document approaching
        // `document_block_count_max`/`container_count_max` was taking longer than
        // `decode_apply_cpu_ms_max` to snapshot on its own, well before reaching the ceiling that
        // is supposed to bound it). Memoizing each distinct parent's children into a `TreeID ->
        // position` index the first time it is needed, and reusing that index for every other
        // node under the same parent, makes this O(n) overall instead: `self.tree.children` is
        // still called once per distinct parent (not once per node), and each lookup afterwards
        // is O(1).
        let mut position_cache: HashMap<TreeParentId, HashMap<TreeID, usize>> = HashMap::new();
        for tree_id in self.tree.nodes() {
            let Some(logical_id) = self.tree_to_id.get(&tree_id).cloned() else {
                // A node this replica has never resolved to a logical id (shouldn't happen once
                // `rebuild_id_cache` has run, but skip defensively rather than panic).
                continue;
            };
            let deleted = self
                .tree
                .is_node_deleted(&tree_id)
                .map_err(|e| map_loro_error(&e, &logical_id))?;
            let parent_tp = self.tree.parent(tree_id).unwrap_or(TreeParentId::Root);
            let parent_logical = match parent_tp {
                TreeParentId::Node(parent_tree_id) => self.tree_to_id.get(&parent_tree_id).cloned(),
                _ => None,
            };
            let positions = position_cache.entry(parent_tp).or_insert_with(|| {
                self.tree
                    .children(parent_tp)
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(index, id)| (id, index))
                    .collect()
            });
            let order_key = zero_padded_order_key(positions.get(&tree_id).copied().unwrap_or(0));

            let meta = self
                .tree
                .get_meta(tree_id)
                .map_err(|e| map_loro_error(&e, &logical_id))?;
            let kind = meta
                .get(META_KIND)
                .and_then(|v| read_string_value(&v))
                .map_or(NodeKind::Block, |s| str_to_kind(&s));
            let text = match meta.get(META_TEXT) {
                Some(ValueOrContainer::Container(Container::Text(text_container))) => text_container.to_string(),
                _ => String::new(),
            };
            let mut properties = BTreeMap::new();
            for key in meta.keys() {
                let key_str: &str = key.as_ref();
                if let Some(prop_name) = key_str.strip_prefix(META_PROPERTY_PREFIX)
                    && let Some(raw) = meta.get(key_str)
                    && let Some(value) = read_string_value(&raw)
                {
                    properties.insert(prop_name.to_string(), value);
                }
            }

            snapshot.nodes.insert(
                logical_id,
                SemanticNode {
                    parent: parent_logical,
                    order_key,
                    kind,
                    text,
                    properties,
                    deleted,
                },
            );
        }
        Ok(snapshot)
    }
}

impl CollabEngine for LoroCollabEngine {
    type Error = CollabError;

    fn load(snapshot: &[u8]) -> Result<Self, Self::Error> {
        InputLimits::default().validate_snapshot(snapshot)?;
        let doc = LoroDoc::new();
        doc.import(snapshot).map_err(|e| CollabError::DecodeFailed {
            input: "snapshot",
            reason: e.to_string(),
        })?;
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut engine = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
        };
        engine.rebuild_id_cache()?;
        Ok(engine)
    }

    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error> {
        InputLimits::default().validate_update(update)?;
        let before = self.doc.state_vv();
        self.doc.import(update).map_err(|e| CollabError::DecodeFailed {
            input: "update",
            reason: e.to_string(),
        })?;
        let after = self.doc.state_vv();
        self.rebuild_id_cache()?;
        Ok(Diff {
            changed: before != after,
        })
    }

    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error> {
        self.doc
            .export(loro::ExportMode::Snapshot)
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })
    }

    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error> {
        let vv = if frontier.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(frontier.as_bytes()).map_err(|e| CollabError::DecodeFailed {
                input: "frontier",
                reason: e.to_string(),
            })?
        };
        self.doc
            .export(loro::ExportMode::updates(&vv))
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })
    }

    fn frontier(&self) -> Frontier {
        Frontier::from_bytes(self.doc.state_vv().encode())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn zero_padded_order_key_matches_the_format_macro_it_replaced() {
        for position in [
            0usize,
            1,
            7,
            8,
            9,
            99,
            9_999,
            10_000,
            99_999_999,
            100_000_000,
            123_456_789,
        ] {
            assert_eq!(zero_padded_order_key(position), format!("{position:08}"));
        }
    }

    #[test]
    fn new_empty_has_no_title_and_empty_semantic_snapshot() {
        let engine = LoroCollabEngine::new_empty(1);
        assert_eq!(engine.title().expect("title reads"), "");
        assert!(engine.semantic_snapshot().expect("snapshot reads").nodes.is_empty());
    }

    #[test]
    fn set_title_round_trips_through_snapshot_export_and_load() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("Untitled Page").expect("set_title succeeds");
        assert_eq!(engine.title().expect("title reads"), "Untitled Page");

        let snapshot = engine.export_snapshot().expect("export succeeds");
        let reloaded = LoroCollabEngine::load(&snapshot).expect("load succeeds");
        assert_eq!(reloaded.title().expect("title reads"), "Untitled Page");
    }

    #[test]
    fn full_snapshot_can_fork_back_to_an_earlier_semantic_frontier() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("before").expect("initial title writes");
        let before = engine.frontier();
        engine.set_title("after").expect("second title writes");
        engine
            .apply_operation(&Operation::CreateNode {
                id: NodeId::from("later-node"),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("later node writes");

        let full = engine.export_snapshot().expect("full snapshot exports");
        let loaded = LoroCollabEngine::load(&full).expect("full snapshot loads");
        let historical = loaded
            .fork_at_frontier(&before)
            .expect("historical frontier is retained");

        assert_eq!(historical.title().expect("title reads"), "before");
        assert!(
            historical.semantic_snapshot().expect("snapshot reads").nodes.is_empty(),
            "the fork must not contain an operation that happened after its frontier"
        );
        assert_eq!(loaded.title().expect("source engine stays current"), "after");
    }

    #[test]
    fn frontier_is_stable_for_an_untouched_document_and_advances_after_commit() {
        let engine = LoroCollabEngine::new_empty(7);
        let empty_frontier = engine.frontier();

        let mut engine = engine;
        engine.set_title("x").expect("set_title succeeds");
        let after_frontier = engine.frontier();
        assert_ne!(empty_frontier, after_frontier);
    }

    #[test]
    fn apply_operation_create_and_semantic_snapshot_agree() {
        let mut engine = LoroCollabEngine::new_empty(1);
        let block = NodeId::from("blk-1");
        engine
            .apply_operation(&Operation::CreateNode {
                id: block.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("create succeeds");
        engine
            .apply_operation(&Operation::InsertText {
                id: block.clone(),
                index: 0,
                text: "hello".to_string(),
            })
            .expect("insert text succeeds");

        let snapshot = engine.semantic_snapshot().expect("snapshot reads");
        let node = snapshot.nodes.get(&block).expect("node exists");
        assert_eq!(node.text, "hello");
        assert_eq!(node.kind, NodeKind::Block);
        assert!(!node.deleted);
    }

    #[test]
    fn fork_is_independent_of_the_original() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("before fork").expect("set_title succeeds");

        let mut forked = engine.fork().expect("fork succeeds");
        assert_eq!(forked.title().expect("title reads"), "before fork");

        forked
            .set_title("mutated only on the fork")
            .expect("set_title succeeds");
        assert_eq!(forked.title().expect("title reads"), "mutated only on the fork");
        // The original must be untouched -- this is the whole point of `fork` over `Clone`.
        assert_eq!(engine.title().expect("title reads"), "before fork");
        assert_ne!(engine.frontier(), forked.frontier());
    }
}
