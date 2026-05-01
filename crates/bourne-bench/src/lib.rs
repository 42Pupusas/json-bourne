//! Shared bench fixtures.
//!
//! Lives in a normal lib crate so each bench file can `use bourne_bench::*`
//! instead of duplicating the corpus inline.
//!
//! Submodules:
//!   - [`pathological`] — well-formed inputs that hit edge cases (wide
//!     integers, max-depth nesting, huge int literals).
//!   - [`realistic`] — corpora shaped like real production JSON (GitHub
//!     events, log lines, geo data, mixed-length strings, unicode bodies).
//!   - [`malformed`] — adversarial / malformed inputs the parser must
//!     reject without panicking. Used to measure rejection latency.

// Fixture-generation casts. This crate produces test inputs from
// synthetic indices like `i % 360` cast to f64 / u32; the casts are
// the *numeric content* of the corpus, and the loss the lint warns
// about cannot occur for the `i % small_constant` shape used here.
// Allowing crate-wide is more honest than per-line `#[allow]`s on
// every fixture builder.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

extern crate alloc;

pub mod malformed;
pub mod pathological;
pub mod realistic;

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

/// A flat array of N floats covering integer, fraction, and exponent forms
/// — exercises the slowest number path.
#[must_use]
pub fn float_array(n: usize) -> String {
    // Five forms repeated mod 5 keeps the lexer touching every branch
    // (integer, leading-`-`, fraction, exponent, sign-on-exponent).
    const SAMPLES: [&str; 5] = ["1.5e10", "-2.7e-5", "3.14159", "0.0", "1e100"];
    let mut s = String::with_capacity(n * 8);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(SAMPLES[i % SAMPLES.len()]);
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

/// A flat array of N 128-bit integer literals near the i128 limit.
///
/// Picks a value wider than `u64::MAX` so the parser cannot fall back to
/// the `as_i64` fast path; every element exercises the bespoke
/// `as_i128` decode.
#[must_use]
pub fn i128_array(n: usize) -> String {
    // ~31 decimal digits — well past 64-bit but inside i128 range.
    const SAMPLE: &str = "1234567890123456789012345678";
    let mut s = String::with_capacity(n * (SAMPLE.len() + 1));
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(SAMPLE);
    }
    s.push(']');
    s
}

/// An object of N entries: `{"k0":0,"k1":1,…}` — stable, escape-free
/// keys. Measures map-build throughput on the borrowed-key fast path.
#[must_use]
pub fn small_keyed_object(n: usize) -> String {
    let mut s = String::with_capacity(n * 16);
    s.push('{');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, "\"k{i}\":{i}");
    }
    s.push('}');
    s
}

/// An object whose keys all carry a `\n` escape sequence — exercises
/// the decode-on-key path. Pairs with `small_keyed_object` to quantify
/// the escape-decode overhead per key.
#[must_use]
pub fn small_object_escaped_keys(n: usize) -> String {
    let mut s = String::with_capacity(n * 24);
    s.push('{');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // Wire form contains backslash-n; decoded key is "k\n0", "k\n1", ...
        let _ = write!(&mut s, "\"k\\n{i}\":{i}");
    }
    s.push('}');
    s
}

/// A flat array of N float-seconds suitable for `Duration` decode.
/// Mixes integer, fractional, and large values to keep the slow paths
/// honest.
#[must_use]
pub fn duration_seconds_array(n: usize) -> String {
    // Mix avoids degenerate optimization on a single shape.
    const SAMPLES: [&str; 5] = ["0.0", "1.5", "60.25", "3600.123", "86400"];
    let mut s = String::with_capacity(n * 8);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str(SAMPLES[i % SAMPLES.len()]);
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
