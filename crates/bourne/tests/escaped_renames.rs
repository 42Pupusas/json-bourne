//! Variant `#[bourne(rename)]` values containing `"`, `\`, or control bytes.
//!
//! The serializer must escape them (audit §3.5: the tag used to be fused as a
//! raw literal, emitting invalid JSON like `"a"b"`), and every parse path must
//! decode them back, including the bare-string unit-variant dispatch that
//! historically used the escape-rejecting fast path.

use json_bourne::{FromJson, ToJson, parse_str, to_string};

#[derive(Debug, PartialEq, FromJson, ToJson)]
enum External {
    #[bourne(rename = "a\"b")]
    Quoted(u32),
    #[bourne(rename = "back\\slash")]
    Backslashed,
    #[bourne(rename = "line\nbreak")]
    Newline(u64, u64),
    #[bourne(rename = "tab\tbed")]
    Tabbed { x: i32 },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "type")]
enum Internal {
    #[bourne(rename = "q\"uote")]
    Quoted { n: u8 },
    #[bourne(rename = "nl\nline")]
    Newline,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind", content = "data")]
enum Adjacent {
    #[bourne(rename = "bs\\one")]
    Backslashed(u32),
    #[bourne(rename = "tb\ttwo")]
    Tabbed { ok: bool },
}

#[test]
fn external_unit_tag_is_escaped_and_round_trips() {
    assert_eq!(
        to_string(&External::Backslashed).unwrap(),
        r#""back\\slash""#
    );
    let v: External = parse_str(r#""back\\slash""#).unwrap();
    assert_eq!(v, External::Backslashed);
}

#[test]
fn external_newtype_key_is_escaped_and_round_trips() {
    assert_eq!(to_string(&External::Quoted(7)).unwrap(), r#"{"a\"b":7}"#);
    let v: External = parse_str(r#"{"a\"b":7}"#).unwrap();
    assert_eq!(v, External::Quoted(7));
}

#[test]
fn external_tuple_key_is_escaped_and_round_trips() {
    assert_eq!(
        to_string(&External::Newline(1, 2)).unwrap(),
        r#"{"line\nbreak":[1,2]}"#
    );
    let v: External = parse_str(r#"{"line\nbreak":[1,2]}"#).unwrap();
    assert_eq!(v, External::Newline(1, 2));
}

#[test]
fn external_struct_key_is_escaped_and_round_trips() {
    assert_eq!(
        to_string(&External::Tabbed { x: 3 }).unwrap(),
        r#"{"tab\tbed":{"x":3}}"#
    );
    let v: External = parse_str(r#"{"tab\tbed":{"x":3}}"#).unwrap();
    assert_eq!(v, External::Tabbed { x: 3 });
}

#[test]
fn internal_tag_value_is_escaped_and_round_trips() {
    assert_eq!(
        to_string(&Internal::Quoted { n: 1 }).unwrap(),
        r#"{"type":"q\"uote","n":1}"#
    );
    assert_eq!(
        to_string(&Internal::Newline).unwrap(),
        r#"{"type":"nl\nline"}"#
    );
    let v: Internal = parse_str(r#"{"type":"q\"uote","n":1}"#).unwrap();
    assert_eq!(v, Internal::Quoted { n: 1 });
    let v: Internal = parse_str(r#"{"type":"nl\nline"}"#).unwrap();
    assert_eq!(v, Internal::Newline);
}

#[test]
fn adjacent_tag_value_is_escaped_and_round_trips() {
    assert_eq!(
        to_string(&Adjacent::Backslashed(9)).unwrap(),
        r#"{"kind":"bs\\one","data":9}"#
    );
    assert_eq!(
        to_string(&Adjacent::Tabbed { ok: true }).unwrap(),
        r#"{"kind":"tb\ttwo","data":{"ok":true}}"#
    );
    let v: Adjacent = parse_str(r#"{"kind":"bs\\one","data":9}"#).unwrap();
    assert_eq!(v, Adjacent::Backslashed(9));
    let v: Adjacent = parse_str(r#"{"kind":"tb\ttwo","data":{"ok":true}}"#).unwrap();
    assert_eq!(v, Adjacent::Tabbed { ok: true });
}

#[test]
fn escaped_tags_reject_unescaped_lookalikes() {
    let r = parse_str::<External>(r#""back\slash""#);
    assert!(
        r.is_err(),
        r"single backslash is a different tag: \s is an invalid escape"
    );
}
