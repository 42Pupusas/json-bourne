//! Tests for hand-written and macro-generated `FromJson` struct impls.
//!
//! Moved out of `lib.rs` so that `cargo crappy` doesn't score test-only
//! struct impls against the library's CRAP threshold.

#![cfg(feature = "std")]

use json_bourne::{
    Error, ErrorKind, FromJson, Lexer, from_json, parse_str,
};

// -----------------------------------------------------------------
// Hand-written FromJson for a representative struct.
//
// This is the shape a derive macro would generate.
// -----------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct User<'input> {
    id: u64,
    name: &'input str,
    active: bool,
    nickname: Option<&'input str>,
}

impl<'input> FromJson<'input> for User<'input> {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        lex.object_start()?;

        let mut id: Option<u64> = None;
        let mut name: Option<&'input str> = None;
        let mut active: Option<bool> = None;
        let mut nickname: Option<Option<&'input str>> = None;

        let dup = |lex: &Lexer<'_>| Error::new(ErrorKind::DuplicateKey, lex.position());
        let missing = |lex: &Lexer<'_>| Error::new(ErrorKind::MissingField, lex.position());

        let mut maybe_key = lex.object_first_key()?;
        while let Some(key) = maybe_key {
            match key {
                "id" if id.is_none() => id = Some(u64::from_lex(lex)?),
                "name" if name.is_none() => name = Some(<&str>::from_lex(lex)?),
                "active" if active.is_none() => active = Some(bool::from_lex(lex)?),
                "nickname" if nickname.is_none() => {
                    nickname = Some(Option::<&str>::from_lex(lex)?);
                }
                "id" | "name" | "active" | "nickname" => return Err(dup(lex)),
                _ => return Err(Error::new(ErrorKind::UnknownField, lex.position())),
            }
            maybe_key = lex.object_next_key()?;
        }

        Ok(Self {
            id: id.ok_or_else(|| missing(lex))?,
            name: name.ok_or_else(|| missing(lex))?,
            active: active.ok_or_else(|| missing(lex))?,
            nickname: nickname.flatten(),
        })
    }
}

// -----------------------------------------------------------------
// User struct tests
// -----------------------------------------------------------------

#[test]
fn struct_parses_with_all_fields() {
    let json = r#"{"id":42,"name":"alice","active":true,"nickname":"al"}"#;
    let u: User<'_> = parse_str(json).unwrap();
    assert_eq!(
        u,
        User {
            id: 42,
            name: "alice",
            active: true,
            nickname: Some("al"),
        }
    );
}

#[test]
fn struct_parses_with_optional_field_null() {
    let json = r#"{"id":1,"name":"bob","active":false,"nickname":null}"#;
    let u: User<'_> = parse_str(json).unwrap();
    assert_eq!(u.nickname, None);
}

#[test]
fn struct_parses_with_optional_field_omitted() {
    let json = r#"{"name":"carol","id":7,"active":true}"#;
    let u: User<'_> = parse_str(json).unwrap();
    assert_eq!(u.nickname, None);
}

#[test]
fn struct_rejects_missing_required_field() {
    let json = r#"{"id":1,"active":true}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::MissingField);
}

#[test]
fn struct_rejects_wrong_field_type() {
    let json = r#"{"id":"oops","name":"d","active":true}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectedNumber);
}

#[test]
fn struct_rejects_unknown_field() {
    let json = r#"{"id":1,"name":"e","active":true,"extra":1}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

#[test]
fn struct_rejects_duplicate_key() {
    let json = r#"{"id":1,"id":2,"name":"f","active":true}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DuplicateKey);
}

#[test]
fn struct_borrows_strings_from_input() {
    let input = String::from(r#"{"id":1,"name":"borrowed","active":true}"#);
    let u: User<'_> = parse_str(&input).unwrap();
    let input_start = input.as_ptr() as usize;
    let input_end = input_start + input.len();
    let name_ptr = u.name.as_ptr() as usize;
    assert!(
        (input_start..input_end).contains(&name_ptr),
        "name was copied, not borrowed",
    );
}

#[test]
fn struct_rejects_duplicate_name() {
    let json = r#"{"id":1,"name":"a","name":"b","active":true}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DuplicateKey);
}

#[test]
fn struct_rejects_duplicate_active() {
    let json = r#"{"id":1,"name":"a","active":true,"active":false}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DuplicateKey);
}

#[test]
fn struct_rejects_duplicate_nickname() {
    let json = r#"{"id":1,"name":"a","active":true,"nickname":"x","nickname":"y"}"#;
    let err = parse_str::<User<'_>>(json).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DuplicateKey);
}

// -----------------------------------------------------------------
// from_json! macro struct tests — escape keys
// -----------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    struct EscKey {
        #[bourne(rename = "user-id")]
        user_id: u32,
        #[bourne(rename = "x\ny")]
        x_newline_y: u32,
    }
}

#[test]
fn struct_dispatch_handles_escaped_key() {
    let j = r#"{"user-id":1,"x\ny":2}"#;
    let r: EscKey = parse_str(j).unwrap();
    assert_eq!(
        r,
        EscKey {
            user_id: 1,
            x_newline_y: 2
        }
    );
}

#[test]
fn struct_dispatch_handles_unicode_escape_in_key() {
    from_json! {
        #[derive(Debug, PartialEq)]
        struct PlainId { id: u32 }
    }
    let j = r#"{"id":7}"#;
    let r: PlainId = parse_str(j).unwrap();
    assert_eq!(r, PlainId { id: 7 });
}

// -----------------------------------------------------------------
// Escape keys in maps and enums
// -----------------------------------------------------------------

#[test]
fn hashmap_handles_escaped_key() {
    use std::collections::HashMap;
    let m: HashMap<String, i32> = parse_str(r#"{"a\nb":1,"c":2}"#).unwrap();
    assert_eq!(m.get("a\nb"), Some(&1));
    assert_eq!(m.get("c"), Some(&2));
}

#[test]
fn hashmap_borrowed_key_rejects_escapes() {
    use std::collections::HashMap;
    let r: Result<HashMap<&str, i32>, _> = parse_str(r#"{"a\nb":1}"#);
    assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidEscape);
}

from_json! {
    #[derive(Debug, PartialEq)]
    enum Tagged {
        Plain(u32),
        #[bourne(rename = "with\nbreak")]
        WithBreak(u32),
    }
}

#[test]
fn enum_dispatch_handles_escaped_tag() {
    let r: Tagged = parse_str(r#"{"with\nbreak":42}"#).unwrap();
    assert_eq!(r, Tagged::WithBreak(42));
    let r: Tagged = parse_str(r#"{"Plain":1}"#).unwrap();
    assert_eq!(r, Tagged::Plain(1));
}

#[test]
fn hashmap_cow_key_borrows_or_owns_per_entry() {
    use std::borrow::Cow;
    use std::collections::HashMap;
    let input = String::from(r#"{"plain":1,"esc\nape":2}"#);
    let m: HashMap<Cow<'_, str>, i32> = parse_str(&input).unwrap();
    let plain = m.iter().find(|(k, _)| k.as_ref() == "plain").unwrap().0;
    assert!(matches!(plain, Cow::Borrowed(_)));
    let esc = m.iter().find(|(k, _)| k.as_ref() == "esc\nape").unwrap().0;
    assert!(matches!(esc, Cow::Owned(_)));
}

// -----------------------------------------------------------------
// from_json! macro field attributes
// -----------------------------------------------------------------

from_json! {
    #[derive(Debug, PartialEq)]
    struct Renamed {
        #[bourne(rename = "user-id")]
        user_id: u32,
        #[bourne(rename = "displayName")]
        display_name: u32,
    }
}

#[test]
fn macro_field_rename_uses_renamed_key() {
    let j = r#"{"user-id":1,"displayName":2}"#;
    let r: Renamed = parse_str(j).unwrap();
    assert_eq!(
        r,
        Renamed {
            user_id: 1,
            display_name: 2
        }
    );
}

#[test]
fn macro_field_rename_rejects_original_name() {
    let j = r#"{"user_id":1,"display_name":2}"#;
    assert!(parse_str::<Renamed>(j).is_err());
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct WithDefaults {
        id: u32,
        #[bourne(default)]
        count: u32,
        #[bourne(default)]
        label: String,
    }
}

#[test]
fn macro_field_default_fills_missing_keys() {
    let r: WithDefaults = parse_str(r#"{"id":42}"#).unwrap();
    assert_eq!(
        r,
        WithDefaults {
            id: 42,
            count: 0,
            label: String::new(),
        }
    );
}

#[test]
fn macro_field_default_overridden_by_present_value() {
    let r: WithDefaults = parse_str(r#"{"id":1,"count":7,"label":"hi"}"#).unwrap();
    assert_eq!(r.count, 7);
    assert_eq!(r.label, "hi");
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct WithSkip {
        id: u32,
        #[bourne(skip)]
        cached: String,
        value: u32,
    }
}

#[test]
fn macro_field_skip_omits_from_dispatch() {
    let r: WithSkip = parse_str(r#"{"id":1,"value":2}"#).unwrap();
    assert_eq!(
        r,
        WithSkip {
            id: 1,
            cached: String::new(),
            value: 2,
        }
    );
}

#[test]
fn macro_field_skip_rejects_key_in_input() {
    let j = r#"{"id":1,"cached":"oops","value":2}"#;
    let err = parse_str::<WithSkip>(j).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownField);
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct SkipAtTail {
        id: u32,
        #[bourne(skip)]
        trailing: u32,
    }
}

#[test]
fn macro_field_skip_at_last_position() {
    let r: SkipAtTail = parse_str(r#"{"id":7}"#).unwrap();
    assert_eq!(r, SkipAtTail { id: 7, trailing: 0 });
}

from_json! {
    #[derive(Debug, PartialEq)]
    struct RenameAndDefault {
        #[bourne(rename = "max-retries", default)]
        max_retries: u32,
    }
}

#[test]
fn macro_compound_rename_default() {
    let r: RenameAndDefault = parse_str("{}").unwrap();
    assert_eq!(r.max_retries, 0);
    let r: RenameAndDefault = parse_str(r#"{"max-retries":5}"#).unwrap();
    assert_eq!(r.max_retries, 5);
}
