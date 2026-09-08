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
