//! Combined derive tests. Each type gets both `FromJson` and `ToJson`
//! from one `#[derive(...)]`, so every case here is a true round-trip
//! rather than a one-directional wire-shape assertion.

#![cfg(all(feature = "std", feature = "derive"))]

use json_bourne::{FromJson, ToJson, parse_str, to_string};

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Plain {
    id: u32,
    name: String,
}

#[test]
fn plain_struct_round_trips() {
    let v = Plain {
        id: 7,
        name: String::from("alice"),
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"id":7,"name":"alice"}"#);
    let back: Plain = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Borrowed<'input> {
    tag: &'input str,
    count: u32,
}

#[test]
fn struct_with_lifetime_round_trips() {
    let json = r#"{"tag":"hi","count":3}"#;
    let v: Borrowed<'_> = parse_str(json).unwrap();
    assert_eq!(
        v,
        Borrowed {
            tag: "hi",
            count: 3
        }
    );
    assert_eq!(to_string(&v).unwrap(), json);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
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
fn rename_skip_skip_if_none() {
    let v = Decorated {
        user_id: 1,
        cached: 99,
        note: None,
        value: 42,
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"user-id":1,"value":42}"#);
    let back: Decorated = parse_str(&s).unwrap();
    assert_eq!(back.user_id, 1);
    assert_eq!(back.value, 42);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct UserId(u64);

#[test]
fn newtype_round_trips() {
    let v = UserId(42);
    let s = to_string(&v).unwrap();
    assert_eq!(s, "42");
    let back: UserId = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Pair(i32, i32);

#[test]
fn tuple_struct_round_trips() {
    let v = Pair(3, -7);
    let s = to_string(&v).unwrap();
    assert_eq!(s, "[3,-7]");
    let back: Pair = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
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
fn externally_tagged_enum_round_trips() {
    let cases: Vec<(Shape, &str)> = vec![
        (Shape::Circle, r#""Circle""#),
        (Shape::Wrapper(7), r#"{"Wrapper":7}"#),
        (Shape::Pair(1, String::from("x")), r#"{"Pair":[1,"x"]}"#),
        (Shape::Box { w: 10, h: 20 }, r#"{"Box":{"w":10,"h":20}}"#),
        (Shape::Triangle, r#""tri""#),
    ];
    for (val, expected) in cases {
        let s = to_string(&val).unwrap();
        assert_eq!(s, expected);
        let back: Shape = parse_str(&s).unwrap();
        assert_eq!(back, val);
    }
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
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
fn internally_tagged_enum_round_trips() {
    let hb = Event::Heartbeat;
    let s = to_string(&hb).unwrap();
    assert_eq!(s, r#"{"type":"Heartbeat"}"#);
    let back: Event = parse_str(&s).unwrap();
    assert_eq!(back, hb);

    let click = Event::Click { x: 1, y: 2 };
    let s = to_string(&click).unwrap();
    assert_eq!(s, r#"{"type":"click","x":1,"y":2}"#);
    let back: Event = parse_str(&s).unwrap();
    assert_eq!(back, click);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "t", content = "c")]
enum Msg {
    Ping,
    Echo(String),
    Pair(u32, u32),
    Body { text: String },
}

#[test]
fn adjacently_tagged_enum_round_trips() {
    let cases: Vec<(Msg, &str)> = vec![
        (Msg::Ping, r#"{"t":"Ping"}"#),
        (Msg::Echo(String::from("hi")), r#"{"t":"Echo","c":"hi"}"#),
        (Msg::Pair(1, 2), r#"{"t":"Pair","c":[1,2]}"#),
        (
            Msg::Body {
                text: String::from("ok"),
            },
            r#"{"t":"Body","c":{"text":"ok"}}"#,
        ),
    ];
    for (val, expected) in cases {
        let s = to_string(&val).unwrap();
        assert_eq!(s, expected);
        let back: Msg = parse_str(&s).unwrap();
        assert_eq!(back, val);
    }
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(untagged)]
enum Mixed {
    Nothing,
    One(u32),
    Two(u32, u32),
    Body { name: String },
}

#[derive(Debug, PartialEq, Eq, FromJson, ToJson)]
pub struct PubFields {
    pub id: u32,
    pub name: String,
    value: u32,
}

#[test]
fn pub_fields_round_trip() {
    let v = PubFields {
        id: 1,
        name: String::from("hi"),
        value: 2,
    };
    let s = to_string(&v).unwrap();
    assert_eq!(s, r#"{"id":1,"name":"hi","value":2}"#);
    let back: PubFields = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[test]
fn untagged_enum_serializes() {
    assert_eq!(to_string(&Mixed::Nothing).unwrap(), "null");
    assert_eq!(to_string(&Mixed::One(42)).unwrap(), "42");
    assert_eq!(to_string(&Mixed::Two(1, 2)).unwrap(), "[1,2]");
    assert_eq!(
        to_string(&Mixed::Body {
            name: String::from("x")
        })
        .unwrap(),
        r#"{"name":"x"}"#,
    );
}
