//! Companion to `decode_apply_budget.rs`: measures the *actual* `collab-isolated-apply-worker`
//! subprocess end to end, via the real, production `isolation::host::isolated_apply` entry point
//! -- not a library-level simulation. This exists to answer one specific question the
//! library-level example cannot: does routing the same decode/apply/snapshot work through the
//! real worker process (fresh `fork`+`exec`, the `CountingAllocator` armed as the process's actual
//! `#[global_allocator]`, real `SIGPROF`/`ITIMER_PROF` arming) cost meaningfully more CPU time than
//! the bare library calls measured in-process, e.g. from the counting allocator's per-allocation
//! atomic bookkeeping (`isolation::alloc::CountingAllocator`) once armed.
//!
//! Run with:
//!   cargo run --release -p collab-core --example `decode_apply_budget_subprocess`
//!
//! # Why this measures the child's CPU time, not wall clock
//!
//! `getrusage(RUSAGE_CHILDREN)` is a kernel-maintained accumulator of exactly the same quantity
//! (`ru_utime` + `ru_stime`, user + system CPU time) that `ITIMER_PROF` decrements against for a
//! process's own execution -- but read from the parent, accumulated over every child that has
//! exited so far. This binary spawns exactly one child per measurement (via `isolated_apply`), so
//! diffing `RUSAGE_CHILDREN` immediately before and after one call isolates that child's total CPU
//! time cleanly. This total includes some work outside the `SIGPROF`-metered window (reading the
//! request off stdin, `LoroCollabEngine::load`), so it is an *upper bound* on the metered cost, not
//! an exact match to it -- `decode_apply_budget.rs`'s in-process phase breakdown is what isolates
//! the individual phases; this binary's job is only to sanity-check that the real subprocess's
//! total isn't dramatically larger than what that in-process breakdown would predict.

#![allow(unsafe_code)]

use std::io::Write;
use std::time::Duration;

use collab_core::isolation::isolated_apply;
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

fn rusage_children_cpu() -> Result<Duration, String> {
    // SAFETY: `usage` is a valid, zero-initialized `libc::rusage` out-parameter;
    // `RUSAGE_CHILDREN` is a valid `who` value on Linux.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `&raw mut usage` is a valid out-pointer matching the zero-initialized value above.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &raw mut usage) };
    if rc != 0 {
        return Err("getrusage(RUSAGE_CHILDREN) failed".to_string());
    }
    let user = timeval_to_duration(usage.ru_utime)?;
    let sys = timeval_to_duration(usage.ru_stime)?;
    Ok(user + sys)
}

fn timeval_to_duration(tv: libc::timeval) -> Result<Duration, String> {
    let secs = u64::try_from(tv.tv_sec).map_err(|_| "negative tv_sec from getrusage".to_string())?;
    let micros = u32::try_from(tv.tv_usec).map_err(|_| "invalid tv_usec from getrusage".to_string())?;
    Ok(Duration::new(secs, micros.saturating_mul(1000)))
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.3}ms", d.as_secs_f64() * 1000.0)
}

/// Builds up to `target_max` nodes through `CHUNK`-sized real `isolated_apply` round trips --
/// exactly the shape `apps/api`'s `submit_create_nodes_in_chunks` uses in production (each
/// accepted chunk's *exported* snapshot becomes the next chunk's base, round-tripping through the
/// real worker subprocess every time, not one continuous in-process engine). This is the pattern
/// `decode_apply_budget.rs`'s single-engine construction does *not* reproduce, and matters if
/// Loro's `Snapshot` export does not fully compact op-log history across repeated encode/decode
/// cycles the way one continuous in-process `LoroDoc` does.
///
/// Prints a row (real subprocess `RUSAGE_CHILDREN` CPU time for that one chunk) every time the
/// running total lands on a value in `report_at`.
fn run_chunked_through_real_worker(
    out: &mut impl Write,
    target_max: usize,
    chunk: usize,
    kind: NodeKind,
    report_at: &[usize],
) -> Result<(), String> {
    let mut current_snapshot = LoroCollabEngine::new_empty(1)
        .export_snapshot()
        .map_err(|e| format!("export_snapshot(empty): {e}"))?;
    let mut count = 0usize;

    while count < target_max {
        let this_chunk = chunk.min(target_max - count);
        let engine = LoroCollabEngine::load(&current_snapshot).map_err(|e| format!("load: {e}"))?;
        let frontier = engine.frontier();
        let mut writer = engine.fork().map_err(|e| format!("fork: {e}"))?;
        for i in count..(count + this_chunk) {
            writer
                .apply_operation(&Operation::CreateNode {
                    id: NodeId::from(format!("n-{i}")),
                    parent: None,
                    index: 0,
                    kind,
                })
                .map_err(|e| format!("CreateNode: {e}"))?;
        }
        let update = writer.export_from(&frontier).map_err(|e| format!("export_from: {e}"))?;

        let before = rusage_children_cpu()?;
        let result = isolated_apply(&current_snapshot, &update);
        let after = rusage_children_cpu()?;
        let cpu = after.saturating_sub(before);

        count += this_chunk;
        match result {
            Ok(success) => {
                current_snapshot = success.snapshot;
            }
            Err(err) => {
                let _ = writeln!(out, "n={count:<6} isolated_apply FAILED at chunk boundary: {err:?}");
                return Ok(());
            }
        }
        if report_at.contains(&count) {
            let _ = writeln!(out, "n={count:<6} child_cpu_total(user+sys)={:>10}", fmt_ms(cpu));
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let _ = writeln!(
        out,
        "=== real collab-isolated-apply-worker subprocess, chunked round trips (matches apps/api's submit_create_nodes_in_chunks) ==="
    );
    let _ = writeln!(
        out,
        "Each row is ONE chunk's real subprocess RUSAGE_CHILDREN CPU time -- an upper bound on the SIGPROF-metered cost (also includes stdin read + unmetered load)."
    );
    let _ = writeln!(
        out,
        "-- container_count_max boundary (NavigatorNode, capped at 10,000) --"
    );
    run_chunked_through_real_worker(
        &mut out,
        10_000,
        1000,
        NodeKind::NavigatorNode,
        &[1000, 3000, 5000, 8000, 10_000],
    )?;

    let _ = writeln!(
        out,
        "-- past the structural ceiling (RecordProperty, uncapped by container_count/document_block_count, to find where SIGPROF actually fires) --"
    );
    run_chunked_through_real_worker(
        &mut out,
        30_000,
        1000,
        NodeKind::RecordProperty,
        &(1..=30).map(|k| k * 1000).collect::<Vec<_>>(),
    )?;

    Ok(())
}
