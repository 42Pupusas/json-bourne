//! Single-case smoke test for `from_json!`. Mirrors the simplest path
//! the test suite exercises: a borrowed-string struct with one
//! `Option<&str>` field. If this case doesn't expand cleanly the
//! macro design is wrong and the wider port is wasted effort.

#![cfg(feature = "std")]

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

#[test]
fn lenient_unknown_at_start_then_known_fields() {
    let json = r#"{"extra":true,"id":7,"name":"eve"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 7);
    assert_eq!(u.name, "eve");
}

#[test]
fn lenient_unknown_at_end_after_known_fields() {
    let json = r#"{"id":8,"name":"finn","extra":false}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 8);
    assert_eq!(u.name, "finn");
}

#[test]
fn lenient_multiple_consecutive_unknowns() {
    let json = r#"{"a":1,"b":2,"c":3,"id":9,"d":4,"e":5,"name":"grace","f":6}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 9);
    assert_eq!(u.name, "grace");
}

#[test]
fn lenient_unknown_with_deeply_nested_value() {
    let json = r#"{"id":10,"deep":[[[[{"a":[{"b":[1,[2,[3]]]}]}]]]],"name":"hank"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 10);
    assert_eq!(u.name, "hank");
}

#[test]
fn lenient_unknown_with_escaped_key() {
    let json = r#"{"id":11,"foo":1,"name":"ivy"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 11);
    assert_eq!(u.name, "ivy");
}

#[test]
fn lenient_unknown_with_null_and_escapes_in_value() {
    let json = r#"{"id":12,"meta":{"x":null,"s":"a\nbB"},"name":"juno"}"#;
    let u: LenientUser<'_> = parse_str(json).unwrap();
    assert_eq!(u.id, 12);
    assert_eq!(u.name, "juno");
}

#[test]
fn lenient_unknown_value_with_malformed_inside_still_errors() {
    // The skip path lexes the value to find its end. A malformed
    // unknown value should still surface an error rather than be
    // silently dropped — protects the invariant that lenient is
    // about *unknown keys*, not about *malformed JSON*.
    let json = r#"{"id":1,"junk":[1,,2],"name":"k"}"#;
    let err = parse_str::<LenientUser<'_>>(json).unwrap_err();
    // The exact error kind depends on lexer specifics; we just want
    // to confirm the parse fails rather than silently succeeding.
    assert!(matches!(
        err.kind,
        ErrorKind::UnexpectedByte(_) | ErrorKind::TypeMismatch | ErrorKind::InvalidNumber
    ));
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
    assert_eq!(
        s,
        Shape::Rect {
            width: 10,
            height: 20
        }
    );
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

// ---------------------------------------------------------------------------
// Internally-tagged enums (#[bourne(tag = "...")]).
// ---------------------------------------------------------------------------

from_json! {
    #[bourne(tag = "type")]
    #[derive(Debug, PartialEq)]
    enum Animal {
        Dog,
        Cat { lives: u32 },
        Fish { species: u32, depth: i32 },
    }
}

#[test]
fn internally_tagged_unit_variant() {
    let a: Animal = parse_str(r#"{"type":"Dog"}"#).unwrap();
    assert_eq!(a, Animal::Dog);
}

#[test]
fn internally_tagged_struct_variant_tag_first() {
    let a: Animal = parse_str(r#"{"type":"Cat","lives":9}"#).unwrap();
    assert_eq!(a, Animal::Cat { lives: 9 });
}

#[test]
fn internally_tagged_struct_variant_tag_last() {
    let a: Animal = parse_str(r#"{"lives":9,"type":"Cat"}"#).unwrap();
    assert_eq!(a, Animal::Cat { lives: 9 });
}

#[test]
fn internally_tagged_struct_variant_tag_middle() {
    let a: Animal = parse_str(r#"{"species":7,"type":"Fish","depth":-200}"#).unwrap();
    assert_eq!(
        a,
        Animal::Fish {
            species: 7,
            depth: -200,
        }
    );
}

#[test]
fn internally_tagged_missing_tag_field() {
    let err = parse_str::<Animal>(r#"{"lives":9}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn internally_tagged_unknown_variant() {
    let err = parse_str::<Animal>(r#"{"type":"Bird"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn internally_tagged_unknown_field_in_variant() {
    let err = parse_str::<Animal>(r#"{"type":"Cat","lives":9,"extra":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn internally_tagged_missing_required_field() {
    let err = parse_str::<Animal>(r#"{"type":"Cat"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn internally_tagged_unit_rejects_extra_fields() {
    let err = parse_str::<Animal>(r#"{"type":"Dog","extra":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

from_json! {
    #[bourne(tag = "kind")]
    #[derive(Debug, PartialEq)]
    enum Renamed {
        #[bourne(rename = "ok")]
        Success { value: i32 },
        #[bourne(rename = "err")]
        Failure,
    }
}

#[test]
fn internally_tagged_variant_rename() {
    let v: Renamed = parse_str(r#"{"kind":"ok","value":42}"#).unwrap();
    assert_eq!(v, Renamed::Success { value: 42 });
    let v: Renamed = parse_str(r#"{"kind":"err"}"#).unwrap();
    assert_eq!(v, Renamed::Failure);
}

#[test]
fn internally_tagged_rename_rejects_original_name() {
    let err = parse_str::<Renamed>(r#"{"kind":"Success","value":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

// ---------------------------------------------------------------------------
// Adjacently-tagged enums (#[bourne(tag = "t", content = "c")]).
// ---------------------------------------------------------------------------

from_json! {
    #[bourne(tag = "t", content = "c")]
    #[derive(Debug, PartialEq)]
    enum Msg {
        Ping,
        Echo(i64),
        Pair(i32, i32),
        Body { code: u32, text: String },
    }
}

#[test]
fn adjacent_unit_variant_no_content() {
    let m: Msg = parse_str(r#"{"t":"Ping"}"#).unwrap();
    assert_eq!(m, Msg::Ping);
}

#[test]
fn adjacent_unit_variant_rejects_content() {
    let err = parse_str::<Msg>(r#"{"t":"Ping","c":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn adjacent_newtype_variant() {
    let m: Msg = parse_str(r#"{"t":"Echo","c":42}"#).unwrap();
    assert_eq!(m, Msg::Echo(42));
}

#[test]
fn adjacent_newtype_content_first() {
    let m: Msg = parse_str(r#"{"c":42,"t":"Echo"}"#).unwrap();
    assert_eq!(m, Msg::Echo(42));
}

#[test]
fn adjacent_tuple_variant() {
    let m: Msg = parse_str(r#"{"t":"Pair","c":[1,2]}"#).unwrap();
    assert_eq!(m, Msg::Pair(1, 2));
}

#[test]
fn adjacent_struct_variant() {
    let m: Msg = parse_str(r#"{"t":"Body","c":{"code":200,"text":"ok"}}"#).unwrap();
    assert_eq!(
        m,
        Msg::Body {
            code: 200,
            text: String::from("ok"),
        }
    );
}

#[test]
fn adjacent_struct_variant_content_first() {
    let m: Msg = parse_str(r#"{"c":{"code":200,"text":"ok"},"t":"Body"}"#).unwrap();
    assert_eq!(
        m,
        Msg::Body {
            code: 200,
            text: String::from("ok"),
        }
    );
}

#[test]
fn adjacent_missing_tag() {
    let err = parse_str::<Msg>(r#"{"c":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn adjacent_missing_content_for_payload_variant() {
    let err = parse_str::<Msg>(r#"{"t":"Echo"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn adjacent_unknown_tag() {
    let err = parse_str::<Msg>(r#"{"t":"Nope","c":1}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn adjacent_extra_field_rejected() {
    let err = parse_str::<Msg>(r#"{"t":"Echo","c":1,"x":2}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn adjacent_duplicate_tag_rejected() {
    let err = parse_str::<Msg>(r#"{"t":"Ping","t":"Echo"}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DuplicateKey);
}

// ---------------------------------------------------------------------------
// Untagged enums (#[bourne(untagged)]).
// ---------------------------------------------------------------------------

from_json! {
    #[bourne(untagged)]
    #[derive(Debug, PartialEq)]
    enum Scalar {
        I(i64),
        S(String),
        Nothing,
    }
}

#[test]
fn untagged_picks_int_branch() {
    let v: Scalar = parse_str("42").unwrap();
    assert_eq!(v, Scalar::I(42));
}

#[test]
fn untagged_picks_string_branch() {
    let v: Scalar = parse_str(r#""hello""#).unwrap();
    assert_eq!(v, Scalar::S(String::from("hello")));
}

#[test]
fn untagged_picks_unit_branch_for_null() {
    let v: Scalar = parse_str("null").unwrap();
    assert_eq!(v, Scalar::Nothing);
}

#[test]
fn untagged_no_match_yields_type_mismatch() {
    let err = parse_str::<Scalar>("true").unwrap_err();
    assert_eq!(err.kind, ErrorKind::TypeMismatch);
}

from_json! {
    #[bourne(untagged)]
    #[derive(Debug, PartialEq)]
    enum Shape2 {
        Pair(i32, i32),
        Triple(i32, i32, i32),
        Single(i32),
    }
}

#[test]
fn untagged_distinguishes_arrays_by_length() {
    let v: Shape2 = parse_str("[1,2]").unwrap();
    assert_eq!(v, Shape2::Pair(1, 2));
    let v: Shape2 = parse_str("[1,2,3]").unwrap();
    assert_eq!(v, Shape2::Triple(1, 2, 3));
    let v: Shape2 = parse_str("7").unwrap();
    assert_eq!(v, Shape2::Single(7));
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct Coord {
        x: i32,
        y: i32,
    }
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct Named {
        name: String,
    }
}

from_json! {
    #[bourne(untagged)]
    #[derive(Debug, PartialEq)]
    enum Either {
        AsCoord(Coord),
        AsNamed(Named),
    }
}

#[test]
fn untagged_picks_struct_by_field_shape() {
    let v: Either = parse_str(r#"{"x":1,"y":2}"#).unwrap();
    assert_eq!(v, Either::AsCoord(Coord { x: 1, y: 2 }));
    let v: Either = parse_str(r#"{"name":"alice"}"#).unwrap();
    assert_eq!(
        v,
        Either::AsNamed(Named {
            name: String::from("alice"),
        })
    );
}

from_json! {
    #[bourne(untagged)]
    #[derive(Debug, PartialEq)]
    enum InlineStruct {
        Point { x: i32, y: i32 },
        Line { from: i32, to: i32 },
    }
}

#[test]
fn untagged_inline_struct_variants() {
    let v: InlineStruct = parse_str(r#"{"x":3,"y":4}"#).unwrap();
    assert_eq!(v, InlineStruct::Point { x: 3, y: 4 });
    let v: InlineStruct = parse_str(r#"{"from":1,"to":10}"#).unwrap();
    assert_eq!(v, InlineStruct::Line { from: 1, to: 10 });
}

#[test]
fn untagged_first_match_wins() {
    // Both Point and Line are objects; an object that matches Point's
    // shape should never reach Line. (Regression guard against the
    // walker not short-circuiting on Ok.)
    let v: InlineStruct = parse_str(r#"{"x":0,"y":0}"#).unwrap();
    assert!(matches!(v, InlineStruct::Point { .. }));
}
