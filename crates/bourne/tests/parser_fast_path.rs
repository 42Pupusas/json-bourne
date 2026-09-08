//! Fast-path / `next_event` interleaving (audit 3.12).
//!
//! `Parser::parse_i64_value` and `parse_str_value` must synchronize the
//! grammar state like their sibling fast-path methods, so a consumer can
//! read a scalar (or array element) through the fast path and resume
//! event-driven parsing — and so trailing data after a scalar root
//! document is rejected instead of parsed as a second document.

use json_bourne::{ErrorKind, Parser};

#[test]
fn scalar_root_then_trailing_data_is_rejected() {
    let mut p: Parser<'_> = Parser::new(b"1 2");
    assert_eq!(p.parse_i64_value().unwrap(), 1);
    let err = p.next_event().unwrap_err();
    assert_eq!(err.kind, ErrorKind::TrailingData);
}

#[test]
fn string_root_then_trailing_data_is_rejected() {
    let mut p: Parser<'_> = Parser::new(br#""a" "b""#);
    assert_eq!(p.parse_str_value().unwrap(), "a");
    let err = p.next_event().unwrap_err();
    assert_eq!(err.kind, ErrorKind::TrailingData);
}

#[test]
fn scalar_root_clean_eof_ends_document() {
    let mut p: Parser<'_> = Parser::new(b"42");
    assert_eq!(p.parse_i64_value().unwrap(), 42);
    assert!(p.next_event().unwrap().is_none());

    let mut p: Parser<'_> = Parser::new(br#""done""#);
    assert_eq!(p.parse_str_value().unwrap(), "done");
    assert!(p.next_event().unwrap().is_none());
}

#[test]
fn array_element_via_fast_path_resumes_event_stream() {
    let mut p: Parser<'_> = Parser::new(b"[1,2]");
    assert!(!p.array_start().unwrap());
    assert_eq!(p.parse_i64_value().unwrap(), 1);
    let ev = p.next_event().unwrap().expect("second element event");
    assert!(matches!(ev, json_bourne::Event::Number(_)));
    let ev = p.next_event().unwrap().expect("end array");
    assert!(matches!(ev, json_bourne::Event::EndArray));
    assert!(p.next_event().unwrap().is_none());
}

#[test]
fn array_string_elements_via_fast_path_resume_event_stream() {
    let mut p: Parser<'_> = Parser::new(br#"["x","y"]"#);
    assert!(!p.array_start().unwrap());
    assert_eq!(p.parse_str_value().unwrap(), "x");
    let ev = p.next_event().unwrap().expect("second element event");
    assert!(matches!(ev, json_bourne::Event::String(_)));
    let ev = p.next_event().unwrap().expect("end array");
    assert!(matches!(ev, json_bourne::Event::EndArray));
    assert!(p.next_event().unwrap().is_none());
}

#[test]
fn object_value_via_fast_path_resumes_event_stream() {
    let mut p: Parser<'_> = Parser::new(br#"{"a":1}"#);
    assert!(matches!(
        p.next_event().unwrap(),
        Some(json_bourne::Event::StartObject)
    ));
    assert_eq!(p.object_first_key().unwrap(), Some("a"));
    assert_eq!(p.parse_i64_value().unwrap(), 1);
    let ev = p.next_event().unwrap().expect("end object");
    assert!(matches!(ev, json_bourne::Event::EndObject));
    assert!(p.next_event().unwrap().is_none());
}

#[test]
fn object_missing_comma_is_rejected_after_fast_path_value() {
    let mut p: Parser<'_> = Parser::new(br#"{"a":1 "b":2}"#);
    assert!(matches!(
        p.next_event().unwrap(),
        Some(json_bourne::Event::StartObject)
    ));
    assert_eq!(p.object_first_key().unwrap(), Some("a"));
    assert_eq!(p.parse_i64_value().unwrap(), 1);
    let err = p.next_event().unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'"'));
}
