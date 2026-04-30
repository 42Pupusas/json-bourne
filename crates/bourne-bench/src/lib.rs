//! Shared bench fixtures.
//!
//! Lives in a normal lib crate so each bench file can `use bourne_bench::*`
//! instead of duplicating the corpus inline.

use core::fmt::Write as _;

/// A representative "twitter-ish" small object — keys, mixed scalars, nesting.
pub const SMALL_OBJECT: &str = r#"{"id":1234567890,"name":"alice","verified":true,"followers":42,"bio":null,"links":["a","b","c"]}"#;

/// A flat array of N integers as a `String`. Useful for measuring number-throughput
/// without object/key overhead.
#[must_use]
pub fn int_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 8);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, "{i}");
    }
    s.push(']');
    s
}

/// A flat array of N short strings without escapes — exercises the borrowed-string fast path.
#[must_use]
pub fn string_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 12);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, "\"item-{i:08}\"");
    }
    s.push(']');
    s
}

/// A flat array of N strings with `\n` escape — exercises the validation path.
#[must_use]
pub fn escaped_string_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 16);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, r#""line-{i}\nwrap""#);
    }
    s.push(']');
    s
}

/// `[[[…]]]` `depth` levels deep with a single `1` at the bottom. Stresses the nesting stack.
#[must_use]
pub fn deep_nesting(depth: usize) -> String {
    let mut s = String::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        s.push('[');
    }
    s.push('1');
    for _ in 0..depth {
        s.push(']');
    }
    s
}
