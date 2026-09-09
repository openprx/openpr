//! Turns a [`collab_core`] document state into the JSON shapes `flow_object_projections` stores.
//!
//! `ADR-0002`: the projection is a rebuildable read replica of the CRDT document, never a second
//! writer.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use collab_core::{Frontier, SemanticSnapshot};
use serde_json::Value;

/// Base64 encoding for an opaque [`Frontier`] on the wire. `Bootstrap.snapshot_base64`
/// elsewhere in the contract sets the precedent that opaque CRDT bytes are base64 in JSON.
#[must_use]
pub fn encode_frontier(frontier: &Frontier) -> String {
    BASE64.encode(frontier.as_bytes())
}

/// The `flow_object_projections.state` JSON for a snapshot: the canonical `{"nodes": {...}}` shape
/// `SemanticSnapshot` already serializes to.
///
/// Empty (`{"nodes":{}}`) for every object this package creates, since no content command exists
/// yet to populate blocks.
///
/// # Errors
/// Only if `serde_json` itself cannot encode the snapshot, which cannot happen for the field
/// types `SemanticSnapshot` carries (see its own `canonical_json` doc comment).
pub fn state_json(snapshot: &SemanticSnapshot) -> Result<Value, serde_json::Error> {
    serde_json::to_value(snapshot)
}

/// Plain-text projection: every live (non-deleted) node's text, in a stable `(order_key, id)`
/// order, space-joined. Trivially empty for a just-created document.
#[must_use]
pub fn plain_text(snapshot: &SemanticSnapshot) -> String {
    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| node_a.order_key.cmp(&node_b.order_key).then(id_a.cmp(id_b)));
    nodes
        .into_iter()
        .map(|(_, node)| node.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Markdown rendering for `render=markdown`.
///
/// The title is followed by every live node's text in deterministic document order. The
/// engine-independent snapshot is the source here: rendering only the title would silently drop
/// content that is already present in the same accepted document.
#[must_use]
pub fn render_markdown(title: &str, snapshot: &SemanticSnapshot) -> String {
    let mut rendered = String::new();
    if !title.is_empty() {
        rendered.push_str("# ");
        rendered.push_str(title);
        rendered.push('\n');
    }

    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| {
        node_a
            .parent
            .cmp(&node_b.parent)
            .then(node_a.order_key.cmp(&node_b.order_key))
            .then(id_a.cmp(id_b))
    });
    for (_, node) in nodes {
        if node.text.is_empty() {
            continue;
        }
        if !rendered.is_empty() && !rendered.ends_with("\n\n") {
            rendered.push('\n');
        }
        rendered.push_str(&node.text);
        rendered.push('\n');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use collab_core::{NodeKind, SemanticNode};

    use super::*;

    #[test]
    fn markdown_contains_live_block_text_in_document_order() {
        let mut snapshot = SemanticSnapshot::default();
        for (id, order_key, text, deleted) in [
            ("later", "0000000001", "second", false),
            ("earlier", "0000000000", "first", false),
            ("deleted", "0000000002", "must not render", true),
        ] {
            snapshot.nodes.insert(
                Arc::<str>::from(id),
                SemanticNode {
                    parent: None,
                    order_key: order_key.to_string(),
                    kind: NodeKind::Block,
                    text: text.to_string(),
                    properties: BTreeMap::new(),
                    deleted,
                },
            );
        }

        assert_eq!(render_markdown("Title", &snapshot), "# Title\n\nfirst\n\nsecond\n");
    }
}
