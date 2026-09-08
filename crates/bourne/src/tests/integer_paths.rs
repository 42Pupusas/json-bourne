//! Lexer-level tests for the fused integer parsers: `parse_i64_value`,
//! `parse_u64_value`, and `parse_i128_value` against `str::parse`, plus
//! the range boundaries and cursor-stop behaviour.

use crate::{ErrorKind, Lexer};

const fn lex(input: &[u8]) -> Lexer<'_> {
    Lexer::new(input)
}

#[test]
fn i64_value_matches_str_parse_chunked() {
    for n in [1u64, 9, 12_345_678, 99_999_999, 100_000_000, 123_456_789] {
        let s = format!("{n}");
        let want = i64::try_from(n).unwrap();
        assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), want);
        let s = format!("-{n}");
        assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), -want);
    }
}

#[test]
fn i64_chunk_boundary_lengths() {
    // 7- and 8-digit literals sit at the length where a wider
    // (SIMD/SWAR) digit reader would start paying; everything
    // shorter and longer must agree with `str::parse` too.
    for s in [
        "1234567",
        "12345678",
        "123456789",
        "123456789012345",
        "1234567890123456",
        "12345678901234567",
    ] {
        let want: i64 = s.parse().unwrap();
        assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), want, "{s}");
    }
}

#[test]
fn i64_stops_at_first_non_digit() {
    // The parser must stop at the first non-digit and leave the
    // cursor there.
    let mut lx = lex(b"12345678,9");
    assert_eq!(lx.parse_i64_value().unwrap(), 12_345_678);
    assert_eq!(lx.position().offset, 8);
}

#[test]
fn i64_trailing_fraction_rejected_after_chunk() {
    let mut lx = lex(b"12345678.5");
    assert_eq!(
        lx.parse_i64_value().unwrap_err().kind,
        ErrorKind::ExpectedNumber
    );
}

#[test]
fn i64_nineteen_digits_ok_twenty_rejected() {
    let ok = "9223372036854775807"; // i64::MAX, 19 digits
    assert_eq!(lex(ok.as_bytes()).parse_i64_value().unwrap(), i64::MAX);
    let over = "9223372036854775808"; // 19 digits + 1
    assert_eq!(
        lex(over.as_bytes()).parse_i64_value().unwrap_err().kind,
        ErrorKind::NumberOutOfRange
    );
    let long = "12345678901234567890"; // 20 digits, out of range
    assert_eq!(
        lex(long.as_bytes()).parse_i64_value().unwrap_err().kind,
        ErrorKind::NumberOutOfRange
    );
}

#[test]
fn u64_value_matches_str_parse_chunked() {
    for n in [
        1u64,
        9,
        12_345_678,
        99_999_999,
        100_000_000,
        u64::MAX,
        u64::MAX - 1,
    ] {
        let s = format!("{n}");
        assert_eq!(lex(s.as_bytes()).parse_u64_value().unwrap(), n);
    }
}

#[test]
fn i128_value_matches_str_parse_chunked() {
    for n in [
        1u128,
        12_345_678,
        100_000_000,
        170_141_183_460_469_231_731_687_303_715_884_105_727, // i128::MAX
    ] {
        let s = format!("{n}");
        let want = i128::try_from(n).unwrap();
        assert_eq!(lex(s.as_bytes()).parse_i128_value().unwrap(), want);
    }
    let neg = "-170141183460469231731687303715884105728"; // i128::MIN
    assert_eq!(lex(neg.as_bytes()).parse_i128_value().unwrap(), i128::MIN);
}

#[test]
fn scan_digit_run_advances_to_first_non_digit() {
    let mut lx = lex(b"12345678901234567890abc");
    lx.read_number().unwrap();
    assert_eq!(lx.position().offset, 20);
}
