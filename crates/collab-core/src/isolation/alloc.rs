//! The counting `GlobalAlloc` that is the online enforcer for `isolated_apply_memory_bytes_max`
//! (`ADR-0014` section 1's table: "计数分配器自身...先把 attempted peak 与 cause 写进共享页,再返回
//! null"; `contracts/limits-v1.md`'s `isolated_apply_memory_bytes_max` = 128 MiB).
//!
//! Unlike `spikes/collab-shared`'s calibration-host allocator, this one has no shared measurement
//! page: it exists only inside `src/bin/isolated_apply_worker.rs`'s single-purpose process, whose
//! entire job is to report an accept/reject decision (plus, on rejection, *why*) back to its
//! parent -- not to produce gate-grade forensic evidence about how far an attempt overshot. The
//! parent (`isolation::host`) only needs to know a rejection happened and why, which it gets from
//! the worker process's exit signal (`SIGABRT`, via Rust's standard `handle_alloc_error` -> abort
//! path below), not from a peak value read out of shared memory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::limits::ISOLATED_APPLY_MEMORY_BYTES_MAX;

/// Process-local running total of active (not-yet-freed) bytes allocated since [`arm`] was called.
static ACTIVE_BYTES: AtomicU64 = AtomicU64::new(0);

/// `false` until [`arm`] is called. Allocations before arming (process startup, argv parsing,
/// reading the request off stdin, loading the base document) pass straight through uncounted --
/// matching `ADR-0014` section 1.3's exclusion of pre-window setup from the metered window.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Opens the metered window: resets the active-byte counter to zero and starts counting.
///
/// Must be called exactly once, immediately before the untrusted update is decoded/applied, by
/// the same single-threaded process this allocator is installed in (`src/bin/isolated_apply_worker.rs`).
pub fn arm() {
    ACTIVE_BYTES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
}

/// Closes the metered window: allocations/frees from this point on pass straight through
/// uncounted again, mirroring [`arm`]'s pre-window passthrough behavior. Matches
/// `child_runtime::disarm_sigprof`'s closing of the CPU-ceiling window at the same point in
/// `src/bin/isolated_apply_worker.rs`'s call sequence, so the memory ceiling covers exactly the
/// same span (decode/apply/shape-validate) as the CPU ceiling, per `contracts/limits-v1.md`'s "从
/// decode 前开始计,到 semantic diff/shape validation 完成结束" scope for both -- serializing the
/// already-accepted result (`export_snapshot`) is not part of that scope, the same way loading the
/// already-validated base document before [`arm`] is called is not.
///
/// Does not reset the active-byte counter: there is nothing left in this worker's lifecycle that
/// reads it after this point (the process reports its outcome and exits), and leaving stale
/// accounting in place is harmless since [`arm`] always resets it back to zero before it is ever
/// consulted again.
pub fn disarm() {
    ARMED.store(false, Ordering::SeqCst);
}

/// A `GlobalAlloc` that enforces [`ISOLATED_APPLY_MEMORY_BYTES_MAX`] as the sole memory gate
/// quantity (`ADR-0014` section 3), once [`arm`] has been called.
///
/// An allocation that would push the process's active byte count past the ceiling is rejected --
/// returning `null` without ever calling into the system allocator for that request -- rather than
/// being allowed through and only detected afterwards. Rust's standard library routes a `null`
/// return from `GlobalAlloc::alloc`/`realloc` through `handle_alloc_error`, which aborts the
/// process (`SIGABRT`); no canonical document state exists in this process to corrupt, and the
/// parent (`isolation::host::classify_signal`) maps that abort back to
/// `isolated_apply_memory_bytes`.
pub struct CountingAllocator;

// SAFETY: `alloc`/`dealloc`/`realloc` all delegate the actual memory operation to `System` (the
// platform allocator, which already correctly implements `GlobalAlloc`'s contract for valid
// non-zero-sized `Layout`s) and only add atomic bookkeeping around it. The bookkeeping itself
// touches no allocator-owned memory and performs no allocation, so it cannot recursively invoke
// this allocator. `alloc_zeroed` is left at its default trait implementation, defined in terms of
// `self.alloc`, so it is covered by the same reasoning.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !ARMED.load(Ordering::SeqCst) {
            // SAFETY: passthrough -- `layout` is forwarded unchanged to `System`, whose
            // `GlobalAlloc::alloc` has the identical safety contract as this method.
            return unsafe { System.alloc(layout) };
        }
        let requested = layout.size() as u64;
        let prospective = ACTIVE_BYTES.load(Ordering::SeqCst).saturating_add(requested);
        if prospective > ISOLATED_APPLY_MEMORY_BYTES_MAX {
            return std::ptr::null_mut();
        }
        // SAFETY: see the `unsafe impl` block comment above -- `layout` is forwarded unchanged.
        let allocated = unsafe { System.alloc(layout) };
        if !allocated.is_null() {
            ACTIVE_BYTES.fetch_add(requested, Ordering::SeqCst);
        }
        allocated
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` are exactly the arguments the caller must supply per
        // `GlobalAlloc::dealloc`'s contract (a pointer previously returned by this allocator's
        // `alloc`/`realloc` with the same layout), forwarded unchanged to `System`.
        unsafe { System.dealloc(ptr, layout) };
        if ARMED.load(Ordering::SeqCst) {
            let freed = layout.size() as u64;
            let _ = ACTIVE_BYTES.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                Some(active.saturating_sub(freed))
            });
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if !ARMED.load(Ordering::SeqCst) {
            // SAFETY: passthrough per `GlobalAlloc::realloc`'s contract, arguments forwarded
            // unchanged.
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        let old_size = layout.size() as u64;
        let requested_new = new_size as u64;
        let current = ACTIVE_BYTES.load(Ordering::SeqCst);
        let prospective = current.saturating_sub(old_size).saturating_add(requested_new);
        if requested_new > old_size && prospective > ISOLATED_APPLY_MEMORY_BYTES_MAX {
            return std::ptr::null_mut();
        }
        // SAFETY: `ptr`/`layout`/`new_size` are exactly the arguments the caller must supply per
        // `GlobalAlloc::realloc`'s contract, forwarded unchanged to `System`.
        let reallocated = unsafe { System.realloc(ptr, layout, new_size) };
        if !reallocated.is_null() {
            ACTIVE_BYTES.store(prospective, Ordering::SeqCst);
        }
        reallocated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test does *not* install [`CountingAllocator`] as the process's actual global
    /// allocator (that only ever happens in `src/bin/isolated_apply_worker.rs`'s `main`, once,
    /// for the whole process) -- it exercises the bookkeeping logic directly, the same way
    /// `spikes/collab-shared/src/isolation/alloc.rs`'s equivalent tests do.
    ///
    /// Both scenarios (below-ceiling accept, above-ceiling reject) live in one test function
    /// rather than two: `ACTIVE_BYTES`/`ARMED` are process-global statics, and `cargo test` runs
    /// tests in parallel by default -- two separate tests each calling `arm()` (which resets
    /// `ACTIVE_BYTES` to zero) would race each other's assertions.
    #[test]
    fn counting_allocator_accepts_below_ceiling_and_rejects_above_it() {
        arm();
        let allocator = CountingAllocator;

        let Ok(layout) = Layout::from_size_align(4096, 8) else {
            return;
        };
        // SAFETY: `layout` is a valid non-zero-sized layout constructed above; this test owns the
        // returned pointer exclusively and deallocates it with the same layout below.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        assert_eq!(ACTIVE_BYTES.load(Ordering::SeqCst), 4096);
        // SAFETY: `ptr` was returned by `alloc` above with this exact `layout`, freed exactly
        // once here.
        unsafe { allocator.dealloc(ptr, layout) };
        assert_eq!(ACTIVE_BYTES.load(Ordering::SeqCst), 0);

        let oversized = usize::try_from(ISOLATED_APPLY_MEMORY_BYTES_MAX + 1).unwrap_or(usize::MAX);
        let Ok(oversized_layout) = Layout::from_size_align(oversized, 16) else {
            return;
        };
        // SAFETY: this call is expected to return null (the layout intentionally exceeds the
        // ceiling), so there is no pointer to deallocate afterwards.
        let oversized_ptr = unsafe { allocator.alloc(oversized_layout) };
        assert!(oversized_ptr.is_null());
        assert_eq!(ACTIVE_BYTES.load(Ordering::SeqCst), 0);
    }
}
