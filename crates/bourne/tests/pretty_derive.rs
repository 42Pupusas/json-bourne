//! Pretty-printing of derive-generated types (audit §3.8: `to_string_pretty`
//! emitted derived structs compactly because the derive fused structural
//! bytes into raw literals the pretty sink treats as opaque text).
//!
//! Compact output must stay byte-identical to the pre-structural-method
//! output: the `FUSES_STRUCTURAL_BYTES` flag keeps the fused path for
//! byte-oriented sinks.

use json_bourne::{FromJson, ToJson, parse_str, to_string, to_string_pretty};

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Repo {
    id: u32,
    name: String,
    tags: Vec<String>,
    owner: Owner,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Owner {
    login: String,
    forks: u64,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
enum Hook {
    Push { branch: String },
    Release(u32),
    Deleted,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "type", content = "data")]
enum Payload {
    Ping { seq: u32 },
    Stop,
}

#[test]
fn derived_struct_pretty_prints() {
    let v = Repo {
        id: 1,
        name: String::from("bourne"),
        tags: vec![String::from("json"), String::from("fast")],
        owner: Owner {
            login: String::from("ada"),
            forks: 3,
        },
    };
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(
        s,
        concat!(
            "{\n",
            "  \"id\": 1,\n",
            "  \"name\": \"bourne\",\n",
            "  \"tags\": [\n",
            "    \"json\",\n",
            "    \"fast\"\n",
            "  ],\n",
            "  \"owner\": {\n",
            "    \"login\": \"ada\",\n",
            "    \"forks\": 3\n",
            "  }\n",
            "}",
        )
    );
    let back: Repo = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[test]
fn derived_enum_pretty_prints() {
    let s = to_string_pretty(&Hook::Push {
        branch: String::from("main"),
    })
    .unwrap();
    assert_eq!(
        s, "{\n  \"Push\": {\n    \"branch\": \"main\"\n  }\n}",
        "external struct variant",
    );
    let s = to_string_pretty(&Hook::Release(7)).unwrap();
    assert_eq!(s, "{\n  \"Release\": 7\n}");
    let s = to_string_pretty(&Hook::Deleted).unwrap();
    assert_eq!(s, "\"Deleted\"");
}

#[test]
fn adjacent_enum_pretty_prints() {
    let s = to_string_pretty(&Payload::Ping { seq: 2 }).unwrap();
    assert_eq!(
        s,
        "{\n  \"type\": \"Ping\",\n  \"data\": {\n    \"seq\": 2\n  }\n}",
    );
    let s = to_string_pretty(&Payload::Stop).unwrap();
    assert_eq!(s, "{\n  \"type\": \"Stop\"\n}");
}

#[test]
fn empty_containers_stay_compact() {
    #[derive(ToJson)]
    struct Empty {
        list: Vec<u8>,
        inner: Inner,
    }
    #[derive(ToJson)]
    struct Inner {
        map: std::collections::BTreeMap<String, u8>,
    }
    let v = Empty {
        list: Vec::new(),
        inner: Inner {
            map: std::collections::BTreeMap::new(),
        },
    };
    assert_eq!(
        to_string_pretty(&v).unwrap(),
        "{\n  \"list\": [],\n  \"inner\": {\n    \"map\": {}\n  }\n}"
    );
}

#[test]
fn compact_output_is_byte_identical() {
    let v = Repo {
        id: 1,
        name: String::from("bourne"),
        tags: vec![String::from("json")],
        owner: Owner {
            login: String::from("ada"),
            forks: 3,
        },
    };
    assert_eq!(
        to_string(&v).unwrap(),
        r#"{"id":1,"name":"bourne","tags":["json"],"owner":{"login":"ada","forks":3}}"#,
    );
}

// ---------------------------------------------------------------------------
// Audit R1 regression matrix: renamed and conditional (skip_if_none) fields
// go through the escaping key path, whose separator was gated on the sink's
// fused flag. Pretty sinks set that flag false, so their members silently
// lost commas. Compact output is pinned above; these pin pretty output and
// round-trip every attribute × shape × tagging-mode combination.

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Renamed {
    a: u32,
    #[bourne(rename = "second")]
    b: u32,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Conditional {
    a: u32,
    #[bourne(skip_if_none)]
    b: Option<u32>,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(rename_all = "camelCase")]
struct CamelConditional {
    alpha: u32,
    #[bourne(skip_if_none)]
    beta_skip: Option<u32>,
    omega: u32,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "t", content = "c")]
enum AdjacentStruct {
    Body {
        a: u32,
        #[bourne(rename = "B")]
        b: u32,
    },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "t")]
enum InternalStruct {
    Point {
        x: u32,
        #[bourne(skip_if_none)]
        y: Option<u32>,
    },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(untagged)]
enum UntaggedVariant {
    Left(Renamed),
    Right(Conditional),
}

#[test]
fn renamed_field_pretty_output_is_valid_and_round_trips() {
    let v = Renamed { a: 1, b: 2 };
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(s, "{\n  \"a\": 1,\n  \"second\": 2\n}");
    assert_eq!(to_string(&v).unwrap(), r#"{"a":1,"second":2}"#);
    let back: Renamed = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[test]
fn conditional_field_pretty_output_is_valid_and_round_trips() {
    let some = Conditional { a: 1, b: Some(2) };
    let s = to_string_pretty(&some).unwrap();
    assert_eq!(s, "{\n  \"a\": 1,\n  \"b\": 2\n}");
    let back: Conditional = parse_str(&s).unwrap();
    assert_eq!(back, some);

    let none = Conditional { a: 1, b: None };
    let s = to_string_pretty(&none).unwrap();
    assert_eq!(s, "{\n  \"a\": 1\n}");
    let back: Conditional = parse_str(&s).unwrap();
    assert_eq!(back, none);
}

#[test]
fn conditional_middle_and_trailing_members_each_get_a_separator() {
    let some = CamelConditional {
        alpha: 1,
        beta_skip: Some(2),
        omega: 3,
    };
    let s = to_string_pretty(&some).unwrap();
    assert_eq!(
        s,
        "{\n  \"alpha\": 1,\n  \"betaSkip\": 2,\n  \"omega\": 3\n}"
    );
    let none = CamelConditional {
        alpha: 1,
        beta_skip: None,
        omega: 3,
    };
    let s = to_string_pretty(&none).unwrap();
    assert_eq!(s, "{\n  \"alpha\": 1,\n  \"omega\": 3\n}");
    let back: CamelConditional = parse_str(&s).unwrap();
    assert_eq!(back, none);
}

#[test]
fn adjacent_struct_variant_with_renamed_fields_pretty_round_trips() {
    let v = AdjacentStruct::Body { a: 1, b: 2 };
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(
        s,
        "{\n  \"t\": \"Body\",\n  \"c\": {\n    \"a\": 1,\n    \"B\": 2\n  }\n}"
    );
    let back: AdjacentStruct = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[test]
fn internal_struct_variant_with_conditional_fields_pretty_round_trips() {
    let v = InternalStruct::Point { x: 3, y: Some(4) };
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(s, "{\n  \"t\": \"Point\",\n  \"x\": 3,\n  \"y\": 4\n}");
    let back: InternalStruct = parse_str(&s).unwrap();
    assert_eq!(back, v);

    let v = InternalStruct::Point { x: 3, y: None };
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(s, "{\n  \"t\": \"Point\",\n  \"x\": 3\n}");
    let back: InternalStruct = parse_str(&s).unwrap();
    assert_eq!(back, v);
}

#[test]
fn untagged_variant_pretty_round_trips() {
    let v = UntaggedVariant::Left(Renamed { a: 5, b: 6 });
    let s = to_string_pretty(&v).unwrap();
    assert_eq!(s, "{\n  \"a\": 5,\n  \"second\": 6\n}");
    let back: UntaggedVariant = parse_str(&s).unwrap();
    assert_eq!(back, v);
}
