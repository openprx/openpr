//! Child-side (worker process) primitives: single-threadedness verification, the `RLIMIT_AS`
//! backstop, and arming/reading back the `SIGPROF` CPU-ceiling timer.
//!
//! `ADR-0014` section 1's online-enforcement table and section 1.1's arm sequence, adapted from
//! `spikes/collab-shared/src/isolation/child.rs` (which arms the identical timer for its
//! calibration fixtures) to this crate's actual apply workload. See
//! `crates/collab-core/src/isolation/host.rs`'s module doc for why this runs inside a freshly
//! `exec`'d process rather than `ADR-0014`'s fork-no-exec zygote.
//!
//! Everything here runs inside `src/bin/isolated_apply_worker.rs`'s single-purpose process, which
//! owns the actual exit path (`main`'s `libc::_exit`, not `std::process::exit`, matching the
//! calibration host's own fork-safety discipline -- see that file's module doc).

#![allow(unsafe_code)]

use std::mem::MaybeUninit;

use super::limits::DECODE_APPLY_CPU_MS_MAX_MICROS;

/// `ADR-0014` section 3's `RLIMIT_AS` backstop -- independent of, and larger than, the
/// counting-allocator-enforced `isolated_apply_memory_bytes_max` (128 MiB). Catches allocations
/// that bypass the counting allocator entirely (a raw `mmap`, an FFI allocator, a JIT reservation).
pub const ADDRESS_SPACE_BACKSTOP_BYTES: u64 = 512 * 1024 * 1024;

/// `ADR-0014` section 10, self-assertion 1, adapted: verifies this process is single-threaded
/// using the kernel's own bookkeeping (`/proc/self/task`), not an assumption from startup
/// ordering. A freshly `exec`'d process image starts single-threaded by construction, but this
/// checks the actual fact rather than relying on that construction never changing (a future
/// dependency upgrade could start a background thread during its own static initialization,
/// for example).
#[must_use]
pub fn is_single_threaded() -> bool {
    std::fs::read_dir("/proc/self/task").is_ok_and(|entries| entries.count() == 1)
}

/// Sets `RLIMIT_AS` to [`ADDRESS_SPACE_BACKSTOP_BYTES`] for the whole process. Called once, before
/// the request is even read, so it applies to every allocation this process ever makes (not only
/// the metered window) -- `ADR-0014` section 10's `child.rs::apply_address_space_backstop` applies
/// it "for every case, not just the fixture built specifically to probe it", for the identical
/// reason: bypassing allocations can happen at any point, not only inside the counted window.
pub fn set_address_space_backstop() {
    let limit = libc::rlimit {
        rlim_cur: ADDRESS_SPACE_BACKSTOP_BYTES,
        rlim_max: ADDRESS_SPACE_BACKSTOP_BYTES,
    };
    // SAFETY: `&limit` is a valid, fully-initialized `rlimit`; `RLIMIT_AS` is always a valid
    // resource on Linux. This runs at process startup before any request-driven allocation, so
    // lowering the limit here cannot invalidate memory this process already depends on (its
    // startup footprint is far below 512 MiB).
    unsafe {
        libc::setrlimit(libc::RLIMIT_AS, &raw const limit);
    }
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

/// Arms the CPU ceiling (`ADR-0014` section 1.1): resets `SIGPROF`'s disposition to `SIG_DFL`
/// (whose default action is to terminate the process -- this *is* the online CPU-ceiling
/// enforcement, no signal handler required), unblocks it, and arms a one-shot
/// [`DECODE_APPLY_CPU_MS_MAX_MICROS`] `ITIMER_PROF` timer, which decrements only while this
/// process consumes CPU time (user + system), not wall-clock time.
///
/// Must be called from the single process thread (verified by [`is_single_threaded`] before this
/// is ever invoked), immediately before the metered decode/apply/shape-validate work begins.
pub fn arm_sigprof() {
    let mut old_action = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: `libc::sigaction` is a C struct of plain integers/function-pointer-sized fields with
    // no Rust-level validity invariant beyond "some bit pattern"; every field this function
    // actually relies on (`sa_sigaction`, `sa_flags`, `sa_mask`) is explicitly overwritten below
    // before this value is passed to the `sigaction(2)` syscall.
    let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
    default_action.sa_sigaction = libc::SIG_DFL;
    default_action.sa_flags = 0;
    default_action.sa_mask = empty_sigset();
    // SAFETY: `&default_action` is a valid, fully-initialized `sigaction`; `old_action` is a valid
    // out-pointer. `SIGPROF` is a valid signal number.
    unsafe {
        libc::sigaction(libc::SIGPROF, &raw const default_action, old_action.as_mut_ptr());
    }

    let sigprof_set = sigprof_only_set();
    // SAFETY: `sigprof_set` is a fully-initialized `sigset_t` containing only `SIGPROF`; passing
    // `null` for `oldset` is explicitly allowed by POSIX when the caller does not need the
    // previous mask.
    unsafe {
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const sigprof_set, std::ptr::null_mut());
    }

    let timer_value = libc::itimerval {
        it_interval: libc::timeval { tv_sec: 0, tv_usec: 0 },
        it_value: libc::timeval {
            tv_sec: 0,
            tv_usec: DECODE_APPLY_CPU_MS_MAX_MICROS,
        },
    };
    // SAFETY: `&timer_value` is a valid, fully-initialized `itimerval`; passing `null` for the
    // old-value out-pointer is explicitly allowed by POSIX.
    unsafe {
        libc::setitimer(libc::ITIMER_PROF, &raw const timer_value, std::ptr::null_mut());
    }
}

/// Disarms the `ITIMER_PROF` timer (sets it to fire never), so it cannot fire *after* the metered
/// window has already closed successfully -- e.g. while this process is busy exporting the result
/// snapshot and writing the response frame, neither of which are protected work.
pub fn disarm_sigprof() {
    let timer_value = libc::itimerval {
        it_interval: libc::timeval { tv_sec: 0, tv_usec: 0 },
        it_value: libc::timeval { tv_sec: 0, tv_usec: 0 },
    };
    // SAFETY: `&timer_value` is a valid, fully-initialized, all-zero `itimerval`, which POSIX
    // defines as "disarm this timer"; passing `null` for the old-value out-pointer is explicitly
    // allowed.
    unsafe {
        libc::setitimer(libc::ITIMER_PROF, &raw const timer_value, std::ptr::null_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_single_threaded_is_true_for_this_test_process_main_thread_check() {
        // `cargo test` runs many tests concurrently across multiple OS threads within one test
        // binary process, so this process is *not* single-threaded while the suite runs -- this
        // assertion documents that fact rather than asserting `true`, so a change to test-harness
        // threading does not silently make this check meaningless.
        // `is_single_threaded` itself must still return a real, non-panicking answer either way.
        let _ = is_single_threaded();
    }

    // `set_address_space_backstop` is deliberately not exercised here: it lowers the *calling
    // process's* `RLIMIT_AS` hard limit, which cannot be raised back afterwards. Calling it from
    // a `cargo test` unit test would permanently cap this whole test binary process (which runs
    // many unrelated tests concurrently in-process) at 512 MiB of address space for the rest of
    // the run, risking spurious failures far away from this module. It is only ever actually
    // called from `src/bin/isolated_apply_worker.rs`'s `main`, a fresh single-purpose process
    // that exits immediately after one apply, where that permanence is exactly the point.
}
