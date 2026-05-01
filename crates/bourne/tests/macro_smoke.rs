//! Single-case smoke test for `from_json!`. Mirrors the simplest path
//! the test suite exercises: a borrowed-string struct with one
//! `Option<&str>` field. If this case doesn't expand cleanly the
//! macro design is wrong and the wider port is wasted effort.

use bourne::{ErrorKind, from_json, parse_str};

from_json! {
    #[derive(Debug, PartialEq)]
    struct DerivedUser<'input> {
        id: u64,
        name: &'input str,
        active: bool,
        nickname: Option<&'input str>,
    }
}

#[test]
fn parses_with_all_fields() {
    let json = r#"{"id":42,"name":"alice","active":true,"nickname":"al"}"#;
    let u: DerivedUser<'_> = parse_str(json).unwrap();
    assert_eq!(
        u,
        DerivedUser {
            id: 42,
            name: "alice",
            active: true,
            nickname: Some("al"),
        }
    );
}

#[test]
fn parses_with_nickname_omitted() {
    let json = r#"{"name":"carol","id":7,"active":true}"#;
    let u: DerivedUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.nickname, None);
}

#[test]
fn rejects_missing_required_field() {
    let json = r#"{"id":1,"active":true}"#;
    let err = parse_str::<DerivedUser<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn rejects_unknown_field() {
    let json = r#"{"id":1,"name":"e","active":true,"extra":1}"#;
    let err = parse_str::<DerivedUser<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

// ---------------------------------------------------------------------------
// Tuple structs.
// ---------------------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    struct UserId(u64);
}

#[test]
fn newtype_parses_bare_value() {
    let id: UserId = parse_str("42").unwrap();
    assert_eq!(id, UserId(42));
}

#[test]
fn newtype_rejects_wrong_inner_type() {
    let r = parse_str::<UserId>("\"not a number\"");
    assert_eq!(r.unwrap_err().kind, ErrorKind::ExpectedNumber);
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct BorrowedTag<'input>(&'input str);
}

#[test]
fn newtype_with_borrow_lifetime() {
    let input = String::from("\"hello\"");
    let t: BorrowedTag<'_> = parse_str(&input).unwrap();
    assert_eq!(t, BorrowedTag("hello"));
    let ptr = t.0.as_ptr() as usize;
    let start = input.as_ptr() as usize;
    assert!((start..start + input.len()).contains(&ptr));
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct Pair(i32, String);
}

#[test]
fn multi_field_tuple_parses_array() {
    let p: Pair = parse_str(r#"[7,"seven"]"#).unwrap();
    assert_eq!(p, Pair(7, "seven".into()));
}

#[test]
fn multi_field_tuple_rejects_too_short() {
    let r = parse_str::<Pair>("[7]");
    assert_eq!(r.unwrap_err().kind, ErrorKind::TypeMismatch);
}

#[test]
fn multi_field_tuple_rejects_too_long() {
    let r = parse_str::<Pair>(r#"[7,"seven","extra"]"#);
    assert_eq!(r.unwrap_err().kind, ErrorKind::TypeMismatch);
}

#[test]
fn multi_field_tuple_rejects_empty_array() {
    let r = parse_str::<Pair>("[]");
    assert_eq!(r.unwrap_err().kind, ErrorKind::TypeMismatch);
}

// ---------------------------------------------------------------------------
// Field-level `#[bourne(rename = "...")]`.
// ---------------------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    struct UserRenamed<'input> {
        #[bourne(rename = "userId")]
        id: u64,
        name: &'input str,
    }
}

#[test]
fn field_rename_uses_renamed_key() {
    let json = r#"{"userId":42,"name":"alice"}"#;
    let u: UserRenamed<'_> = parse_str(json).unwrap();
    assert_eq!(
        u,
        UserRenamed {
            id: 42,
            name: "alice",
        }
    );
}

#[test]
fn field_rename_rejects_original_name() {
    let json = r#"{"id":42,"name":"alice"}"#;
    let err = parse_str::<UserRenamed<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

// ---------------------------------------------------------------------------
// Field-level `#[bourne(default)]`.
// ---------------------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    struct ConfigDefaulted {
        name: String,
        #[bourne(default)]
        retries: u32,
        #[bourne(default)]
        note: String,
    }
}

#[test]
fn default_fills_missing_primitive() {
    let json = r#"{"name":"primary"}"#;
    let c: ConfigDefaulted = parse_str(json).unwrap();
    assert_eq!(
        c,
        ConfigDefaulted {
            name: "primary".into(),
            retries: 0,
            note: String::new(),
        }
    );
}

#[test]
fn default_yields_to_present_value() {
    let json = r#"{"name":"primary","retries":7,"note":"hi"}"#;
    let c: ConfigDefaulted = parse_str(json).unwrap();
    assert_eq!(
        c,
        ConfigDefaulted {
            name: "primary".into(),
            retries: 7,
            note: "hi".into(),
        }
    );
}

#[test]
fn default_does_not_apply_to_required_field() {
    let json = r#"{"retries":3}"#;
    let err = parse_str::<ConfigDefaulted>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

// ---------------------------------------------------------------------------
// Container-level `#[bourne(deny_unknown_fields = false)]`.
// ---------------------------------------------------------------------------

from_json! {
    #[bourne(deny_unknown_fields = false)]
    #[derive(Debug, PartialEq)]
    struct LenientUser<'input> {
        id: u64,
        name: &'input str,
    }
}

#[test]
fn lenient_skips_unknown_primitive() {
    let json = r#"{"id":1,"extra":1,"name":"alice"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 1);
    assert_eq!(u.name, "alice");
}

#[test]
fn lenient_skips_unknown_array() {
    let json = r#"{"id":3,"tags":["a","b","c"],"name":"carol"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 3);
    assert_eq!(u.name, "carol");
}

#[test]
fn lenient_skips_unknown_nested_object() {
    let json = r#"{"id":4,"meta":{"a":[1,2,{"b":3}],"c":null},"name":"dave"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 4);
    assert_eq!(u.name, "dave");
}

#[test]
fn lenient_still_rejects_missing_required() {
    let json = r#"{"name":"frank","extra":99}"#;
    let err = parse_str::<LenientUser<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

// ---------------------------------------------------------------------------
// Externally-tagged enums.
// ---------------------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    enum Color {
        Red,
        Green,
        Blue,
    }
}

#[test]
fn unit_variant_parses_from_string() {
    assert_eq!(parse_str::<Color>("\"Red\"").unwrap(), Color::Red);
    assert_eq!(parse_str::<Color>("\"Green\"").unwrap(), Color::Green);
    assert_eq!(parse_str::<Color>("\"Blue\"").unwrap(), Color::Blue);
}

#[test]
fn unit_variant_rejects_unknown_tag() {
    let err = parse_str::<Color>("\"Yellow\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn unit_variant_rejects_non_string_value() {
    let err = parse_str::<Color>("42").unwrap_err();
    assert_eq!(err.kind, ErrorKind::TypeMismatch);
}

from_json! {
    #[derive(Debug, PartialEq)]
    enum Shape<'input> {
        Empty,
        Circle(f64),
        Line(f64, f64),
        Rect { width: u32, height: u32 },
        Tagged(&'input str),
    }
}

#[test]
fn enum_unit_variant_in_mixed_enum() {
    assert_eq!(parse_str::<Shape<'_>>("\"Empty\"").unwrap(), Shape::Empty);
}

#[test]
fn enum_newtype_variant_parses_bare_value() {
    let s: Shape<'_> = parse_str(r#"{"Circle":4.5}"#).unwrap();
    assert!(matches!(s, Shape::Circle(v) if (v - 4.5).abs() < 1e-9));
}

#[test]
fn enum_tuple_variant_parses_array() {
    let s: Shape<'_> = parse_str(r#"{"Line":[1.0,2.5]}"#).unwrap();
    assert!(matches!(s, Shape::Line(a, b) if (a - 1.0).abs() < 1e-9 && (b - 2.5).abs() < 1e-9));
}

#[test]
fn enum_struct_variant_parses_nested_object() {
    let s: Shape<'_> = parse_str(r#"{"Rect":{"width":10,"height":20}}"#).unwrap();
    assert_eq!(s, Shape::Rect { width: 10, height: 20 });
}

#[test]
fn enum_borrowed_str_variant_zero_copy() {
    let input = String::from(r#"{"Tagged":"hello"}"#);
    let s: Shape<'_> = parse_str(&input).unwrap();
    let Shape::Tagged(inner) = s else {
        panic!("expected Tagged");
    };
    let start = input.as_ptr() as usize;
    let ptr = inner.as_ptr() as usize;
    assert!((start..start + input.len()).contains(&ptr));
}

#[test]
fn enum_rejects_unknown_tag_in_object_form() {
    let err = parse_str::<Shape<'_>>(r#"{"Triangle":42}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn enum_rejects_extra_tag_keys() {
    let err = parse_str::<Shape<'_>>(r#"{"Circle":3.14,"Extra":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

from_json! {
    #[derive(Debug, PartialEq)]
    enum Direction {
        #[bourne(rename = "N")]
        North,
        #[bourne(rename = "S")]
        South,
        East,
    }
}

#[test]
fn variant_rename_uses_renamed_tag() {
    assert_eq!(parse_str::<Direction>("\"N\"").unwrap(), Direction::North);
    assert_eq!(parse_str::<Direction>("\"S\"").unwrap(), Direction::South);
    assert_eq!(parse_str::<Direction>("\"East\"").unwrap(), Direction::East);
}

#[test]
fn variant_rename_rejects_original_name() {
    let err = parse_str::<Direction>("\"North\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}
