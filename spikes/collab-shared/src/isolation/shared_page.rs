//! The zygote's `MAP_SHARED|MAP_ANONYMOUS` measurement page (ADR-0014 sections 0 and 2).
//!
//! Mapped once, before any `fork()`, so the physical pages are shared (not copy-on-write) between
//! the zygote/parent and every forked child: a `SIGKILL`ed child's last atomic writes are still
//! visible to the parent afterwards, which is the entire reason the ADR requires `fork`-not-`exec`
//! (an `exec`'d child would lose the anonymous mapping).

use std::ptr::NonNull;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Sentinel for an unset monotonic-clock timestamp field.
pub const TIMESTAMP_UNSET: i64 = i64::MIN;

/// The shared measurement region's field layout.
///
/// `#[repr(C)]` is not strictly required for correctness here (parent and child are always the
/// exact same compiled binary via `fork`, so the default Rust layout is identical on both sides
/// regardless), but it documents the layout as stable and makes the struct safe to reason about
/// independent of that fork-specific guarantee.
#[repr(C)]
pub struct SharedPageLayout {
    /// Case nonce. Reset (along with every other field below) immediately before each `fork()`
    /// so a previous case's high-water marks or cause flags cannot leak into the next one
    /// (ADR-0014 section 10, calibration self-assertion 2).
    pub generation: AtomicU64,
    pub allocated_active_peak_bytes: AtomicU64,
    pub allocated_attempted_peak_bytes: AtomicU64,
    /// 0/1 boolean: the counting allocator rejected an allocation that would have crossed the
    /// 128 MiB ceiling.
    pub memory_cause_flag: AtomicU32,
    /// 0/1 boolean: a raw allocation that bypassed the counting allocator (e.g. a direct `mmap`)
    /// hit the 512 MiB `RLIMIT_AS` backstop.
    pub address_space_backstop_flag: AtomicU32,
    /// 0/1 boolean: the child successfully armed `SIGPROF` (`SIG_DFL` disposition +
    /// unblocked mask) before opening its metering window.
    pub sigprof_armed_flag: AtomicU32,
    /// 0/1 boolean: the child's window-close readback found `SIGPROF` disposition or mask
    /// different from what arm-time recorded (ADR-0014 section 1.1.3).
    pub meter_tampered_flag: AtomicU32,
    /// Diagnostic only (ADR-0014 section 3: does not participate in any gate judgment). Written
    /// by the child from `/proc/self/status` `VmHWM` right before it reports; stays 0 if the
    /// child never reaches that point (e.g. it was `SIGKILL`ed).
    pub rss_peak_bytes: AtomicU64,
    /// `CLOCK_MONOTONIC` nanoseconds, written by the zygote immediately before calling `fork()`.
    pub fork_decided_at_ns: AtomicI64,
    /// `CLOCK_MONOTONIC` nanoseconds, written by the child immediately after it finishes arming
    /// `SIGPROF` and opens its metering window — this is what `fork_overhead_ms` is measured
    /// against, and it is written to the shared page (not just returned in the result frame) so
    /// it survives the child being killed before it can report anything else.
    pub child_window_open_at_ns: AtomicI64,
}

impl SharedPageLayout {
    /// All-zero/unset initial state.
    const fn zeroed() -> Self {
        Self {
            generation: AtomicU64::new(0),
            allocated_active_peak_bytes: AtomicU64::new(0),
            allocated_attempted_peak_bytes: AtomicU64::new(0),
            memory_cause_flag: AtomicU32::new(0),
            address_space_backstop_flag: AtomicU32::new(0),
            sigprof_armed_flag: AtomicU32::new(0),
            meter_tampered_flag: AtomicU32::new(0),
            rss_peak_bytes: AtomicU64::new(0),
            fork_decided_at_ns: AtomicI64::new(TIMESTAMP_UNSET),
            child_window_open_at_ns: AtomicI64::new(TIMESTAMP_UNSET),
        }
    }

    /// Resets every field to its initial state and stamps a new `generation` nonce. Must be
    /// called before every `fork()` (ADR-0014 section 10, self-assertion 2).
    pub fn reset_for_next_case(&self, generation: u64) {
        self.allocated_active_peak_bytes.store(0, Ordering::SeqCst);
        self.allocated_attempted_peak_bytes.store(0, Ordering::SeqCst);
        self.memory_cause_flag.store(0, Ordering::SeqCst);
        self.address_space_backstop_flag.store(0, Ordering::SeqCst);
        self.sigprof_armed_flag.store(0, Ordering::SeqCst);
        self.meter_tampered_flag.store(0, Ordering::SeqCst);
        self.rss_peak_bytes.store(0, Ordering::SeqCst);
        self.fork_decided_at_ns.store(TIMESTAMP_UNSET, Ordering::SeqCst);
        self.child_window_open_at_ns.store(TIMESTAMP_UNSET, Ordering::SeqCst);
        // Generation is written last: a reader that observes the new generation is guaranteed
        // (under `SeqCst`) to observe every reset field above it too.
        self.generation.store(generation, Ordering::SeqCst);
    }
}

/// Errors mapping/unmapping the shared page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedPageError {
    MmapFailed { errno: i32 },
}

/// An owned handle to the zygote's `mmap`'d measurement page. `Drop` unmaps it.
///
/// Not `Send`/`Sync`: this crate never shares the page across *threads* (the whole isolation
/// host is deliberately single-threaded — see ADR-0014 section 10, self-assertion 1); it is only
/// ever shared across the `fork()` boundary, which is a distinct process-level sharing mechanism
/// that does not need Rust's thread-safety marker traits.
pub struct SharedPage {
    ptr: NonNull<SharedPageLayout>,
    mapped_len: usize,
}

impl SharedPage {
    /// Maps a new zero-initialized shared page sized to hold one [`SharedPageLayout`], rounded up
    /// to the platform page size.
    ///
    /// # Errors
    /// Returns [`SharedPageError::MmapFailed`] if the `mmap(2)` syscall fails.
    pub fn map() -> Result<Self, SharedPageError> {
        let struct_len = std::mem::size_of::<SharedPageLayout>();
        // SAFETY: `sysconf(_SC_PAGESIZE)` takes no pointer arguments and has no preconditions;
        // its only failure mode is returning -1, which we handle by falling back to the common
        // 4096-byte page size rather than propagating an error for a purely advisory rounding.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page_size = usize::try_from(page_size).unwrap_or(4096);
        let mapped_len = struct_len.div_ceil(page_size) * page_size;

        // SAFETY: all arguments are simple values with no aliasing/lifetime requirements:
        // `addr = null` lets the kernel choose the mapping address, `fd = -1` and `offset = 0`
        // are required by POSIX for `MAP_ANONYMOUS`. The returned pointer is checked for
        // `MAP_FAILED` before any use.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapped_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            // SAFETY: `errno` is thread-local libc state; reading it right after the failing
            // call, before any other libc call can clobber it, is the standard safe pattern.
            let errno = unsafe { *libc::__errno_location() };
            return Err(SharedPageError::MmapFailed { errno });
        }

        // SAFETY: `raw` is a valid, non-null, `mapped_len`-byte writable mapping (checked above)
        // that is at least as large as `SharedPageLayout` (by construction of `mapped_len`) and
        // is `mmap`-page-aligned, which satisfies `SharedPageLayout`'s alignment requirement
        // (its largest field is 8-byte aligned). The kernel zero-fills anonymous mappings, and
        // an all-zero-bytes `SharedPageLayout` is a valid bit pattern for every field (atomic
        // integer wrappers over plain integers), so writing the fully-initialized zeroed struct
        // here does not read any uninitialized memory it depends on; it exists to make the
        // "valid `SharedPageLayout` value" invariant explicit rather than relying on incidental
        // zero-page semantics.
        let ptr = unsafe {
            let typed = raw.cast::<SharedPageLayout>();
            typed.write(SharedPageLayout::zeroed());
            NonNull::new_unchecked(typed)
        };

        Ok(Self { ptr, mapped_len })
    }

    #[must_use]
    pub const fn layout(&self) -> &SharedPageLayout {
        // SAFETY: `self.ptr` was produced by a successful `mmap` in `map()` and is never
        // reassigned or freed before `Drop::drop` unmaps it, so it remains valid for the
        // lifetime of `&self`. Returning a shared reference to the atomics inside is sound:
        // every field is an atomic type, so concurrent access from another process sharing the
        // same physical pages (the intended use, across `fork()`) is exactly what atomics are
        // for.
        unsafe { self.ptr.as_ref() }
    }
}

impl Drop for SharedPage {
    fn drop(&mut self) {
        // SAFETY: `self.ptr`/`self.mapped_len` are exactly the pointer and length returned by
        // the `mmap` call in `map()` that constructed this `SharedPage`, and this is the only
        // place that unmaps them (no other code holds or frees this mapping).
        unsafe {
            let _ = libc::munmap(self.ptr.as_ptr().cast::<libc::c_void>(), self.mapped_len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn map_produces_zeroed_page() {
        let page = match SharedPage::map() {
            Ok(page) => page,
            Err(error) => {
                // mmap can genuinely fail under a restrictive sandbox; report rather than fail
                // the whole suite on an environment limitation unrelated to this code's logic.
                eprintln_test(&format!("skipping: mmap failed: {error:?}"));
                return;
            }
        };
        let layout = page.layout();
        assert_eq!(layout.generation.load(Ordering::SeqCst), 0);
        assert_eq!(layout.allocated_active_peak_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(layout.fork_decided_at_ns.load(Ordering::SeqCst), TIMESTAMP_UNSET);
    }

    #[test]
    fn reset_for_next_case_clears_stale_high_water_and_bumps_generation() {
        let page = match SharedPage::map() {
            Ok(page) => page,
            Err(error) => {
                eprintln_test(&format!("skipping: mmap failed: {error:?}"));
                return;
            }
        };
        let layout = page.layout();
        layout.allocated_active_peak_bytes.store(999, Ordering::SeqCst);
        layout.memory_cause_flag.store(1, Ordering::SeqCst);
        layout.generation.store(1, Ordering::SeqCst);

        layout.reset_for_next_case(2);

        assert_eq!(layout.allocated_active_peak_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(layout.memory_cause_flag.load(Ordering::SeqCst), 0);
        assert_eq!(layout.generation.load(Ordering::SeqCst), 2);
    }

    /// Tests in this crate must not use `println!`/`eprintln!` directly (workspace clippy denies
    /// `print_stdout`/`print_stderr`); this helper is the one place that intentionally routes a
    /// diagnostic through `eprintln!`, isolated so the `#[allow]` has the smallest possible scope.
    #[allow(clippy::print_stderr)]
    fn eprintln_test(message: &str) {
        eprintln!("{message}");
    }
}
