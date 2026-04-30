//! A `#[global_allocator]` that counts allocations and bytes.
//!
//! Wraps the system allocator with atomic counters. Tests can snapshot
//! before/after a region and assert that no allocation happened — locking
//! in `bourne`'s zero-alloc guarantee as a regression gate, not just an
//! aspirational claim in the docs.
//!
//! The allocator is global, so this crate exists *only* for these tests.
//! It is not published.

// Implementing GlobalAlloc requires unsafe; this is the only crate in the
// workspace that does, and the only place this attribute appears.
#![allow(unsafe_code)]

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::System;

#[derive(Debug, Default)]
pub struct CountingAllocator {
    pub allocs: AtomicUsize,
    pub bytes: AtomicUsize,
}

// SAFETY: forwards every allocation request to the system allocator unchanged;
// only side effect is incrementing the counters, which is `Relaxed` and cannot
// affect memory safety.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: layout is a valid Layout; System upholds GlobalAlloc invariants.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr/layout came from a paired alloc; forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarded with the original layout.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        // Charge only the growth, since the existing region is already counted.
        self.bytes.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        // SAFETY: ptr/layout came from a paired alloc; new_size respects realloc rules.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
pub static ALLOCATOR: CountingAllocator = CountingAllocator {
    allocs: AtomicUsize::new(0),
    bytes: AtomicUsize::new(0),
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub allocs: usize,
    pub bytes: usize,
}

impl Snapshot {
    pub fn now() -> Self {
        Self {
            allocs: ALLOCATOR.allocs.load(Ordering::Relaxed),
            bytes: ALLOCATOR.bytes.load(Ordering::Relaxed),
        }
    }

    #[must_use]
    pub const fn delta_since(self, earlier: Self) -> Self {
        Self {
            allocs: self.allocs - earlier.allocs,
            bytes: self.bytes - earlier.bytes,
        }
    }
}

/// Run `f` and return the allocations it caused.
///
/// IMPORTANT: the counters are global. If multiple threads allocate
/// concurrently with `f`, the delta will include their allocations too.
/// Tests using this MUST run with `--test-threads=1` (set via
/// `.cargo/config.toml` for this crate). Concurrent test execution is the
/// only realistic source of false positives — `bourne` itself does not
/// spawn threads from a parse.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Snapshot) {
    let before = Snapshot::now();
    let result = f();
    let after = Snapshot::now();
    (result, after.delta_since(before))
}
