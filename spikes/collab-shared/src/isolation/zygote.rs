//! Parent/zygote-side orchestration: the single-thread precondition check, `fork()`, the wall
//! watchdog, `wait4` reaping, and assembling the raw observation `termination.rs` classifies.
//!
//! ADR-0014 section 10 requires four things be true of the calibration tool itself (not just
//! measured as data): the zygote must assert `/proc/self/task == 1` before every `fork()`; the
//! shared page must be reset per case and cases run serially; CPU comparisons must use a
//! window-delta baseline; and `fork`/shared-page/child failures must become explicit outcomes,
//! never silently skipped. This module is where all four are enforced.

use std::mem::MaybeUninit;
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use crate::isolation::child::{self, ChildReport, FaultMode, FixtureKind};
use crate::isolation::frame::{self, FrameDecodeError};
use crate::isolation::shared_page::{SharedPage, SharedPageError, TIMESTAMP_UNSET};
use crate::isolation::termination::{self, RawObservation, TerminationEvent};
use crate::result::IsolationOracle;

/// Reasons a case could not be run to a termination-classifiable conclusion at all.
///
/// Per ADR-0014 section 10: "都必须成为 calibration outcome，不得静默跳过" — every variant here is
/// something the calibration binary prints and counts, never something it swallows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFailureReason {
    /// `/proc/self/task` reported a task count other than 1 immediately before the planned
    /// `fork()`; the zygote refused to fork.
    ZygoteNotSingleThreaded {
        task_count: usize,
    },
    SharedPageMapFailed {
        errno: i32,
    },
    PipeFailed {
        errno: i32,
    },
    ForkFailed {
        errno: i32,
    },
    Wait4Failed {
        errno: i32,
    },
}

/// Everything the zygote observed about one case that ran to a termination-classifiable
/// conclusion (which includes being killed — "ran" does not mean "completed cleanly").
#[derive(Debug, Clone)]
pub struct CaseObservation {
    pub generation: u64,
    pub fixture: FixtureKind,
    pub fault_mode: FaultMode,
    pub expected_oracle: IsolationOracle,
    pub raw: RawObservation,
    pub raw_wait_status: i32,
    pub termination_causes: Vec<TerminationEvent>,
    pub primary_cause: IsolationOracle,
    pub verdict_passed: bool,
    pub allocated_active_peak_bytes: u64,
    pub allocated_attempted_peak_bytes: u64,
    pub rss_peak_bytes: u64,
    pub sigprof_armed: bool,
    pub cpu_ms: f64,
    pub wall_ms: f64,
    pub fork_overhead_ms: f64,
    pub total_wall_ms: f64,
    pub child_report: Option<ChildReport>,
    pub report_decode_error: Option<ReportDecodeError>,
}

/// Why the child's result frame could not be turned into a [`ChildReport`].
///
/// Either the frame protocol itself rejected the bytes (bad CRC, truncated, oversized), or the
/// frame was valid but its payload was not the expected JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportDecodeError {
    Frame(FrameDecodeError),
    NotValidJson,
}

#[derive(Debug, Clone)]
pub enum CaseRunOutcome {
    Ran(Box<CaseObservationInner>),
    Failure(CaseFailureReason),
}

/// Boxed because [`CaseObservation`] is not `Copy` (it owns a `Vec`/`String`); this thin wrapper
/// lets [`CaseRunOutcome`] stay small while still being easy to pattern-match at call sites.
///
/// Named `Inner` only to avoid a name collision with [`CaseObservation`] itself in the `Box`.
pub type CaseObservationInner = CaseObservation;

fn monotonic_now_ns() -> i64 {
    let mut ts = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `CLOCK_MONOTONIC` is always supported on Linux; `ts` is a valid, properly aligned
    // `timespec` out-pointer for the duration of this call.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, ts.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: `clock_gettime` returned 0 (success), so `ts` was fully initialized by the kernel.
    let ts = unsafe { ts.assume_init() };
    ts.tv_sec.saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec)
}

fn last_errno() -> i32 {
    // SAFETY: `errno` is thread-local libc state; reading it immediately after the failing call
    // that set it, before any other libc call can clobber it, is the standard safe pattern.
    unsafe { *libc::__errno_location() }
}

/// ADR-0014 section 10, self-assertion 1: "zygote 每次 fork 前断言 `/proc/self/task` 恰为 1".
///
/// Uses the real task count from the kernel's own bookkeeping rather than any in-process
/// thread-spawn ordering assumption — the ADR explicitly warns that "started before any thread
/// creation" does not by itself prove single-threadedness (a C library or engine's global init
/// could have started a helper thread).
#[must_use]
pub fn current_task_count() -> Option<usize> {
    std::fs::read_dir("/proc/self/task").ok().map(Iterator::count)
}

/// Runs one case: assert single-threaded, reset the shared page under a fresh generation nonce,
/// `fork()`, run the child's fixture workload, wall-watchdog it, reap it, and classify the
/// outcome.
///
/// Cases are run strictly serially by this function's own blocking, synchronous structure — there
/// is no concurrency here (ADR-0014 section 10, self-assertion 2).
#[must_use]
pub fn run_one_case(
    generation: u64,
    fixture: FixtureKind,
    fault_mode: FaultMode,
    wall_budget_ms: u64,
) -> CaseRunOutcome {
    let Some(task_count) = current_task_count() else {
        // `/proc/self/task` is unreadable — cannot prove single-threadedness, so refuse to fork
        // rather than assume it. `usize::MAX` makes the failure visually distinct from a real
        // observed count in the printed outcome.
        return CaseRunOutcome::Failure(CaseFailureReason::ZygoteNotSingleThreaded { task_count: usize::MAX });
    };
    if task_count != 1 {
        return CaseRunOutcome::Failure(CaseFailureReason::ZygoteNotSingleThreaded { task_count });
    }

    let page = match SharedPage::map() {
        Ok(page) => page,
        Err(SharedPageError::MmapFailed { errno }) => {
            return CaseRunOutcome::Failure(CaseFailureReason::SharedPageMapFailed { errno });
        }
    };
    page.layout().reset_for_next_case(generation);

    let mut fds: [i32; 2] = [-1, -1];
    // SAFETY: `fds` is a valid 2-element `i32` array; `pipe(2)` writes both ends into it.
    let pipe_result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if pipe_result != 0 {
        return CaseRunOutcome::Failure(CaseFailureReason::PipeFailed { errno: last_errno() });
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    page.layout()
        .fork_decided_at_ns
        .store(monotonic_now_ns(), std::sync::atomic::Ordering::SeqCst);
    let fork_start_instant = Instant::now();

    // SAFETY: `fork()` has no arguments and no preconditions beyond being called from a
    // single-threaded process (asserted above via `/proc/self/task`), which the ADR notes is
    // what makes this safe: at the fork point there is exactly one thread, so no lock held by
    // another thread can be left in an inconsistent state in the child.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let errno = last_errno();
        // SAFETY: `read_fd`/`write_fd` are the two valid fds just created by `pipe(2)` above,
        // and closing both here (fork failed, no child exists to own either end) does not
        // double-close anything else in this process.
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return CaseRunOutcome::Failure(CaseFailureReason::ForkFailed { errno });
    }

    if pid == 0 {
        // SAFETY: `read_fd` is this (child) process's copy of the pipe's read end, which the
        // child never uses; closing it here is the standard "close the unused end" pipe pattern.
        unsafe {
            libc::close(read_fd);
        }
        // SAFETY: `write_fd` is this (child) process's copy of the pipe's write end, a valid
        // open fd this process now uniquely owns responsibility for (the parent already closed
        // its own copy below, and the child closed its read-end copy just above) — exactly the
        // precondition `OwnedFd::from_raw_fd` requires.
        let owned_write = unsafe { OwnedFd::from_raw_fd(write_fd) };
        child::run_child(fixture, fault_mode, page.layout(), owned_write);
        // `run_child` never returns (it always calls `libc::_exit`).
    }

    // SAFETY: `write_fd` is the parent's copy of the pipe's write end, which the parent never
    // uses (only the child writes); closing it here lets the child's `close`/`_exit` be the only
    // remaining writer, so the parent's `read`/`poll` on `read_fd` sees EOF once the child is
    // truly done (or gone).
    unsafe {
        libc::close(write_fd);
    }

    let (frame_bytes, wall_watchdog_fired) = read_with_wall_watchdog(read_fd, pid, wall_budget_ms);
    // SAFETY: `read_fd` is the parent's copy of the pipe's read end, only closed here after the
    // watchdog loop is done reading from it.
    unsafe {
        libc::close(read_fd);
    }

    let mut status: i32 = 0;
    // SAFETY: `libc::rusage` is `#[repr(C)]` and all-zero bytes are a valid (if meaningless
    // until populated) bit pattern for its plain-integer/`timeval` fields.
    let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `pid` is the child pid returned by the successful `fork()` above; `&mut status`
    // and `&mut rusage` are valid out-pointers for the duration of this call.
    let waited_pid = unsafe { libc::wait4(pid, &raw mut status, 0, &raw mut rusage) };
    let total_wall_ms = fork_start_instant.elapsed().as_secs_f64() * 1000.0;
    if waited_pid < 0 {
        return CaseRunOutcome::Failure(CaseFailureReason::Wait4Failed { errno: last_errno() });
    }

    let wifexited = libc::WIFEXITED(status);
    let wexitstatus = if wifexited { libc::WEXITSTATUS(status) } else { 0 };
    let wifsignaled = libc::WIFSIGNALED(status);
    let wtermsig = if wifsignaled { libc::WTERMSIG(status) } else { 0 };

    let frame_decode_result: Result<ChildReport, ReportDecodeError> = frame::decode(&frame_bytes)
        .map_err(ReportDecodeError::Frame)
        .and_then(|payload| {
            serde_json::from_slice::<ChildReport>(&payload).map_err(|_| ReportDecodeError::NotValidJson)
        });

    let frame_integrity_ok = if wifsignaled {
        true
    } else if wifexited && wexitstatus == 0 {
        frame_decode_result.is_ok()
    } else {
        true
    };

    let parent_rusage_cpu_ms = rusage_cpu_ms(&rusage);
    let child_self_measured_cpu_ms = frame_decode_result.as_ref().ok().map(|report| report.window_cpu_ms);

    let layout = page.layout();
    let raw = RawObservation {
        wifexited,
        wexitstatus,
        wifsignaled,
        wtermsig,
        wall_watchdog_fired,
        parent_rusage_cpu_ms,
        child_self_measured_cpu_ms,
        memory_cause_flag: layout.memory_cause_flag.load(std::sync::atomic::Ordering::SeqCst) != 0,
        address_space_backstop_flag: layout
            .address_space_backstop_flag
            .load(std::sync::atomic::Ordering::SeqCst)
            != 0,
        meter_tampered_flag: layout.meter_tampered_flag.load(std::sync::atomic::Ordering::SeqCst) != 0,
        frame_integrity_ok,
    };

    let termination_causes = termination::resolve(&raw);
    let primary_cause = termination::primary_cause(&termination_causes);
    let expected_oracle = fixture.expected_oracle();
    let verdict_passed = termination::verdict_passed(&raw, expected_oracle, primary_cause);

    let fork_decided_at_ns = layout.fork_decided_at_ns.load(std::sync::atomic::Ordering::SeqCst);
    let child_window_open_at_ns = layout.child_window_open_at_ns.load(std::sync::atomic::Ordering::SeqCst);
    let fork_overhead_ms = if fork_decided_at_ns != TIMESTAMP_UNSET && child_window_open_at_ns != TIMESTAMP_UNSET {
        // `fork()` overhead is at most a handful of milliseconds in practice, so this nanosecond
        // delta is many orders of magnitude under f64's exact-integer range.
        #[allow(clippy::cast_precision_loss)]
        let overhead_ns = (child_window_open_at_ns - fork_decided_at_ns) as f64;
        overhead_ns / 1_000_000.0
    } else {
        0.0
    };

    let cpu_ms = child_self_measured_cpu_ms.unwrap_or(parent_rusage_cpu_ms);
    let wall_ms = (total_wall_ms - fork_overhead_ms).max(0.0);

    let observation = CaseObservation {
        generation,
        fixture,
        fault_mode,
        expected_oracle,
        raw,
        raw_wait_status: status,
        termination_causes,
        primary_cause,
        verdict_passed,
        allocated_active_peak_bytes: layout
            .allocated_active_peak_bytes
            .load(std::sync::atomic::Ordering::SeqCst),
        allocated_attempted_peak_bytes: layout
            .allocated_attempted_peak_bytes
            .load(std::sync::atomic::Ordering::SeqCst),
        rss_peak_bytes: layout.rss_peak_bytes.load(std::sync::atomic::Ordering::SeqCst),
        sigprof_armed: layout.sigprof_armed_flag.load(std::sync::atomic::Ordering::SeqCst) != 0,
        cpu_ms,
        wall_ms,
        fork_overhead_ms,
        total_wall_ms,
        child_report: frame_decode_result.as_ref().ok().cloned(),
        report_decode_error: frame_decode_result.err(),
    };

    CaseRunOutcome::Ran(Box::new(observation))
}

// `tv_sec`/`tv_usec` are process CPU-time accumulators; even a long-running calibration case
// stays many orders of magnitude under f64's exact-integer range (2^52 seconds is far beyond any
// process lifetime), so these conversions never lose precision.
#[allow(clippy::cast_precision_loss)]
fn rusage_cpu_ms(rusage: &libc::rusage) -> f64 {
    let user_ms = (rusage.ru_utime.tv_sec as f64).mul_add(1000.0, rusage.ru_utime.tv_usec as f64 / 1000.0);
    let sys_ms = (rusage.ru_stime.tv_sec as f64).mul_add(1000.0, rusage.ru_stime.tv_usec as f64 / 1000.0);
    user_ms + sys_ms
}

/// Reads the child's result frame off `read_fd` until EOF, `SIGKILL`ing `child_pid` if no EOF
/// arrives within `wall_budget_ms` (ADR-0014's independent wall watchdog). Returns the raw bytes
/// read (possibly a truncated/partial frame, or none) and whether the watchdog had to fire.
fn read_with_wall_watchdog(read_fd: i32, child_pid: i32, wall_budget_ms: u64) -> (Vec<u8>, bool) {
    let deadline = Instant::now() + Duration::from_millis(wall_budget_ms);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut wall_watchdog_fired = false;

    loop {
        if !wall_watchdog_fired && Instant::now() >= deadline {
            // SAFETY: `child_pid` is the pid returned by the successful `fork()` this watchdog
            // is guarding; `SIGKILL` is always a valid signal to send to one's own child.
            unsafe {
                libc::kill(child_pid, libc::SIGKILL);
            }
            wall_watchdog_fired = true;
        }

        let timeout_ms: i32 = if wall_watchdog_fired {
            50
        } else {
            let remaining = deadline.saturating_duration_since(Instant::now()).as_millis();
            i32::try_from(remaining).unwrap_or(i32::MAX)
        };

        let mut pollfd = libc::pollfd {
            fd: read_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `&mut pollfd` is a valid single-element `pollfd` array pointer with a valid
        // open fd; `timeout_ms` is a non-negative (or `-1`, not used here) millisecond timeout.
        let poll_result = unsafe { libc::poll(&raw mut pollfd, 1, timeout_ms) };

        if poll_result > 0 {
            // SAFETY: `chunk.as_mut_ptr()` points at a valid, writable buffer of `chunk.len()`
            // bytes for the duration of this call, matching the `count` argument passed.
            let read_result = unsafe { libc::read(read_fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if read_result > 0 {
                #[allow(clippy::cast_sign_loss)]
                let read_len = read_result as usize;
                // `read(2)`'s contract guarantees `read_len <= chunk.len()` (it was called with
                // `chunk.len()` as the count), but `.get(..)` avoids a panicking index
                // expression rather than relying on that contract holding at this call site too
                // (`indexing_slicing` is a deny-level workspace lint); an out-of-contract kernel
                // return would just silently not extend the buffer for this read.
                if let Some(read_slice) = chunk.get(..read_len) {
                    buffer.extend_from_slice(read_slice);
                }
            } else if read_result == 0 {
                break; // EOF: the child (or its inherited fd table, after being killed) closed the write end.
            } else if last_errno() != libc::EINTR {
                break;
            }
        } else if poll_result == 0 {
            if wall_watchdog_fired {
                // Already killed and gave it one more bounded grace-period poll; stop waiting.
                break;
            }
        } else if last_errno() != libc::EINTR {
            break;
        }

        if buffer.len() > frame::MAX_FRAME_PAYLOAD_BYTES + 8 {
            // Defensive cap: never buffer more than one frame's worth plus header, regardless of
            // what a misbehaving child writes.
            break;
        }
    }

    (buffer, wall_watchdog_fired)
}

impl FixtureKind {
    /// What termination oracle this fixture *should* produce when the isolation host is working
    /// correctly. Used as `expected_oracle` for both the fixture's own verdict and every fault
    /// mode applied on top of [`FixtureKind::CpuHog`] — the fault modes exist specifically to
    /// prove the host still does *not* report this expected value when the meter is sabotaged.
    #[must_use]
    pub const fn expected_oracle(self) -> IsolationOracle {
        match self {
            Self::Completed | Self::AllocationBaseline => IsolationOracle::Completed,
            Self::CpuHog => IsolationOracle::CpuCeiling,
            Self::WallHog => IsolationOracle::WallCeiling,
            Self::MemoryHog => IsolationOracle::MemoryCeiling,
            Self::AddressSpaceBackstop => IsolationOracle::AddressSpaceBackstop,
            Self::Crashed => IsolationOracle::Crashed,
        }
    }
}
