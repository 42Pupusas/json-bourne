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
}
