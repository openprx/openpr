use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A logical, engine-agnostic node identifier chosen by the fixture generator. Never an engine
/// peer id, tree id, or client id — those stay inside each adapter.
pub type NodeId = Arc<str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A Page's block-tree node (paragraph/heading/list/code carry rich text).
    Block,
    /// A Collection's field definition, ordered as a child of the fields root.
    CollectionField,
    /// A Collection's saved view, ordered as a child of the views root.
    CollectionView,
    /// A Record's scalar/relation property, keyed by name in `properties`.
    RecordProperty,
    /// A Navigator tree node (workspace/project/page shortcut entries).
    NavigatorNode,
}

/// One entry in a deterministic, replayable operation log.
///
/// `Operation` is the single vocabulary both `collab-loro` and `collab-yrs-yjs` translate into
/// their own engine calls — the fixture generator and corpus runner never know which engine is
/// underneath.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    CreateNode {
        id: NodeId,
        parent: Option<NodeId>,
        index: u32,
        kind: NodeKind,
    },
    MoveNode {
        id: NodeId,
        new_parent: Option<NodeId>,
        index: u32,
    },
    DeleteNode {
        id: NodeId,
    },
    InsertText {
        id: NodeId,
        index: u32,
        text: String,
    },
    DeleteText {
        id: NodeId,
        index: u32,
        len: u32,
    },
    SetProperty {
        id: NodeId,
        key: String,
        value: String,
    },
}

impl Operation {
    #[must_use]
    pub const fn target(&self) -> &NodeId {
        match self {
            Self::CreateNode { id, .. }
            | Self::MoveNode { id, .. }
            | Self::DeleteNode { id }
            | Self::InsertText { id, .. }
            | Self::DeleteText { id, .. }
            | Self::SetProperty { id, .. } => id,
        }
    }
}
