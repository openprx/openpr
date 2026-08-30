//! One-off diagnostic: dumps a wire-framed isolated-apply request (base snapshot + update) for a
//! flat 10,000-node document at the exact `container_count_max`/`document_block_count_max`
//! boundary, to a file, so the real `collab-isolated-apply-worker` binary can be profiled directly
//! (e.g. `perf record`) by piping that file to its stdin -- outside of `getrusage`/`clock_gettime`
//! sampling, for cross-validation.
//!
//! Run with:
//!   cargo run --release -p collab-core --example `dump_worker_request` -- /path/to/output.bin

#![allow(unsafe_code)]

use collab_core::isolation::wire;
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

fn main() -> Result<(), String> {
    let out_path = std::env::args()
        .nth(1)
        .ok_or_else(|| "usage: dump_worker_request <output-path>".to_string())?;

    let total = 10_000usize;
    let update_count = 1000usize;
    let base_count = total - update_count;

    let mut base_engine = LoroCollabEngine::new_empty(1);
    for i in 0..base_count {
        base_engine
            .apply_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n-{i}")),
                parent: None,
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .map_err(|e| format!("CreateNode: {e}"))?;
    }
    let base_snapshot = base_engine
        .export_snapshot()
        .map_err(|e| format!("export_snapshot(base): {e}"))?;
    let base_frontier = base_engine.frontier();

    let mut writer = base_engine.fork().map_err(|e| format!("fork: {e}"))?;
    for i in base_count..total {
        writer
            .apply_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n-{i}")),
                parent: None,
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .map_err(|e| format!("CreateNode: {e}"))?;
    }
    let update = writer
        .export_from(&base_frontier)
        .map_err(|e| format!("export_from: {e}"))?;

    let mut file = std::fs::File::create(&out_path).map_err(|e| format!("create {out_path}: {e}"))?;
    wire::write_request(&mut file, &base_snapshot, &update).map_err(|e| format!("write_request: {e}"))?;
    Ok(())
}
