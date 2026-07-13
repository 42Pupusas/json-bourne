//! Tests for lenient mode (`#[bourne(deny_unknown_fields = false)]`)
//! combined with both derives. A type needing round-trip impls *and*
//! tolerance of unknown keys derives `FromJson` + `ToJson` together.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, FromJson, ToJson, parse_str, to_string};

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(deny_unknown_fields = false)]
struct Lenient {
    id: u64,
    name: String,
}

#[test]
fn lenient_json_ignores_unknown_fields() {
    let src = r#"{"id":1,"name":"alice","extra":true,"more":[1,2,3]}"#;
    let v: Lenient = parse_str(src).unwrap();
    assert_eq!(
        v,
        Lenient {
            id: 1,
            name: String::from("alice"),
        }
    );
}

#[test]
fn lenient_json_still_serializes() {
    let v = Lenient {
        id: 7,
        name: String::from("bob"),
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":7,"name":"bob"}"#);
}

#[test]
fn lenient_json_still_requires_declared_fields() {
    let err = parse_str::<Lenient>(r#"{"id":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(deny_unknown_fields = false)]
struct LenientBorrowed<'input> {
    id: u64,
    name: &'input str,
}

#[test]
fn lenient_json_borrowed_lifetime_ignores_unknown() {
    let src = r#"{"id":9,"name":"carol","skip":"me"}"#;
    let v: LenientBorrowed<'_> = parse_str(src).unwrap();
    assert_eq!(
        v,
        LenientBorrowed {
            id: 9,
            name: "carol",
        }
    );
    assert_eq!(to_string(&v).unwrap(), r#"{"id":9,"name":"carol"}"#);
}

// Control: a plain derived struct (no container attr) stays strict.
#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Strict {
    id: u64,
}

#[test]
fn strict_json_still_rejects_unknown() {
    let err = parse_str::<Strict>(r#"{"id":1,"extra":2}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}
