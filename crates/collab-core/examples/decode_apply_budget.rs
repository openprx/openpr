//! Empirical, release-mode measurement of the `ADR-0014` / `contracts/limits-v1.md`
//! `decode_apply_cpu_ms_max` (50ms) budget, at document sizes approaching the v0.4 structural
//! ceilings (`container_count_max` / `document_block_count_max` = 10,000 nodes,
//! `document_text_chars_max` = 1,000,000 chars).
//!
//! Run with:
//!   cargo run --release -p collab-core --example `decode_apply_budget`
//!
//! # Why CPU time, not wall clock
//!
//! `ADR-0014`'s actual online enforcement (`crates/collab-core/src/isolation/child_runtime.rs`,
//! `arm_sigprof`) arms `ITIMER_PROF`, which POSIX defines as decrementing "both when the process
//! executes and when the system is executing on behalf of the process" -- i.e. user + system CPU
//! time, explicitly *not* wall clock (a process blocked on I/O or descheduled by the kernel does
//! not burn its `ITIMER_PROF` budget). To measure the same quantity the production ceiling
//! actually enforces, this binary reads `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)` around each
//! phase instead of `Instant::now()`. `CLOCK_PROCESS_CPUTIME_ID` is the POSIX clock defined to
//! track that same "CPU time consumed by this process" quantity (summed across all its threads);
//! unlike `ITIMER_PROF` it is a plain readable clock rather than a signal-firing timer, so it can
//! be sampled at arbitrary points without arming/disarming a real process-terminating signal
//! disposition inside a long-lived measurement process.
//!
//! This binary is single-threaded and performs no blocking I/O inside a measured span, so CPU
//! time and wall time are expected to coincide closely here -- but the metric read is still CPU
//! time specifically, both to match what production actually gates on and to stay correct if that
//! assumption ever stops holding.
//!
//! # Phases measured
//!
//! Mirrors `src/bin/isolated_apply_worker.rs::decode_apply_and_validate`'s actual sequence:
//! `import_update` (decode + apply) -> `semantic_snapshot` -> `check_snapshot` -> (today, also)
//! `export_snapshot`. `LoroCollabEngine::load` of the base document is measured separately and
//! reported as unmetered context only, matching production (`main`'s `load` call happens before
//! `run_metered`/`arm_sigprof`).

#![allow(unsafe_code)]

use std::io::Write as _;
use std::time::Duration;

use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

/// Matches `apps/api`'s `TEXT_CHUNK_CHARS`: the largest single-`InsertText` chunk that still fits
/// under `update_bytes_max` (65,536 bytes) for this binary's ASCII (1 byte/char) filler text.
const FINAL_CHUNK_CHARS: usize = 50_000;

/// Reads `CLOCK_PROCESS_CPUTIME_ID` (see module doc for why this, not `Instant`).
///
/// # Errors
/// Only if the kernel does not support this clock id, which does not happen on any Linux this
/// workspace targets.
fn cpu_time_now() -> Result<Duration, String> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`;
    // `CLOCK_PROCESS_CPUTIME_ID` is a clock id always supported by the Linux kernels this
    // workspace targets. No precondition beyond the pointer's validity.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return Err("clock_gettime(CLOCK_PROCESS_CPUTIME_ID) failed".to_string());
    }
    let secs = u64::try_from(ts.tv_sec).map_err(|_| "negative tv_sec from clock_gettime".to_string())?;
    let nanos = u32::try_from(ts.tv_nsec).map_err(|_| "invalid tv_nsec from clock_gettime".to_string())?;
    Ok(Duration::new(secs, nanos))
}

/// Times `f` by CPU time, returning `(result, cpu_elapsed)`.
fn timed<T>(f: impl FnOnce() -> T) -> Result<(T, Duration), String> {
    let start = cpu_time_now()?;
    let value = f();
    let end = cpu_time_now()?;
    Ok((value, end.saturating_sub(start)))
}

/// Builds a flat document (every node a direct child of root, `index: 0` on every insert -- the
/// exact shape `apps/api`'s `ws_structural_limit_container_count`/`document_block_count` fixtures
/// use via `submit_create_nodes_in_chunks`) with `node_count` nodes of `kind`, each carrying
/// `chars_per_node` characters of text.
fn build_flat_document(node_count: usize, kind: NodeKind, chars_per_node: usize, id_prefix: &str) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(1);
    let filler = "x".repeat(chars_per_node);
    for i in 0..node_count {
        let id = NodeId::from(format!("{id_prefix}-{i}"));
        // Boundary fixtures never fail to apply locally within the structural ceilings this
        // binary stays under; a failure here is a bug in this measurement harness itself, worth
        // surfacing loudly rather than silently producing a smaller-than-intended document.
        if let Err(err) = engine.apply_operation(&Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: 0,
            kind,
        }) {
            eprint_fatal(&format!("build_flat_document: CreateNode failed: {err}"));
        }
        if chars_per_node > 0
            && let Err(err) = engine.apply_operation(&Operation::InsertText {
                id,
                index: 0,
                text: filler.clone(),
            })
        {
            eprint_fatal(&format!("build_flat_document: InsertText failed: {err}"));
        }
    }
    engine
}

fn eprint_fatal(message: &str) -> ! {
    use std::io::Write as _;
    let stderr = std::io::stderr();
    let mut lock = stderr.lock();
    let _ = writeln!(lock, "FATAL: {message}");
    std::process::exit(1);
}

struct PhaseTimings {
    node_count: usize,
    load_base: Duration,
    import_update: Duration,
    semantic_snapshot: Duration,
    check_snapshot: Duration,
    export_snapshot: Duration,
}

impl PhaseTimings {
    fn metered_total(&self) -> Duration {
        self.import_update + self.semantic_snapshot + self.check_snapshot
    }

    fn metered_total_including_export(&self) -> Duration {
        self.metered_total() + self.export_snapshot
    }
}

/// Runs one measurement: a base document at `node_count - update_count` nodes, then a single
/// `import_update` that appends `update_count` more flat nodes -- the exact shape of the *last*
/// chunk in `apps/api`'s `submit_create_nodes_in_chunks` when it lands exactly on
/// `container_count_max`/`document_block_count_max`.
fn measure_node_count_scenario(node_count: usize, update_count: usize, kind: NodeKind) -> Result<PhaseTimings, String> {
    let base_count = node_count - update_count;
    let base_engine = build_flat_document(base_count, kind, 0, "n");
    let base_snapshot = base_engine
        .export_snapshot()
        .map_err(|e| format!("export_snapshot(base): {e}"))?;
    let base_frontier = base_engine.frontier();

    let mut writer = base_engine.fork().map_err(|e| format!("fork: {e}"))?;
    for i in base_count..node_count {
        let id = NodeId::from(format!("n-{i}"));
        writer
            .apply_operation(&Operation::CreateNode {
                id,
                parent: None,
                index: 0,
                kind,
            })
            .map_err(|e| format!("CreateNode: {e}"))?;
    }
    let update = writer
        .export_from(&base_frontier)
        .map_err(|e| format!("export_from: {e}"))?;

    run_metered_phases(node_count, &base_snapshot, &update)
}

/// `document_text_chars`/`text_block_chars` boundary scenario: `full_blocks` blocks already at
/// exactly `text_block_chars_max`, plus one more block already holding
/// `text_block_chars_max - final_chunk_chars` characters (so it is `final_chunk_chars` away from
/// its own per-block ceiling). The timed `import_update` inserts exactly `final_chunk_chars` more
/// characters into that last block, landing it (and, when `full_blocks == 9`, the whole document)
/// exactly on the relevant ceiling in one call -- matching `apps/api`'s
/// `submit_block_text_in_chunks`, which grows a block in `TEXT_CHUNK_CHARS` (50,000-char) steps
/// for the same reason `import_update`'s own `update_bytes_max` (65,536 bytes) gate would reject
/// a single 100,000-char update outright.
///
/// `final_chunk_chars` must stay under `update_bytes_max` in raw bytes; the ASCII filler used
/// here is 1 byte/char, so any value up to `text_block_chars_max` while also `<= 65_536` is safe.
fn measure_text_scenario(
    full_blocks: usize,
    text_block_chars_max: usize,
    final_chunk_chars: usize,
) -> Result<PhaseTimings, String> {
    let mut base_engine = build_flat_document(full_blocks, NodeKind::Block, text_block_chars_max, "blk");
    let last_block_existing = text_block_chars_max - final_chunk_chars;
    let last_block_id = NodeId::from("blk-last");
    base_engine
        .apply_operation(&Operation::CreateNode {
            id: last_block_id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .map_err(|e| format!("CreateNode(last): {e}"))?;
    base_engine
        .apply_operation(&Operation::InsertText {
            id: last_block_id.clone(),
            index: 0,
            text: "x".repeat(last_block_existing),
        })
        .map_err(|e| format!("InsertText(last, existing): {e}"))?;

    let base_snapshot = base_engine
        .export_snapshot()
        .map_err(|e| format!("export_snapshot(base): {e}"))?;
    let base_frontier = base_engine.frontier();
    let total_chars = full_blocks * text_block_chars_max + text_block_chars_max;

    let last_block_existing_index =
        u32::try_from(last_block_existing).map_err(|_| "last_block_existing overflowed u32".to_string())?;
    let mut writer = base_engine.fork().map_err(|e| format!("fork: {e}"))?;
    writer
        .apply_operation(&Operation::InsertText {
            id: last_block_id,
            index: last_block_existing_index,
            text: "y".repeat(final_chunk_chars),
        })
        .map_err(|e| format!("InsertText(last, final chunk): {e}"))?;
    let update = writer
        .export_from(&base_frontier)
        .map_err(|e| format!("export_from: {e}"))?;

    run_metered_phases(total_chars, &base_snapshot, &update)
}

/// The actual timed pipeline, mirroring `isolated_apply_worker.rs::decode_apply_and_validate`
/// exactly (including today's inclusion of `export_snapshot` inside what would be the metered
/// window in production -- see this binary's `main` for the finding that follows from measuring
/// it explicitly here).
fn run_metered_phases(label_count: usize, base_snapshot: &[u8], update: &[u8]) -> Result<PhaseTimings, String> {
    let (load_result, load_base) = timed(|| LoroCollabEngine::load(base_snapshot))?;
    let mut engine = load_result.map_err(|e| format!("load: {e}"))?;

    let ((), import_update) = timed(|| {
        // Real errors here are a harness bug (constructed updates always stay within the
        // structural ceilings this binary is measuring), so they are only ever unreachable here in
        // practice; still routed through `Result` end-to-end rather than assumed away.
        if let Err(err) = engine.import_update(update) {
            eprint_fatal(&format!("import_update failed at n={label_count}: {err}"));
        }
    })?;

    let (semantic, semantic_snapshot) = timed(|| match engine.semantic_snapshot() {
        Ok(s) => s,
        Err(err) => eprint_fatal(&format!("semantic_snapshot failed at n={label_count}: {err}")),
    })?;

    let limits = DocumentLimits::default();
    let (check_result, check_snapshot_time) = timed(|| check_snapshot(&semantic, &limits))?;
    if let Err(violation) = check_result {
        eprint_fatal(&format!(
            "check_snapshot unexpectedly rejected at n={label_count}: {violation:?}"
        ));
    }

    let (_exported, export_snapshot_time) = timed(|| match engine.export_snapshot() {
        Ok(bytes) => bytes,
        Err(err) => eprint_fatal(&format!("export_snapshot failed at n={label_count}: {err}")),
    })?;

    Ok(PhaseTimings {
        node_count: label_count,
        load_base,
        import_update,
        semantic_snapshot,
        check_snapshot: check_snapshot_time,
        export_snapshot: export_snapshot_time,
    })
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.3}ms", d.as_secs_f64() * 1000.0)
}

fn print_row(w: &mut impl std::io::Write, t: &PhaseTimings) -> std::io::Result<()> {
    writeln!(
        w,
        "n={:<6} load(unmetered)={:>10} import_update={:>10} semantic_snapshot={:>10} check_snapshot={:>10} export_snapshot={:>10} | metered(decode+snapshot+check)={:>10} metered+export={:>10}",
        t.node_count,
        fmt_ms(t.load_base),
        fmt_ms(t.import_update),
        fmt_ms(t.semantic_snapshot),
        fmt_ms(t.check_snapshot),
        fmt_ms(t.export_snapshot),
        fmt_ms(t.metered_total()),
        fmt_ms(t.metered_total_including_export()),
    )
}

fn main() -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let _ = writeln!(
        out,
        "=== container_count / document_block_count scenario (flat, no text) ==="
    );
    let _ = writeln!(
        out,
        "Base document holds (n - 1000) nodes; the timed `import_update` is the final 1000-node chunk landing exactly on n."
    );
    for &n in &[1000usize, 3000, 5000, 8000, 10_000] {
        let update_count = 1000.min(n);
        let timings = measure_node_count_scenario(n, update_count, NodeKind::NavigatorNode)?;
        print_row(&mut out, &timings).map_err(|e| e.to_string())?;
    }

    let limits = DocumentLimits::default();

    let _ = writeln!(out);
    let _ = writeln!(out, "=== text_block_chars scenario (single block, exact boundary) ===");
    let _ = writeln!(
        out,
        "One block already {FINAL_CHUNK_CHARS} chars below text_block_chars_max; the timed `import_update` is the final {FINAL_CHUNK_CHARS}-char chunk landing exactly on text_block_chars_max."
    );
    let timings = measure_text_scenario(0, limits.text_block_chars_max, FINAL_CHUNK_CHARS)?;
    print_row(&mut out, &timings).map_err(|e| e.to_string())?;

    let _ = writeln!(out);
    let _ = writeln!(out, "=== document_text_chars scenario (10 blocks, exact boundary) ===");
    let _ = writeln!(
        out,
        "9 blocks already at text_block_chars_max (100,000 chars) plus a 10th block {FINAL_CHUNK_CHARS} chars below it; the timed `import_update` is the final {FINAL_CHUNK_CHARS}-char chunk landing exactly on document_text_chars_max (1,000,000)."
    );
    let timings = measure_text_scenario(9, limits.text_block_chars_max, FINAL_CHUNK_CHARS)?;
    print_row(&mut out, &timings).map_err(|e| e.to_string())?;

    Ok(())
}
