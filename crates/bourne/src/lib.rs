#![cfg_attr(not(feature = "std"), no_std)]

//! `bourne` — type-driven JSON parsing.
//!
//! The top-level entry point is [`parse`], which deserializes directly into
//! the caller's chosen type. There is no generic `Value` middle layer.
//!
//! ```
//! use bourne::parse_str;
//!
//! let n: u32 = parse_str("42").unwrap();
//! assert_eq!(n, 42);
//! ```

#[cfg(feature = "alloc")]
extern crate alloc;

mod de;

pub use bourne_core::{Error, ErrorKind, Event, JsonNum, JsonStr, Lexer, Parser, Position, ValueKind};
pub use de::{FromJson, parse, parse_str};

mod macros;

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written `FromJson` for a representative struct.
    ///
    /// This is the shape a derive macro would generate. Validating the trait
    /// shape with a real struct before writing the macro means we know the
    /// generated code can be both correct and ergonomic.
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
            let mut nickname: Option<&'input str> = None;

            let mut maybe_key = lex.object_first_key()?;
            while let Some(key) = maybe_key {
                match key {
                    "id" => {
                        if id.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, lex.position()));
                        }
                        id = Some(u64::from_lex(lex)?);
                    }
                    "name" => {
                        if name.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, lex.position()));
                        }
                        name = Some(<&str>::from_lex(lex)?);
                    }
                    "active" => {
                        if active.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, lex.position()));
                        }
                        active = Some(bool::from_lex(lex)?);
                    }
                    "nickname" => {
                        if nickname.is_some() {
                            return Err(Error::new(ErrorKind::DuplicateKey, lex.position()));
                        }
                        nickname = Option::<&str>::from_lex(lex)?;
                    }
                    _ => {
                        return Err(Error::new(ErrorKind::UnknownField, lex.position()));
                    }
                }
                maybe_key = lex.object_next_key()?;
            }

            Ok(Self {
                id: id.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
                name: name.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
                active: active
                    .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
                nickname,
            })
        }
    }

    /// Regression: after `object_first_key` consumes the key + `:`, the
    /// parser must be in a state where `next_event` correctly reads the
    /// next byte as the start of a value (not as the start of a key).
    /// Mixing the fast-path object API with `next_event` per-value is a
    /// supported pattern — `UserBourne::from_event` in the bench does
    /// exactly this for fields whose typed path doesn't have a fused
    /// `parse_*_value` method (e.g. `bool`, `Option<&str>`, `Vec<&str>`).
    #[test]
    fn object_first_key_then_next_event_for_value() {
        let mut p: Parser<'_> = Parser::new(br#"{"flag":true}"#);
        let start = p.next_event().unwrap().unwrap();
        assert!(matches!(start, Event::StartObject));

        // Fast-path: read the key, leaving cursor past `:`.
        let key = p.object_first_key().unwrap().unwrap();
        assert_eq!(key, "flag");

        // Now fall back to next_event for the value.
        let val = p.next_event().unwrap().unwrap();
        assert_eq!(val, Event::Bool(true));

        // Object close.
        let close = p.next_event().unwrap().unwrap();
        assert!(matches!(close, Event::EndObject));
        assert!(p.next_event().unwrap().is_none());
    }

    /// Same shape, but using `object_next_key` after the first value to
    /// pull the next key. Exercises both fast-path entry points.
    #[test]
    fn object_next_key_handoff_to_next_event() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":1,"b":true}"#);
        let _ = p.next_event().unwrap().unwrap(); // StartObject

        let k1 = p.object_first_key().unwrap().unwrap();
        assert_eq!(k1, "a");
        let v1 = p.parse_i64_value().unwrap();
        assert_eq!(v1, 1);

        let k2 = p.object_next_key().unwrap().unwrap();
        assert_eq!(k2, "b");
        let v2 = p.next_event().unwrap().unwrap();
        assert_eq!(v2, Event::Bool(true));

        assert!(p.object_next_key().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn primitives_round_through_typed_parse() {
        assert!(parse_str::<bool>("true").unwrap());
        assert!(!parse_str::<bool>("false").unwrap());
        assert_eq!(parse_str::<i32>("-7").unwrap(), -7);
        assert_eq!(parse_str::<u64>("9999999999").unwrap(), 9_999_999_999);
        let f: f64 = parse_str("1.5e2").unwrap();
        assert!((f - 150.0).abs() < f64::EPSILON);
        assert_eq!(parse_str::<&str>("\"hello\"").unwrap(), "hello");
    }

    #[test]
    fn integer_overflow_is_rejected_at_parse_site() {
        let r = parse_str::<u8>("256");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
    }

    #[test]
    fn type_mismatch_is_rejected_at_parse_site() {
        let r = parse_str::<u32>("\"five\"");
        assert_eq!(r.unwrap_err().kind, ErrorKind::ExpectedNumber);
    }

    #[test]
    fn option_handles_null_and_value() {
        let none: Option<u32> = parse_str("null").unwrap();
        assert_eq!(none, None);
        let some: Option<u32> = parse_str("17").unwrap();
        assert_eq!(some, Some(17));
    }

    #[test]
    fn fixed_array_exact_length() {
        let arr: [i32; 3] = parse_str("[1,2,3]").unwrap();
        assert_eq!(arr, [1, 2, 3]);
    }

    #[test]
    fn fixed_array_too_short_errors() {
        let r = parse_str::<[i32; 3]>("[1,2]");
        assert!(r.is_err());
    }

    #[test]
    fn fixed_array_too_long_errors() {
        let r = parse_str::<[i32; 3]>("[1,2,3,4]");
        assert!(r.is_err());
    }

    #[test]
    fn tuple_heterogeneous() {
        let v: (i32, &str, bool) = parse_str(r#"[1, "hi", true]"#).unwrap();
        assert_eq!(v, (1, "hi", true));
    }

    #[test]
    fn borrowed_str_lifetime() {
        let input = String::from(r#""borrowed""#);
        let s: &str = parse_str(&input).unwrap();
        assert_eq!(s, "borrowed");
    }

    #[test]
    fn rejects_trailing_data_at_typed_layer() {
        let r = parse_str::<u32>("1 2");
        assert!(r.is_err());
    }

    /// Regression: `parse_i64_value`'s fast-path accumulator must accept
    /// `i64::MIN` (text `"-9223372036854775808"`). Earlier versions
    /// accumulated as `i64`, so the unsigned magnitude (= `i64::MAX + 1`)
    /// overflowed before the negation step and the input was rejected as
    /// `NumberOutOfRange`. Pin both the value and the boundary +/- 1.
    #[test]
    fn i64_min_is_parseable() {
        assert_eq!(parse_str::<i64>("-9223372036854775808").unwrap(), i64::MIN);
        assert_eq!(
            parse_str::<i64>("-9223372036854775807").unwrap(),
            i64::MIN + 1,
        );
        assert_eq!(parse_str::<i64>("9223372036854775807").unwrap(), i64::MAX);
        // Just past i64::MIN must reject.
        assert!(parse_str::<i64>("-9223372036854775809").is_err());
        // Just past i64::MAX must reject.
        assert!(parse_str::<i64>("9223372036854775808").is_err());
        // Same boundaries via Vec<i64> (the bench-fixture path that surfaced this).
        let v: Vec<i64> = parse_str("[-9223372036854775808]").unwrap();
        assert_eq!(v, vec![i64::MIN]);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn vec_and_string() {
        let v: Vec<i32> = parse_str("[10, 20, 30]").unwrap();
        assert_eq!(v, vec![10, 20, 30]);
        let s: String = parse_str(r#""owned""#).unwrap();
        assert_eq!(s, "owned");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn nested_vec() {
        let v: Vec<Vec<i32>> = parse_str("[[1,2],[3]]").unwrap();
        assert_eq!(v, vec![vec![1, 2], vec![3]]);
    }

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
        // Field order is irrelevant; missing optional collapses to None.
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
        // The pointer must lie inside the input buffer — proving zero copy.
        let input_start = input.as_ptr() as usize;
        let input_end = input_start + input.len();
        let name_ptr = u.name.as_ptr() as usize;
        assert!(
            (input_start..input_end).contains(&name_ptr),
            "name was copied, not borrowed",
        );
    }

    // -----------------------------------------------------------------
    // Escape decoding into String / Cow<str>.
    // -----------------------------------------------------------------

    #[cfg(feature = "alloc")]
    #[test]
    fn string_decodes_simple_escapes() {
        let s: String = parse_str(r#""line1\nline2\ttab\"quote\\back\/slash""#).unwrap();
        assert_eq!(s, "line1\nline2\ttab\"quote\\back/slash");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_decodes_b_and_f_escapes() {
        // \b = U+0008, \f = U+000C — the rarely-used pair.
        let s: String = parse_str(r#""\b\f""#).unwrap();
        assert_eq!(s, "\u{0008}\u{000C}");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_decodes_unicode_bmp_escape() {
        // Latin-1 (1-byte codepoint), Latin-Extended (2-byte UTF-8),
        // and CJK (3-byte UTF-8) — covers each BMP encoding length.
        let s: String = parse_str(r#""café 中文 ~""#).unwrap();
        assert_eq!(s, "café 中文 ~");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_decodes_surrogate_pair() {
        // U+1F600 (😀, GRINNING FACE) encodes as the surrogate pair
        // 😀 in JSON. Decoded form is 4 bytes of UTF-8.
        let s: String = parse_str(r#""hi 😀 there""#).unwrap();
        assert_eq!(s, "hi 😀 there");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_rejects_lone_surrogate() {
        let r = parse_str::<String>(r#""\uD800""#);
        assert_eq!(r.unwrap_err().kind, ErrorKind::UnpairedSurrogate);
        let r = parse_str::<String>(r#""\uDC00""#);
        assert_eq!(r.unwrap_err().kind, ErrorKind::UnpairedSurrogate);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_rejects_high_surrogate_then_non_low() {
        // \uD800 followed by a non-surrogate \u escape.
        let r = parse_str::<String>(r#""\uD800A""#);
        assert_eq!(r.unwrap_err().kind, ErrorKind::UnpairedSurrogate);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn string_decodes_long_mixed_input() {
        // Mix literal stretches and several escapes — exercises the literal-
        // run fast path and verifies the cursor advances correctly across
        // multiple escapes in one string.
        let json = r#""one\ttwo\nthreeAfour\\five\"six""#;
        let s: String = parse_str(json).unwrap();
        assert_eq!(s, "one\ttwo\nthreeAfour\\five\"six");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn vec_string_with_escapes_roundtrips() {
        // Regression: this is the case `Vec<String>` could not parse
        // before the decoder landed. Pin both empty-string and
        // multi-escape-per-element forms.
        let json = r#"["","a","\n","mixéd","\\\"\/"]"#;
        let v: Vec<String> = parse_str(json).unwrap();
        assert_eq!(v, vec!["", "a", "\n", "mixéd", "\\\"/"]);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn cow_borrows_when_no_escapes() {
        use std::borrow::Cow;
        let input = String::from(r#""borrowed""#);
        let c: Cow<'_, str> = parse_str(&input).unwrap();
        assert_eq!(c, "borrowed");
        // Borrowed variant means the pointer lies inside the input buffer.
        // A copy would land outside it.
        match c {
            Cow::Borrowed(s) => {
                let input_start = input.as_ptr() as usize;
                let input_end = input_start + input.len();
                let s_ptr = s.as_ptr() as usize;
                assert!(
                    (input_start..input_end).contains(&s_ptr),
                    "Cow::Borrowed pointer should be inside input",
                );
            }
            Cow::Owned(_) => panic!("expected Borrowed for un-escaped input"),
        }
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn cow_owns_when_escapes_present() {
        use std::borrow::Cow;
        let c: Cow<'_, str> = parse_str(r#""line\nwrap""#).unwrap();
        assert_eq!(c, "line\nwrap");
        assert!(matches!(c, Cow::Owned(_)));
    }
}
