//! Pathological-but-legal JSON corpora.
//!
//! These inputs are well-formed (every helper produces a parseable
//! document) but probe the corners that uniform fixtures miss:
//!   - integers wide enough to expose SIMD digit-scan wins
//!   - the full nesting depth budget
//!   - degenerate but legal shapes (millions of empty objects, arrays of
//!     null, single huge integer literal, etc.)
//!
//! The goal of these benches is *not* to brag about throughput. It's to
//! prevent the regressions that uniform fixtures hide. The SIMD digit
//! scan was reverted because `int_array(N)` produces 1-5 digit numbers
//! and the SIMD setup cost outpaced the win — but on `wide_int_array`
//! the same code is a clean win. Without this corpus we'd have shipped
//! the wrong conclusion.

use core::fmt::Write as _;

/// Array of N "wide" integers — 13-19 digits, the range where SIMD digit
/// scanning starts to pay. Mimics `unix_nano` timestamps, twitter-snowflake
/// IDs, or 64-bit u64 monotonic IDs.
///
/// Mix of widths: half are 13-digit (millisecond timestamps), half are 19
/// digits (i64::MAX-ish). All positive, no exponent.
#[must_use]
pub fn wide_int_array(n: usize) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(n * 22);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // 13-digit ~ ms timestamp range, 19-digit ~ i64-MAX range.
        // Cycle through so the lexer sees mixed widths.
        let v: u64 = if i % 2 == 0 {
            1_700_000_000_000_u64.wrapping_add(i as u64)
        } else {
            9_000_000_000_000_000_000_u64.wrapping_sub(i as u64)
        };
        let _ = write!(&mut s, "{v}");
    }
    s.push(']');
    s
}

/// A single JSON document containing one integer literal with `digits`
/// decimal digits. Tests the lexer's handling of arbitrary-precision
/// integer text — `parse_i64_value` rejects anything wider than i64 with
/// `NumberOutOfRange`, but `read_number` still has to walk the full
/// literal. 1000 digits surfaces any quadratic behavior.
#[must_use]
pub fn huge_int_literal(digits: usize) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(digits + 2);
    // Avoid leading-zero rejection: first digit is non-zero.
    s.push('1');
    for _ in 1..digits {
        s.push('7');
    }
    s
}

/// Array of N empty objects: `[{},{},…,{}]`. Stress-tests StartObject /
/// EndObject dispatch and frame push/pop without giving the value-parsing
/// path anything to do.
#[must_use]
pub fn empty_object_array(n: usize) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(n * 3 + 2);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{}");
    }
    s.push(']');
    s
}

/// Array of N nulls. The cheapest possible value — 4 bytes plus a comma.
/// Forces dispatch to be the bottleneck instead of value parsing.
#[must_use]
pub fn null_array(n: usize) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(n * 5);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push_str("null");
    }
    s.push(']');
    s
}

/// Maximum-legal nested array: `[ [ … [1] … ] ]` exactly at the parser's
/// `DEFAULT_MAX_DEPTH = 128` limit. One past this would be rejected as
/// `DepthLimitExceeded`. Verifies the depth-check itself isn't a bottleneck
/// at the boundary.
#[must_use]
pub fn max_depth_legal() -> alloc::string::String {
    crate::deep_nesting(128)
}

/// The longest legal i64 literal: `i64::MIN` text is `-9223372036854775808`
/// (20 chars including the sign). Wraps it in a single-element array so
/// the typed `Vec<i64>` fast path handles it. Exercises the slow-path
/// arithmetic in `parse_i64_value` (the count >= 18 branch with
/// `checked_*` arithmetic).
#[must_use]
pub fn longest_legal_i64_array() -> alloc::string::String {
    "[-9223372036854775808]".to_string()
}

/// Array with a mix of legal i64 widths spanning the full range:
/// 1-digit, ~10-digit (32-bit boundary), 19-digit (i64 boundary). One of
/// each per group of 3, repeated. The regression we want to catch: any
/// path that's fast for one width and slow for another.
#[must_use]
pub fn mixed_width_int_array(groups: usize) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(groups * 36);
    s.push('[');
    for g in 0..groups {
        if g > 0 {
            s.push(',');
        }
        // 1-digit, ~10-digit, 19-digit.
        s.push_str("7,2147483647,9223372036854775807");
    }
    s.push(']');
    s
}

extern crate alloc;
