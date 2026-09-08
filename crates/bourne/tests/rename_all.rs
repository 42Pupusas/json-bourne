//! `#[bourne(rename_all = "…")]` — container-level casing for struct
//! fields and enum variant tags. Explicit per-field / per-variant
//! `rename` still wins.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, FromJson, ToJson, parse_str, to_string};

// --- struct fields, camelCase ---------------------------------------------
#[derive(Debug, PartialEq, FromJson)]
#[bourne(rename_all = "camelCase")]
struct CamelUser {
    user_id: u64,
    full_name: String,
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
#[derive(Debug, PartialEq, FromJson)]
#[bourne(rename_all = "SCREAMING_SNAKE_CASE")]
struct Screaming {
    max_depth: u32,
}

#[test]
fn struct_screaming_snake() {
    let v: Screaming = parse_str(r#"{"MAX_DEPTH":128}"#).unwrap();
    assert_eq!(v, Screaming { max_depth: 128 });
}

// --- explicit rename wins over rename_all ---------------------------------
#[derive(Debug, PartialEq, FromJson)]
#[bourne(rename_all = "camelCase")]
struct Mixed<'input> {
    user_id: u64,
    #[bourne(rename = "NAME")]
    full_name: &'input str,
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
#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(rename_all = "camelCase")]
struct RtUser {
    user_id: u64,
    full_name: String,
    #[bourne(rename = "ID2")]
    secondary_id: u64,
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

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(rename_all = "kebab-case")]
struct Kebab {
    max_retry_count: u32,
    base_url: String,
}

#[test]
fn json_kebab_roundtrips() {
    let src = r#"{"max-retry-count":3,"base-url":"x"}"#;
    let v: Kebab = parse_str(src).unwrap();
    assert_eq!(to_string(&v).unwrap(), src);
}

// --- externally-tagged enum variant tags, rename_all ----------------------
#[derive(Debug, PartialEq, FromJson)]
#[bourne(rename_all = "snake_case")]
enum Event {
    UserLoggedIn,
    PageViewed(u32),
    #[bourne(rename = "BOOM")]
    SystemCrashed,
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
#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(rename_all = "kebab-case")]
enum Cmd {
    StartUp,
    SetLevel(u8),
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

// --- internally-tagged enums: rename_all applies on parse too -------------
// Regression for audit 3.4: the parse side passed `&None` for rename_all,
// so these enums serialized their cased tags and then rejected their own
// output with UnknownField.
#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind", rename_all = "snake_case")]
enum InternalCased {
    JobDone,
    Status { status_code: u32 },
}

#[test]
fn internal_tag_unit_accepts_cased_tag() {
    let v: InternalCased = parse_str(r#"{"kind":"job_done"}"#).unwrap();
    assert_eq!(v, InternalCased::JobDone);
}

#[test]
fn internal_tag_unit_rejects_original_name() {
    let err = parse_str::<InternalCased>(r#"{"kind":"JobDone"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn internal_tag_struct_variant_accepts_cased_tag() {
    let v: InternalCased = parse_str(r#"{"kind":"status","status_code":200}"#).unwrap();
    assert_eq!(v, InternalCased::Status { status_code: 200 });
}

#[test]
fn internal_tag_roundtrips() {
    for v in [
        InternalCased::JobDone,
        InternalCased::Status { status_code: 404 },
    ] {
        let s = to_string(&v).unwrap();
        let back: InternalCased = parse_str(&s).expect("own output must re-parse");
        assert_eq!(back, v, "round-trip through {s}");
    }
}

// --- adjacently-tagged enums: rename_all applies on parse too --------------
#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind", content = "data", rename_all = "kebab-case")]
enum AdjacentCased {
    PlainValue,
    WithPayload(u64),
}

#[test]
fn adjacent_tag_unit_accepts_cased_tag() {
    let v: AdjacentCased = parse_str(r#"{"kind":"plain-value"}"#).unwrap();
    assert_eq!(v, AdjacentCased::PlainValue);
}

#[test]
fn adjacent_tag_newtype_accepts_cased_tag() {
    let v: AdjacentCased = parse_str(r#"{"kind":"with-payload","data":9}"#).unwrap();
    assert_eq!(v, AdjacentCased::WithPayload(9));
}

#[test]
fn adjacent_tag_content_first_still_accepts_cased_tag() {
    let v: AdjacentCased = parse_str(r#"{"data":5,"kind":"with-payload"}"#).unwrap();
    assert_eq!(v, AdjacentCased::WithPayload(5));
}

#[test]
fn adjacent_tag_rejects_original_name() {
    let err = parse_str::<AdjacentCased>(r#"{"kind":"WithPayload","data":9}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn adjacent_tag_roundtrips() {
    for v in [AdjacentCased::PlainValue, AdjacentCased::WithPayload(12)] {
        let s = to_string(&v).unwrap();
        let back: AdjacentCased = parse_str(&s).expect("own output must re-parse");
        assert_eq!(back, v, "round-trip through {s}");
    }
}

// --- untagged is unaffected: there is no tag to case -----------------------
#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(untagged, rename_all = "snake_case")]
enum UntaggedCased {
    Num(u64),
    Text(String),
}

#[test]
fn untagged_still_matches_by_shape() {
    assert_eq!(
        parse_str::<UntaggedCased>("7").unwrap(),
        UntaggedCased::Num(7)
    );
    assert_eq!(
        parse_str::<UntaggedCased>(r#""hi""#).unwrap(),
        UntaggedCased::Text(String::from("hi"))
    );
}
