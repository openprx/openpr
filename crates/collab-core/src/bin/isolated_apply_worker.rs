//! The isolated-apply worker: a single-purpose process spawned once per
//! `LoroCollabEngine::import_update` call by `collab_core::isolation::host::isolated_apply`
//! (`ADR-0014`, `contracts/limits-v1.md`'s "Isolated decode/apply" table). See
//! `crates/collab-core/src/isolation/host.rs`'s module doc for the overall design.
//!
//! Protocol, over stdin/stdout:
//! 1. Read the harness-envelope request (`[base_len][base][update_len][update]`) off stdin --
//!    unmetered (`ADR-0014` section 1.3 excludes harness envelope deserialization from the
//!    metered window).
//! 2. Load the base document -- also unmetered: it is already-validated state this process is
//!    only rehydrating, analogous to how the calibration host excludes `fork()` itself from its
//!    window.
//! 3. Arm the CPU-ceiling timer (`SIGPROF`/`ITIMER_PROF`) and the counting allocator. This opens
//!    the metered window.
//! 4. `import_update` the untrusted update, compute the resulting `semantic_snapshot`, and run
//!    `check_snapshot` against the frozen `contracts/limits-v1.md` structural ceilings -- decode +
//!    shape validation, exactly the scope `limits-v1.md`'s "从 decode 前开始计,到 semantic
//!    diff/shape validation 完成结束" describes.
//! 5. Disarm the timer and the counting allocator (closing the window) as soon as step 4 has a
//!    verdict, *before* serializing an accepted result: `export_snapshot` operates on already-
//!    validated in-memory state (the same reasoning that keeps step 2's `load` outside the window),
//!    not on the untrusted input step 4 just finished validating, so it is not part of the budget
//!    that bounds processing that untrusted input.
//! 6. Report the outcome back over stdout as a length/CRC32-framed payload.
//!
//! If the process is killed by `SIGPROF` (step 3/4 overran the CPU ceiling), aborts itself via the
//! counting allocator's rejection path (overran the memory ceiling, `SIGABRT`), or is `SIGKILL`ed
//! by the parent's wall watchdog (overran the wall ceiling), no response frame is ever written --
//! the parent classifies those cases from the exit signal alone (`isolation::host::classify_signal`).
//!
//! Never calls `std::process::exit` or lets a panic unwind past `main` (this workspace denies
//! `unwrap`/`expect`/`panic` in production code, and unwinding through a partially-shared runtime
//! state is not something this process needs to make safe -- every path here terminates through
//! [`respond_and_exit`], which always calls `libc::_exit`).

#![allow(unsafe_code)]

use std::io::Write;

use collab_core::error::CollabError;
use collab_core::isolation::{alloc, child_runtime, host, wire};
use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::{CollabEngine, LoroCollabEngine};

#[global_allocator]
static ALLOCATOR: alloc::CountingAllocator = alloc::CountingAllocator;

fn main() {
    // Set before reading anything from the caller: catches allocations that bypass the counting
    // allocator (a raw `mmap`, an FFI allocator) at any point in this process's life, not only
    // inside the metered window (`ADR-0014` section 10's `child.rs::apply_address_space_backstop`
    // reasoning, ported to this workload).
    child_runtime::set_address_space_backstop();

    if !child_runtime::is_single_threaded() {
        // An invariant this process's own design guarantees (a freshly `exec`'d binary starts
        // single-threaded) failed to hold -- refuse to run rather than arm a CPU timer whose
        // semantics assume a single thread. No response frame; the parent sees a non-zero exit
        // and classifies it as a host failure.
        exit_without_response(1);
    }

    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let Ok((base_snapshot, update)) = wire::read_request(&mut stdin_lock, wire::MAX_REQUEST_FIELD_BYTES) else {
        exit_without_response(1);
    };

    // `LoroCollabEngine::load`'s error is reported via the normal response frame (it is a real,
    // well-typed `CollabError` a caller-facing rejection can be built from), not treated as a
    // host failure -- so this cannot be a `let...else` (clippy's suggested rewrite): the
    // divergent branch needs the `Err` payload, not just "diverge".
    #[allow(clippy::manual_let_else)]
    let base_engine = match LoroCollabEngine::load(&base_snapshot) {
        Ok(engine) => engine,
        Err(err) => respond_and_exit(&wire::Outcome::Rejected(err)),
    };

    respond_and_exit(&decode_apply_check_and_export(base_engine, &update));
}

/// Everything between arming the metered window and closing it: `import_update`, deriving the
/// semantic snapshot, and `check_snapshot`. Timer and allocator disarm happen as the very first
/// step after this has a verdict, on every path -- see [`decode_apply_check_and_export`]'s doc
/// comment for why `export_snapshot` itself runs after the window closes, not inside it.
///
/// Returns the accepted, mutated `engine` (ready to export) on success, or the definitive
/// `wire::Outcome::Rejected` to report otherwise.
///
/// Split from [`decode_apply_check_and_export`] so this actual `import_update`/`check_snapshot`
/// logic is unit-testable without arming a real, process-wide `SIGPROF` timer with its default
/// (process-terminating) disposition -- doing that inside a `cargo test` binary, which runs many
/// tests concurrently in one process, would risk killing the entire test run under real CPU load,
/// not just this one test's work. The real armed path is only ever exercised by spawning the
/// actual compiled worker binary as a subprocess, which `isolation::host`'s own tests do.
fn run_metered(engine: LoroCollabEngine, update: &[u8]) -> Result<LoroCollabEngine, wire::Outcome> {
    child_runtime::arm_sigprof();
    alloc::arm();
    let outcome = decode_apply_and_check(engine, update);
    child_runtime::disarm_sigprof();
    alloc::disarm();
    outcome
}

/// The pure decode/apply/shape-validate logic, with no timer or allocator side effects of its own
/// -- safe to unit test directly (see [`run_metered`]'s doc comment for why that function itself
/// is not).
fn decode_apply_and_check(mut engine: LoroCollabEngine, update: &[u8]) -> Result<LoroCollabEngine, wire::Outcome> {
    if let Err(err) = engine.import_update(update) {
        return Err(wire::Outcome::Rejected(err));
    }

    let semantic = match engine.semantic_snapshot() {
        Ok(semantic) => semantic,
        Err(err) => return Err(wire::Outcome::Rejected(err)),
    };

    if let Err(violation) = check_snapshot(&semantic, &DocumentLimits::default()) {
        return Err(wire::Outcome::Rejected(CollabError::from(violation)));
    }

    Ok(engine)
}

/// Composes [`run_metered`] with the unmetered `export_snapshot` step that follows it, producing
/// the full `wire::Outcome` [`respond_and_exit`] reports.
///
/// `export_snapshot` serializes state this process already validated and committed to in-memory
/// (the accepted, merged `engine`) -- not the untrusted `update` input `run_metered` just finished
/// processing -- so, exactly like step 2's `LoroCollabEngine::load` of the base document, it runs
/// after the CPU/memory metered window closes rather than inside it. `contracts/limits-v1.md`'s
/// "从 decode 前开始计,到 semantic diff/shape validation 完成结束" scope for `decode_apply_cpu_ms_max`
/// and `isolated_apply_memory_bytes_max` ends at shape validation (`check_snapshot`), not at
/// "serialize the accepted result"; a release-mode measurement of this worker at the
/// `container_count_max`/`document_block_count_max` boundary (10,000 nodes) found
/// `export_snapshot` costing roughly as much CPU time as `import_update` itself, so leaving it
/// inside the window (as an earlier version of this file did) was spending real budget on work the
/// contract does not ask this ceiling to bound.
fn decode_apply_check_and_export(engine: LoroCollabEngine, update: &[u8]) -> wire::Outcome {
    match run_metered(engine, update) {
        Ok(accepted) => match accepted.export_snapshot() {
            Ok(snapshot) => wire::Outcome::Success { snapshot },
            Err(err) => wire::Outcome::Rejected(err),
        },
        Err(outcome) => outcome,
    }
}

/// Encodes and writes `outcome` as a length/CRC32-framed payload to stdout, flushes, and exits
/// `0`. Never returns.
///
/// Always writes `host::RESPONSE_MARKER_BYTE` first, before the framed response -- the parent's
/// `isolation::host::read_response_two_phase` uses it to tell "still doing unmetered setup" apart
/// from "response is now being written", so its strict `decode_apply_wall_ms_max` watchdog can
/// start counting from the right moment instead of from process spawn. See that function's doc
/// comment for the full reasoning.
fn respond_and_exit(outcome: &wire::Outcome) -> ! {
    let payload = wire::encode_outcome(outcome).unwrap_or_default();
    let Ok(framed) = host::encode_response_frame(&payload) else {
        exit_without_response(1);
    };

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(&[host::RESPONSE_MARKER_BYTE]);
    let _ = lock.write_all(&framed);
    let _ = lock.flush();

    // SAFETY: `_exit` takes a plain `c_int` status code and has no preconditions; using it
    // (rather than `std::process::exit`) avoids re-running any of the parent's already-scheduled
    // atexit handlers or global destructors a second time, matching standard fork/exec worker
    // discipline.
    unsafe {
        libc::_exit(0);
    }
}

/// Exits `status` without writing any response frame at all -- used only for failures the parent
/// must recognize as a host failure (non-zero exit), never as a business rejection.
fn exit_without_response(status: i32) -> ! {
    // SAFETY: `_exit` takes a plain `c_int` status code and has no preconditions.
    unsafe {
        libc::_exit(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `run_metered` is the one function in this binary with real logic (as opposed to I/O
    /// plumbing) -- exercised directly here rather than only through a spawned-process test,
    /// which `crates/collab-core/src/isolation/host.rs`'s own tests cover for the full
    /// spawn/watchdog/frame path.
    #[test]
    fn run_metered_accepts_a_well_formed_update_and_rejects_an_oversized_document() {
        let base = LoroCollabEngine::new_empty(1);
        let base_frontier = base.frontier();
        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("hello").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let outcome = decode_apply_check_and_export(base, &update);
        let wire::Outcome::Success { snapshot } = outcome else {
            panic!("expected Success, a title-only update stays well within every ceiling");
        };
        let reloaded = LoroCollabEngine::load(&snapshot).expect("exported snapshot reloads");
        assert_eq!(reloaded.title().expect("title reads"), "hello");
    }

    #[test]
    fn run_metered_rejects_an_update_that_would_exceed_document_text_chars() {
        // The oversized content lives in the *base* document (loaded from a snapshot, only
        // bounded by `InputLimits::snapshot_bytes_max`), not the update itself: a single update
        // carrying `document_text_chars_max + 1` characters would first be rejected as
        // `InputTooLarge` by `import_update`'s own `update_bytes_max` (64 KiB) byte-length gate,
        // before `check_snapshot` is ever reached -- that would test the wrong ceiling. Ten
        // blocks each at exactly `text_block_chars_max` sum to exactly `document_text_chars_max`
        // (a compliant document); the tiny update on top adds one more character in an eleventh
        // block, tipping the *document* total over without any single block exceeding its own
        // per-block ceiling.
        let limits = DocumentLimits::default();
        let full_block_text = "x".repeat(limits.text_block_chars_max);
        let mut base = LoroCollabEngine::new_empty(1);
        for block_index in 0..(limits.document_text_chars_max / limits.text_block_chars_max) {
            let id = collab_core::NodeId::from(format!("blk-{block_index}"));
            base.apply_operation(&collab_core::Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: collab_core::NodeKind::Block,
            })
            .expect("create succeeds");
            base.apply_operation(&collab_core::Operation::InsertText {
                id,
                index: 0,
                text: full_block_text.clone(),
            })
            .expect("insert_text succeeds");
        }
        let base_frontier = base.frontier();
        let mut writer = base.fork().expect("fork succeeds");
        writer
            .apply_operation(&collab_core::Operation::CreateNode {
                id: collab_core::NodeId::from("blk-extra"),
                parent: None,
                index: 0,
                kind: collab_core::NodeKind::Block,
            })
            .expect("create succeeds");
        writer
            .apply_operation(&collab_core::Operation::InsertText {
                id: collab_core::NodeId::from("blk-extra"),
                index: 0,
                text: "y".to_string(),
            })
            .expect("insert_text succeeds at the engine level (check_snapshot, not the engine, enforces this ceiling)");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let outcome = decode_apply_check_and_export(base, &update);
        let wire::Outcome::Rejected(CollabError::LimitExceeded { limit_kind, .. }) = outcome else {
            panic!("expected a document_text_chars rejection, got {outcome:?}");
        };
        assert_eq!(limit_kind, "document_text_chars");
    }
}
