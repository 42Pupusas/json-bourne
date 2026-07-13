//! Tests for the combined `json!` macro's lenient mode
//! (`#[bourne(deny_unknown_fields = false)]`), which mirrors the arm
//! already offered by `from_json!`. Before this, `json!` could only
//! emit strict `FromJson` impls, so a type needing both round-trip impls
//! *and* tolerance of unknown keys had no single-macro path.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, json, parse_str, to_string};

json! {
    #[bourne(deny_unknown_fields = false)]
    #[derive(Debug, PartialEq)]
    struct Lenient {
        id: u64,
        name: String,
    }
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

json! {
    #[bourne(deny_unknown_fields = false)]
    #[derive(Debug, PartialEq)]
    struct LenientBorrowed<'input> {
        id: u64,
        name: &'input str,
    }
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

// Control: a plain `json!` struct (no container attr) stays strict.
json! {
    #[derive(Debug, PartialEq)]
    struct Strict {
        id: u64,
    }
}

#[test]
fn strict_json_still_rejects_unknown() {
    let err = parse_str::<Strict>(r#"{"id":1,"extra":2}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}
