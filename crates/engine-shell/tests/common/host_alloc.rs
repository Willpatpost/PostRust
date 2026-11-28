//! Counts Rust allocations and requested bytes on the measuring thread.
//! Const-initialized, destructor-free cells keep the allocator hook allocation-free.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static TOTALS: Cell<(u64, usize)> = const { Cell::new((0, 0)) };
}

#[global_allocator]
static HOST_ALLOC_COUNTER: HostAllocCounter = HostAllocCounter;

pub struct HostAllocScope;

impl HostAllocScope {
    pub fn new() -> Self {
        TOTALS.set((0, 0));
        MEASURING.set(true);
        Self
    }

    pub fn totals(&self) -> (u64, usize) {
        TOTALS.get()
    }
}

impl Drop for HostAllocScope {
    fn drop(&mut self) {
        MEASURING.set(false);
    }
}

fn record(bytes: usize) {
    if MEASURING.get() {
        let (count, total_bytes) = TOTALS.get();
        TOTALS.set((count.wrapping_add(1), total_bytes.wrapping_add(bytes)));
    }
}

struct HostAllocCounter;

#[allow(unsafe_code)]
// SAFETY: every method passes its arguments unchanged to System. The counting
// hook only updates thread-local cells and cannot re-enter the allocator.
unsafe impl GlobalAlloc for HostAllocCounter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(ptr, old_layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
