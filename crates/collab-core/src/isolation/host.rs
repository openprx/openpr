//! Parent-side orchestration for [`isolated_apply`]: spawns `src/bin/isolated_apply_worker.rs` as
//! a fresh process per call, writes the harness-envelope request, wall-watchdogs the response with
//! an independent `SIGKILL` deadline, and classifies the outcome from the child's exit signal and
//! response frame.
//!
//! # Why a subprocess instead of `ADR-0014`'s fork-no-exec zygote
//!
//! `ADR-0014`'s isolation host (`spikes/collab-shared/src/isolation`) requires the zygote to be
//! genuinely single-threaded at every `fork()` call -- verified against `/proc/self/task`, not
//! assumed from startup ordering (`ADR-0014` section 10). That precondition cannot hold here:
//! `apps/api` is a multi-threaded Tokio server, and by the time any request reaches this write
//! path the process already has many OS threads. `fork()`ing from such a thread would either be
//! refused by that same single-threadedness check (correctly, since it is not actually true) or,
//! if attempted anyway, be unsound for the exact reason `ADR-0014` gives for why fork-only-from-
//! single-threaded *is* safe: at the fork point there must be exactly one thread, so no lock held
//! by another thread is left in an inconsistent state in the child. Pre-forking a dedicated zygote
//! before Tokio starts would satisfy this, but requires wiring into `apps/api::main` before any
//! runtime thread exists, which is outside this package's (`collab-write-path`) file scope.
//!
//! Spawning a fresh process via [`std::process::Command`] sidesteps the precondition entirely: a
//! freshly `exec`'d process image starts single-threaded regardless of how many threads its parent
//! has, because `Command::spawn` on Linux is `fork` immediately followed by `execve` -- the child
//! never runs any of the parent's code (Tokio's runtime included) between those two calls. This
//! adopts `ADR-0014`'s *online-enforcement* mechanisms over that different process-creation
//! primitive: `SIGPROF`/`setitimer(ITIMER_PROF)` for the CPU ceiling (section 1.1, in
//! `isolation::child_runtime`), a counting `GlobalAlloc` for the memory ceiling (sections 1 and 3,
//! in `isolation::alloc`), and an independent wall-clock watchdog with `SIGKILL` (section 1,
//! below).
//!
//! It does **not** carry over section 2's shared measurement page: that exists to recover
//! forensic peak/cause telemetry from a killed process for *gate evidence*. This production call
//! site has no use for that telemetry -- it only needs a correct accept/reject decision, which it
//! gets from the exit signal alone (`SIGPROF` -> [`IsolatedApplyError::CpuCeiling`], `SIGABRT`
//! (the allocator's own rejection routed through Rust's `handle_alloc_error`) ->
//! [`IsolatedApplyError::MemoryCeiling`], a watchdog-issued `SIGKILL` -> [`IsolatedApplyError::WallCeiling`]).
//!
//! Per `ADR-0014` section 8, the calibration host it comes from is "a v0.3 selection/acceptance
//! facility, not a v0.4 production safety boundary", and production enforcement is scoped to
//! "another ADR". This module *is* that production enforcement for the collab write path -- no
//! v0.3 spike evidence is cited as proof it works; it is a fresh implementation, verified by this
//! module's own tests spawning the real worker binary.

#![allow(unsafe_code)]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::CollabError;

use super::limits::DECODE_APPLY_WALL_MS_MAX;
use super::wire::{self, MAX_RESPONSE_PAYLOAD_BYTES, Outcome};

const RESPONSE_FRAME_HEADER_BYTES: usize = 8;

/// Overrides the worker binary's location; set for tests and for deployments that do not place
/// `collab-isolated-apply-worker` next to the `apps/api` executable (see [`worker_binary_path`]).
pub const WORKER_BINARY_PATH_ENV: &str = "COLLAB_ISOLATED_APPLY_WORKER_PATH";

const WORKER_BINARY_NAME: &str = "collab-isolated-apply-worker";

#[derive(Debug)]
pub struct IsolatedApplySuccess {
    pub snapshot: Vec<u8>,
}

/// Why [`isolated_apply`] did not return a candidate document.
#[derive(Debug)]
pub enum IsolatedApplyError {
    /// The worker reported an ordinary, in-band rejection: `import_update` failed to decode, or
    /// the merged result violated a `check_snapshot` structural ceiling. Identical in shape to
    /// what a direct in-process call would have produced.
    Collab(CollabError),
    /// The worker was terminated by its own `SIGPROF` timer before it could report anything else.
    CpuCeiling,
    /// This host's independent wall-clock watchdog `SIGKILL`ed the worker before it produced a
    /// response.
    WallCeiling,
    /// The worker aborted itself (`SIGABRT`): the counting allocator (or the `RLIMIT_AS`
    /// backstop) rejected an allocation that would have crossed the memory ceiling.
    MemoryCeiling,
    /// The isolation mechanism itself did not function as intended -- could not spawn the worker,
    /// its response frame/payload was corrupt, or it exited/was killed for a reason unrelated to
    /// any ceiling. Not a business rejection; callers should treat this as an internal error.
    HostFailure(String),
}

/// Resolves the isolated-apply worker binary's path.
///
/// Checked in order: [`WORKER_BINARY_PATH_ENV`] (an explicit override, used by this module's own
/// tests and available for deployments that ship the worker binary somewhere other than next to
/// `apps/api`'s executable); the directory containing the current executable (the normal
/// deployment shape -- `apps/api` and `collab-isolated-apply-worker` built and shipped together,
/// see `crates/collab-core/Cargo.toml`'s `[[bin]]` doc comment); and, for `cargo test`'s layout
/// (where a test binary's `current_exe()` resolves under `target/<profile>/deps/`, one directory
/// below where `[[bin]]` targets are actually placed), the current executable's grandparent
/// directory joined with the worker's name.
///
/// Never resolved via `$PATH`: a `$PATH`-relative lookup would let anything able to influence this
/// process's environment substitute an arbitrary executable for the one that is about to run with
/// this process's own privileges.
///
/// # Errors
/// [`IsolatedApplyError::HostFailure`] if `current_exe()` cannot be resolved, or if the worker
/// binary cannot be found at any of the candidate locations.
fn worker_binary_path() -> Result<std::path::PathBuf, IsolatedApplyError> {
    if let Ok(overridden) = std::env::var(WORKER_BINARY_PATH_ENV) {
        return Ok(std::path::PathBuf::from(overridden));
    }

    let current_exe = std::env::current_exe()
        .map_err(|err| IsolatedApplyError::HostFailure(format!("could not resolve current_exe: {err}")))?;
    let dir = current_exe
        .parent()
        .ok_or_else(|| IsolatedApplyError::HostFailure("current_exe has no parent directory".to_string()))?;

    let sibling = dir.join(WORKER_BINARY_NAME);
    if sibling.is_file() {
        return Ok(sibling);
    }

    if let Some(grandparent) = dir.parent() {
        let candidate = grandparent.join(WORKER_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(IsolatedApplyError::HostFailure(format!(
        "worker binary '{WORKER_BINARY_NAME}' not found next to '{}'; set {WORKER_BINARY_PATH_ENV} to override",
        current_exe.display()
    )))
}

/// Runs `LoroCollabEngine::load(base_snapshot)` + `import_update(update)` + `semantic_snapshot()` +
/// `check_snapshot` inside a freshly spawned, resource-ceilinged worker process, per
/// `contracts/limits-v1.md`'s "Isolated decode/apply" table.
///
/// Blocking: performs process spawn, pipe I/O, and `wait` synchronously. Callers on an async
/// executor (`apps/api`'s `flow::collab::write::hydrate_and_apply`) must run this inside
/// `tokio::task::spawn_blocking`.
///
/// # Errors
/// See [`IsolatedApplyError`].
pub fn isolated_apply(base_snapshot: &[u8], update: &[u8]) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    run_isolated_apply(base_snapshot, update, &[])
}

/// Same as [`isolated_apply`], but additionally sets `extra_env` on the *spawned child's own*
/// environment before it runs (never this process's own environment). [`isolated_apply`] always
/// calls this with an empty slice, so passing environment variables through here has no effect on
/// the production call path -- it exists so this module's own tests can drive
/// `src/bin/isolated_apply_worker.rs`'s `cfg(debug_assertions)`-gated synthetic workloads (see
/// that file's `test_injection` module) by setting `COLLAB_ISOLATION_TEST_INJECT` on the one child
/// process a test spawns, without mutating this whole test binary's process-wide environment the
/// way `WORKER_BINARY_PATH_ENV` (necessarily) does.
fn run_isolated_apply(
    base_snapshot: &[u8],
    update: &[u8],
    extra_env: &[(&str, &str)],
) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    let worker_path = worker_binary_path()?;

    let mut command = Command::new(&worker_path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|err| IsolatedApplyError::HostFailure(format!("spawn of isolated-apply worker failed: {err}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| IsolatedApplyError::HostFailure("child stdin unavailable".to_string()))?;
        let write_result = wire::write_request(&mut stdin, base_snapshot, update);
        // `stdin` drops here regardless of `write_result`, closing the write end so the child's
        // `read_exact` calls see a clean EOF rather than hanging on a half-written request.
        drop(stdin);
        write_result.map_err(|err| IsolatedApplyError::HostFailure(format!("request write failed: {err}")))?;
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| IsolatedApplyError::HostFailure("child stdout unavailable".to_string()))?;

    let read_outcome = read_response_two_phase(&mut stdout, &child);
    drop(stdout);

    let wait_status = child
        .wait()
        .map_err(|err| IsolatedApplyError::HostFailure(format!("wait on isolated-apply worker failed: {err}")))?;

    let response_bytes = match read_outcome {
        ReadOutcome::SetupPhaseTimedOut => {
            return Err(IsolatedApplyError::HostFailure(
                "isolated-apply worker exceeded its setup-phase safety timeout (spawn + base document load) \
                 before opening its metered window"
                    .to_string(),
            ));
        }
        ReadOutcome::MeteredPhaseTimedOut => return Err(IsolatedApplyError::WallCeiling),
        ReadOutcome::ExitedBeforeResponding => Vec::new(),
        ReadOutcome::Completed(bytes) => bytes,
    };

    if let Some(signal) = wait_status.signal() {
        return Err(classify_signal(signal));
    }

    if wait_status.code() != Some(0) {
        return Err(IsolatedApplyError::HostFailure(format!(
            "isolated-apply worker exited with non-zero status {:?}",
            wait_status.code()
        )));
    }

    let payload = decode_response_frame(&response_bytes)
        .map_err(|err| IsolatedApplyError::HostFailure(format!("response frame invalid: {err}")))?;
    let outcome = wire::decode_outcome(&payload)
        .map_err(|err| IsolatedApplyError::HostFailure(format!("response payload invalid: {err}")))?;

    match outcome {
        Outcome::Success { snapshot } => Ok(IsolatedApplySuccess { snapshot }),
        Outcome::Rejected(err) => Err(IsolatedApplyError::Collab(err)),
    }
}

fn classify_signal(signal: i32) -> IsolatedApplyError {
    if signal == libc::SIGPROF {
        IsolatedApplyError::CpuCeiling
    } else if signal == libc::SIGABRT {
        IsolatedApplyError::MemoryCeiling
    } else if signal == libc::SIGKILL {
        // This host only ever sends `SIGKILL` itself from the wall watchdog, which already
        // returns early (via `wall_watchdog_fired`) before this function is reached. A `SIGKILL`
        // observed here means something *else* killed the child (the kernel OOM killer, an
        // operator's `kill -9`, ...) -- a host-level failure, not a business rejection.
        IsolatedApplyError::HostFailure("isolated-apply worker received an unrecognized SIGKILL".to_string())
    } else {
        IsolatedApplyError::HostFailure(format!(
            "isolated-apply worker terminated by unexpected signal {signal}"
        ))
    }
}

/// Generous backstop for the unmetered setup phase (process spawn + `execve` + loading the base
/// document -- all of which happen before the child ever arms the CPU/memory ceilings). *Not*
/// itself an `ADR-0014`/`contracts/limits-v1.md` ceiling: sized well above realistic spawn +
/// rehydration cost (including in an unoptimized debug build) so it only ever fires for a
/// genuinely stuck child, not as a disguised version of the strict `decode_apply_wall_ms_max`
/// ceiling below. A production deployment with much larger cached documents may need to widen
/// this; see this module's own doc comment on the performance trade-off of rehydrating the whole
/// base document per call.
const SETUP_PHASE_SAFETY_TIMEOUT_MS: u64 = 5_000;

/// Written as the very first byte of every response the worker ever sends
/// (`respond_and_exit` in `src/bin/isolated_apply_worker.rs`), before the framed outcome that
/// follows it. Its only job is to let [`read_response_two_phase`] tell "still doing unmetered
/// setup" apart from "done with setup, response is now being written" -- so the *strict*
/// `decode_apply_wall_ms_max` watchdog can start counting from the moment that is actually true,
/// instead of from process spawn (which would double-count spawn + base-document-load time
/// against a ceiling `contracts/limits-v1.md` scopes to decode/apply/shape-validate only: "从
/// decode 前开始计,到 semantic diff/shape validation 完成结束").
pub const RESPONSE_MARKER_BYTE: u8 = 0xA5;

/// What [`read_response_two_phase`] observed.
enum ReadOutcome {
    /// No byte at all arrived within [`SETUP_PHASE_SAFETY_TIMEOUT_MS`]; the child was `SIGKILL`ed.
    /// Not a `decode_apply_wall_ms` ceiling hit -- this is the unmetered setup phase overrunning a
    /// generous safety backstop, a host-level problem.
    SetupPhaseTimedOut,
    /// The marker byte arrived, but the framed response that should follow it did not within
    /// [`DECODE_APPLY_WALL_MS_MAX`] of that point; the child was `SIGKILL`ed. This *is* the
    /// `decode_apply_wall_ms_max` ceiling.
    MeteredPhaseTimedOut,
    /// The child closed its stdout (exited) before writing even the marker byte -- e.g. it never
    /// got past reading the request (`main`'s `exit_without_response` paths in
    /// `src/bin/isolated_apply_worker.rs`). Not a timeout; `isolated_apply` falls through to its
    /// existing exit-code/signal classification, which will find a non-zero/signalled exit.
    ExitedBeforeResponding,
    /// The marker arrived and the rest of the response was read to EOF within its own deadline.
    /// Carries only the bytes *after* the marker (the framed response itself).
    Completed(Vec<u8>),
}

/// Reads the child's response in two phases against two different deadlines, `SIGKILL`ing `child`
/// if either fires -- see [`SETUP_PHASE_SAFETY_TIMEOUT_MS`] and [`RESPONSE_MARKER_BYTE`]'s doc
/// comments for why this is split rather than one flat deadline from process spawn. `ADR-0014`
/// section 1.1: "wall watchdog 命中...是独立的 wall 上限, 不是 CPU 上限的替代品" -- this watchdog
/// remains independent of (does not rely on) the CPU ceiling's own `SIGPROF` enforcement.
fn read_response_two_phase(stdout: &mut ChildStdout, child: &Child) -> ReadOutcome {
    let fd = stdout.as_raw_fd();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 65536];

    let setup_deadline = Instant::now() + Duration::from_millis(SETUP_PHASE_SAFETY_TIMEOUT_MS);
    let (setup_timed_out, exited_before_marker) =
        read_until(stdout, fd, child, &mut buffer, &mut chunk, setup_deadline, 1);
    if setup_timed_out {
        return ReadOutcome::SetupPhaseTimedOut;
    }
    if buffer.is_empty() {
        // `read_until`'s only other early-exit besides "reached `min_bytes`" or "deadline" is
        // EOF -- an empty buffer at this point means genuine EOF before any byte arrived.
        debug_assert!(
            exited_before_marker,
            "empty buffer implies EOF, not a bug in read_until"
        );
        return ReadOutcome::ExitedBeforeResponding;
    }

    // The single-poll-then-read pattern below can (and in practice often does) return more than
    // just the marker byte in one `read` call -- pipes have no "one write = one read" guarantee.
    // Split off the marker and keep whatever response bytes already arrived alongside it, rather
    // than discarding them.
    let response_prefix = if buffer.len() > 1 {
        buffer.split_off(1)
    } else {
        Vec::new()
    };
    buffer = response_prefix;

    let metered_deadline = Instant::now() + Duration::from_millis(DECODE_APPLY_WALL_MS_MAX);
    let (metered_timed_out, _) = read_until(stdout, fd, child, &mut buffer, &mut chunk, metered_deadline, usize::MAX);
    if metered_timed_out {
        return ReadOutcome::MeteredPhaseTimedOut;
    }

    ReadOutcome::Completed(buffer)
}

/// Appends to `buffer` (via `poll`/`read` on `fd`) until it holds at least `min_bytes`, EOF is
/// reached, or `deadline` passes. On a `deadline` hit, `SIGKILL`s `child` and keeps polling
/// briefly for whatever the killed process still manages to flush. Returns
/// `(deadline_fired, reached_eof)`.
fn read_until(
    stdout: &mut ChildStdout,
    fd: std::os::fd::RawFd,
    child: &Child,
    buffer: &mut Vec<u8>,
    chunk: &mut [u8; 65536],
    deadline: Instant,
    min_bytes: usize,
) -> (bool, bool) {
    let mut watchdog_fired = false;

    loop {
        if buffer.len() >= min_bytes {
            return (false, false);
        }

        if !watchdog_fired && Instant::now() >= deadline {
            // SAFETY: `child.id()` is this process's own live child pid (the child has not been
            // reaped yet -- `isolated_apply` calls `child.wait()` strictly after
            // `read_response_two_phase` returns); `SIGKILL` is always valid to send to one's own
            // child.
            unsafe {
                libc::kill(pid_of(child), libc::SIGKILL);
            }
            watchdog_fired = true;
        }

        let timeout_ms: i32 = if watchdog_fired {
            50
        } else {
            let remaining = deadline.saturating_duration_since(Instant::now()).as_millis();
            i32::try_from(remaining).unwrap_or(i32::MAX)
        };

        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `&mut pollfd` is a valid single-element array with a live, open fd (`stdout`
        // outlives this whole function call); `timeout_ms` is non-negative.
        let poll_result = unsafe { libc::poll(&raw mut pollfd, 1, timeout_ms) };

        match poll_result.cmp(&0) {
            std::cmp::Ordering::Greater => match stdout.read(chunk) {
                Ok(0) => return (watchdog_fired, true), // EOF: the child closed its stdout.
                Ok(read_len) => {
                    if let Some(read_slice) = chunk.get(..read_len) {
                        buffer.extend_from_slice(read_slice);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return (watchdog_fired, false),
            },
            std::cmp::Ordering::Equal => {
                if watchdog_fired {
                    // Already killed and given one more bounded grace-period poll; stop waiting.
                    return (true, false);
                }
            }
            std::cmp::Ordering::Less => {
                let last_error = std::io::Error::last_os_error();
                if last_error.kind() != std::io::ErrorKind::Interrupted {
                    return (watchdog_fired, false);
                }
            }
        }

        if buffer.len() > MAX_RESPONSE_PAYLOAD_BYTES + RESPONSE_FRAME_HEADER_BYTES + 1 {
            // Defensive cap: never buffer much more than one frame's worth plus header (plus the
            // marker byte), regardless of what a misbehaving worker writes.
            return (watchdog_fired, false);
        }
    }
}

/// `Child::id()` returns `u32`; `libc::kill` wants `libc::pid_t` (`i32` on Linux). A live child
/// pid from `fork`/`exec` is always representable in both.
fn pid_of(child: &Child) -> libc::pid_t {
    libc::pid_t::try_from(child.id()).unwrap_or(libc::pid_t::MAX)
}

/// Wraps `payload` as `[u32 length][u32 crc32][payload]`, matching `ADR-0014` section 2's frame
/// protocol (reused here for the same reason: a `SIGKILL`ed or crashed worker can leave a torn
/// write on `stdout`, and the CRC lets the parent detect that rather than trusting a partial
/// payload).
///
/// # Errors
/// [`std::io::Error`] if `payload` exceeds [`super::wire::MAX_RESPONSE_PAYLOAD_BYTES`].
pub fn encode_response_frame(payload: &[u8]) -> std::io::Result<Vec<u8>> {
    if payload.len() > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "response payload exceeds cap",
        ));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "response payload too large"))?;
    let crc = crc32fast::hash(payload);
    let mut out = Vec::with_capacity(RESPONSE_FRAME_HEADER_BYTES + payload.len());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Decodes a frame written by [`encode_response_frame`]. Every rejection here means "treat this
/// case as a host failure", never "trust a partially-parsed payload" (`ADR-0014` section 2).
fn decode_response_frame(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (Some(length_bytes), Some(crc_bytes)) = (
        bytes.get(0..4).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
        bytes.get(4..8).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
    ) else {
        return Err("frame shorter than the 8-byte header");
    };
    let declared_length = u32::from_le_bytes(length_bytes);
    let declared_crc32 = u32::from_le_bytes(crc_bytes);

    let declared_length_usize =
        usize::try_from(declared_length).map_err(|_| "declared frame length does not fit usize")?;
    if declared_length_usize > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err("declared frame length exceeds cap");
    }

    let payload_start = RESPONSE_FRAME_HEADER_BYTES;
    let payload_end = payload_start.saturating_add(declared_length_usize);
    let Some(payload) = bytes.get(payload_start..payload_end) else {
        return Err("frame truncated before declared length");
    };
    if bytes.len() > payload_end {
        return Err("trailing bytes after declared frame length");
    }

    let computed_crc32 = crc32fast::hash(payload);
    if computed_crc32 != declared_crc32 {
        return Err("frame crc32 mismatch");
    }

    Ok(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    use parking_lot::Mutex;

    /// Serializes every test in this module that reads or writes [`WORKER_BINARY_PATH_ENV`]:
    /// `cargo test` runs tests in one process across many threads by default, and process
    /// environment is process-global state, so two such tests running concurrently could observe
    /// each other's value mid-test.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Resolves the real, just-built worker binary for end-to-end tests, skipping (not failing)
    /// when it is not present -- `cargo test -p collab-core --all-features` builds every target in
    /// this package including the `[[bin]]`, so in practice it is always there, but a narrower
    /// invocation (e.g. `cargo test --lib`) would not build it.
    fn real_worker_binary_for_tests() -> Option<std::path::PathBuf> {
        for profile in ["debug", "release"] {
            let candidate = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target"))
                .join(profile)
                .join(WORKER_BINARY_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Runs `isolated_apply` with [`WORKER_BINARY_PATH_ENV`] pointed at the real worker binary,
    /// holding [`env_lock`] for the duration so no other test in this module can observe or
    /// change that env var mid-call.
    fn isolated_apply_with_real_worker(
        base_snapshot: &[u8],
        update: &[u8],
    ) -> Option<Result<IsolatedApplySuccess, IsolatedApplyError>> {
        let worker_path = real_worker_binary_for_tests()?;
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let result = isolated_apply(base_snapshot, update);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        Some(result)
    }

    // ---- Boundary tests: `decode_apply_cpu_ms_max` / `decode_apply_wall_ms_max` /
    // `isolated_apply_memory_bytes_max`, and their correct signal classification.
    //
    // These do NOT reuse `isolated_apply_with_real_worker`'s self-skip-when-missing pattern above:
    // a boundary test that silently skips when the worker binary is absent is a false green, not
    // a partial one. They instead panic via `required_debug_worker_binary_for_tests` when the
    // binary is missing.
    //
    // They also cannot use real Loro CRDT content to land on these three boundaries -- see
    // `src/bin/isolated_apply_worker.rs`'s `test_injection` module doc comment for why (real
    // content is machine-speed-dependent for CPU/wall, and the memory ceiling is not reachable at
    // all with legitimate content: anything over 128 MiB of decode-time working memory is already
    // well past the 10,000-node structural ceilings and gets rejected by `check_snapshot` first).
    // Instead, they drive that module's `cfg(debug_assertions)`-gated synthetic workloads through
    // `COLLAB_ISOLATION_TEST_INJECT`, set on the spawned child's own environment only (via
    // `run_isolated_apply`'s `extra_env`) -- never on this test process's own environment, and
    // never reachable from a `--release` build (see that module's doc comment for the full
    // guarantee this cannot leak into production).

    /// Mirrors `src/bin/isolated_apply_worker.rs`'s `test_injection::ENV_VAR`. That module lives
    /// in a separate binary crate target (`[[bin]] collab-isolated-apply-worker`), not something
    /// this library crate can `use` directly, so the literal name is duplicated here -- both sides
    /// are commented as the shared contract between them.
    const TEST_INJECT_ENV_VAR: &str = "COLLAB_ISOLATION_TEST_INJECT";

    /// Resolves the debug-profile worker binary specifically -- unlike
    /// [`real_worker_binary_for_tests`]'s debug-then-release fallback above, the synthetic
    /// workloads these boundary tests inject only exist in a `cfg(debug_assertions)` build (see
    /// `src/bin/isolated_apply_worker.rs`'s `test_injection` module doc for why); falling back to
    /// a release binary would silently run the real decode/apply/validate pipeline against
    /// meaningless trivial input instead of the intended synthetic workload, which these tests
    /// could not tell apart from a real ceiling-enforcement failure. Panics (does not skip) when
    /// the debug binary is missing -- `cargo test -p collab-core --all-features` builds it by
    /// default, and this workspace's own `scripts/verify-flow-limits-v0.4.sh` explicitly runs
    /// `cargo build -p collab-core --bin collab-isolated-apply-worker` before this test group, so
    /// a silent skip here would hide a real regression rather than report one.
    fn required_debug_worker_binary_for_tests() -> std::path::PathBuf {
        let candidate = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target"))
            .join("debug")
            .join(WORKER_BINARY_NAME);
        assert!(
            candidate.is_file(),
            "collab-isolated-apply-worker debug binary not found at {candidate:?} -- build it \
             first with `cargo build -p collab-core --bin collab-isolated-apply-worker` (a \
             --release build will not do: this test's synthetic workload injection only exists in \
             a cfg(debug_assertions) build) before running this boundary test"
        );
        candidate
    }

    /// Runs [`run_isolated_apply`] against the real debug-profile worker binary with
    /// `COLLAB_ISOLATION_TEST_INJECT=<env_value>` set on the *child's* environment only, so the
    /// worker's `test_injection` module substitutes a controllable synthetic workload for the real
    /// decode/apply/validate pipeline. `base_snapshot`/`update` content is irrelevant to every one
    /// of these workloads (none of them ever call `import_update`), so this always uses a fresh
    /// empty document and an empty update.
    fn isolated_apply_injected(env_value: &str) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
        let worker_path = required_debug_worker_binary_for_tests();
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let base_snapshot = crate::LoroCollabEngine::new_empty(1)
            .export_snapshot()
            .expect("a fresh empty document always exports");
        let result = run_isolated_apply(&base_snapshot, &[], &[(TEST_INJECT_ENV_VAR, env_value)]);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        result
    }

    #[test]
    fn decode_apply_cpu_ms_ceiling_accepts_just_under_and_kills_with_cpu_ceiling_just_over_the_boundary() {
        let ceiling_micros = super::super::DECODE_APPLY_CPU_MS_MAX * 1_000;
        // Comfortably on each side of the ceiling rather than at a literal +/-1us edge: `SIGPROF`
        // delivery/scheduling latency and the polling busy-loop's own check granularity (see
        // `test_injection::burn_cpu_micros`) both add real-world slack, so a margin narrow enough
        // to be flaky under CI contention would not actually prove anything a wider one -- which
        // still stays strictly on the correct side of `DECODE_APPLY_CPU_MS_MAX` on every real run
        // -- does not already prove.
        let under_micros = ceiling_micros - ceiling_micros / 3; // ~33ms: comfortably under 50ms.
        let over_micros = ceiling_micros * 3; // ~150ms: comfortably over 50ms, and still well
        // under what a broken (non-firing) SIGPROF would need to reach before the independent
        // 100ms wall watchdog caught it instead -- see the panic message below for why that
        // distinction matters.

        let accepted = isolated_apply_injected(&format!("cpu_burn_micros={under_micros}"));
        assert!(
            accepted.is_ok(),
            "a {under_micros}us CPU burn (under the {ceiling_micros}us decode_apply_cpu_ms_max \
             ceiling) must succeed, got {accepted:?}"
        );

        let rejected = isolated_apply_injected(&format!("cpu_burn_micros={over_micros}"));
        match rejected {
            Err(IsolatedApplyError::CpuCeiling) => {}
            other => panic!(
                "a {over_micros}us-targeted CPU burn (over the {ceiling_micros}us \
                 decode_apply_cpu_ms_max ceiling) must be killed by SIGPROF and classified as \
                 CpuCeiling specifically -- not WallCeiling (which would mean SIGPROF never fired \
                 and the independent 100ms wall watchdog caught it instead, a different \
                 mechanism entirely), not MemoryCeiling, not HostFailure -- got {other:?}"
            ),
        }
    }

    #[test]
    fn decode_apply_wall_ms_ceiling_accepts_just_under_and_kills_with_wall_ceiling_just_over_the_boundary() {
        let ceiling_millis = super::super::DECODE_APPLY_WALL_MS_MAX;
        let under_millis = ceiling_millis - ceiling_millis / 3; // ~67ms: comfortably under 100ms.
        let over_millis = ceiling_millis * 3; // ~300ms: comfortably over 100ms.

        let accepted = isolated_apply_injected(&format!("wall_sleep_millis={under_millis}"));
        assert!(
            accepted.is_ok(),
            "a {under_millis}ms sleep (under the {ceiling_millis}ms decode_apply_wall_ms_max \
             ceiling, and consuming effectively no CPU, so it cannot trip the CPU ceiling either) \
             must succeed, got {accepted:?}"
        );

        let rejected = isolated_apply_injected(&format!("wall_sleep_millis={over_millis}"));
        match rejected {
            Err(IsolatedApplyError::WallCeiling) => {}
            other => panic!(
                "a {over_millis}ms sleep (over the {ceiling_millis}ms decode_apply_wall_ms_max \
                 ceiling) must be SIGKILLed by the parent's independent wall watchdog and \
                 classified as WallCeiling specifically -- not CpuCeiling (a sleep consumes \
                 effectively no process CPU time, so SIGPROF has no budget to ever fire here), \
                 not MemoryCeiling, not HostFailure -- got {other:?}"
            ),
        }
    }

    #[test]
    fn isolated_apply_memory_bytes_ceiling_accepts_the_exact_byte_and_rejects_the_next_byte_over() {
        let ceiling = super::super::ISOLATED_APPLY_MEMORY_BYTES_MAX;
        let Ok(ceiling_usize) = usize::try_from(ceiling) else {
            panic!("ISOLATED_APPLY_MEMORY_BYTES_MAX ({ceiling}) does not fit usize on this target");
        };

        // Exact boundary, no timing involved at all: a single allocation request of precisely
        // `ceiling` bytes must succeed (the counting allocator's own check is `prospective >
        // ceiling`, strictly greater -- see `isolation::alloc::CountingAllocator::alloc`), and one
        // byte more must fail on that exact same allocation attempt.
        let accepted = isolated_apply_injected(&format!("alloc_bytes={ceiling_usize}"));
        assert!(
            accepted.is_ok(),
            "a single {ceiling_usize}-byte allocation (exactly isolated_apply_memory_bytes_max) \
             must succeed, got {accepted:?}"
        );

        let over_usize = ceiling_usize + 1;
        let rejected = isolated_apply_injected(&format!("alloc_bytes={over_usize}"));
        match rejected {
            Err(IsolatedApplyError::MemoryCeiling) => {}
            other => panic!(
                "a single {over_usize}-byte allocation (one byte over \
                 isolated_apply_memory_bytes_max) must make the counting allocator return null, \
                 aborting the process (SIGABRT) via Rust's own handle_alloc_error, and must be \
                 classified as MemoryCeiling specifically -- not CpuCeiling, not WallCeiling (a \
                 single allocation attempt takes negligible CPU or wall time either way), not \
                 HostFailure -- got {other:?}"
            ),
        }
    }

    #[test]
    fn response_frame_round_trips() {
        let payload = b"a response payload".to_vec();
        let framed = encode_response_frame(&payload).expect("encode succeeds");
        let decoded = decode_response_frame(&framed).expect("decode succeeds");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn response_frame_rejects_a_corrupted_crc() {
        let payload = b"a response payload".to_vec();
        let mut framed = encode_response_frame(&payload).expect("encode succeeds");
        if let Some(last) = framed.last_mut() {
            *last ^= 0xFF;
        }
        assert!(decode_response_frame(&framed).is_err());
    }

    #[test]
    fn response_frame_rejects_a_truncated_frame() {
        let payload = b"a response payload".to_vec();
        let mut framed = encode_response_frame(&payload).expect("encode succeeds");
        framed.truncate(framed.len().saturating_sub(3));
        assert!(decode_response_frame(&framed).is_err());
    }

    #[test]
    fn worker_binary_path_honors_the_env_override() {
        let guard = env_lock().lock();
        let previous = std::env::var(WORKER_BINARY_PATH_ENV).ok();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this runs.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, "/nonexistent/isolated-apply-worker");
        }
        let resolved = worker_binary_path();
        match previous {
            Some(value) => {
                // SAFETY: same reasoning as the `set_var` call above.
                unsafe { std::env::set_var(WORKER_BINARY_PATH_ENV, value) }
            }
            None => {
                // SAFETY: same reasoning as the `set_var` call above.
                unsafe { std::env::remove_var(WORKER_BINARY_PATH_ENV) }
            }
        }
        drop(guard);
        let path = resolved.expect("env override resolves without touching current_exe");
        assert_eq!(path, std::path::PathBuf::from("/nonexistent/isolated-apply-worker"));
    }

    // ---- End-to-end tests: spawn the real, just-built `collab-isolated-apply-worker` binary.
    //
    // Everything above this point tests pure logic (frame codec, path resolution). These tests
    // exercise the actual mechanism the parent trusts: a real `fork`+`exec`'d process, real
    // `SIGPROF`/`ITIMER_PROF` arming, real pipe I/O, real `wait4` reaping. If
    // `real_worker_binary_for_tests` cannot find the compiled binary (a narrower invocation than
    // `cargo test -p collab-core --all-features` was used), they report and skip rather than fail
    // the suite over a build-layout mismatch unrelated to this module's own logic.

    use crate::limits::DocumentLimits;
    use crate::operation::{NodeId, NodeKind, Operation};
    use crate::{CollabEngine, LoroCollabEngine};

    /// Tests in this crate must not use `println!`/`eprintln!` directly (workspace clippy denies
    /// `print_stdout`/`print_stderr`); this helper is the one place that intentionally routes a
    /// diagnostic through `eprintln!`, isolated so the `#[allow]` has the smallest possible scope
    /// (matches `spikes/collab-shared/src/isolation/shared_page.rs`'s identical helper).
    #[allow(clippy::print_stderr)]
    fn eprintln_test(message: &str) {
        eprintln!("{message}");
    }

    #[test]
    fn isolated_apply_end_to_end_accepts_a_well_formed_update() {
        let base = LoroCollabEngine::new_empty(1);
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("end to end").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test(
                "skipped: collab-isolated-apply-worker binary not found (build it with `cargo build -p collab-core --bin collab-isolated-apply-worker`)",
            );
            return;
        };
        let success = result.expect("a title-only update stays well within every ceiling");
        let reloaded = LoroCollabEngine::load(&success.snapshot).expect("returned snapshot reloads");
        assert_eq!(reloaded.title().expect("title reads"), "end to end");
    }

    #[test]
    fn isolated_apply_end_to_end_reports_decode_failed_for_garbage_update_bytes() {
        let base = LoroCollabEngine::new_empty(1);
        let base_snapshot = base.export_snapshot().expect("export succeeds");
        let garbage_update = b"this is not a loro update".to_vec();

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &garbage_update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            Err(IsolatedApplyError::Collab(CollabError::DecodeFailed { input, .. })) => {
                assert_eq!(input, "update");
            }
            other => panic!("expected Collab(DecodeFailed), got {other:?}"),
        }
    }

    #[test]
    fn isolated_apply_end_to_end_reports_text_block_chars_via_check_snapshot() {
        // Builds a base document that already has one block whose text is `text_block_chars_max +
        // 1` characters long -- over the ceiling before the (trivial) update is even applied.
        // `check_snapshot` inside the worker must catch this on the *merged* result, not just on
        // what the update itself added, which is exactly what this proves end to end (the local
        // `check_operation`-shaped construction here never runs `check_snapshot` at all).
        //
        // Deliberately a `text_block_chars` violation (one block, a very long string) rather than
        // a `document_block_count`/`container_count` violation (which would need `limit + 1 =
        // 10,001` *nodes*): `LoroCollabEngine::semantic_snapshot`'s `order_key_for` recomputes
        // each node's sibling position by re-listing and linearly searching *all* of its parent's
        // children (`engine.rs`), which -- empirically, in both debug and release builds -- makes
        // building the semantic snapshot for a document with that many nodes cost more than
        // `decode_apply_cpu_ms_max` (50ms) on its own, regardless of how the nodes are distributed
        // across parents. That is a real, pre-existing performance characteristic of `engine.rs`'s
        // `semantic_snapshot`, not a bug in this isolation host -- see this module's own doc
        // comment addendum below and this package's delivery report for the finding. A single
        // long string avoids it entirely (one node), letting this test demonstrate the
        // `check_snapshot` cross-process wiring cleanly.
        let oversized_text = "x".repeat(DocumentLimits::default().text_block_chars_max + 1);
        let mut base = LoroCollabEngine::new_empty(1);
        base.apply_operation(&Operation::CreateNode {
            id: NodeId::from("blk-1"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create succeeds");
        base.apply_operation(&Operation::InsertText {
            id: NodeId::from("blk-1"),
            index: 0,
            text: oversized_text,
        })
        .expect("insert_text succeeds at the engine level (check_snapshot, not the engine, enforces this ceiling)");
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("trivial").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            Err(IsolatedApplyError::Collab(CollabError::LimitExceeded {
                limit_kind,
                limit,
                observed,
            })) => {
                assert_eq!(limit_kind, "text_block_chars");
                assert_eq!(limit, DocumentLimits::default().text_block_chars_max as u64);
                assert!(observed > limit, "observed {observed} must exceed the limit {limit}");
            }
            other => panic!("expected Collab(LimitExceeded(text_block_chars)), got {other:?}"),
        }
    }

    #[test]
    fn isolated_apply_end_to_end_kills_a_pathologically_deep_chain_and_reports_a_ceiling() {
        // A base document built as one very wide, shallow "star" (one root, many direct children)
        // is legitimate CRDT content (no limit stops its *construction*; `check_snapshot`'s
        // sibling-count limits are only enforced downstream inside the isolated worker, which is
        // the whole point of this isolation host existing) and forces the worker's metered window
        // to spend real CPU reconstructing the whole thing in `semantic_snapshot`, regardless of
        // what the trivial update on top of it contains. Deliberately wide/shallow (depth 1), not
        // a long chain: an earlier version of this fixture built a long linear chain instead and
        // reliably crashed the *test* process itself with a native stack overflow well before
        // reaching a large enough N -- `LoroDoc`'s own tree bookkeeping (unrelated to this crate's
        // code) recurses per level for a linear chain's construction and/or drop. A wide star has
        // no such depth to recurse through.
        //
        // `SIBLING_COUNT` needs real headroom above `container_count_max`/`document_block_count_max`
        // (10,000): `semantic_snapshot`'s per-parent sibling lookup is already O(n) (not O(n^2), a
        // prior fix -- see `engine.rs::semantic_snapshot`'s doc comment), and `export_snapshot` no
        // longer runs inside the metered window at all (`src/bin/isolated_apply_worker.rs`'s
        // `decode_apply_check_and_export`), so a document merely *at* the structural ceiling now
        // finishes well under `decode_apply_cpu_ms_max` and would just be rejected by
        // `check_snapshot`'s own `container_count` check -- not by this CPU/wall ceiling test.
        // 35,000 keeps a real-machine measured margin (this repository's isolated-apply CPU-budget
        // investigation found the real worker subprocess's `SIGPROF` kill point for this shape
        // around ~29,000 nodes on the machine that measurement ran on) while staying fast enough
        // for `cargo test` to build the base document in-process (debug-mode `apply_operation` is
        // the dominant cost of this fixture, not the metered call itself).
        const SIBLING_COUNT: usize = 35_000;
        let mut base = LoroCollabEngine::new_empty(1);
        base.apply_operation(&Operation::CreateNode {
            id: NodeId::from("root"),
            parent: None,
            index: 0,
            kind: NodeKind::NavigatorNode,
        })
        .expect("create succeeds");
        for index in 0..SIBLING_COUNT {
            base.apply_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n-{index}")),
                parent: Some(NodeId::from("root")),
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .expect("create succeeds");
        }
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("trivial").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            // The `SIGPROF` timer is a process CPU-time (not wall-clock) mechanism, so it is
            // possible in principle for scheduling delay/contention on the machine running this
            // test to let wall time run out first even though the ceiling logic is armed
            // correctly -- both accepted as a pass here (either still proves the pathological
            // chain was killed and never produced a candidate document), but `CpuCeiling` is the
            // expected, primary outcome for this fixture.
            Err(IsolatedApplyError::CpuCeiling | IsolatedApplyError::WallCeiling) => {}
            other => panic!("expected the pathological chain to be killed by the CPU or wall ceiling, got {other:?}"),
        }
    }
}
