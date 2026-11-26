//! The host-allocation counter (corrective finding 10's missing
//! gate — shared by the lavapipe gate binaries; `tests/common/` is
//! the cargo convention for test-support modules that are NOT
//! standalone test targets, so this adds no result line anywhere).
//!
//! `renderer.allocations()` counts Vulkan objects; it cannot see
//! heap construction on the Rust side. This module is the beside-it
//! counter the review asked for: a `#[global_allocator]` wrapper
//! around `std::alloc::System` that counts `alloc`/`alloc_zeroed`
//! calls on the CURRENT THREAD while a `HostAllocScope` guard is
//! alive — the warmed render under measurement runs between the
//! scope's creation and its `count()`, and everything else (capture
//! extraction, test assertions, startup) stays outside.
//!
//! SAFETY (the `unsafe impl GlobalAlloc`): every method delegates
//! to `std::alloc::System` unchanged, inheriting its guarantees;
//! the counting state is `thread_local!` `Cell`s incremented with
//! no allocation inside the allocator path (no reentrancy hazard);
//! `thread_local!` access inside `alloc` is sound for the
//! pre-initialized non-const thread-local form used here (the
//! fast-path check reads a `Cell<bool>` without destructors, so no
//! registration/alloc can occur on first access).

// The workspace denies `unsafe_code` (engine-renderer overrides it
// where Vulkan lives); this test-support module needs the one
// `unsafe impl GlobalAlloc` that a counting allocator IS — the
// scoped allow keeps the deny everywhere else in the test crates.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static ALLOC_COUNT: Cell<u64> = const { Cell::new(0) };
}

#[global_allocator]
static HOST_ALLOC_COUNTER: HostAllocCounter = HostAllocCounter;

/// Enables allocation counting for the current thread until
/// dropped. The count starts at zero on creation; `count()` reads
/// it at any point inside the scope.
pub struct HostAllocScope;

impl HostAllocScope {
    pub fn new() -> Self {
        ALLOC_COUNT.with(|c| c.set(0));
        MEASURING.with(|m| m.set(true));
        HostAllocScope
    }

    /// The allocations counted so far in this scope.
    pub fn count(&self) -> u64 {
        ALLOC_COUNT.with(|c| c.get())
    }
}

impl Drop for HostAllocScope {
    fn drop(&mut self) {
        MEASURING.with(|m| m.set(false));
    }
}

struct HostAllocCounter;

unsafe impl GlobalAlloc for HostAllocCounter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        MEASURING.with(|m| {
            if m.get() {
                ALLOC_COUNT.with(|c| c.set(c.get().wrapping_add(1)));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        // THE REALLOC COUNT (the sign-off review's finding 22
        // corrective): a warmed scratch vector growing under
        // measurement is allocation churn the pre-corrective
        // counter never saw (the event forwarded to System
        // uncounted — a buffer could grow while the gate read
        // zero). Every realloc inside a measuring scope counts as
        // one event, exactly like a fresh alloc (the conservative,
        // fail-visible direction: an in-place shrink counts too).
        MEASURING.with(|m| {
            if m.get() {
                ALLOC_COUNT.with(|c| c.set(c.get().wrapping_add(1)));
            }
        });
        unsafe { System.realloc(ptr, old_layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        MEASURING.with(|m| {
            if m.get() {
                ALLOC_COUNT.with(|c| c.set(c.get().wrapping_add(1)));
            }
        });
        unsafe { System.alloc_zeroed(layout) }
    }
}
