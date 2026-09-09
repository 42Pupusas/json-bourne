//! Streaming-API decode coverage: `JsonStr::as_str` and the `JsonNum`
//! accessors have no callers inside the crate (typed parsing never
//! materializes them), so the test suite was the only thing that could
//! exercise them — first flagged by the CRAP gate (audit F4).

use crate::{Event, Parser};

fn first_event(input: &[u8]) -> Event {
    let mut p = Parser::new(input);
    p.next_event().expect("parse").expect("an event")
}

#[test]
fn json_str_as_str_borrows_escape_free_strings() {
    let input = br#""plain""#;
    let Event::String(s) = first_event(input) else {
        panic!("expected a string event");
    };
    assert!(!s.has_escapes());
    assert_eq!(s.as_str(input), Some("plain"));
}

#[test]
fn json_str_as_str_rejects_escapes() {
    let input = br#""a\nb""#;
    let Event::String(s) = first_event(input) else {
        panic!("expected a string event");
    };
    assert!(s.has_escapes());
    assert_eq!(s.as_str(input), None);
}

#[test]
fn json_str_as_str_rejects_foreign_buffer() {
    let input = br#""plain""#;
    let Event::String(s) = first_event(input) else {
        panic!("expected a string event");
    };
    // Span indices are meaningless against a different buffer.
    assert_eq!(s.as_str(b""), None);
}

#[test]
fn json_num_decodes_integers_through_every_accessor() {
    let input = b"42";
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    assert_eq!(n.as_i64(input), Ok(42));
    assert_eq!(n.as_u64(input), Ok(42));
    assert_eq!(n.as_f64(input), Ok(42.0));
    assert_eq!(n.as_str(input), "42");
    assert!(!n.is_float(input));
}

#[test]
fn json_num_flags_and_decodes_float_literals() {
    let input = b"-2.5e3";
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    assert!(n.is_float(input));
    assert_eq!(n.as_f64(input), Ok(-2500.0));
}

#[test]
fn json_num_wide_integers_route_through_str_parse() {
    let signed = i128::MAX.to_string();
    let input = signed.as_bytes();
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    assert_eq!(n.as_i128(input), Ok(i128::MAX));

    let unsigned = u128::MAX.to_string();
    let input = unsigned.as_bytes();
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    assert_eq!(n.as_u128(input), Ok(u128::MAX));
}

#[test]
fn json_num_reports_out_of_range_not_invalid() {
    let input = b"99999999999999999999999999";
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    assert_eq!(n.as_i64(input), Err(crate::ErrorKind::NumberOutOfRange));
    assert_eq!(n.as_u64(input), Err(crate::ErrorKind::NumberOutOfRange));
}

#[test]
fn json_num_reports_invalid_for_foreign_buffer() {
    let input = b"42";
    let Event::Number(n) = first_event(input) else {
        panic!("expected a number event");
    };
    // `raw_bytes` fails before decoding starts, which is the
    // `InvalidNumber` arm — unreachable when the buffer matches.
    assert_eq!(n.as_i64(b""), Err(crate::ErrorKind::InvalidNumber));
    assert_eq!(n.as_str(b""), "");
    assert!(!n.is_float(b""));
}
