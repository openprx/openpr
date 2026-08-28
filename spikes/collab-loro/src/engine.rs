//! Real `loro` engine adapter implementing the shared [`collab_shared::CollabEngine`] /
//! [`collab_shared::CorpusEngine`] contracts.
//!
//! Every business-facing id in this module is a [`NodeId`] chosen by the fixture generator.
//! Loro's own [`TreeID`] (which embeds a [`loro::PeerID`]) never crosses out of this file: it is
//! looked up from a purely local, non-synced cache (`id_to_tree` / `tree_to_id`), rebuilt after
//! every `load`/`import_update` by reading back a `logical_id` field this adapter itself writes
//! into each tree node's meta map. That `logical_id` field is business data flowing *into* the
//! engine (so this replica can find "the node the fixture calls `blk-abc123`" again after a
//! remote peer creates it) — it is not the engine exposing its own identity outward.

use std::collections::BTreeMap;
use std::collections::HashMap;

use collab_shared::{
    CollabEngine, CollabError, CorpusEngine, Diff, Frontier, InputLimits, NodeId, NodeKind, Operation, SemanticNode,
    SemanticSnapshot,
};
use loro::{Container, LoroDoc, LoroText, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer, VersionVector};

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

const TREE_CONTAINER: &str = "tree";
const META_LOGICAL_ID: &str = "logical_id";
const META_KIND: &str = "kind";
const META_TEXT: &str = "text";
const META_PROPERTY_PREFIX: &str = "prop:";

pub struct LoroCollabEngine {
    doc: LoroDoc,
    tree: LoroTree,
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

fn read_string_value(value_or_container: &ValueOrContainer) -> Option<String> {
    match value_or_container {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.as_ref().to_string()),
        _ => None,
    }
}

impl LoroCollabEngine {
    fn attach_tree(doc: &LoroDoc) -> LoroTree {
        let tree = doc.get_tree(TREE_CONTAINER);
        // jitter=0 keeps sibling ordering fully deterministic for a given operation log, which
        // matters for reproducible fixture output; Loro still resolves *concurrent* insert/move
        // collisions correctly (this is exactly what the moveable-tree CRDT is for).
        tree.enable_fractional_index(0);
        tree
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

    /// Fixture-generated indices are rolled independently of the actual sibling count at apply
    /// time (deliberately: it keeps the fixture generator simple and deterministic). Real editors
    /// have the same problem — a client's intended insert position can be stale by the time the
    /// op applies — so clamping to the valid range here, rather than requiring the caller to
    /// somehow predict it, mirrors realistic adapter behavior instead of being a fixture-only
    /// workaround.
    fn clamp_index(&self, parent: TreeParentId, requested: u32) -> usize {
        let children_count = self.tree.children_num(parent).unwrap_or(0);
        (requested as usize).min(children_count)
    }

    fn order_key_for(&self, tree_id: TreeID, parent: TreeParentId) -> String {
        let siblings = self.tree.children(parent).unwrap_or_default();
        let position = siblings.iter().position(|candidate| *candidate == tree_id).unwrap_or(0);
        format!("{position:08}")
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
        let mut engine = Self {
            doc,
            tree,
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

impl CorpusEngine for LoroCollabEngine {
    fn new_empty(replica_seed: u64) -> Self {
        let doc = LoroDoc::new();
        // PeerID 0 is reserved by loro's internals for some sentinel comparisons; keep every
        // replica's seed in the non-zero range while staying deterministic.
        let peer_id = replica_seed.wrapping_add(1).max(1);
        // A brand-new, uncommitted doc always accepts a peer id change.
        let _ = doc.set_peer_id(peer_id);
        let tree = Self::attach_tree(&doc);
        Self {
            doc,
            tree,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
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

    fn semantic_snapshot(&self) -> Result<SemanticSnapshot, Self::Error> {
        let mut snapshot = SemanticSnapshot::default();
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
            let order_key = self.order_key_for(tree_id, parent_tp);

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
