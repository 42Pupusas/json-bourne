//! Unsigned integer parsing across every entry path.
//!
//! Regression coverage for audit finding 3.3: `Vec<u64>` and derived
//! `u64` fields used to route through the signed `parse_i64_value` fast
//! path and reject every value above `i64::MAX`, while top-level
//! `parse::<u64>` (via `JsonNum::as_u64`) accepted them. They also
//! accepted `-0`, which `as_u64` rejected — the two paths disagreed in
//! both directions.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, FromJson, parse_str};

#[derive(Debug, PartialEq, FromJson)]
struct Record {
    id: u64,
    size: usize,
    small: u8,
}

#[test]
fn top_level_u64_accepts_full_range() {
    assert_eq!(parse_str::<u64>("18446744073709551615").unwrap(), u64::MAX);
    assert_eq!(
        parse_str::<u64>("9223372036854775808").unwrap(),
        i64::MAX as u64 + 1
    );
    assert_eq!(parse_str::<u64>("0").unwrap(), 0);
}

#[test]
fn vec_u64_accepts_full_range() {
    let v: Vec<u64> = parse_str("[0,1,9223372036854775808,18446744073709551615]").unwrap();
    assert_eq!(v, vec![0, 1, i64::MAX as u64 + 1, u64::MAX]);
}

#[test]
fn derived_u64_field_accepts_full_range() {
    let r: Record =
        parse_str(r#"{"id":18446744073709551615,"size":18446744073709551615,"small":255}"#)
            .unwrap();
    assert_eq!(r.id, u64::MAX);
    assert_eq!(r.size, usize::MAX);
    assert_eq!(r.small, 255);
}

#[test]
fn u64_range_rejections_are_consistent() {
    // One past u64::MAX must be rejected at top level, in a Vec, and in
    // a derived field alike.
    assert!(parse_str::<u64>("18446744073709551616").is_err());
    let r: Result<Vec<u64>, _> = parse_str("[18446744073709551616]");
    assert!(r.is_err());
    let r: Result<Record, _> = parse_str(r#"{"id":18446744073709551616,"size":1,"small":1}"#);
    assert!(r.is_err());
}

#[test]
fn negative_zero_is_rejected_consistently() {
    // Top-level as_u64 always rejected `-0`; the fused Vec path used to
    // accept it. Both paths now reject: `-0` is negative and u64 has no
    // negation of zero.
    assert!(parse_str::<u64>("-0").is_err());
    let r: Result<Vec<u64>, _> = parse_str("[-0]");
    assert_eq!(
        r.err().map(|e| e.kind),
        Some(ErrorKind::NumberOutOfRange),
        "Vec<u64> must reject -0 exactly like top-level u64"
    );
    let r: Result<Record, _> = parse_str(r#"{"id":-0,"size":1,"small":1}"#);
    assert!(r.is_err());
}

#[test]
fn negative_values_are_rejected_for_all_unsigned_paths() {
    assert!(parse_str::<u64>("-1").is_err());
    let r: Result<Vec<u64>, _> = parse_str("[-1]");
    assert!(r.is_err());
    let r: Result<Record, _> = parse_str(r#"{"id":-1,"size":1,"small":1}"#);
    assert!(r.is_err());
}

#[test]
fn fractional_rejection_is_consistent_with_signed_path() {
    for literal in ["1.5", "1e2", "0.0", "1E2"] {
        assert!(parse_str::<u64>(literal).is_err(), "{literal}");
        let r: Result<Vec<u64>, _> = parse_str(&format!("[{literal}]"));
        assert!(r.is_err(), "[{literal}]");
    }
}

#[test]
fn unsigned_vec_fast_path_matches_jsonnum_path_on_proptest_shapes() {
    // Spot-grid around the two boundaries the fast paths branch on:
    // 19/20-digit values and the i64::MAX edge.
    let cases: [u64; 9] = [
        0,
        1,
        9,
        u64::from(u32::MAX),
        4_294_967_296,
        1_000_000_000_000_000_000,
        i64::MAX as u64,
        i64::MAX as u64 + 1,
        u64::MAX,
    ];
    let json: Vec<String> = cases.iter().map(ToString::to_string).collect();
    let joined = format!("[{}]", json.join(","));
    let parsed: Vec<u64> = parse_str(&joined).unwrap();
    assert_eq!(parsed, cases);
}
