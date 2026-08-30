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
//! 3. Write `host::RESPONSE_MARKER_BYTE` to stdout and flush -- the unmetered setup phase is now
//!    over. This is the signal `isolation::host::read_response_two_phase` uses to start its own
//!    independent `decode_apply_wall_ms_max` watchdog from the *right* moment (the metered window
//!    is about to open), not from whenever the full response happens to be ready. See
//!    [`write_marker_or_exit`]'s doc comment for why this must happen here and not folded into the
//!    final response write.
//! 4. Arm the CPU-ceiling timer (`SIGPROF`/`ITIMER_PROF`) and the counting allocator. This opens
//!    the metered window.
//! 5. `import_update` the untrusted update, compute the resulting `semantic_snapshot`, and run
//!    `check_snapshot` against the frozen `contracts/limits-v1.md` structural ceilings -- decode +
//!    shape validation, exactly the scope `limits-v1.md`'s "从 decode 前开始计,到 semantic
//!    diff/shape validation 完成结束" describes.
//! 6. Disarm the timer and the counting allocator (closing the window) as soon as step 5 has a
//!    verdict, *before* serializing an accepted result: `export_snapshot` operates on already-
//!    validated in-memory state (the same reasoning that keeps step 2's `load` outside the window),
//!    not on the untrusted input step 5 just finished validating, so it is not part of the budget
//!    that bounds processing that untrusted input.
//! 7. Report the outcome back over stdout as a length/CRC32-framed payload (the marker byte was
//!    already sent in step 3, so this is just the framed payload itself).
//!
//! If the process is killed by `SIGPROF` (step 4/5 overran the CPU ceiling), aborts itself via the
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
        Err(err) => {
            write_marker_or_exit();
            respond_and_exit(&wire::Outcome::Rejected(err));
        }
    };

    write_marker_or_exit();
    respond_and_exit(&decode_apply_check_and_export(base_engine, &update));
}

/// Writes [`host::RESPONSE_MARKER_BYTE`] and flushes, marking the exact moment this process's
/// unmetered setup (reading the request, loading the base document) is over. Split out from
/// [`respond_and_exit`] -- an earlier version of this file wrote the marker together with the
/// final framed response, which made `isolation::host::read_response_two_phase`'s
/// `decode_apply_wall_ms_max` watchdog measure only the time to flush an *already-finished*
/// result, not the metered window's own duration: a worker that spends, say, 300ms blocked
/// (not burning CPU) inside step 4 and then completes normally would have produced its marker and
/// full response back-to-back at the very end, and the watchdog's own 100ms deadline -- started
/// only once the marker arrives -- would never have had anything left to time. Writing the marker
/// here instead, right as the metered window is about to open, makes the watchdog actually bound
/// step 4's wall-clock duration, matching `contracts/limits-v1.md`'s "从 decode 前开始计" scope for
/// `decode_apply_wall_ms_max`.
///
/// Exits (without a further response) on any I/O failure, the same way every other setup-phase
/// failure in `main` does -- a write failure this early means the parent cannot receive a useful
/// response of any shape.
fn write_marker_or_exit() {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if lock.write_all(&[host::RESPONSE_MARKER_BYTE]).is_err() || lock.flush().is_err() {
        drop(lock);
        exit_without_response(1);
    }
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
    let outcome = metered_workload(engine, update);
    child_runtime::disarm_sigprof();
    alloc::disarm();
    outcome
}

/// The work that actually runs inside the armed metered window. In every `--release` build (this
/// workspace's only production build shape -- see the `test_injection` module doc below) this is
/// nothing more than a direct call to [`decode_apply_and_check`]; the `cfg(debug_assertions)`
/// branch exists only so `isolation::host`'s own boundary tests can substitute a precisely
/// controllable synthetic workload for the real decode/apply/validate pipeline, still inside this
/// exact arm/disarm pair.
fn metered_workload(engine: LoroCollabEngine, update: &[u8]) -> Result<LoroCollabEngine, wire::Outcome> {
    #[cfg(debug_assertions)]
    if let Some(workload) = test_injection::workload_from_env() {
        return Ok(test_injection::run(&workload, engine));
    }
    decode_apply_and_check(engine, update)
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
/// Does *not* write `host::RESPONSE_MARKER_BYTE` -- every caller already sent it via
/// [`write_marker_or_exit`] the moment unmetered setup finished, before entering (or definitively
/// skipping) the metered window. See that function's doc comment for why the marker must be sent
/// there and not here.
fn respond_and_exit(outcome: &wire::Outcome) -> ! {
    let payload = wire::encode_outcome(outcome).unwrap_or_default();
    let Ok(framed) = host::encode_response_frame(&payload) else {
        exit_without_response(1);
    };

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
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

/// Test-only synthetic workloads that let `isolation::host`'s own boundary tests drive this
/// worker's *real* enforcement primitives (the exact `child_runtime::arm_sigprof`/`alloc::arm`
/// calls [`run_metered`] uses in production, via [`metered_workload`]'s `cfg(debug_assertions)`
/// branch) against a precisely controllable amount of CPU time, wall-clock blocking, or a single
/// allocation size -- rather than hunting for real Loro CRDT content that happens to land near a
/// millisecond/byte boundary. That is fragile and machine-speed-dependent for the CPU/wall
/// ceilings, and not reliably reachable at all for the memory ceiling with legitimate content: a
/// document large enough to need over 128 MiB of *decode-time* working memory is already well
/// past `container_count_max`/`document_block_count_max` (10,000 nodes) and gets rejected by
/// `check_snapshot` on structural grounds long before it could ever pressure the allocator (see
/// `crates/collab-core/examples/decode_apply_budget.rs`'s measurements at that boundary).
///
/// # Why this cannot run in production
/// Gated on `cfg(debug_assertions)`, which is `false` for every `--release` build -- this
/// workspace's standard production build (`cargo build --release --all-features`, per this
/// repository's `CLAUDE.md`) does not compile this module in at all. There is no environment
/// variable, flag, or other runtime toggle that can make a `--release` compile of
/// `collab-isolated-apply-worker` execute a single line of this module; it is not merely inert,
/// its code does not exist in that binary. It is *additionally* gated on [`ENV_VAR`]'s presence,
/// so even a debug build behaves identically to the production shape unless a test deliberately
/// sets that variable on the *child's own* environment before spawning it --
/// `isolation::host::isolated_apply` (the production call site) never sets it, and never forwards
/// it from its own caller (`apps/api`) either.
#[cfg(debug_assertions)]
mod test_injection {
    use std::time::Duration;

    use super::LoroCollabEngine;

    /// Read by [`workload_from_env`]. Format: `<kind>=<value>`, one of `cpu_burn_micros=<u64>`,
    /// `wall_sleep_millis=<u64>`, `alloc_bytes=<usize>`.
    pub const ENV_VAR: &str = "COLLAB_ISOLATION_TEST_INJECT";

    pub enum Workload {
        /// Busy-loops, polling `CLOCK_PROCESS_CPUTIME_ID`, until this process has consumed at
        /// least this many microseconds of its own CPU time -- for boundary-testing
        /// `decode_apply_cpu_ms_max`'s `SIGPROF` ceiling with a duration that does not depend on
        /// document shape or machine speed.
        CpuBurnMicros(u64),
        /// Sleeps (consuming effectively no CPU) for this many milliseconds -- for
        /// boundary-testing `decode_apply_wall_ms_max`'s independent wall watchdog without also
        /// risking the CPU ceiling firing first.
        WallSleepMillis(u64),
        /// Attempts one single allocation of exactly this many bytes through the process's real
        /// global allocator (`alloc::CountingAllocator`, installed by this binary's `main` as
        /// `#[global_allocator]`) -- for boundary-testing `isolated_apply_memory_bytes_max` at the
        /// exact byte, which real document content cannot reach (see this module's own doc
        /// comment).
        AllocBytes(usize),
    }

    /// Parses [`ENV_VAR`] into a [`Workload`], if set and well-formed. Any absence or parse
    /// failure returns `None`, falling `metered_workload` through to the real
    /// decode/apply/validate pipeline -- this is a test-only convenience with no caller-facing
    /// error reporting of its own.
    pub fn workload_from_env() -> Option<Workload> {
        let raw = std::env::var(ENV_VAR).ok()?;
        let (kind, value) = raw.split_once('=')?;
        match kind {
            "cpu_burn_micros" => value.parse().ok().map(Workload::CpuBurnMicros),
            "wall_sleep_millis" => value.parse().ok().map(Workload::WallSleepMillis),
            "alloc_bytes" => value.parse().ok().map(Workload::AllocBytes),
            _ => None,
        }
    }

    /// Reads `CLOCK_PROCESS_CPUTIME_ID` -- the same POSIX clock `contracts/limits-v1.md`'s
    /// `decode_apply_cpu_ms_max` and `ITIMER_PROF` (`child_runtime::arm_sigprof`) both track (user
    /// and system CPU time combined, not wall clock). `None` only if the kernel does not support
    /// this clock id, which does not happen on any Linux this workspace targets.
    fn cpu_time_now() -> Option<Duration> {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`;
        // `CLOCK_PROCESS_CPUTIME_ID` is a clock id always supported by the Linux kernels this
        // workspace targets. No precondition beyond the pointer's validity.
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut ts) };
        if rc != 0 {
            return None;
        }
        let secs = u64::try_from(ts.tv_sec).ok()?;
        let nanos = u32::try_from(ts.tv_nsec).ok()?;
        Some(Duration::new(secs, nanos))
    }

    /// Spins until this process has burned at least `target_micros` of its own CPU time (or until
    /// `cpu_time_now` cannot be read at all, in which case this returns early rather than looping
    /// forever on an unreadable clock -- the calling test still gets a definitive ceiling verdict
    /// either way, just possibly under-shooting the intended burn). `std::hint::black_box` keeps
    /// the loop body from being optimized away entirely.
    fn burn_cpu_micros(target_micros: u64) {
        let Some(start) = cpu_time_now() else { return };
        let target = Duration::from_micros(target_micros);
        loop {
            for spin in 0..10_000u64 {
                std::hint::black_box(spin);
            }
            let Some(now) = cpu_time_now() else { return };
            if now.saturating_sub(start) >= target {
                return;
            }
        }
    }

    /// Runs `workload` inside the caller's already-armed metered window ([`super::run_metered`]
    /// calls this, via [`super::metered_workload`], between `arm_sigprof`/`alloc::arm` and
    /// `disarm_sigprof`/`alloc::disarm` -- exactly where it would otherwise call
    /// `super::decode_apply_and_check`), and hands back the untouched input `engine` unmodified if
    /// it returns at all -- [`super::metered_workload`]'s caller then exports and reports it as a
    /// trivial `Success`. The point of every one of these workloads is that the *ceiling itself*
    /// (`SIGPROF`, the wall watchdog, or the counting allocator's
    /// null return -> `handle_alloc_error` -> abort) decides whether this function ever returns --
    /// a workload sized to stay under its ceiling returns normally; one sized to exceed it does
    /// not return at all, and the parent classifies the kill from the child's exit signal (or, for
    /// the wall ceiling, from its own independent watchdog) exactly as it would for real content.
    ///
    /// Returns plain `LoroCollabEngine`, not a `Result` -- none of the three arms below ever
    /// produce a business rejection; the only way this function does not eventually return is the
    /// process being killed out from under it (`SIGPROF`/`SIGKILL`/`SIGABRT`), which by
    /// construction never reaches a `return` statement at all.
    pub fn run(workload: &Workload, engine: LoroCollabEngine) -> LoroCollabEngine {
        match *workload {
            Workload::CpuBurnMicros(micros) => {
                burn_cpu_micros(micros);
                engine
            }
            Workload::WallSleepMillis(millis) => {
                std::thread::sleep(Duration::from_millis(millis));
                engine
            }
            Workload::AllocBytes(bytes) => {
                // A single allocation request of exactly `bytes`: `Vec::<u8>::with_capacity`
                // computes `Layout::array::<u8>(bytes)` (size = bytes, align = 1, no rounding for
                // a byte-sized element) and hands it to the process's installed global allocator
                // unchanged -- the identical `CountingAllocator::alloc` path a real oversized
                // decode/apply would hit, just with a size this test controls exactly instead of
                // whatever a real update happened to need. Returning null from `GlobalAlloc::alloc`
                // routes through Rust's `handle_alloc_error`, which aborts the process (`SIGABRT`)
                // -- this function never returns in that case, by design.
                let mut buffer: Vec<u8> = Vec::with_capacity(bytes);
                // Writes one byte so the allocation cannot be optimized away as dead, without
                // paying the cost of zeroing the whole buffer the way `vec![0u8; bytes]` would.
                if bytes > 0 {
                    buffer.push(0);
                }
                drop(buffer);
                engine
            }
        }
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
