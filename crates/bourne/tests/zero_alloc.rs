//! Zero-allocation guarantees.
//!
//! Locks in `bourne`'s zero-copy story as a regression gate, not just an
//! aspirational doc claim. The counting allocator is installed as the
//! `#[global_allocator]` for this test binary only — each `tests/*.rs` is
//! its own binary in Cargo, so other integration tests are unaffected.
//!
//! All assertions live inside a single `#[test]` function so they execute
//! sequentially within a single test thread. The counter is global; if
//! cargo ran two of these in parallel, each would observe the other's
//! allocations and report false positives.

// The counting allocator implements GlobalAlloc, which is fundamentally
// unsafe. This is the only place in the workspace that bypasses the
// `unsafe_code = "deny"` lint, and it's confined to a single test file.
#![allow(unsafe_code)]
#![cfg(feature = "std")]

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::System;

use bourne::parse;
use bourne::Parser;

struct CountingAllocator {
    allocs: AtomicUsize,
    bytes: AtomicUsize,
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
        self.bytes
            .fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        // SAFETY: ptr/layout came from a paired alloc; new_size respects realloc rules.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator {
    allocs: AtomicUsize::new(0),
    bytes: AtomicUsize::new(0),
};

#[derive(Copy, Clone, Debug)]
struct Snapshot {
    allocs: usize,
    bytes: usize,
}

impl Snapshot {
    fn now() -> Self {
        Self {
            allocs: ALLOCATOR.allocs.load(Ordering::Relaxed),
            bytes: ALLOCATOR.bytes.load(Ordering::Relaxed),
        }
    }

    const fn delta_since(self, earlier: Self) -> Self {
        Self {
            allocs: self.allocs - earlier.allocs,
            bytes: self.bytes - earlier.bytes,
        }
    }
}

fn measure<R>(f: impl FnOnce() -> R) -> (R, Snapshot) {
    let before = Snapshot::now();
    let result = f();
    let after = Snapshot::now();
    (result, after.delta_since(before))
}

#[test]
fn zero_alloc_guarantees() {
    // Streaming.
    {
        let input = br#"{"id":42,"name":"alice","tags":["a","b","c"]}"#;
        let (_, delta) = measure(|| {
            let mut p: Parser<'_> = Parser::new(input);
            let mut count = 0;
            while p.next_event().unwrap().is_some() {
                count += 1;
            }
            count
        });
        assert_eq!(delta.allocs, 0, "streaming parse allocated {delta:?}");
    }

    // Borrowed &str — input outlives the parsed value, no copy.
    {
        let input = br#""hello world""#;
        let (s, delta) = measure(|| parse::<&str>(input).unwrap());
        assert_eq!(s, "hello world");
        assert_eq!(delta.allocs, 0, "borrowed &str parse allocated {delta:?}");
    }

    // Primitives.
    let (_, d1) = measure(|| parse::<u64>(b"12345").unwrap());
    let (_, d2) = measure(|| parse::<i64>(b"-12345").unwrap());
    let (_, d3) = measure(|| parse::<f64>(b"1.5e10").unwrap());
    let (_, d4) = measure(|| parse::<bool>(b"true").unwrap());
    assert_eq!(d1.allocs, 0, "u64 parse allocated {d1:?}");
    assert_eq!(d2.allocs, 0, "i64 parse allocated {d2:?}");
    assert_eq!(d3.allocs, 0, "f64 parse allocated {d3:?}");
    assert_eq!(d4.allocs, 0, "bool parse allocated {d4:?}");

    // Fixed-size array of primitives — `[Option<T>; N]` is on the stack.
    {
        let (arr, delta) = measure(|| parse::<[i32; 5]>(b"[1,2,3,4,5]").unwrap());
        assert_eq!(arr, [1, 2, 3, 4, 5]);
        assert_eq!(delta.allocs, 0, "[i32; 5] parse allocated {delta:?}");
    }

    // Tuple of mixed primitives + borrowed &str.
    {
        let input = br#"[1, "hi", true]"#;
        let (v, delta) = measure(|| parse::<(i32, &str, bool)>(input).unwrap());
        assert_eq!(v, (1, "hi", true));
        assert_eq!(delta.allocs, 0, "tuple parse allocated {delta:?}");
    }

    // Option<&str> — both branches.
    {
        let (v, delta) = measure(|| parse::<Option<&str>>(br#""x""#).unwrap());
        assert_eq!(v, Some("x"));
        assert_eq!(
            delta.allocs, 0,
            "Option<&str> Some parse allocated {delta:?}"
        );
    }
    {
        let (v, delta) = measure(|| parse::<Option<&str>>(b"null").unwrap());
        assert_eq!(v, None);
        assert_eq!(
            delta.allocs, 0,
            "Option<&str> None parse allocated {delta:?}"
        );
    }

    // Vec<f64> — locks in that the fused `parse_f64_value` fast path
    // does not allocate per element. The only allocations should be
    // the Vec's growth steps (push-driven amortized doubling). Pin the
    // exact count against `Vec<i64>` over the same shape — they share
    // the same `vec_from_lex` scaffold and must allocate identically.
    {
        let f_input = b"[0.0,1.5,2.7,3.14,4.2,5.0,6.28,7.5,8.0,9.9]";
        let i_input = b"[0,1,2,3,4,5,6,7,8,9]";
        let (vf, df) = measure(|| parse::<Vec<f64>>(f_input).unwrap());
        let (vi, di) = measure(|| parse::<Vec<i64>>(i_input).unwrap());
        assert_eq!(vf.len(), 10);
        assert_eq!(vi.len(), 10);
        assert_eq!(
            df.allocs, di.allocs,
            "Vec<f64> alloc count {df:?} should match Vec<i64> {di:?} \
             — both go through fused vec_from_lex and pay only Vec growth",
        );
    }

    // Larger Vec<f64> — confirm the per-element zero-alloc property
    // scales. With 1024 elements, the only allocations should be the
    // Vec growth chain (capacity 4 → 8 → 16 → … → 1024 = 9 reallocs
    // plus the initial alloc, so 10 total). If `parse_f64_value`
    // accidentally allocates per element, this jumps to 1024+.
    {
        let big = build_float_array(1024);
        let (v, delta) = measure(|| parse::<Vec<f64>>(big.as_bytes()).unwrap());
        assert_eq!(v.len(), 1024);
        assert!(
            delta.allocs <= 16,
            "Vec<f64>/1024 allocated {delta:?} — expected only Vec growth (≤16), \
             a higher count means parse_f64_value is leaking a per-element alloc",
        );
    }
}

/// Build a JSON array of `n` floats covering integer / fractional /
/// exponent / signed-exponent shapes.
fn build_float_array(n: usize) -> String {
    use std::fmt::Write as _;
    const SAMPLES: [&str; 5] = ["1.5e10", "-2.7e-5", "3.14159", "0.0", "1e100"];
    let mut s = String::with_capacity(n * 8);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, "{}", SAMPLES[i % SAMPLES.len()]);
    }
    s.push(']');
    s
}
