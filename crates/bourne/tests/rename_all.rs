//! `#[bourne(rename_all = "…")]` — container-level casing for struct
//! fields and enum variant tags. Explicit per-field / per-variant
//! `rename` still wins.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, from_json, json, parse_str, to_string};

// --- struct fields, camelCase ---------------------------------------------
from_json! {
    #[bourne(rename_all = "camelCase")]
    #[derive(Debug, PartialEq)]
    struct CamelUser {
        user_id: u64,
        full_name: String,
    }
}

#[test]
fn struct_camel_accepts_camel_keys() {
    let v: CamelUser = parse_str(r#"{"userId":1,"fullName":"al"}"#).unwrap();
    assert_eq!(
        v,
        CamelUser {
            user_id: 1,
            full_name: String::from("al"),
        }
    );
}

#[test]
fn struct_camel_rejects_snake_keys() {
    let err = parse_str::<CamelUser>(r#"{"user_id":1,"full_name":"al"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

// --- struct fields, SCREAMING_SNAKE_CASE ----------------------------------
from_json! {
    #[bourne(rename_all = "SCREAMING_SNAKE_CASE")]
    #[derive(Debug, PartialEq)]
    struct Screaming {
        max_depth: u32,
    }
}

#[test]
fn struct_screaming_snake() {
    let v: Screaming = parse_str(r#"{"MAX_DEPTH":128}"#).unwrap();
    assert_eq!(v, Screaming { max_depth: 128 });
}

// --- explicit rename wins over rename_all ---------------------------------
from_json! {
    #[bourne(rename_all = "camelCase")]
    #[derive(Debug, PartialEq)]
    struct Mixed<'input> {
        user_id: u64,
        #[bourne(rename = "NAME")]
        full_name: &'input str,
    }
}

#[test]
fn explicit_rename_overrides_rename_all() {
    let v: Mixed<'_> = parse_str(r#"{"userId":9,"NAME":"z"}"#).unwrap();
    assert_eq!(
        v,
        Mixed {
            user_id: 9,
            full_name: "z",
        }
    );
}

// --- json! round-trip: casing symmetric on both sides ---------------------
json! {
    #[bourne(rename_all = "camelCase")]
    #[derive(Debug, PartialEq)]
    struct RtUser {
        user_id: u64,
        full_name: String,
        #[bourne(rename = "ID2")]
        secondary_id: u64,
    }
}

#[test]
fn json_rename_all_roundtrips() {
    let src = r#"{"userId":1,"fullName":"al","ID2":2}"#;
    let v: RtUser = parse_str(src).unwrap();
    assert_eq!(
        v,
        RtUser {
            user_id: 1,
            full_name: String::from("al"),
            secondary_id: 2,
        }
    );
    // Serialize back: cased keys + explicit rename preserved, field order kept.
    assert_eq!(to_string(&v).unwrap(), src);
}

json! {
    #[bourne(rename_all = "kebab-case")]
    #[derive(Debug, PartialEq)]
    struct Kebab {
        max_retry_count: u32,
        base_url: String,
    }
}

#[test]
fn json_kebab_roundtrips() {
    let src = r#"{"max-retry-count":3,"base-url":"x"}"#;
    let v: Kebab = parse_str(src).unwrap();
    assert_eq!(to_string(&v).unwrap(), src);
}

// --- externally-tagged enum variant tags, rename_all ----------------------
from_json! {
    #[bourne(rename_all = "snake_case")]
    #[derive(Debug, PartialEq)]
    enum Event {
        UserLoggedIn,
        PageViewed(u32),
        #[bourne(rename = "BOOM")]
        SystemCrashed,
    }
}

#[test]
fn enum_unit_tag_snake_cased() {
    let v: Event = parse_str(r#""user_logged_in""#).unwrap();
    assert_eq!(v, Event::UserLoggedIn);
}

#[test]
fn enum_newtype_tag_snake_cased() {
    let v: Event = parse_str(r#"{"page_viewed":7}"#).unwrap();
    assert_eq!(v, Event::PageViewed(7));
}

#[test]
fn enum_explicit_variant_rename_wins() {
    let v: Event = parse_str(r#""BOOM""#).unwrap();
    assert_eq!(v, Event::SystemCrashed);
}

#[test]
fn enum_rejects_original_variant_name() {
    let err = parse_str::<Event>(r#""UserLoggedIn""#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

// --- enum round-trip via json! --------------------------------------------
json! {
    #[bourne(rename_all = "kebab-case")]
    #[derive(Debug, PartialEq)]
    enum Cmd {
        StartUp,
        SetLevel(u8),
    }
}

#[test]
fn enum_json_roundtrips_unit() {
    let v: Cmd = parse_str(r#""start-up""#).unwrap();
    assert_eq!(v, Cmd::StartUp);
    assert_eq!(to_string(&v).unwrap(), r#""start-up""#);
}

#[test]
fn enum_json_roundtrips_newtype() {
    let src = r#"{"set-level":3}"#;
    let v: Cmd = parse_str(src).unwrap();
    assert_eq!(v, Cmd::SetLevel(3));
    assert_eq!(to_string(&v).unwrap(), src);
}
