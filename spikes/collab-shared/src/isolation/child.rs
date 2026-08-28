//! Child-side logic: arming `SIGPROF` (ADR-0014 section 1.1), the fixture workloads under test,
//! the three built-in `SIGPROF` fault-injection modes (ADR-0014 section 10, self-assertion 4).
//!
//! Also reports back through the pipe/frame protocol.
//!
//! Every function here runs **after** `fork()`, inside the single-threaded child. Nothing in this
//! module may call `std::process::exit` (it would run atexit/global destructors duplicated from
//! the parent) or panic (unwinding through a forked child's partially-shared runtime state is not
//! something this host tries to make safe) — every path terminates via [`finish`], which always
//! calls `libc::_exit`.

use std::io::Write;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::isolation::frame;
use crate::isolation::shared_page::SharedPageLayout;
use crate::isolation::termination::{DECODE_APPLY_CPU_MS_MAX, ISOLATED_APPLY_MEMORY_BYTES_MAX};

/// What the child actually does under the metering window. Each variant is a deliberately simple,
/// deterministic workload chosen to exercise exactly one termination path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    /// Trivial fast workload; should finish well inside every ceiling.
    Completed,
    /// Tight CPU-bound loop; should be killed by `SIGPROF` at the 50ms ceiling.
    CpuHog,
    /// Sleeps (low CPU, high wall) past the 100ms wall ceiling.
    WallHog,
    /// Allocates through the counting allocator past the 128 MiB ceiling.
    MemoryHog,
    /// Bypasses the counting allocator with a raw `mmap` sized to exceed the 512 MiB
    /// `RLIMIT_AS` backstop.
    AddressSpaceBackstop,
    /// Deliberately raises `SIGSEGV`, unrelated to any tracked ceiling — proves the "none of the
    /// above, but it died" path resolves to `crashed`.
    Crashed,
    /// Calibration-only fixture (work package 3a question 2, `rss_allocator_ratio_bound`):
    /// allocates a fixed, representative amount through the counting allocator and finishes
    /// normally, so both `allocated_active_peak_bytes` and RSS before/after are available.
    AllocationBaseline,
}

/// Allocation size used by [`FixtureKind::AllocationBaseline`] — representative of a mid-size
/// apply, well under the 128 MiB ceiling so this fixture always completes normally.
pub const ALLOCATION_BASELINE_BYTES: usize = 16 * 1024 * 1024;

impl FixtureKind {
    #[must_use]
    pub const fn fixture_id(self) -> &'static str {
        match self {
            Self::Completed => "isolation_completed_v1",
            Self::CpuHog => "isolation_cpu_hog_v1",
            Self::WallHog => "isolation_wall_hog_v1",
            Self::MemoryHog => "isolation_memory_hog_v1",
            Self::AddressSpaceBackstop => "isolation_address_space_backstop_v1",
            Self::Crashed => "isolation_crashed_v1",
            Self::AllocationBaseline => "isolation_allocation_baseline_v1",
        }
    }
}

/// The three required `SIGPROF` fault-injection modes (ADR-0014 section 10, self-assertion 4).
/// Applied to a [`FixtureKind::CpuHog`] workload; none of them may result in `verdict: passed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    /// No fault injected — the normal arm sequence runs unmodified.
    None,
    /// Block `SIGPROF` right after arming and never unblock it.
    PersistentBlock,
    /// Block `SIGPROF`, burn CPU well past the ceiling while blocked, then **drain** the now-
    /// pending signal with a non-blocking `sigtimedwait` before restoring the mask — so the
    /// window-close disposition/mask readback alone cannot see anything wrong.
    TransientBlockThenRestore,
    /// Set the disposition to `SIG_IGN` right after arming.
    Ignore,
}

/// What the child reports back over the frame, in addition to what the shared page already
/// carries (which is the only channel guaranteed to survive the child being killed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildReport {
    pub fixture_id: String,
    /// `CLOCK_PROCESS_CPUTIME_ID` delta between arm time and window close, in milliseconds — the
    /// precise "window delta" CPU comparison ADR-0014 section 1.3 requires, as opposed to
    /// `wait4`'s cumulative-since-fork rusage.
    pub window_cpu_ms: f64,
    pub self_reported_safety_cap_triggered: bool,
    /// `VmHWM` sampled immediately after arming, before the fixture workload runs. Only
    /// meaningful for fixtures that survive to report normally (used by
    /// [`FixtureKind::AllocationBaseline`] for the `rss_allocator_ratio_bound` calibration
    /// question); left at whatever an early sample shows for every other fixture, which is
    /// harmless diagnostic data no consumer relies on.
    pub rss_baseline_bytes: u64,
}

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

fn process_cpu_time_ns() -> i64 {
    let mut ts = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `CLOCK_PROCESS_CPUTIME_ID` is always supported on Linux; `ts` is a valid, properly
    // aligned `timespec` out-pointer for the duration of this call.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, ts.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: `clock_gettime` returned 0 (success), so `ts` was fully initialized by the kernel.
    let ts = unsafe { ts.assume_init() };
    ts.tv_sec.saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec)
}

fn empty_sigset() -> libc::sigset_t {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `set` is a valid, properly aligned `sigset_t` out-pointer.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        set.assume_init()
    }
}

fn sigprof_only_set() -> libc::sigset_t {
    let mut set = empty_sigset();
    // SAFETY: `set` was just initialized by `sigemptyset` above via `empty_sigset`.
    unsafe {
        libc::sigaddset(&raw mut set, libc::SIGPROF);
    }
    set
}

/// Result of arming `SIGPROF` (ADR-0014 section 1.1): resets disposition to `SIG_DFL`, unblocks
/// the signal, and arms a one-shot 50ms `ITIMER_PROF`. Returns the CPU-time baseline the window
/// delta is measured from.
fn arm_sigprof() -> i64 {
    let mut old_action = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: `libc::sigaction` is a C struct of plain integers/function-pointer-sized fields
    // with no Rust-level validity invariant beyond "some bit pattern"; every field this function
    // actually relies on (`sa_sigaction`, `sa_flags`, `sa_mask`) is explicitly overwritten below
    // before this value is passed to the `sigaction(2)` syscall.
    let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
    default_action.sa_sigaction = libc::SIG_DFL;
    default_action.sa_flags = 0;
    default_action.sa_mask = empty_sigset();
    // SAFETY: `&default_action` is a valid, fully-initialized `sigaction`; `old_action` is a
    // valid out-pointer. `SIGPROF` is a valid signal number.
    unsafe {
        libc::sigaction(libc::SIGPROF, &raw const default_action, old_action.as_mut_ptr());
    }

    let sigprof_set = sigprof_only_set();
    // SAFETY: `sigprof_set` is a fully-initialized `sigset_t` containing only `SIGPROF`;
    // passing `null` for `oldset` is explicitly allowed by POSIX when the caller does not need
    // the previous mask.
    unsafe {
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const sigprof_set, std::ptr::null_mut());
    }

    let baseline_ns = process_cpu_time_ns();

    let timer_value = libc::itimerval {
        it_interval: libc::timeval { tv_sec: 0, tv_usec: 0 },
        it_value: libc::timeval {
            tv_sec: 0,
            tv_usec: 50_000,
        },
    };
    // SAFETY: `&timer_value` is a valid, fully-initialized `itimerval`; passing `null` for the
    // old-value out-pointer is explicitly allowed by POSIX.
    unsafe {
        libc::setitimer(libc::ITIMER_PROF, &raw const timer_value, std::ptr::null_mut());
    }

    baseline_ns
}

/// Reads back current `SIGPROF` disposition and mask and compares them against the expected
/// armed state (`SIG_DFL` disposition, unblocked). Returns `true` if tampered.
fn read_back_meter_tampered() -> bool {
    let mut current_action = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: passing `null` for the new-action pointer means "query only, do not change"
    // (explicitly supported by `sigaction(2)`); `current_action` is a valid out-pointer.
    unsafe {
        libc::sigaction(libc::SIGPROF, std::ptr::null(), current_action.as_mut_ptr());
    }
    // SAFETY: the `sigaction` call above succeeded in populating `current_action` (query-only
    // calls with a valid signal number do not fail).
    let current_action = unsafe { current_action.assume_init() };
    let disposition_tampered = current_action.sa_sigaction != libc::SIG_DFL;

    let mut current_mask = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: passing `null` for the new-mask pointer means "query only, do not change"; the
    // `how` argument is then ignored by POSIX. `current_mask` is a valid out-pointer.
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), current_mask.as_mut_ptr());
    }
    // SAFETY: the `pthread_sigmask` call above succeeded in populating `current_mask`.
    let current_mask = unsafe { current_mask.assume_init() };
    // SAFETY: `&current_mask` and `SIGPROF` are both valid arguments to `sigismember`.
    let sigprof_blocked = unsafe { libc::sigismember(&raw const current_mask, libc::SIGPROF) } == 1;

    disposition_tampered || sigprof_blocked
}

fn apply_fault_mode(mode: FaultMode) {
    match mode {
        FaultMode::None => {}
        FaultMode::PersistentBlock => {
            let sigprof_set = sigprof_only_set();
            // SAFETY: `sigprof_set` is fully initialized; `null` for `oldset` is explicitly
            // allowed.
            unsafe {
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const sigprof_set, std::ptr::null_mut());
            }
        }
        FaultMode::TransientBlockThenRestore => {
            let sigprof_set = sigprof_only_set();
            // SAFETY: same as above.
            unsafe {
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const sigprof_set, std::ptr::null_mut());
            }
            // Burn CPU past the ceiling *while blocked*, so the pending SIGPROF cannot be
            // delivered under its default (process-terminating) disposition.
            burn_cpu_at_least_ms(DECODE_APPLY_CPU_MS_MAX + 15.0);
            // Non-blockingly drain the now-pending SIGPROF without letting its default
            // disposition run, then restore the mask — by window-close time, disposition is
            // still SIG_DFL (never touched) and the mask is unblocked again, matching the armed
            // state exactly. Only the `resolve()` self-measured-CPU cross-check in
            // `termination.rs` can still catch this.
            let mut info = MaybeUninit::<libc::siginfo_t>::uninit();
            let zero_timeout = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: `sigprof_set`/`info`/`zero_timeout` are all valid; a zero timeout makes
            // this call non-blocking per POSIX, returning immediately whether or not SIGPROF was
            // pending. The return value (drained or `EAGAIN`) is intentionally not checked: this
            // is a best-effort drain attempting the strongest evasion this fault mode models,
            // not a correctness precondition for the rest of this function.
            unsafe {
                let _ = libc::sigtimedwait(&raw const sigprof_set, info.as_mut_ptr(), &raw const zero_timeout);
            }
            // SAFETY: same as the `PersistentBlock` arm above.
            unsafe {
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const sigprof_set, std::ptr::null_mut());
            }
        }
        FaultMode::Ignore => {
            // SAFETY: same reasoning as the `default_action` zero-init in `arm_sigprof` above —
            // every field this function relies on is explicitly overwritten before use.
            let mut ignore_action: libc::sigaction = unsafe { std::mem::zeroed() };
            ignore_action.sa_sigaction = libc::SIG_IGN;
            ignore_action.sa_flags = 0;
            ignore_action.sa_mask = empty_sigset();
            // SAFETY: `&ignore_action` is a valid, fully-initialized `sigaction`; `null` for the
            // old-action out-pointer is explicitly allowed.
            unsafe {
                libc::sigaction(libc::SIGPROF, &raw const ignore_action, std::ptr::null_mut());
            }
        }
    }
}

/// Busy-loops until `CLOCK_PROCESS_CPUTIME_ID` shows at least `min_ms` of CPU consumed since this
/// call started, with a generous wall-clock safety cap independent of the mechanism under test
/// (2 seconds) so a broken environment cannot hang the calibration run forever.
fn burn_cpu_at_least_ms(min_ms: f64) -> bool {
    let start_cpu_ns = process_cpu_time_ns();
    let start_wall_ns = monotonic_now_ns();
    let mut counter: u64 = 0;
    loop {
        for _ in 0..200_000 {
            counter = counter.wrapping_add(std::hint::black_box(counter ^ 0x9E37_79B9));
        }
        std::hint::black_box(counter);
        // Nanosecond deltas here are bounded by the 2-second wall-clock safety cap below, many
        // orders of magnitude under f64's exact-integer range (2^52 ns is over 52 days), so this
        // conversion never loses precision.
        #[allow(clippy::cast_precision_loss)]
        let elapsed_cpu_ms = (process_cpu_time_ns() - start_cpu_ns) as f64 / 1_000_000.0;
        if elapsed_cpu_ms >= min_ms {
            return false;
        }
        #[allow(clippy::cast_precision_loss)]
        let elapsed_wall_ms = (monotonic_now_ns() - start_wall_ns) as f64 / 1_000_000.0;
        if elapsed_wall_ms >= 2_000.0 {
            return true;
        }
    }
}

fn read_rss_peak_bytes() -> u64 {
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

/// Runs one case's fixture workload and reports the outcome. Never returns: every path ends by
/// calling [`finish`], which calls `libc::_exit`.
pub fn run_child(fixture: FixtureKind, fault_mode: FaultMode, page: &SharedPageLayout, report_fd: OwnedFd) -> ! {
    let baseline_cpu_ns = arm_sigprof();
    page.sigprof_armed_flag.store(1, std::sync::atomic::Ordering::SeqCst);
    page.child_window_open_at_ns
        .store(monotonic_now_ns(), std::sync::atomic::Ordering::SeqCst);

    apply_address_space_backstop();

    let rss_baseline_bytes = read_rss_peak_bytes();

    apply_fault_mode(fault_mode);

    crate::isolation::alloc::arm_for_case(page);

    let safety_cap_triggered = if fault_mode == FaultMode::TransientBlockThenRestore {
        // `apply_fault_mode` already performed this fault mode's entire "sabotaged CPU burn"
        // workload above (block, burn past the ceiling, drain the pending signal, restore the
        // mask). Running the fixture body's own burn on top of that would push total CPU/wall
        // consumption well past the 100ms wall ceiling regardless of whether the drain trick
        // worked, guaranteeing a wall-watchdog kill and masking the one thing this fault mode
        // exists to prove: that the self-measured-CPU cross-check in `termination::resolve`
        // (not the wall watchdog) is what catches this specific evasion when the process
        // otherwise finishes within the wall budget.
        false
    } else {
        match fixture {
            FixtureKind::Completed => run_completed(),
            FixtureKind::CpuHog => burn_cpu_at_least_ms(DECODE_APPLY_CPU_MS_MAX + 100.0),
            FixtureKind::WallHog => run_wall_hog(),
            FixtureKind::MemoryHog => run_memory_hog(),
            FixtureKind::AddressSpaceBackstop => run_address_space_backstop(page),
            FixtureKind::Crashed => run_crashed(),
            FixtureKind::AllocationBaseline => run_allocation_baseline(),
        }
    };

    let tampered = read_back_meter_tampered();
    page.meter_tampered_flag
        .store(u32::from(tampered), std::sync::atomic::Ordering::SeqCst);
    page.rss_peak_bytes
        .store(read_rss_peak_bytes(), std::sync::atomic::Ordering::SeqCst);

    let window_cpu_ns = process_cpu_time_ns() - baseline_cpu_ns;
    // A single metering window's CPU delta, bounded by the wall watchdog to well under a second
    // in practice -- far inside f64's exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    let cpu_ms = window_cpu_ns as f64 / 1_000_000.0;
    let report = ChildReport {
        fixture_id: fixture.fixture_id().to_string(),
        window_cpu_ms: cpu_ms,
        self_reported_safety_cap_triggered: safety_cap_triggered,
        rss_baseline_bytes,
    };
    finish(&report, report_fd);
}

/// Sets `RLIMIT_AS` to the 512 MiB backstop (ADR-0014 section 3) for **every** case, not just
/// [`FixtureKind::AddressSpaceBackstop`] — it exists to catch allocations that bypass the
/// counting allocator entirely (raw `mmap`, FFI allocators, JIT reservation), which could happen
/// in any case, not only the fixture built specifically to probe it. This was found missing
/// during real end-to-end testing of this module: without it, `FixtureKind::AddressSpaceBackstop`
/// silently mapped its oversized region successfully and finished as `completed` instead of
/// tripping `address_space_backstop`, since the process had no address-space limit configured at
/// all. See the delivery report for the observed before/after.
fn apply_address_space_backstop() {
    let limit_bytes = crate::isolation::termination::ADDRESS_SPACE_BACKSTOP_BYTES_MAX;
    let limit = libc::rlimit {
        rlim_cur: limit_bytes,
        rlim_max: limit_bytes,
    };
    // SAFETY: `&limit` is a valid, fully-initialized `rlimit`; `RLIMIT_AS` is always a valid
    // resource on Linux. This runs single-threaded, post-fork, before any allocation this
    // fixture's own workload performs, so lowering the limit here cannot invalidate memory this
    // process already depends on (its existing footprint is far below 512 MiB).
    unsafe {
        libc::setrlimit(libc::RLIMIT_AS, &raw const limit);
    }
}

fn run_allocation_baseline() -> bool {
    let mut buffer: Vec<u8> = Vec::with_capacity(ALLOCATION_BASELINE_BYTES);
    buffer.resize(ALLOCATION_BASELINE_BYTES, 7u8);
    std::hint::black_box(&buffer);
    false
}

fn run_completed() -> bool {
    // A trivial, fast "decode + semantic validation" stand-in: sum a small buffer. This is not a
    // real Flow update decode — work package 3a's calibration fixtures are synthetic resource
    // probes, not document-apply operations.
    let buffer = [1u8; 256];
    let checksum: u64 = buffer.iter().map(|byte| u64::from(*byte)).sum();
    std::hint::black_box(checksum);
    false
}

fn run_wall_hog() -> bool {
    // Low CPU, high wall: sleeps past the 100ms wall ceiling while barely touching the CPU
    // meter, so the parent's wall watchdog — not `SIGPROF` — must be what terminates this case.
    std::thread::sleep(Duration::from_millis(300));
    false
}

fn run_memory_hog() -> bool {
    // A single allocation request that alone exceeds the 128 MiB ceiling. The counting
    // allocator's ceiling check (see `isolation::alloc`) runs *before* it calls into `System`, so
    // this is rejected immediately -- no memset, no page-touching, no CPU cost of consequence --
    // which is deliberate: an earlier version of this fixture incrementally allocated+filled
    // 8 MiB chunks in a loop, and on a debug (unoptimized) build the per-chunk `memset`-equivalent
    // fill was slow enough that the 50ms `SIGPROF` ceiling fired *before* the loop ever
    // accumulated 128 MiB, misclassifying this fixture as `cpu_ceiling` instead of
    // `memory_ceiling`. A single oversized request sidesteps that race entirely, and does so for
    // the right reason: this fixture exists to test the allocator's *rejection* path, not to
    // benchmark fill bandwidth.
    // The ceiling plus a 16 MiB margin is far below usize::MAX on every real target; the fallback
    // only matters on a hypothetical platform where it would not fit, and saturating to MAX there
    // still yields an oversized request, which is this fixture's whole point.
    let oversized_bytes = usize::try_from(ISOLATED_APPLY_MEMORY_BYTES_MAX + 16 * 1024 * 1024).unwrap_or(usize::MAX);
    let Ok(layout) = std::alloc::Layout::from_size_align(oversized_bytes, 16) else {
        return false;
    };
    // SAFETY: `layout` is a valid non-zero-sized `Layout` constructed above; the returned pointer
    // is checked for null and, on the (expected) rejection path, never dereferenced. On the
    // (unexpected) success path it is immediately deallocated with the same layout, matching
    // `GlobalAlloc`'s contract.
    let ptr = unsafe { std::alloc::alloc(layout) };
    // Real end-to-end testing found `std::alloc::alloc`'s result, when used only for a null
    // check with the underlying memory never read or written, gets eliminated by LLVM as a dead
    // allocation — the call never reaches the registered `#[global_allocator]` at all, silently
    // skipping this fixture's entire rejection path. `black_box` defeats that: it forces the
    // compiler to treat `ptr` as possibly observed elsewhere, which keeps the call live. Every
    // other fixture in this file that allocates (`run_completed`, `run_allocation_baseline`)
    // already routes through a `Vec`, which independently defeats the same optimization via
    // `RawVec`'s more complex allocation path — this is the one fixture that calls the allocator
    // directly, which is why only it needed this explicit guard.
    let ptr = std::hint::black_box(ptr);
    if ptr.is_null() {
        // Expected: the counting allocator rejected this before calling `System.alloc`, already
        // wrote `memory_cause_flag`/`allocated_attempted_peak_bytes`, and returned null here.
        // Per `GlobalAlloc`'s contract, a null return from `alloc` must be routed through the
        // standard alloc-error handler (which aborts) rather than used directly.
        std::alloc::handle_alloc_error(layout);
    }
    // Unexpected: the allocator did not reject an over-ceiling request. Free it and report
    // `false` (not a safety-cap trigger) so this surfaces as a plain classification mismatch
    // (`primary_cause` would end up `completed`, not the fixture's expected `memory_ceiling`)
    // rather than being silently swallowed.
    // SAFETY: `ptr` was just returned by `alloc` above with this exact `layout`, non-null,
    // deallocated exactly once.
    unsafe {
        std::alloc::dealloc(ptr, layout);
    }
    false
}

fn run_address_space_backstop(page: &SharedPageLayout) -> bool {
    // Same reasoning as `run_memory_hog`'s `oversized_bytes` above: this backstop constant plus a
    // 64 MiB margin is far below usize::MAX on every real target, and a saturated fallback would
    // still be an oversized request.
    let backstop_bytes =
        usize::try_from(crate::isolation::termination::ADDRESS_SPACE_BACKSTOP_BYTES_MAX).unwrap_or(usize::MAX);
    let oversized_bytes = backstop_bytes.saturating_add(64 * 1024 * 1024);
    // SAFETY: all arguments are simple values with no aliasing/lifetime requirements, identical
    // in shape to the shared-page mapping in `shared_page.rs`; this call is expected to fail
    // (that is the fixture's entire point) and the return value is checked for `MAP_FAILED`
    // before any use — no memory from a successful mapping is ever touched.
    let raw = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            oversized_bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if raw == libc::MAP_FAILED {
        page.address_space_backstop_flag
            .store(1, std::sync::atomic::Ordering::SeqCst);
    } else {
        // Unexpected success (the backstop was not actually hit, e.g. an overcommit-friendly
        // environment) — unmap immediately so this fixture never actually holds 500+ MiB.
        // SAFETY: `raw`/`oversized_bytes` are exactly the pointer and length just returned by
        // the successful `mmap` above.
        unsafe {
            let _ = libc::munmap(raw, oversized_bytes);
        }
    }
    false
}

fn run_crashed() -> bool {
    // A genuine invalid memory write, not `libc::raise(SIGSEGV)`: real end-to-end testing showed
    // `raise` doesn't work here. Rust's standard library installs its own `SIGSEGV`/`SIGBUS`
    // handler at startup to detect stack overflows via guard pages. That handler runs first,
    // determines this fault address is not within any registered guard page, and returns
    // normally -- which harmlessly resumes execution right after the `raise()` call (a
    // software-raised signal has no faulting instruction to retry, unlike a real hardware trap),
    // so the process never actually terminated. An actual out-of-bounds write is a genuine
    // hardware trap: even though the same handler runs first and resets the disposition to
    // `SIG_DFL` before returning, returning from a hardware-trap handler resumes at the *same*
    // faulting instruction, which then re-faults and this time hits the (now default) terminating
    // action.
    let invalid_address = std::ptr::dangling_mut::<u8>();
    // SAFETY: this fixture's entire purpose is to crash the process via a genuine invalid memory
    // access, to exercise the "unrelated fatal signal" classification path. `write_volatile`
    // (rather than a plain write) prevents the optimizer from proving this is immediate UB and
    // eliding the instruction that must actually execute for the fault to occur.
    unsafe {
        std::ptr::write_volatile(invalid_address, 1u8);
    }
    false
}

/// Encodes and writes the final [`ChildReport`] through the frame protocol, then `_exit(0)`s.
/// This is the **only** exit path for a child that runs to completion (the fault/crash/ceiling
/// fixtures above are expected to be killed by a signal before ever reaching here).
fn finish(report: &ChildReport, report_fd: OwnedFd) -> ! {
    let payload = serde_json::to_vec(report).unwrap_or_default();
    if let Ok(framed) = frame::encode(&payload) {
        let mut file = std::fs::File::from(report_fd);
        let _ = file.write_all(&framed);
        let _ = file.flush();
    }
    // SAFETY: `_exit` takes a plain `c_int` status code and has no preconditions; using it
    // (rather than `std::process::exit`) is the standard fork-safety practice that avoids
    // running the parent's already-scheduled atexit handlers and global destructors a second
    // time inside this child.
    unsafe {
        libc::_exit(0);
    }
}
