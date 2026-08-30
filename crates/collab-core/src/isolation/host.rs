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

use super::wire::{self, MAX_RESPONSE_PAYLOAD_BYTES, Outcome};

/// `decode_apply_cpu_ms_max` (`contracts/limits-v1.md`); documented here for
/// [`IsolatedApplyError::CpuCeiling`]'s reported `observed`/`limit` pair.
pub const DECODE_APPLY_CPU_MS_MAX: u64 = 50;
/// `decode_apply_wall_ms_max`; the independent watchdog deadline this module enforces itself.
pub const DECODE_APPLY_WALL_MS_MAX: u64 = 100;
/// `isolated_apply_memory_bytes_max`.
pub const ISOLATED_APPLY_MEMORY_BYTES_MAX: u64 = 134_217_728;

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
    let worker_path = worker_binary_path()?;

    let mut child = Command::new(&worker_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
        // `LoroCollabEngine::semantic_snapshot`'s `order_key_for` recomputes a child's sibling
        // position by re-listing *all* of its parent's children and linearly searching for it
        // (`engine.rs`: "let position = siblings.iter().position(...)") -- for one parent with N
        // children, building the semantic snapshot is O(n^2) in the sibling count, not O(n). A
        // base document built as one very wide, shallow "star" (one root, many direct children)
        // is legitimate CRDT content (no limit stops its *construction*; `check_snapshot`'s
        // sibling-of-limits checks are only enforced downstream inside the isolated worker, which
        // is the whole point of this isolation host existing) and forces the worker's metered
        // window to spend real CPU proportional to that O(n^2) cost during `semantic_snapshot`,
        // regardless of what the trivial update on top of it contains. Deliberately wide/shallow
        // (depth 1), not a long chain: an earlier version of this fixture built a long linear
        // chain instead and reliably crashed the *test* process itself with a native stack
        // overflow well before reaching a large enough N -- `LoroDoc`'s own tree bookkeeping
        // (unrelated to this crate's code) recurses per level for a linear chain's construction
        // and/or drop. A wide star has no such depth to recurse through.
        const SIBLING_COUNT: usize = 20_000;
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
