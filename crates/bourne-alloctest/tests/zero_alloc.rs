//! Zero-allocation guarantees.
//!
//! All assertions live inside one `#[test]` function so they execute
//! sequentially within a single test thread. The counting allocator is
//! global; if cargo ran two of these tests in parallel, each would observe
//! the other's allocations and report false positives.
//!
//! If any of these assertions fail in CI, we have shipped a regression
//! against the zero-copy story.

use bourne::parse;
use bourne_alloctest::measure;
use bourne_core::Parser;

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
        assert_eq!(delta.allocs, 0, "Option<&str> Some parse allocated {delta:?}");
    }
    {
        let (v, delta) = measure(|| parse::<Option<&str>>(b"null").unwrap());
        assert_eq!(v, None);
        assert_eq!(delta.allocs, 0, "Option<&str> None parse allocated {delta:?}");
    }

    // Vec<f64> — locks in that the fused `parse_f64_value` fast path
    // does not allocate per element. The only allocations should be
    // the Vec's growth steps (push-driven amortized doubling), which
    // for 1024 elements is ~10 reallocations starting from 0. Pin the
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
        // Inline mini-fixture so this test crate doesn't pick up the
        // whole bench dep tree just for a corpus-builder helper. Mix
        // covers integer, fractional, and exponent forms so every
        // branch of `parse_f64_value` is touched.
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

/// Build a JSON array of `n` floats covering the integer / fractional /
/// exponent / signed-exponent shapes. Mirrors `bourne_bench::float_array`
/// but lives here so this crate avoids the bench-side dep tree.
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
