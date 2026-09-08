//! `Vec<T>::vec_from_lex` fast-path coverage. The default
//! `FromJson::vec_from_lex` impl drives `from_lex` per element; the
//! primitive types (`&str`, integers via macros, `Duration`) override
//! it to skip the per-element Event detour. These overrides have
//! independent code paths from the default and need their own tests.

use crate::parse_str;
use alloc::vec::Vec;

/// `Vec<&str>::vec_from_lex` — borrowed strings, fused fast path.
#[test]
fn vec_borrowed_str_fast_path() {
    // Empty array — early-return branch.
    let v: Vec<&str> = parse_str("[]").expect("empty");
    assert_eq!(v, Vec::<&str>::new());

    // Single element — first push only, no while loop.
    let v: Vec<&str> = parse_str(r#"["only"]"#).expect("single");
    assert_eq!(v, ["only"]);

    // Many elements — exercises the while-loop body.
    let v: Vec<&str> = parse_str(r#"["a","b","c","d","e"]"#).expect("many");
    assert_eq!(v, ["a", "b", "c", "d", "e"]);

    // Reject escape-bearing string (the borrowed path requires no escapes).
    let r: Result<Vec<&str>, _> = parse_str(r#"["plain","esc\nbad"]"#);
    assert!(r.is_err(), "borrowed path must reject escape");
}

/// `Vec<Duration>::vec_from_lex` — fused, with non-finite / negative rejection.
#[cfg(feature = "std")]
#[test]
fn vec_duration_fast_path() {
    use std::time::Duration;

    // Empty — early return.
    let v: Vec<Duration> = parse_str("[]").expect("empty");
    assert!(v.is_empty());

    // Single — first push.
    let v: Vec<Duration> = parse_str("[1.5]").expect("single");
    assert_eq!(v, [Duration::from_secs_f64(1.5)]);

    // Many — loop body.
    let v: Vec<Duration> = parse_str("[0.0,1.0,1.5,2.25,100.125]").expect("many");
    assert_eq!(v.len(), 5);
    assert_eq!(v[0], Duration::ZERO);
    assert_eq!(v[4], Duration::from_secs_f64(100.125));

    // Negative — rejected.
    let r: Result<Vec<Duration>, _> = parse_str("[1.0,-2.0]");
    assert!(r.is_err());
}

/// `Vec<i64>::vec_from_lex` — exercises the macro-generated override
/// in `de.rs::impl_int!`, including the bounds check on the empty branch.
#[test]
fn vec_i64_fast_path() {
    let v: Vec<i64> = parse_str("[]").expect("empty");
    assert!(v.is_empty());

    let v: Vec<i64> = parse_str("[42]").expect("single");
    assert_eq!(v, [42]);

    let v: Vec<i64> = parse_str("[1,-1,9223372036854775807,-9223372036854775808]").expect("many");
    assert_eq!(v, [1, -1, i64::MAX, i64::MIN]);

    // Out-of-range for u32 should error.
    let r: Result<Vec<u32>, _> = parse_str("[1,99999999999]");
    assert!(r.is_err(), "u32 should overflow");
}

/// `Vec<u128>::vec_from_lex` — wide-int macro path.
#[test]
fn vec_u128_fast_path() {
    let v: Vec<u128> = parse_str("[]").expect("empty");
    assert!(v.is_empty());

    let v: Vec<u128> = parse_str("[0,170141183460469231731687303715884105727]").expect("many");
    assert_eq!(v, [0u128, i128::MAX as u128]);
}
