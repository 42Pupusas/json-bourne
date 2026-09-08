//! `ToJson` derive tests. Each test pairs a `ToJson`-derived type against
//! a manually-defined `FromJson` mirror so the round-trip exercises both
//! derives on equivalent shapes.
//!
//! Public API only, so these live in `tests/` and additionally pin that
//! the derives resolve for an out-of-crate consumer.

#![cfg(all(feature = "std", feature = "derive"))]

use json_bourne::{FromJson, ToJson, parse_str, to_string};

// The simplest possible shape: plain struct, no attrs.
#[derive(Debug, PartialEq, ToJson)]
struct Plain {
    id: u32,
    name: String,
}

#[derive(Debug, PartialEq, FromJson)]
struct PlainParse {
    id: u32,
    name: String,
}

#[test]
fn plain_struct_emits_object() {
    let v = Plain {
        id: 7,
        name: String::from("alice"),
    };
    let s = to_string(&v).unwrap();
    // Field order matches declaration order.
    assert_eq!(s, r#"{"id":7,"name":"alice"}"#);
    // Parse back through the equivalent FromJson type.
    let back: PlainParse = parse_str(&s).unwrap();
    assert_eq!(
        back,
        PlainParse {
            id: 7,
            name: String::from("alice")
        }
    );
}

#[derive(Debug, PartialEq, ToJson)]
struct Borrowed<'input> {
    tag: &'input str,
    count: u32,
}

#[test]
fn struct_with_lifetime() {
    let v = Borrowed {
        tag: "hi",
        count: 3,
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"tag":"hi","count":3}"#);
}

// rename, skip, skip_if_none.
#[derive(Debug, PartialEq, ToJson)]
struct Decorated {
    #[bourne(rename = "user-id")]
    user_id: u32,
    #[bourne(skip)]
    cached: u32,
    #[bourne(skip_if_none)]
    note: Option<String>,
    value: u32,
}

#[test]
fn rename_emits_new_key() {
    let v = Decorated {
        user_id: 1,
        cached: 99,
        note: None,
        value: 42,
    };
    let s = to_string(&v).unwrap();
    // user-id renamed; cached omitted; note omitted (None); value present.
    assert_eq!(s, r#"{"user-id":1,"value":42}"#);
}

#[test]
fn skip_if_none_emits_when_some() {
    let v = Decorated {
        user_id: 1,
        cached: 0,
        note: Some(String::from("hi")),
        value: 7,
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"user-id":1,"note":"hi","value":7}"#);
}

// Regression: a *plain* (un-renamed) field followed by a skipped
// `skip_if_none` field, then a present field. The plain fast path folds
// the comma+key into a compile-time literal; it must still record that
// output was committed so the later runtime comma (gated on `!__first`)
// is emitted. Prior to the fix this produced `.."b""c":..` (missing
// comma) whenever the optional middle field was None.
#[derive(Debug, PartialEq, ToJson)]
struct PlainThenSkip {
    a: u32,
    b: u32,
    #[bourne(skip_if_none)]
    mid: Option<u32>,
    c: u32,
}

#[test]
fn plain_field_before_skipped_option_keeps_comma() {
    // mid = None -> the fast-path plain fields must still separate from `c`.
    let v = PlainThenSkip {
        a: 1,
        b: 2,
        mid: None,
        c: 3,
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"a":1,"b":2,"c":3}"#);
    // mid = Some -> comma on both sides of the optional.
    let v = PlainThenSkip {
        a: 1,
        b: 2,
        mid: Some(9),
        c: 3,
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"a":1,"b":2,"mid":9,"c":3}"#);
}

// Empty struct edge case.
#[derive(Debug, PartialEq, ToJson)]
struct Empty {}

#[test]
fn empty_struct_emits_empty_object() {
    assert_eq!(to_string(&Empty {}).unwrap(), "{}");
}

// String escaping inside emitted values (sanity — should already
// work via the ToJson<String> impl, but the macro shouldn't
// double-escape or corrupt the output).
#[derive(Debug, PartialEq, ToJson)]
struct WithEscape {
    text: String,
}

#[test]
fn macro_passes_strings_to_escape_path() {
    let v = WithEscape {
        text: String::from("a\nb\"c"),
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"text":"a\nb\"c"}"#);
}

// Newtype tuple struct — emits the inner value bare.
#[derive(Debug, PartialEq, ToJson)]
struct UserId(u64);

#[test]
fn newtype_emits_bare_value() {
    let v = UserId(42);
    assert_eq!(to_string(&v).unwrap(), "42");
}

#[derive(Debug, PartialEq, ToJson)]
struct BorrowedTag<'input>(&'input str);

#[test]
fn newtype_with_lifetime() {
    let v = BorrowedTag("hello");
    assert_eq!(to_string(&v).unwrap(), r#""hello""#);
}

// Multi-field tuple struct — emits a JSON array.
#[derive(Debug, PartialEq, ToJson)]
struct Point(i32, i32);

#[test]
fn tuple_struct_emits_array() {
    let v = Point(3, -7);
    assert_eq!(to_string(&v).unwrap(), "[3,-7]");
}

#[derive(Debug, PartialEq, ToJson)]
struct Triple(i32, String, bool);

#[test]
fn three_field_tuple_struct() {
    let v = Triple(1, String::from("hi"), true);
    assert_eq!(to_string(&v).unwrap(), r#"[1,"hi",true]"#);
}

// Externally-tagged enum — the default encoding.
#[derive(Debug, PartialEq, ToJson)]
enum Shape {
    Circle,
    Wrapper(u32),
    Pair(u32, String),
    Box {
        w: u32,
        h: u32,
    },
    #[bourne(rename = "tri")]
    Triangle,
}

#[test]
fn enum_unit_emits_string() {
    assert_eq!(to_string(&Shape::Circle).unwrap(), r#""Circle""#);
}

#[test]
fn enum_renamed_unit() {
    assert_eq!(to_string(&Shape::Triangle).unwrap(), r#""tri""#);
}

#[test]
fn enum_newtype_emits_object() {
    assert_eq!(to_string(&Shape::Wrapper(7)).unwrap(), r#"{"Wrapper":7}"#);
}

#[test]
fn enum_tuple_emits_object_with_array() {
    let v = Shape::Pair(1, String::from("x"));
    assert_eq!(to_string(&v).unwrap(), r#"{"Pair":[1,"x"]}"#);
}

#[test]
fn enum_struct_variant_emits_nested_object() {
    let v = Shape::Box { w: 10, h: 20 };
    assert_eq!(to_string(&v).unwrap(), r#"{"Box":{"w":10,"h":20}}"#);
}

// Internally-tagged enum.
#[derive(Debug, PartialEq, ToJson)]
#[bourne(tag = "type")]
enum Event {
    Heartbeat,
    #[bourne(rename = "click")]
    Click {
        x: u32,
        y: u32,
    },
}

#[test]
fn internal_tag_unit_emits_object_with_tag() {
    assert_eq!(
        to_string(&Event::Heartbeat).unwrap(),
        r#"{"type":"Heartbeat"}"#
    );
}

#[test]
fn internal_tag_struct_inlines_fields() {
    let v = Event::Click { x: 1, y: 2 };
    assert_eq!(to_string(&v).unwrap(), r#"{"type":"click","x":1,"y":2}"#);
}

// Adjacently-tagged enum.
#[derive(Debug, PartialEq, ToJson)]
#[bourne(tag = "t", content = "c")]
enum Msg {
    Ping,
    Echo(String),
    Pair(u32, u32),
    Body { text: String },
}

#[test]
fn adjacent_unit_emits_only_tag() {
    assert_eq!(to_string(&Msg::Ping).unwrap(), r#"{"t":"Ping"}"#);
}

#[test]
fn adjacent_newtype_emits_content() {
    let v = Msg::Echo(String::from("hi"));
    assert_eq!(to_string(&v).unwrap(), r#"{"t":"Echo","c":"hi"}"#);
}

#[test]
fn adjacent_tuple_emits_content_array() {
    let v = Msg::Pair(1, 2);
    assert_eq!(to_string(&v).unwrap(), r#"{"t":"Pair","c":[1,2]}"#);
}

#[test]
fn adjacent_struct_emits_content_object() {
    let v = Msg::Body {
        text: String::from("ok"),
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"t":"Body","c":{"text":"ok"}}"#);
}

// Untagged enum.
#[derive(Debug, PartialEq, ToJson)]
#[bourne(untagged)]
enum Mixed {
    Nothing,
    One(u32),
    Two(u32, u32),
    Body { name: String },
}

#[test]
fn untagged_unit_emits_null() {
    assert_eq!(to_string(&Mixed::Nothing).unwrap(), "null");
}

#[test]
fn untagged_newtype_emits_inner() {
    assert_eq!(to_string(&Mixed::One(42)).unwrap(), "42");
}

#[test]
fn untagged_tuple_emits_array() {
    assert_eq!(to_string(&Mixed::Two(1, 2)).unwrap(), "[1,2]");
}

#[test]
fn untagged_struct_emits_object() {
    let v = Mixed::Body {
        name: String::from("x"),
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"name":"x"}"#);
}
