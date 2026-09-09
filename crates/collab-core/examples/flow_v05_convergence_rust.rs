//! Rust half of the Flow v0.5 ten-client Rust/WASM convergence corpus.

#![allow(clippy::print_stderr)] // CLI instrument: diagnostics must stay off the JSON stdout stream.

use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use collab_core::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
use serde_json::json;

fn usage() -> ! {
    eprintln!("usage: flow_v05_convergence_rust generate CLIENTS OUT_DIR | verify SNAPSHOT EXPECTED_HASH OUT_JSON");
    std::process::exit(2);
}

fn replay_order(clients: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..clients).filter(|index| index % 2 == 1).rev().collect();
    order.extend((0..clients).filter(|index| index % 2 == 0));
    order
}

fn replay(base: &[u8], updates: &[Vec<u8>], order: &[usize]) -> Result<LoroCollabEngine, String> {
    let mut merged = LoroCollabEngine::load(base).map_err(|err| err.to_string())?;
    for index in order {
        let update = updates
            .get(*index)
            .ok_or_else(|| format!("replay order references missing client {index}"))?;
        merged.import_update(update).map_err(|err| err.to_string())?;
        // Accepted egress may repeat a persisted update. Importing it twice must not change the
        // converged semantic state, and makes this corpus cross the duplicate-injection branch.
        merged.import_update(update).map_err(|err| err.to_string())?;
    }
    Ok(merged)
}

fn generate(clients: usize, out_dir: &Path) -> Result<(), String> {
    if clients == 0 {
        return Err("clients must be greater than zero".to_string());
    }
    fs::create_dir_all(out_dir).map_err(|err| err.to_string())?;

    let base_engine = LoroCollabEngine::new_empty(424_242);
    let base_frontier = base_engine.frontier();
    let base = base_engine.export_snapshot().map_err(|err| err.to_string())?;
    fs::write(out_dir.join("base.bin"), &base).map_err(|err| err.to_string())?;

    let mut updates = Vec::with_capacity(clients);
    for index in 0..clients {
        let mut client = LoroCollabEngine::load(&base).map_err(|err| err.to_string())?;
        let id: Arc<str> = Arc::from(format!("client-{index:02}"));
        client
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: u32::try_from(index).map_err(|err| err.to_string())?,
                kind: NodeKind::Block,
            })
            .map_err(|err| err.to_string())?;
        client
            .apply_operation(&Operation::InsertText {
                id: id.clone(),
                index: 0,
                text: format!("client {index}: 离线編集 ✨ e\u{301}"),
            })
            .map_err(|err| err.to_string())?;
        client
            .apply_operation(&Operation::SetProperty {
                id,
                key: "author".to_string(),
                value: format!("client-{index:02}"),
            })
            .map_err(|err| err.to_string())?;
        let update = client.export_from(&base_frontier).map_err(|err| err.to_string())?;
        if update.is_empty() {
            return Err(format!("client {index} produced an empty update"));
        }
        fs::write(out_dir.join(format!("update-{index:02}.bin")), &update).map_err(|err| err.to_string())?;
        updates.push(update);
    }

    let order = replay_order(clients);
    let merged = replay(&base, &updates, &order)?;
    let semantic = merged.semantic_snapshot().map_err(|err| err.to_string())?;
    let semantic_hash = semantic.semantic_hash();
    let merged_snapshot = merged.export_snapshot().map_err(|err| err.to_string())?;
    fs::write(out_dir.join("rust-merged.bin"), &merged_snapshot).map_err(|err| err.to_string())?;
    fs::write(
        out_dir.join("rust-semantic.json"),
        semantic.canonical_json().map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;

    let reverse: Vec<usize> = (0..clients).rev().collect();
    let reverse_hash = replay(&base, &updates, &reverse)?
        .semantic_snapshot()
        .map_err(|err| err.to_string())?
        .semantic_hash();
    let mutation_order = order
        .get(..order.len().saturating_sub(1))
        .ok_or_else(|| "could not construct the dropped-client negative control".to_string())?;
    let mutation_hash = replay(&base, &updates, mutation_order)?
        .semantic_snapshot()
        .map_err(|err| err.to_string())?
        .semantic_hash();

    let report = json!({
        "engine": "collab-core/LoroCollabEngine",
        "clients": clients,
        "base_snapshot_bytes": base.len(),
        "updates": updates.iter().map(Vec::len).collect::<Vec<_>>(),
        "replay_order": order,
        "duplicate_imports_per_update": 1,
        "semantic_node_count": semantic.nodes.len(),
        "semantic_hash": semantic_hash,
        "reverse_replay_hash": reverse_hash,
        "reverse_replay_equal": reverse_hash == semantic_hash,
        "mutation": {
            "kind": "drop_last_replayed_client_update",
            "observed_hash": mutation_hash,
            "detected": mutation_hash != semantic_hash,
        },
    });
    fs::write(
        out_dir.join("rust-result.json"),
        serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn verify(snapshot: &Path, expected_hash: &str, out_json: &Path) -> Result<(), String> {
    let bytes = fs::read(snapshot).map_err(|err| err.to_string())?;
    let engine = LoroCollabEngine::load(&bytes).map_err(|err| err.to_string())?;
    let semantic = engine.semantic_snapshot().map_err(|err| err.to_string())?;
    let observed_hash = semantic.semantic_hash();
    let matched = observed_hash == expected_hash;
    let report = json!({
        "snapshot": snapshot,
        "snapshot_bytes": bytes.len(),
        "semantic_node_count": semantic.nodes.len(),
        "expected_hash": expected_hash,
        "observed_hash": observed_hash,
        "matched": matched,
    });
    fs::write(
        out_json,
        serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    if matched {
        Ok(())
    } else {
        Err("Rust replay of the WASM snapshot produced a different semantic hash".to_string())
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let result = match args.as_slice() {
        [_, command, clients, out_dir] if command == "generate" => {
            let clients = clients.parse::<usize>().unwrap_or_else(|_| usage());
            generate(clients, Path::new(out_dir))
        }
        [_, command, snapshot, expected_hash, out_json] if command == "verify" => {
            verify(Path::new(snapshot), expected_hash, Path::new(out_json))
        }
        _ => usage(),
    };
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
