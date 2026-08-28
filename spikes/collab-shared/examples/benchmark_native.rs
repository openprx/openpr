//! Native (P1, in-process) cross-candidate benchmark for the Sylvode Flow v0.3 engine-selection
//! decision, matching `testing/benchmark-spec.md` and the four *native*-side budgeted metrics
//! from `docs/schemas/sylvode-flow-benchmark-result-v1.schema.json`'s `budgets` object:
//!
//! | metric                              | budget      |
//! |--------------------------------------|------------:|
//! | `apply_update_ms` p95 (<=8 KiB)       |       20 ms |
//! | `bootstrap_10k_ops_ms` p95            |     1000 ms |
//! | `bootstrap_100k_ops_ms` p95           |     5000 ms |
//! | `peak_memory_100k_ops_bytes`          | 268435456 B |
//!
//! The browser-side budgets (`browser_engine_bundle_gzip_bytes_max`, `cold_start_ms_p95_max`)
//! are out of scope for this binary and are never reported here.
//!
//! # This never touches the ADR-0014 isolation host (P1, not P2/P3)
//!
//! This file imports nothing from `collab_shared::isolation` (in fact it never even sees that
//! module: `collab_shared::isolation` is only `#[cfg(target_os = "linux")]`-compiled and this
//! file's own imports below list everything it pulls in). The only process-spawning this file
//! does at all is [`spawn_peak_memory_child`], a single plain `std::process::Command::new(exe)`
//! re-invocation of *this same test binary* with a `--peak-memory-child <candidate>` argument --
//! no `fork()`, no `mmap`, no `sigaction`/`setitimer`, no shared page, nothing from
//! `crate::isolation::zygote`/`child`/`shared_page`. That subprocess exists purely so
//! [`read_vm_hwm_bytes`] (a plain `/proc/self/status` read, also no `unsafe`) reports a
//! per-candidate peak RSS that is not contaminated by whatever the *other* candidate's bootstrap
//! already pushed `VmHWM` to earlier in the same process -- see that function's doc comment.
//! ADR-0014 section 5 requires the engine-selection benchmark stay on the native in-process (P1)
//! path precisely so fork/spawn overhead never pollutes engine-selection data; nothing in this
//! file spawns anything on the per-sample timed path (`sample_alternating`'s closures below never
//! call [`spawn_peak_memory_child]`), only once each for the one-shot peak-memory figure.
//!
//! # Why this is an `[[example]]` with `harness = false`, run via `cargo run --release --example`
//!
//! `collab-shared` cannot take `collab-loro-spike`/`collab-yrs-yjs-spike` as ordinary
//! `[dependencies]` (each already depends on `collab-shared`; Cargo rejects that dependency
//! cycle). It *can* take them as `[dev-dependencies]` -- Cargo explicitly supports a dev-dep
//! cycle since dev-dependencies are only linked into test/bench/example targets, never into the
//! library itself, and `[dev-dependencies]` are available to `[[example]]` targets exactly as
//! they are to `[[test]]` targets. This binary needs to drive both real engines side by side in
//! one process (to interleave them -- see [`collab_shared::benchmark::sample_alternating`]), so
//! it has to be a target that gets `[dev-dependencies]`.
//!
//! It is deliberately an `[[example]]`, not a `[[test]]`: a plain `cargo test` builds an
//! `[[example]]` target but never *executes* it, whereas even a `harness = false` `[[test]]`
//! target is run automatically by every `cargo test` invocation. This benchmark's 100k-op-scale
//! cases can legitimately run for minutes (see the yrs-yjs superlinear-cost finding below and in
//! the delivery report) -- an earlier version of this file *was* a `[[test]]`, and an unrelated
//! `cargo test` sweep ended up unknowingly running the full multi-minute benchmark and being
//! killed/truncated partway through as a result. `harness = false` still means this file owns its
//! own `main`/argv parsing instead of libtest's per-`#[test]`-function harness, which matters here
//! because this is one serial, alternating, wall-clock-timed run, not a set of independent (and by
//! default concurrently/arbitrarily scheduled) test cases.
//!
//! # Loro's "same logical id, two physical tree nodes" caveat
//!
//! `same_key_nested_container_creation_case` (see `collab_shared::fixture` and
//! `tests/corpus.rs`'s `same_key_nested_container_creation_resolves_via_map_lww`) proves that
//! when two replicas concurrently `CreateNode` under the same logical id, Loro's tree keeps two
//! distinct physical `TreeID` nodes (this adapter's semantic projection then folds them to one
//! logical id deterministically). The bootstrap/apply workloads this binary measures are single-
//! replica, sequential-apply logs (no concurrent duplicate creates), so that specific engine
//! behavior does not appear in the numbers below -- but it is exactly why a Loro
//! `snapshot_bytes`/`peak_memory` figure and a yrs-yjs one are not safe to read as "identical
//! logical content -> identical engine-state size" in general. See `loro_dual_node_caveat` in
//! this binary's own JSON output, and the delivery report, for where this does and does not
//! apply to the numbers measured here.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::Command;
use std::time::Instant;

use collab_loro_spike::LoroCollabEngine;
use collab_shared::benchmark::{SampleStats, sample_alternating};
use collab_shared::corpus::apply_all;
use collab_shared::fixture::page_ops;
use collab_shared::rng::SplitMix64;
use collab_shared::{CollabEngine, CollabError, CorpusEngine, NodeId, Operation};
use collab_yrs_yjs_spike::YrsCollabEngine;

/// Deterministic corpus seed for every fixture this binary generates (`page_ops`, replica
/// identity, filler text) -- "固定 corpus seed" per `testing/benchmark-spec.md`.
const CORPUS_SEED: u64 = 0x0053_594C_564F_4445; // ASCII-derived, arbitrary but fixed.
/// Fixed replica identity seed for every engine instance this binary constructs. Only affects
/// each adapter's internal peer/client id, never the applied operation log, so using the same
/// constant for both candidates (and every sample) does not bias either candidate.
const REPLICA_SEED: u64 = 1;

const APPLY_UPDATE_TARGET_BYTES_MAX: usize = 8 * 1024;
const BOOTSTRAP_10K_OPS: usize = 10_000;
const BOOTSTRAP_100K_OPS: usize = 100_000;
/// Steady-state base page the `apply_update` case's small incremental edit is layered on top of
/// -- large enough to be a realistic mid-size document, far under `BOOTSTRAP_10K_OPS` so this
/// case measures "apply one small delta to an existing document", not another bootstrap.
const APPLY_UPDATE_BASE_OPS: usize = 1_000;

const APPLY_UPDATE_MS_P95_MAX: f64 = 20.0;
const BOOTSTRAP_10K_MS_P95_MAX: f64 = 1000.0;
const BOOTSTRAP_100K_MS_P95_MAX: f64 = 5000.0;
const PEAK_MEMORY_100K_BYTES_MAX: u64 = 268_435_456;

const WARMUP: usize = 5;
const SAMPLES: usize = 30;

/// Warmup/sample counts for the two 100k-op-scale metrics (`bootstrap_100k_ops_ms`,
/// `restore_100k_ops_ms`), overridable via env vars. The *code* default stays spec-compliant
/// (5 warmup + 30 samples, same as every other metric) for a machine/engine-state where that is
/// actually affordable. This run overrides both to smaller values via
/// `BOOTSTRAP_100K_WARMUP`/`BOOTSTRAP_100K_SAMPLES` because live calibration (see
/// `--calibrate-ops`, and the delivery report) measured the yrs-yjs candidate's bootstrap cost at
/// this scale growing propotionally to roughly n^2.1-n^2.3 (empirically fit from n=1000..20000),
/// putting a single n=100,000 bootstrap at an estimated several minutes -- 30 samples at that
/// per-sample cost would take on the order of hours, which is not "跑不完就等着"; per this task's
/// explicit instruction, the honest response is fewer real samples with the actual count/cost
/// disclosed, not a silent reduction pretending to be >=30.
fn bootstrap_100k_warmup() -> usize {
    std::env::var("BOOTSTRAP_100K_WARMUP")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(WARMUP)
}

fn bootstrap_100k_samples() -> usize {
    std::env::var("BOOTSTRAP_100K_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SAMPLES)
}

/// Like [`collab_shared::benchmark::sample_alternating`] but WITHOUT the `MIN_WARMUP`/
/// `MIN_SAMPLES` floor -- deliberately, for the two 100k-op-scale metrics above, where forcing at
/// least 30 samples at several-minutes-each is not a "let it run longer" situation but an
/// hours-long one. `sample_count`/`warmup_count` in the returned [`SampleStats`] report the
/// *actual* counts used, so a reduced run is self-describing in its own JSON output rather than
/// silently looking like every other (real, >=30-sample) metric in this document.
fn sample_alternating_unclamped<FA, FB>(
    warmup: usize,
    samples: usize,
    mut work_a: FA,
    mut work_b: FB,
) -> (SampleStats, SampleStats)
where
    FA: FnMut() -> f64,
    FB: FnMut() -> f64,
{
    for _ in 0..warmup {
        let _ = work_a();
        let _ = work_b();
    }
    let mut raw_a = Vec::with_capacity(samples);
    let mut raw_b = Vec::with_capacity(samples);
    for _ in 0..samples {
        raw_a.push(work_a());
        raw_b.push(work_b());
    }
    (stats_from_unclamped(warmup, raw_a), stats_from_unclamped(warmup, raw_b))
}

fn stats_from_unclamped(warmup_count: usize, raw_samples: Vec<f64>) -> SampleStats {
    let sample_count = raw_samples.len();
    let (median, p95, p99, min, max) = collab_shared::benchmark::distribution_stats(&raw_samples);
    SampleStats {
        warmup_count,
        sample_count,
        median,
        p95,
        p99,
        min,
        max,
        raw_samples,
    }
}

/// Writes `value` as pretty JSON to `evidence/<name>.json` under the scratchpad-relative
/// `BENCHMARK_PARTIAL_DIR` (or the current directory if unset) as soon as it is computed, so a
/// forced kill mid-run (e.g. hitting an external time budget on a metric far more expensive than
/// expected) still leaves every metric computed *before* that point safely on disk instead of
/// losing the entire run's output (this binary's normal path only prints its combined JSON once,
/// at the very end).
fn write_partial(name: &str, value: &serde_json::Value) {
    let dir = std::env::var("BENCHMARK_PARTIAL_DIR").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&dir).join(format!("{name}.json"));
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            if let Err(error) = std::fs::write(&path, text) {
                eprintln!("warning: failed to write partial result {}: {error}", path.display());
            } else {
                eprintln!("partial result written: {}", path.display());
            }
        }
        Err(error) => eprintln!("warning: failed to serialize partial result {name}: {error}"),
    }
}

/// Prints `context: error` to stderr and exits with status 1.
///
/// Used in place of `.expect()`/`panic!()` throughout this binary: every failure this function
/// reports is a setup/environment problem (a corpus op that must always apply cleanly, a snapshot
/// that must always re-decode, a child process that must always spawn) rather than a caller
/// input the operator could recover from mid-run, so a loud, immediate, non-zero-exit failure is
/// the correct outcome -- just reached via an explicit exit rather than an unwind.
fn fatal(context: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("error: {context}: {error}");
    std::process::exit(1);
}

/// Like [`fatal`] but for an [`Option`] that unexpectedly held `None` (no separate error value to
/// display).
fn fatal_msg(context: &str) -> ! {
    eprintln!("error: {context}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--calibrate-ops") {
        let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2000);
        run_calibrate_ops(n);
        return;
    }
    if args.get(1).map(String::as_str) == Some("--peak-memory-child") {
        let Some(candidate) = args.get(2) else {
            eprintln!("error: --peak-memory-child requires a candidate name");
            std::process::exit(2);
        };
        run_peak_memory_child(candidate);
        return;
    }
    run_full_benchmark();
}

/// Builds a fixed-length `page_ops` log truncated to exactly `total_ops` entries. `page_ops`
/// itself already emits exactly `2 * op_count` operations per call (one `CreateNode` + one
/// `InsertText` per loop iteration, unconditionally), so `op_count = total_ops.div_ceil(2)`
/// followed by a defensive truncate produces exactly `total_ops` operations for any `total_ops`,
/// even/odd alike, without relying on that internal "exactly 2x" shape as a hard contract.
fn exact_op_log(seed: u64, total_ops: usize) -> Vec<Operation> {
    let op_count = u32::try_from(total_ops.div_ceil(2)).unwrap_or(u32::MAX);
    let mut ops = page_ops(seed, op_count);
    ops.truncate(total_ops);
    ops
}

/// A fixed, deterministic ASCII filler string of `char_len` characters -- the same technique
/// `collab_shared::corpus::tune_update_to_exact_bytes` uses internally, reimplemented here
/// (that helper's own `filler` closure is private to `corpus.rs`) so [`tune_edit_under_bytes`]
/// can drive its own byte-budget search over a *realistic incremental edit on an existing
/// document* rather than `tune_update_to_exact_bytes`'s single-block-from-empty boundary fixture.
fn filler(char_len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ";
    // ALPHABET's length (65) fits comfortably in a u32 regardless of target pointer width.
    #[allow(clippy::cast_possible_truncation)]
    let alphabet_len = ALPHABET.len() as u32;
    let mut rng = SplitMix64::new(0xF177_3F17_7357);
    (0..char_len)
        .map(|_| {
            let idx = rng.next_below(alphabet_len) as usize;
            ALPHABET.get(idx).copied().unwrap_or(b' ') as char
        })
        .collect()
}

struct ApplyUpdatePayload {
    base_snapshot: Vec<u8>,
    update: Vec<u8>,
}

/// Builds a steady-state base document (`base_ops` applied to a fresh replica), then binary-
/// searches over an appended-text length for the **largest** single `InsertText` edit on an
/// existing block whose exported delta (`export_from` against the base frontier) stays at or
/// under `max_bytes` -- a realistic "burst of typing lands, syncs to a peer" update, sized as
/// close to the byte budget as this engine's own wire format allows, rather than a fixed
/// (and therefore not representative for both candidates') operation count.
///
/// Each candidate's own serialization overhead means the *number of characters* that fits under
/// the same byte budget differs between Loro and yrs -- expected and unavoidable (two different
/// CRDT wire formats cannot share update bytes), not a fairness violation: the workload being
/// benchmarked is "apply an update that respects the real <=8 KiB transport ceiling", and both
/// candidates are tuned against that same external ceiling.
fn build_apply_update_payload<E: CorpusEngine<Error = CollabError>>(
    base_ops: &[Operation],
    max_bytes: usize,
) -> ApplyUpdatePayload {
    let mut base = E::new_empty(REPLICA_SEED);
    apply_all(&mut base, base_ops).unwrap_or_else(|error| fatal("apply_update base ops must apply cleanly", error));
    let base_snapshot = base
        .export_snapshot()
        .unwrap_or_else(|error| fatal("export apply_update base snapshot", error));
    let base_frontier = base.frontier();
    let edit_target: NodeId = base_ops
        .first()
        .map(Operation::target)
        .cloned()
        .unwrap_or_else(|| fatal_msg("apply_update base ops must be non-empty"));

    let build_update = |char_len: usize| -> Vec<u8> {
        let mut probe =
            E::load(&base_snapshot).unwrap_or_else(|error| fatal("reload apply_update base snapshot for probe", error));
        probe
            .apply_operation(&Operation::InsertText {
                id: edit_target.clone(),
                index: 0,
                text: filler(char_len),
            })
            .unwrap_or_else(|error| fatal("apply probe insert-text edit", error));
        probe
            .export_from(&base_frontier)
            .unwrap_or_else(|error| fatal("export probe update against base frontier", error))
    };

    // Binary search for the largest char_len whose exported update size stays <= max_bytes.
    // Export size is monotonic non-decreasing in filler length for both real candidates.
    let mut low = 0usize;
    let mut high = max_bytes;
    let mut best_update = build_update(0);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate_update = build_update(mid);
        if candidate_update.len() <= max_bytes {
            best_update = candidate_update;
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    assert!(
        best_update.len() <= max_bytes,
        "tuned apply_update payload must respect the byte budget"
    );

    ApplyUpdatePayload {
        base_snapshot,
        update: best_update,
    }
}

fn read_vm_hwm_bytes() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
            if let Ok(kib) = digits.parse::<u64>() {
                return kib.saturating_mul(1024);
            }
        }
    }
    0
}

/// Runs the 100k-op bootstrap for exactly one candidate in this (freshly spawned, otherwise-idle)
/// process, then prints the `VmHWM` delta (peak resident set observed in *this* process, minus
/// its own near-startup baseline) to stdout as a bare integer. Run in a dedicated subprocess
/// (see [`spawn_peak_memory_child`]) rather than measured in-line in the main run, because
/// `VmHWM` is monotonic non-decreasing for the lifetime of a process: measuring both candidates'
/// 100k-op peak memory back-to-back in the *same* process would have the second candidate's
/// figure include a floor contributed by the first candidate's already-resident allocations,
/// which is not "candidate B's own peak memory" at all.
fn run_calibrate_ops(n: usize) {
    let ops = exact_op_log(CORPUS_SEED, n);
    eprintln!("calibrate: n={n} ops built, timing single-sample bootstrap for each candidate...");

    let start = Instant::now();
    let mut loro_engine = LoroCollabEngine::new_empty(REPLICA_SEED);
    apply_all(&mut loro_engine, &ops).unwrap_or_else(|error| fatal("loro calibrate bootstrap", error));
    let loro_ms = start.elapsed().as_secs_f64() * 1000.0;
    std::hint::black_box(&loro_engine);
    println!("loro n={n} bootstrap_ms={loro_ms:.3}");

    let start = Instant::now();
    let mut yrs_engine = YrsCollabEngine::new_empty(REPLICA_SEED);
    apply_all(&mut yrs_engine, &ops).unwrap_or_else(|error| fatal("yrs calibrate bootstrap", error));
    let yrs_ms = start.elapsed().as_secs_f64() * 1000.0;
    std::hint::black_box(&yrs_engine);
    println!("yrs-yjs n={n} bootstrap_ms={yrs_ms:.3}");
}

fn run_peak_memory_child(candidate: &str) {
    let baseline = read_vm_hwm_bytes();
    let ops = exact_op_log(CORPUS_SEED, BOOTSTRAP_100K_OPS);
    match candidate {
        "loro" => {
            let mut engine = LoroCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops)
                .unwrap_or_else(|error| fatal("loro 100k-op bootstrap for peak-memory child", error));
            std::hint::black_box(&engine);
        }
        "yrs-yjs" => {
            let mut engine = YrsCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops)
                .unwrap_or_else(|error| fatal("yrs 100k-op bootstrap for peak-memory child", error));
            std::hint::black_box(&engine);
        }
        other => {
            eprintln!("error: unknown --peak-memory-child candidate {other:?}");
            std::process::exit(2);
        }
    }
    let peak = read_vm_hwm_bytes();
    println!("{}", peak.saturating_sub(baseline));
}

/// Spawns a fresh copy of this exact test binary with `--peak-memory-child <candidate>` (plain
/// `std::process::Command`, not `fork()`/the ADR-0014 isolation host -- see this file's module
/// docs) and parses its one-line stdout as the candidate's peak-memory-over-baseline byte count.
fn spawn_peak_memory_child(candidate: &str) -> u64 {
    let exe = std::env::current_exe()
        .unwrap_or_else(|error| fatal("resolve current test binary path for peak-memory child spawn", error));
    let output = Command::new(&exe)
        .arg("--peak-memory-child")
        .arg(candidate)
        .output()
        .unwrap_or_else(|error| fatal(&format!("failed to spawn peak-memory child for {candidate}"), error));
    if !output.status.success() {
        fatal_msg(&format!(
            "peak-memory child for {candidate} exited with {:?}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or_else(|error| {
            fatal(
                &format!("could not parse peak-memory child stdout for {candidate}"),
                error,
            )
        })
}

const fn status_str(pass: bool) -> &'static str {
    if pass { "passed" } else { "failed" }
}

fn shell_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn read_first_matching_field(path: &str, prefix: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn gather_environment() -> serde_json::Value {
    let cpu_model =
        read_first_matching_field("/proc/cpuinfo", "model name\t:").unwrap_or_else(|| "unknown".to_string());
    let cpu_cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let ram_bytes = read_first_matching_field("/proc/meminfo", "MemTotal:")
        .and_then(|value| value.trim_end_matches(" kB").parse::<u64>().ok())
        .map(|kib| kib.saturating_mul(1024));
    let kernel = shell_stdout("uname", &["-r"]).unwrap_or_else(|| "unknown".to_string());
    let os = shell_stdout("uname", &["-s"]).unwrap_or_else(|| std::env::consts::OS.to_string());
    let rustc_version = shell_stdout("rustc", &["--version"]).unwrap_or_else(|| "unknown".to_string());
    let source_commit = shell_stdout("git", &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = shell_stdout("git", &["status", "--porcelain"]).is_none_or(|s| !s.is_empty());

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let lockfile_path = std::path::Path::new(&manifest_dir).join("../../Cargo.lock");
    let lockfile_hash = std::fs::read(&lockfile_path).map_or_else(
        |_| "unavailable".to_string(),
        |bytes| {
            use sha2::{Digest, Sha256};
            use std::fmt::Write as _;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let digest = hasher.finalize();
            digest.iter().fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            })
        },
    );

    serde_json::json!({
        "cpu_model": cpu_model,
        "cpu_cores": cpu_cores,
        "ram_bytes": ram_bytes,
        "os": os,
        "kernel": kernel,
        "rust_toolchain": rustc_version,
        "release_flags": [
            "cargo test --release (cargo's default [profile.release]: opt-level=3, debug=false, \
             lto=false, codegen-units=16, panic=unwind, overflow-checks=false, debug-assertions=false; \
             workspace root Cargo.toml defines no [profile.release] override)"
        ],
        "engine_versions": {
            "loro": collab_loro_spike::metadata().rust_engine_version,
            "yrs-yjs": collab_yrs_yjs_spike::metadata().rust_engine_version,
        },
        "lockfile_hash_sha256": lockfile_hash,
        "source_commit": source_commit,
        "source_working_tree_dirty": dirty,
        "optimized_build": !cfg!(debug_assertions),
    })
}

// `_10k`/`_100k` op-count suffixes throughout this function intentionally share every
// character but the count -- that is the naming's whole point (the same metric at two
// corpus scales).
#[allow(clippy::similar_names)]
fn run_full_benchmark() {
    let generated_at = shell_stdout("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| "unknown".to_string());
    let environment = gather_environment();

    // --- Fixed, shared-by-reference corpora (built once, outside every timed loop). ---
    let ops_10k = exact_op_log(CORPUS_SEED, BOOTSTRAP_10K_OPS);
    let ops_100k = exact_op_log(CORPUS_SEED, BOOTSTRAP_100K_OPS);
    let apply_update_base_ops = exact_op_log(CORPUS_SEED.wrapping_add(1), APPLY_UPDATE_BASE_OPS);
    assert_eq!(ops_10k.len(), BOOTSTRAP_10K_OPS);
    assert_eq!(ops_100k.len(), BOOTSTRAP_100K_OPS);

    eprintln!("tuning <= 8 KiB apply_update payloads (untimed, once per candidate)...");
    let loro_apply_payload =
        build_apply_update_payload::<LoroCollabEngine>(&apply_update_base_ops, APPLY_UPDATE_TARGET_BYTES_MAX);
    let yrs_apply_payload =
        build_apply_update_payload::<YrsCollabEngine>(&apply_update_base_ops, APPLY_UPDATE_TARGET_BYTES_MAX);
    let loro_update_bytes = loro_apply_payload.update.len();
    let yrs_update_bytes = yrs_apply_payload.update.len();

    // --- Metric 1: apply <= 8 KiB update, interleaved. ---
    eprintln!("measuring apply_update_ms ({WARMUP} warmup + {SAMPLES} samples, interleaved)...");
    let (loro_apply_update_ms, yrs_apply_update_ms) = sample_alternating(
        WARMUP,
        SAMPLES,
        || {
            let mut sink = LoroCollabEngine::load(&loro_apply_payload.base_snapshot)
                .unwrap_or_else(|error| fatal("reload loro apply_update sink", error));
            let start = Instant::now();
            sink.import_update(&loro_apply_payload.update)
                .unwrap_or_else(|error| fatal("loro import_update", error));
            start.elapsed().as_secs_f64() * 1000.0
        },
        || {
            let mut sink = YrsCollabEngine::load(&yrs_apply_payload.base_snapshot)
                .unwrap_or_else(|error| fatal("reload yrs apply_update sink", error));
            let start = Instant::now();
            sink.import_update(&yrs_apply_payload.update)
                .unwrap_or_else(|error| fatal("yrs import_update", error));
            start.elapsed().as_secs_f64() * 1000.0
        },
    );
    write_partial(
        "apply_update_ms",
        &serde_json::json!({"loro": loro_apply_update_ms, "yrs-yjs": yrs_apply_update_ms}),
    );

    // --- Metric 2: 10k-op bootstrap, interleaved. ---
    eprintln!("measuring bootstrap_10k_ops_ms ({WARMUP} warmup + {SAMPLES} samples, interleaved)...");
    let (loro_bootstrap_10k_ms, yrs_bootstrap_10k_ms) = sample_alternating(
        WARMUP,
        SAMPLES,
        || {
            let start = Instant::now();
            let mut engine = LoroCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops_10k).unwrap_or_else(|error| fatal("loro 10k-op bootstrap", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&engine);
            elapsed
        },
        || {
            let start = Instant::now();
            let mut engine = YrsCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops_10k).unwrap_or_else(|error| fatal("yrs 10k-op bootstrap", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&engine);
            elapsed
        },
    );
    write_partial(
        "bootstrap_10k_ops_ms",
        &serde_json::json!({"loro": loro_bootstrap_10k_ms, "yrs-yjs": yrs_bootstrap_10k_ms}),
    );

    // --- Metric 3: 100k-op bootstrap, interleaved. See `bootstrap_100k_warmup`/`_samples` doc
    // comments for why this uses the unclamped sampler with env-overridable counts instead of
    // the standard >=30-sample `sample_alternating`. ---
    let b100k_warmup = bootstrap_100k_warmup();
    let b100k_samples = bootstrap_100k_samples();
    eprintln!("measuring bootstrap_100k_ops_ms ({b100k_warmup} warmup + {b100k_samples} samples, interleaved)...");
    // Captures the *last* built engine from each closure (rather than discarding it via
    // black_box, as every other timed closure in this file does) so the snapshot/restore step
    // right below can reuse it as its 100k-op source document instead of paying for a second,
    // separately-timed-away 100k-op bootstrap per candidate -- material at this scale given the
    // measured yrs-yjs superlinear cost (see the module docs and the delivery report).
    let mut loro_100k_last_built: Option<LoroCollabEngine> = None;
    let mut yrs_100k_last_built: Option<YrsCollabEngine> = None;
    let (loro_bootstrap_100k_ms, yrs_bootstrap_100k_ms) = sample_alternating_unclamped(
        b100k_warmup,
        b100k_samples,
        || {
            let start = Instant::now();
            let mut engine = LoroCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops_100k).unwrap_or_else(|error| fatal("loro 100k-op bootstrap", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            loro_100k_last_built = Some(engine);
            elapsed
        },
        || {
            let start = Instant::now();
            let mut engine = YrsCollabEngine::new_empty(REPLICA_SEED);
            apply_all(&mut engine, &ops_100k).unwrap_or_else(|error| fatal("yrs 100k-op bootstrap", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            yrs_100k_last_built = Some(engine);
            elapsed
        },
    );
    write_partial(
        "bootstrap_100k_ops_ms",
        &serde_json::json!({"loro": loro_bootstrap_100k_ms, "yrs-yjs": yrs_bootstrap_100k_ms, "warmup_used": b100k_warmup, "samples_used": b100k_samples}),
    );

    // --- Supplemental: snapshot size + restore (load) time for the 100k-op document. Reuses the
    // last engine built by the bootstrap_100k_ops_ms loop just above (falling back to building
    // one fresh only if that loop ran zero iterations) instead of a second from-scratch 100k-op
    // bootstrap per candidate. ---
    eprintln!(
        "exporting 100k-op snapshots for restore-time measurement (from the last bootstrap_100k_ops_ms sample)..."
    );
    let loro_100k_source = loro_100k_last_built.unwrap_or_else(|| {
        let mut engine = LoroCollabEngine::new_empty(REPLICA_SEED);
        apply_all(&mut engine, &ops_100k)
            .unwrap_or_else(|error| fatal("loro 100k-op source bootstrap for snapshot", error));
        engine
    });
    let loro_100k_snapshot = loro_100k_source
        .export_snapshot()
        .unwrap_or_else(|error| fatal("export loro 100k-op snapshot", error));
    write_partial(
        "snapshot_bytes_100k_ops",
        &serde_json::json!({"loro": loro_100k_snapshot.len()}),
    );

    let yrs_100k_source = yrs_100k_last_built.unwrap_or_else(|| {
        let mut engine = YrsCollabEngine::new_empty(REPLICA_SEED);
        apply_all(&mut engine, &ops_100k)
            .unwrap_or_else(|error| fatal("yrs 100k-op source bootstrap for snapshot", error));
        engine
    });
    let yrs_100k_snapshot = yrs_100k_source
        .export_snapshot()
        .unwrap_or_else(|error| fatal("export yrs 100k-op snapshot", error));
    write_partial(
        "snapshot_bytes_100k_ops",
        &serde_json::json!({"loro": loro_100k_snapshot.len(), "yrs-yjs": yrs_100k_snapshot.len()}),
    );

    eprintln!("measuring restore_100k_ops_ms ({b100k_warmup} warmup + {b100k_samples} samples, interleaved)...");
    let (loro_restore_ms, yrs_restore_ms) = sample_alternating_unclamped(
        b100k_warmup,
        b100k_samples,
        || {
            let start = Instant::now();
            let engine = LoroCollabEngine::load(&loro_100k_snapshot)
                .unwrap_or_else(|error| fatal("loro 100k-op restore", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&engine);
            elapsed
        },
        || {
            let start = Instant::now();
            let engine =
                YrsCollabEngine::load(&yrs_100k_snapshot).unwrap_or_else(|error| fatal("yrs 100k-op restore", error));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&engine);
            elapsed
        },
    );

    write_partial(
        "restore_100k_ops_ms",
        &serde_json::json!({"loro": loro_restore_ms, "yrs-yjs": yrs_restore_ms}),
    );

    // --- Metric 4: 100k-op peak memory, one dedicated subprocess per candidate. ---
    eprintln!("measuring peak_memory_100k_ops_bytes (one dedicated subprocess per candidate)...");
    let loro_peak_memory_bytes = spawn_peak_memory_child("loro");
    write_partial(
        "peak_memory_100k_ops_bytes",
        &serde_json::json!({"loro": loro_peak_memory_bytes}),
    );
    let yrs_peak_memory_bytes = spawn_peak_memory_child("yrs-yjs");
    write_partial(
        "peak_memory_100k_ops_bytes",
        &serde_json::json!({"loro": loro_peak_memory_bytes, "yrs-yjs": yrs_peak_memory_bytes}),
    );

    let output = serde_json::json!({
        "schema_note": "Field names/shapes are aligned to docs/schemas/sylvode-flow-benchmark-result-v1.schema.json \
             wherever this round has data for them, but this document is NOT a valid instance of that schema: \
             the browser-side budgets/measurements/environment fields (browser_engine_bundle, cold_start_ms, \
             environment.browser/node_version/bun_version/bundle_build_tool/gzip_tool) are out of scope for this \
             round and are omitted rather than fabricated. raw_samples is also inlined here (an array) rather than \
             indirected through the schema's `artifact` {path, sha256} shape, since this document itself already \
             is the raw evidence file.",
        "schema_version_target": "sylvode.flow.benchmark-result.v1",
        "generated_at": generated_at,
        "environment": environment,
        "corpus": {
            "seed": CORPUS_SEED,
            "apply_update_base_seed": CORPUS_SEED.wrapping_add(1),
            "generator": "collab_shared::fixture::page_ops (Page/Block fixed model)",
            "bootstrap_10k_ops": BOOTSTRAP_10K_OPS,
            "bootstrap_100k_ops": BOOTSTRAP_100K_OPS,
            "apply_update_base_ops": APPLY_UPDATE_BASE_OPS,
            "apply_update_target_bytes_max": APPLY_UPDATE_TARGET_BYTES_MAX,
        },
        "alternation": {
            "strategy": "sample_alternating: work_a() then work_b() every iteration, for both warmup and measured samples -- never candidate A run to completion before candidate B starts",
            "first_candidate_per_iteration": "loro",
        },
        "isolation_path": "P1 native in-process; this binary never imports collab_shared::isolation. The only subprocess spawn is a plain std::process::Command re-invocation of this same binary for the one-shot peak-memory figure (see run_peak_memory_child / spawn_peak_memory_child doc comments) -- not the ADR-0014 fork/mmap isolation host.",
        "budgets": {
            "apply_update_ms_p95_max": APPLY_UPDATE_MS_P95_MAX,
            "bootstrap_10k_ops_ms_p95_max": BOOTSTRAP_10K_MS_P95_MAX,
            "bootstrap_100k_ops_ms_p95_max": BOOTSTRAP_100K_MS_P95_MAX,
            "peak_memory_100k_ops_bytes_max": PEAK_MEMORY_100K_BYTES_MAX,
        },
        "candidates": {
            "loro": candidate_output(
                "loro",
                collab_loro_spike::metadata().rust_engine_version,
                &loro_apply_update_ms,
                loro_update_bytes,
                &loro_bootstrap_10k_ms,
                &loro_bootstrap_100k_ms,
                &loro_restore_ms,
                loro_100k_snapshot.len(),
                loro_peak_memory_bytes,
            ),
            "yrs-yjs": candidate_output(
                "yrs-yjs",
                collab_yrs_yjs_spike::metadata().rust_engine_version,
                &yrs_apply_update_ms,
                yrs_update_bytes,
                &yrs_bootstrap_10k_ms,
                &yrs_bootstrap_100k_ms,
                &yrs_restore_ms,
                yrs_100k_snapshot.len(),
                yrs_peak_memory_bytes,
            ),
        },
        "loro_dual_node_caveat": "Loro's tree CRDT keeps two distinct physical TreeID nodes when two \
            replicas concurrently CreateNode under the same logical id (verified deterministic/stable in \
            tests/corpus.rs::same_key_nested_container_creation_resolves_via_map_lww); this adapter's semantic \
            projection folds them to one logical id, so 'same projected node count' does NOT imply 'same engine \
            state size'. The bootstrap_10k_ops/bootstrap_100k_ops/apply_update workloads measured in this \
            document are single-replica, sequential-apply logs with no concurrent duplicate creates, so this \
            effect is not present in the snapshot_bytes/peak_memory_100k_ops_bytes numbers above -- it would \
            need to be accounted for separately in any future benchmark of merge-heavy or concurrent-create-heavy \
            workloads, where a Loro snapshot/memory figure and a yrs-yjs one for the 'same' logical document \
            would not be comparable at face value.",
        "not_measured_this_round": ["browser_engine_bundle_gzip_bytes_max", "cold_start_ms_p95_max"],
    });

    match serde_json::to_string_pretty(&output) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("error: failed to serialize benchmark output: {error}");
            std::process::exit(1);
        }
    }
}

// `_10k`/`_100k` op-count suffixes intentionally share every character but the count.
#[allow(clippy::similar_names, clippy::too_many_arguments)]
fn candidate_output(
    candidate: &str,
    engine_version: &str,
    apply_update_ms: &SampleStats,
    update_bytes: usize,
    bootstrap_10k_ops_ms: &SampleStats,
    bootstrap_100k_ops_ms: &SampleStats,
    restore_100k_ops_ms: &SampleStats,
    snapshot_100k_ops_bytes: usize,
    peak_memory_100k_ops_bytes: u64,
) -> serde_json::Value {
    let apply_update_pass = apply_update_ms.p95 <= APPLY_UPDATE_MS_P95_MAX;
    let bootstrap_10k_pass = bootstrap_10k_ops_ms.p95 <= BOOTSTRAP_10K_MS_P95_MAX;
    let bootstrap_100k_pass = bootstrap_100k_ops_ms.p95 <= BOOTSTRAP_100K_MS_P95_MAX;
    let peak_memory_pass = peak_memory_100k_ops_bytes <= PEAK_MEMORY_100K_BYTES_MAX;
    let all_pass = apply_update_pass && bootstrap_10k_pass && bootstrap_100k_pass && peak_memory_pass;

    serde_json::json!({
        "candidate": candidate,
        "engine_version": engine_version,
        "measurements": {
            "apply_update_ms": apply_update_ms,
            "bootstrap_10k_ops_ms": bootstrap_10k_ops_ms,
            "bootstrap_100k_ops_ms": bootstrap_100k_ops_ms,
            "peak_memory_100k_ops_bytes": peak_memory_100k_ops_bytes,
        },
        "supplemental_metrics": {
            "update_bytes": update_bytes,
            "snapshot_bytes_100k_ops": snapshot_100k_ops_bytes,
            "restore_100k_ops_ms": restore_100k_ops_ms,
        },
        "budget_checks": {
            "apply_update_ms_p95_max": status_str(apply_update_pass),
            "bootstrap_10k_ops_ms_p95_max": status_str(bootstrap_10k_pass),
            "bootstrap_100k_ops_ms_p95_max": status_str(bootstrap_100k_pass),
            "peak_memory_100k_ops_bytes_max": status_str(peak_memory_pass),
        },
        "benchmark_budgets_met_native_subset": status_str(all_pass),
    })
}
