//! Field attributes (`rename`, `rename_all`, `skip`, `default`) inside enum
//! struct variants (audit §3.6: they were silently ignored — the variant
//! readers matched the raw Rust field name and the writer fused it verbatim).

use json_bourne::{FromJson, ToJson, parse_str, to_string};

#[derive(Debug, PartialEq, FromJson, ToJson)]
enum External {
    #[allow(dead_code)]
    Rect {
        #[bourne(rename = "w")]
        width: u32,
        #[bourne(rename = "h")]
        height: u32,
    },
    Plain {
        v: u8,
    },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "type", rename_all = "snake_case")]
enum Internal {
    JobDone {
        job_id: u32,
        #[bourne(rename = "workerName")]
        worker_name: String,
        #[bourne(skip)]
        trace_id: u32,
        #[bourne(default)]
        attempts: u8,
    },
    Reset,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind", content = "data", rename_all = "snake_case")]
enum Adjacent {
    Point {
        #[bourne(rename = "X")]
        x: i32,
        y: i32,
    },
    Nothing,
}

#[test]
fn external_variant_field_renames_round_trip() {
    let v = External::Rect {
        width: 10,
        height: 20,
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"Rect":{"w":10,"h":20}}"#);
    let back: External = parse_str(r#"{"Rect":{"w":10,"h":20}}"#).unwrap();
    assert_eq!(back, v);
    let r: Result<External, _> = parse_str(r#"{"Rect":{"width":10,"height":20}}"#);
    assert!(r.is_err(), "pre-fix spelling must not be accepted");
}

#[test]
fn external_variant_rename_borrows_skips_default() {
    #[derive(Debug, PartialEq, FromJson)]
    enum E {
        #[allow(dead_code)]
        V {
            #[bourne(rename = "label")]
            s: String,
            #[bourne(skip)]
            hidden: bool,
            #[bourne(default)]
            retries: u8,
        },
    }
    let v: E = parse_str(r#"{"V":{"label":"hi"}}"#).unwrap();
    assert_eq!(
        v,
        E::V {
            s: String::from("hi"),
            hidden: false,
            retries: 0
        }
    );
    let r: Result<E, _> = parse_str(r#"{"V":{"label":"hi","hidden":true}}"#);
    assert!(r.is_err(), "skipped field must not match a key");
    let r: Result<E, _> = parse_str(r#"{"V":{}}"#);
    assert!(r.is_err(), "renamed+required field must stay required");
}

#[test]
fn internal_variant_rename_all_and_attrs_round_trip() {
    let v = Internal::JobDone {
        job_id: 7,
        worker_name: String::from("ada"),
        trace_id: 0,
        attempts: 0,
    };
    assert_eq!(
        to_string(&v).unwrap(),
        r#"{"type":"job_done","job_id":7,"workerName":"ada","attempts":0}"#
    );
    let back: Internal = parse_str(r#"{"type":"job_done","job_id":7,"workerName":"ada"}"#).unwrap();
    assert_eq!(back, v);
    let with_extra: Result<Internal, _> =
        parse_str(r#"{"type":"job_done","job_id":7,"workerName":"ada","trace_id":9}"#);
    assert!(with_extra.is_err(), "skipped key is unknown, not absorbed");
}

#[test]
fn internal_variant_default_fill() {
    let v: Internal =
        parse_str(r#"{"type":"job_done","job_id":1,"workerName":"w","attempts":3}"#).unwrap();
    assert_eq!(
        v,
        Internal::JobDone {
            job_id: 1,
            worker_name: String::from("w"),
            trace_id: 0,
            attempts: 3,
        }
    );
}

#[test]
fn adjacent_variant_field_rename_round_trips() {
    let v = Adjacent::Point { x: 1, y: 2 };
    assert_eq!(
        to_string(&v).unwrap(),
        r#"{"kind":"point","data":{"X":1,"y":2}}"#
    );
    let back: Adjacent = parse_str(r#"{"kind":"point","data":{"X":1,"y":2}}"#).unwrap();
    assert_eq!(back, v);
    let r: Result<Adjacent, _> = parse_str(r#"{"kind":"point","data":{"x":1,"y":2}}"#);
    assert!(r.is_err());
}
