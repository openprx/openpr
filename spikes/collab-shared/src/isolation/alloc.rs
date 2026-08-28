//! The counting `GlobalAlloc` that is the *online enforcer* for the 128 MiB memory ceiling
//! (ADR-0014 section 1's table: "计数分配器自身...先把 attempted peak 与 cause 写进共享页，再返回 null").
//!
//! This type is deliberately **not** installed via `#[global_alloc]` in this library crate — only
//! `src/bin/isolation_calibrate.rs` does that, so linking `collab-shared` from the `collab-loro-spike`
//! / `collab-yrs-yjs-spike` crates (or from `collab-shared`'s own test binary) never swaps out their
//! allocator. The counting logic lives here so it can be unit-tested without needing to be the
//! process's actual global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::isolation::shared_page::SharedPageLayout;
use crate::isolation::termination::ISOLATED_APPLY_MEMORY_BYTES_MAX;

/// Saturate recorded "attempted peak" at 4x the ceiling rather than let a maliciously huge
/// requested size wrap a `u64` (ADR-0014 section 3: "`u64` 在超过上限 4 倍时饱和而非回绕").
const ATTEMPTED_PEAK_SATURATION_BYTES: u64 = ISOLATED_APPLY_MEMORY_BYTES_MAX * 4;

/// Process-local running total of active (not-yet-freed) bytes. Kept separate from the
/// shared-page high-water mark (`allocated_active_peak_bytes`) because this counter needs to go
/// both up and down as allocations free, while the shared-page field only ever tracks the
/// high-water mark via `fetch_max`.
static ACTIVE_BYTES: AtomicU64 = AtomicU64::new(0);

/// Set once (by `arm_for_case`) to the shared page this allocator should record into. `null`
/// means "not armed yet" — allocations before arming (process startup, argv parsing, etc.) are
/// simply not tracked, matching ADR-0014 section 1.3's exclusion of pre-window setup from the
/// metered window.
static TARGET_PAGE: AtomicPtr<SharedPageLayout> = AtomicPtr::new(ptr::null_mut());

/// Points `CountingAllocator` at the shared page for the case about to run, and resets the
/// process-local active-byte counter to zero.
///
/// Must be called once per case, after `SharedPageLayout::reset_for_next_case` and before any
/// case-specific allocation.
pub fn arm_for_case(page: &SharedPageLayout) {
    ACTIVE_BYTES.store(0, Ordering::SeqCst);
    TARGET_PAGE.store(ptr::from_ref(page).cast_mut(), Ordering::SeqCst);
}

fn target_page() -> Option<NonNull<SharedPageLayout>> {
    NonNull::new(TARGET_PAGE.load(Ordering::SeqCst))
}

fn record_attempted_peak(page: &SharedPageLayout, attempted_bytes: u64) {
    let saturated = attempted_bytes.min(ATTEMPTED_PEAK_SATURATION_BYTES);
    page.allocated_attempted_peak_bytes
        .fetch_max(saturated, Ordering::AcqRel);
    page.memory_cause_flag.store(1, Ordering::SeqCst);
}

fn record_active_peak(page: &SharedPageLayout, active_bytes: u64) {
    page.allocated_active_peak_bytes
        .fetch_max(active_bytes, Ordering::AcqRel);
}

/// A `GlobalAlloc` that enforces `ISOLATED_APPLY_MEMORY_BYTES_MAX` as the *sole* memory gate
/// quantity (ADR-0014 section 3).
///
/// An allocation that would push the process's active byte count past the ceiling is rejected —
/// with the attempted peak and cause flag written to the shared page *before* rejecting, so a
/// since-killed case's peak is still legible — without ever calling into the system allocator for
/// that request. This type never allocates on its own behalf; every accounting operation is a
/// fixed-size atomic read/write against `TARGET_PAGE`/`ACTIVE_BYTES`.
pub struct CountingAllocator;

// SAFETY: `alloc`/`dealloc`/`realloc` all delegate the actual memory operation to `System`
// (the platform allocator, which already correctly implements `GlobalAlloc`'s contract for valid
// non-zero-sized `Layout`s) and only add atomic bookkeeping around it. The bookkeeping itself
// touches no allocator-owned memory and performs no allocation, so it cannot recursively invoke
// this allocator. `alloc_zeroed` is left at its default trait implementation, which is defined
// in terms of `self.alloc` and is therefore covered by the same reasoning.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(page_ptr) = target_page() else {
            // Not armed for a case yet (startup allocations) — pass through uncounted.
            // SAFETY: `layout` is exactly the caller-supplied `Layout`, forwarded unchanged to
            // `System`, whose `GlobalAlloc::alloc` has the identical safety contract as this
            // method (the standard passthrough allocator wrapper pattern).
            return unsafe { System.alloc(layout) };
        };
        // SAFETY: `page_ptr` was stored by `arm_for_case` from a live `&SharedPageLayout`
        // reference that outlives the case (the caller holds the `SharedPage` for the whole
        // case), so dereferencing it here for the duration of this call is sound.
        let page = unsafe { page_ptr.as_ref() };

        let requested = layout.size() as u64;
        let prospective_active = ACTIVE_BYTES.load(Ordering::SeqCst).saturating_add(requested);
        if prospective_active > ISOLATED_APPLY_MEMORY_BYTES_MAX {
            record_attempted_peak(page, prospective_active);
            return ptr::null_mut();
        }

        // SAFETY: see the `unsafe impl` block comment above — `layout` is forwarded unchanged.
        let allocated = unsafe { System.alloc(layout) };
        if !allocated.is_null() {
            let new_active = ACTIVE_BYTES.fetch_add(requested, Ordering::SeqCst) + requested;
            record_active_peak(page, new_active);
        }
        allocated
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` are exactly the arguments the caller must supply per
        // `GlobalAlloc::dealloc`'s contract (a pointer previously returned by this allocator's
        // `alloc`/`realloc` with the same layout), forwarded unchanged to `System`.
        unsafe { System.dealloc(ptr, layout) };
        let freed = layout.size() as u64;
        ACTIVE_BYTES
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                Some(active.saturating_sub(freed))
            })
            .ok();
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some(page_ptr) = target_page() else {
            // SAFETY: passthrough per `GlobalAlloc::realloc`'s contract, arguments forwarded
            // unchanged.
            return unsafe { System.realloc(ptr, layout, new_size) };
        };
        // SAFETY: see `alloc` above — `page_ptr` outlives the case.
        let page = unsafe { page_ptr.as_ref() };

        let old_size = layout.size() as u64;
        let requested_new = new_size as u64;
        let current_active = ACTIVE_BYTES.load(Ordering::SeqCst);
        // ADR-0014 section 3: "`realloc` 按差值调整" — only the delta beyond the existing
        // allocation's own size needs to fit under the ceiling.
        let prospective_active = current_active.saturating_sub(old_size).saturating_add(requested_new);
        if requested_new > old_size && prospective_active > ISOLATED_APPLY_MEMORY_BYTES_MAX {
            record_attempted_peak(page, prospective_active);
            return ptr::null_mut();
        }

        // SAFETY: `ptr`/`layout`/`new_size` are exactly the arguments the caller must supply per
        // `GlobalAlloc::realloc`'s contract, forwarded unchanged to `System`.
        let reallocated = unsafe { System.realloc(ptr, layout, new_size) };
        if !reallocated.is_null() {
            let new_active = current_active.saturating_sub(old_size).saturating_add(requested_new);
            ACTIVE_BYTES.store(new_active, Ordering::SeqCst);
            record_active_peak(page, new_active);
        }
        reallocated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation::shared_page::SharedPage;

    #[test]
    fn alloc_below_ceiling_updates_active_peak_and_dealloc_frees_it() {
        let Ok(page) = SharedPage::map() else { return };
        arm_for_case(page.layout());
        let allocator = CountingAllocator;
        let layout = Layout::from_size_align(4096, 8).unwrap_or(Layout::new::<u8>());

        // SAFETY: `layout` is a valid non-zero-sized layout constructed above; this test owns
        // the returned pointer exclusively and deallocates it with the same layout below.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null());
        assert_eq!(page.layout().allocated_active_peak_bytes.load(Ordering::SeqCst), 4096);
        assert_eq!(page.layout().memory_cause_flag.load(Ordering::SeqCst), 0);

        // SAFETY: `ptr` was returned by `alloc` above with this exact `layout`, and is freed
        // exactly once here.
        unsafe { allocator.dealloc(ptr, layout) };
        assert_eq!(ACTIVE_BYTES.load(Ordering::SeqCst), 0);
        // The high-water mark is a peak, not a live gauge, so it must not drop after freeing.
        assert_eq!(page.layout().allocated_active_peak_bytes.load(Ordering::SeqCst), 4096);
    }

    #[test]
    fn alloc_past_ceiling_is_rejected_and_records_attempted_peak_before_returning_null() {
        let Ok(page) = SharedPage::map() else { return };
        arm_for_case(page.layout());
        let allocator = CountingAllocator;
        let oversized = ISOLATED_APPLY_MEMORY_BYTES_MAX + 1;
        // `oversized` is the memory ceiling plus 1, far below usize::MAX on every real target; a
        // saturated fallback would still exceed the ceiling, which is this test's whole point.
        let oversized_usize = usize::try_from(oversized).unwrap_or(usize::MAX);
        let Ok(layout) = Layout::from_size_align(oversized_usize, 8) else {
            return;
        };

        // SAFETY: this call is expected to return null (the layout intentionally exceeds the
        // ceiling), so there is no pointer to deallocate afterwards.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(ptr.is_null());
        assert_eq!(page.layout().memory_cause_flag.load(Ordering::SeqCst), 1);
        assert_eq!(
            page.layout().allocated_attempted_peak_bytes.load(Ordering::SeqCst),
            oversized
        );
        // Nothing was actually allocated, so the active peak must stay at zero.
        assert_eq!(page.layout().allocated_active_peak_bytes.load(Ordering::SeqCst), 0);
    }
}
