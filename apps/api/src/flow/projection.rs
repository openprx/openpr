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

/// Minimal Markdown rendering for `render=markdown`.
///
/// This package ships no content commands, so every document is title-only; a heading is the only
/// faithful rendering of that state. Block rendering is deferred to the command-endpoint package
/// that actually populates blocks.
#[must_use]
pub fn render_markdown(title: &str) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!("# {title}\n")
    }
}
