use std::io::{self, Read};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use collab_core::{CollabEngine, LoroCollabEngine};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
struct ReplayInput {
    snapshot_base64: String,
    tail_updates_base64: Vec<String>,
    expected_head_frontier_base64: String,
    projection: ProjectionInput,
}

#[derive(Deserialize)]
struct ProjectionInput {
    title: String,
    state: Value,
    plain_text: String,
}

#[derive(Serialize)]
struct ReplayOutput {
    frontier_base64: String,
    expected_head_frontier_base64: String,
    frontier_matches_head_byte_for_byte: bool,
    semantic_hash: String,
    title: String,
    state: Value,
    plain_text: String,
    projection_title_matches: bool,
    projection_state_matches: bool,
    projection_plain_text_matches: bool,
    applied_tail_updates: usize,
}

fn plain_text(snapshot: &collab_core::SemanticSnapshot) -> String {
    let mut nodes: Vec<_> = snapshot.nodes.iter().filter(|(_, node)| !node.deleted).collect();
    nodes.sort_by(|(id_a, node_a), (id_b, node_b)| {
        node_a.order_key.cmp(&node_b.order_key).then(id_a.cmp(id_b))
    });
    nodes
        .into_iter()
        .map(|(_, node)| node.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn run() -> Result<ReplayOutput, String> {
    let mut raw = String::new();
    io::stdin()
        .read_to_string(&mut raw)
        .map_err(|error| format!("read replay input: {error}"))?;
    let input: ReplayInput =
        serde_json::from_str(&raw).map_err(|error| format!("parse replay input: {error}"))?;

    let snapshot = BASE64
        .decode(input.snapshot_base64.as_bytes())
        .map_err(|error| format!("decode snapshot_base64: {error}"))?;
    let expected_frontier = BASE64
        .decode(input.expected_head_frontier_base64.as_bytes())
        .map_err(|error| format!("decode expected_head_frontier_base64: {error}"))?;
    let mut engine = LoroCollabEngine::load(&snapshot)
        .map_err(|error| format!("load canonical snapshot: {error}"))?;

    for (index, encoded) in input.tail_updates_base64.iter().enumerate() {
        let update = BASE64
            .decode(encoded.as_bytes())
            .map_err(|error| format!("decode tail update {index}: {error}"))?;
        engine
            .import_update(&update)
            .map_err(|error| format!("apply tail update {index}: {error}"))?;
    }

    let actual_frontier = engine.frontier().as_bytes().to_vec();
    let semantic = engine
        .semantic_snapshot()
        .map_err(|error| format!("derive semantic snapshot: {error}"))?;
    let state = serde_json::to_value(&semantic)
        .map_err(|error| format!("serialize semantic projection: {error}"))?;
    let title = engine
        .title()
        .map_err(|error| format!("read canonical title: {error}"))?;
    let text = plain_text(&semantic);

    Ok(ReplayOutput {
        frontier_base64: BASE64.encode(&actual_frontier),
        expected_head_frontier_base64: input.expected_head_frontier_base64,
        frontier_matches_head_byte_for_byte: actual_frontier == expected_frontier,
        semantic_hash: semantic.semantic_hash(),
        projection_title_matches: title == input.projection.title,
        projection_state_matches: state == input.projection.state,
        projection_plain_text_matches: text == input.projection.plain_text,
        title,
        state,
        plain_text: text,
        applied_tail_updates: input.tail_updates_base64.len(),
    })
}

fn main() {
    match run() {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("serialize replay output: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
