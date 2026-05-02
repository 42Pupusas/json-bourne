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
mod ser;

pub use bourne_core::{
    Checkpoint, Error, ErrorKind, Event, JsonNum, JsonStr, Lexer, Parser, Position, ValueKind,
};
pub use de::{FromJson, parse, parse_str};
#[cfg(feature = "alloc")]
pub use de::{MapKey, key_to_cow};
pub use ser::{JsonWrite, ToJson};
#[cfg(feature = "alloc")]
pub use ser::{MapKeyOut, StringSink, to_string, to_vec};

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
    #[cfg(feature = "std")]
    #[test]
    fn duration_round_trips_fractional_seconds() {
        use std::time::Duration;
        let d: Duration = parse_str("1.5").unwrap();
        assert_eq!(d, Duration::new(1, 500_000_000));
        let d: Duration = parse_str("0").unwrap();
        assert_eq!(d, Duration::ZERO);
        // Negative is rejected.
        assert!(parse_str::<Duration>("-1").is_err());
    }

    #[cfg(feature = "std")]
    #[test]
    fn ip_and_socket_addrs_parse_from_strings() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
        let v4: Ipv4Addr = parse_str(r#""127.0.0.1""#).unwrap();
        assert_eq!(v4, Ipv4Addr::LOCALHOST);
        let v6: Ipv6Addr = parse_str(r#""::1""#).unwrap();
        assert_eq!(v6, Ipv6Addr::LOCALHOST);
        let ip: IpAddr = parse_str(r#""10.0.0.1""#).unwrap();
        assert!(matches!(ip, IpAddr::V4(_)));
        let sa: SocketAddr = parse_str(r#""127.0.0.1:8080""#).unwrap();
        assert_eq!(sa.port(), 8080);
        // Garbage rejected.
        assert!(parse_str::<Ipv4Addr>(r#""not-an-ip""#).is_err());
    }

    #[cfg(feature = "std")]
    #[test]
    fn pathbuf_parses_from_string() {
        use std::path::PathBuf;
        let p: PathBuf = parse_str(r#""/etc/hosts""#).unwrap();
        assert_eq!(p, PathBuf::from("/etc/hosts"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_string_key() {
        use std::collections::HashMap;
        let m: HashMap<String, i32> = parse_str(r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(m.get("a"), Some(&1));
        assert_eq!(m.get("b"), Some(&2));
        assert_eq!(m.len(), 2);
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_borrowed_key_zero_copy() {
        use std::collections::HashMap;
        let input = String::from(r#"{"alpha":1,"beta":2}"#);
        let m: HashMap<&str, i32> = parse_str(&input).unwrap();
        // Both keys must point inside the input buffer.
        let input_start = input.as_ptr() as usize;
        let input_end = input_start + input.len();
        for k in m.keys() {
            let p = k.as_ptr() as usize;
            assert!((input_start..input_end).contains(&p), "key was copied");
        }
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn btreemap_round_trips() {
        use std::collections::BTreeMap;
        let m: BTreeMap<String, Vec<i32>> = parse_str(r#"{"x":[1,2],"y":[3]}"#).unwrap();
        assert_eq!(m["x"], vec![1, 2]);
        assert_eq!(m["y"], vec![3]);
    }

    #[cfg(feature = "std")]
    #[test]
    fn map_rejects_duplicate_key() {
        use std::collections::HashMap;
        let r: Result<HashMap<String, i32>, _> = parse_str(r#"{"a":1,"a":2}"#);
        assert_eq!(r.unwrap_err().kind, ErrorKind::DuplicateKey);
    }

    #[cfg(feature = "std")]
    #[test]
    fn map_rejects_non_object() {
        use std::collections::HashMap;
        let r: Result<HashMap<String, i32>, _> = parse_str("[1,2]");
        assert!(r.is_err());
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashset_round_trips() {
        use std::collections::HashSet;
        let s: HashSet<i32> = parse_str("[1,2,3,2,1]").unwrap();
        assert_eq!(s.len(), 3);
        assert!(s.contains(&1));
        assert!(s.contains(&2));
        assert!(s.contains(&3));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn btreeset_round_trips() {
        use std::collections::BTreeSet;
        let s: BTreeSet<i32> = parse_str("[3,1,2]").unwrap();
        assert_eq!(s.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn box_rc_arc_are_transparent_wrappers() {
        let b: Box<u32> = parse_str("42").unwrap();
        assert_eq!(*b, 42);
        let r: std::rc::Rc<&str> = parse_str(r#""hello""#).unwrap();
        assert_eq!(*r, "hello");
        let a: std::sync::Arc<Vec<i32>> = parse_str("[1,2,3]").unwrap();
        assert_eq!(*a, vec![1, 2, 3]);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn char_accepts_single_scalar() {
        assert_eq!(parse_str::<char>(r#""a""#).unwrap(), 'a');
        assert_eq!(parse_str::<char>(r#""中""#).unwrap(), '中');
        // Escape that decodes to a single scalar.
        assert_eq!(parse_str::<char>(r#""\n""#).unwrap(), '\n');
        // Surrogate pair → single char above the BMP.
        assert_eq!(parse_str::<char>(r#""😀""#).unwrap(), '😀');
    }

    #[cfg(feature = "alloc")]
    #[test]
    #[allow(clippy::unicode_not_nfc)] // intentional: tests NFD input
    fn char_rejects_empty_or_multiple() {
        assert!(parse_str::<char>(r#""""#).is_err());
        assert!(parse_str::<char>(r#""ab""#).is_err());
        // Combining mark sequence is multiple scalars even though it
        // renders as one grapheme — char is a Unicode scalar, not a
        // grapheme cluster.
        assert!(parse_str::<char>(r#""é""#).is_err());
        // Wrong type.
        assert!(parse_str::<char>("42").is_err());
    }

    #[test]
    fn i128_round_trips_full_range() {
        assert_eq!(parse_str::<i128>("0").unwrap(), 0);
        assert_eq!(
            parse_str::<i128>("170141183460469231731687303715884105727").unwrap(),
            i128::MAX,
        );
        assert_eq!(
            parse_str::<i128>("-170141183460469231731687303715884105728").unwrap(),
            i128::MIN,
        );
        assert!(parse_str::<i128>("170141183460469231731687303715884105728").is_err());
    }

    #[test]
    fn u128_round_trips_full_range() {
        assert_eq!(parse_str::<u128>("0").unwrap(), 0);
        assert_eq!(
            parse_str::<u128>("340282366920938463463374607431768211455").unwrap(),
            u128::MAX,
        );
        assert!(parse_str::<u128>("340282366920938463463374607431768211456").is_err());
        // Negative literals are not valid u128 input.
        assert!(parse_str::<u128>("-1").is_err());
    }

    /// Pin the 38/39-digit boundary in `parse_i128_value` /
    /// `parse_u128_value`. The fast path skips overflow checks for the
    /// first 38 digits; the 39th switches to checked arithmetic. A
    /// regression that off-by-ones the boundary would either accept an
    /// out-of-range 39-digit literal or wrongly reject the largest
    /// in-range one.
    #[test]
    fn i128_38_vs_39_digit_boundary() {
        // 38 nines = 10^38 - 1. Comfortably inside i128 range.
        let thirty_eight_nines = "9".repeat(38);
        let v: i128 = parse_str(&thirty_eight_nines).unwrap();
        assert_eq!(v.to_string(), thirty_eight_nines);

        // 10^38 — the smallest 39-digit value. Inside i128 range.
        let ten_pow_38 = format!("1{}", "0".repeat(38));
        let v: i128 = parse_str(&ten_pow_38).unwrap();
        assert_eq!(v.to_string(), ten_pow_38);

        // 39 nines = 10^39 - 1, larger than i128::MAX. Must reject.
        let thirty_nine_nines = "9".repeat(39);
        assert!(parse_str::<i128>(&thirty_nine_nines).is_err());
    }

    #[test]
    fn vec_i128_round_trips() {
        // Exercises the new `vec_from_lex` override on the wide-int impl.
        let v: Vec<i128> = parse_str(
            "[0,1,-1,170141183460469231731687303715884105727,-170141183460469231731687303715884105728]",
        )
        .unwrap();
        assert_eq!(v, vec![0, 1, -1, i128::MAX, i128::MIN]);
    }

    #[test]
    fn vec_u128_round_trips() {
        let v: Vec<u128> = parse_str("[0,1,2,340282366920938463463374607431768211455]").unwrap();
        assert_eq!(v, vec![0, 1, 2, u128::MAX]);
    }

    /// JSON's grammar excludes non-finite floats. The lexer prevents
    /// `inf`/`NaN`/`Infinity` from ever appearing as input text (the
    /// number opener must be `-` or a digit), so the failure mode that
    /// matters is *finite* literals whose magnitude overflows `f64` —
    /// `str::parse::<f64>` silently returns `±inf` on those, and we
    /// must reject them.
    #[test]
    fn f64_rejects_overflow_to_infinity() {
        let r = parse_str::<f64>("1e400");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
        let r = parse_str::<f64>("-1e400");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
    }

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

    // -----------------------------------------------------------------
    // Escaped object keys.
    //
    // The lexer's `_lex`-suffixed key methods carry escapes through to
    // the typed layer, which decodes them via `key_to_cow`. Pin both
    // simple and \uXXXX-escape forms in struct dispatch, enum dispatch,
    // and map deserialization.
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
        // The JSON key `x\ny` (backslash + n in the wire bytes) decodes
        // to `"x\ny"` (literal newline). The renamed Rust field matches
        // that decoded form.
        let j = r#"{"user-id":1,"x\ny":2}"#;
        let r: EscKey = parse_str(j).unwrap();
        assert_eq!(r, EscKey { user_id: 1, x_newline_y: 2 });
    }

    #[test]
    fn struct_dispatch_handles_unicode_escape_in_key() {
        // id = "id". Decoded match should hit the renamed
        // `user_id` arm because we renamed it to `"user-id"`. Use a
        // fresh struct that doesn't have a rename to keep the assertion
        // simple.
        from_json! {
            #[derive(Debug, PartialEq)]
            struct PlainId { id: u32 }
        }
        let j = r#"{"id":7}"#;
        let r: PlainId = parse_str(j).unwrap();
        assert_eq!(r, PlainId { id: 7 });
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_handles_escaped_key() {
        use std::collections::HashMap;
        let m: HashMap<String, i32> = parse_str(r#"{"a\nb":1,"c":2}"#).unwrap();
        assert_eq!(m.get("a\nb"), Some(&1));
        assert_eq!(m.get("c"), Some(&2));
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_borrowed_key_rejects_escapes() {
        // &str keys can't represent decoded escape output (the buffer
        // doesn't live inside the input), so an escaped key errors
        // with InvalidEscape.
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
        // The renamed variant tag contains a literal newline in its
        // decoded form. Wire form: backslash-n in JSON.
        let r: Tagged = parse_str(r#"{"with\nbreak":42}"#).unwrap();
        assert_eq!(r, Tagged::WithBreak(42));
        // Plain still works.
        let r: Tagged = parse_str(r#"{"Plain":1}"#).unwrap();
        assert_eq!(r, Tagged::Plain(1));
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_cow_key_borrows_or_owns_per_entry() {
        use std::borrow::Cow;
        use std::collections::HashMap;
        let input = String::from(r#"{"plain":1,"esc\nape":2}"#);
        let m: HashMap<Cow<'_, str>, i32> = parse_str(&input).unwrap();
        // "plain" should borrow from the input.
        let plain = m.iter().find(|(k, _)| k.as_ref() == "plain").unwrap().0;
        assert!(matches!(plain, Cow::Borrowed(_)));
        // "esc\nape" must be owned (decoded).
        let esc = m.iter().find(|(k, _)| k.as_ref() == "esc\nape").unwrap().0;
        assert!(matches!(esc, Cow::Owned(_)));
    }

    // -----------------------------------------------------------------
    // from_json! macro field attributes.
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
        assert_eq!(r, Renamed { user_id: 1, display_name: 2 });
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
        // The skipped field is never read — it's always Default::default().
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
        // Strict mode: presenting the skipped field as a key is an
        // unknown field, since the dispatch arm was omitted.
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
        // Missing → default.
        let r: RenameAndDefault = parse_str("{}").unwrap();
        assert_eq!(r.max_retries, 0);
        // Present under the renamed key.
        let r: RenameAndDefault = parse_str(r#"{"max-retries":5}"#).unwrap();
        assert_eq!(r.max_retries, 5);
        // Original name does not work — strict mode rejects unknown.
        assert!(parse_str::<RenameAndDefault>(r#"{"max_retries":5}"#).is_err());
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

/// Round-trip tests for [`crate::ToJson`] primitives. Pairs each impl
/// against the existing `FromJson` impl: serialize a value, parse it
/// back, assert equality. This is the contract that the two sides agree
/// on the wire format — if a primitive's encoding ever drifts, one of
/// these tests breaks before any user code does.
#[cfg(all(test, feature = "alloc"))]
mod ser_roundtrip {
    use super::{parse_str, to_string};

    /// Test helper: takes the value by value so call sites can pass
    /// `rt(0_i64)` instead of `rt(&0_i64)`. The lint warning that this
    /// "could take &T" is correct in the abstract but ergonomic loss
    /// outweighs the (zero-cost) copy for `Copy` primitives, and the
    /// owned `String` test variants are small.
    #[allow(clippy::needless_pass_by_value)]
    fn rt<T>(v: T)
    where
        T: crate::ToJson + for<'a> crate::FromJson<'a> + PartialEq + core::fmt::Debug,
    {
        let s = to_string(&v).expect("serialize");
        let v2: T = parse_str(&s).expect("parse back");
        assert_eq!(v, v2, "round-trip diverged via {s:?}");
    }

    #[test]
    fn bool_roundtrips() {
        rt(true);
        rt(false);
    }

    #[test]
    fn unit_roundtrips() {
        rt(());
    }

    #[test]
    fn signed_ints_roundtrip() {
        rt(0_i8);
        rt(i8::MIN);
        rt(i8::MAX);
        rt(0_i16);
        rt(i16::MIN);
        rt(i16::MAX);
        rt(0_i32);
        rt(i32::MIN);
        rt(i32::MAX);
        rt(0_i64);
        rt(i64::MIN);
        rt(i64::MAX);
        rt(0_isize);
        rt(isize::MIN);
        rt(isize::MAX);
    }

    #[test]
    fn unsigned_ints_roundtrip() {
        rt(0_u8);
        rt(u8::MAX);
        rt(0_u16);
        rt(u16::MAX);
        rt(0_u32);
        rt(u32::MAX);
        rt(0_u64);
        rt(u64::MAX);
        rt(0_usize);
        rt(usize::MAX);
    }

    #[test]
    fn wide_ints_roundtrip() {
        rt(0_i128);
        rt(i128::MIN);
        rt(i128::MAX);
        rt(0_u128);
        rt(u128::MAX);
    }

    #[test]
    fn strings_roundtrip() {
        // Plain ASCII, escapes, control bytes, non-ASCII UTF-8.
        for s in [
            "",
            "hello",
            "a\\b",
            "with \"quotes\"",
            "tab\tnewline\nreturn\rbs\x08ff\x0cnul\x00ctl\x1f",
            "café 中文 😀",
        ] {
            rt(String::from(s));
        }
    }

    /// Cow can't go through the generic `rt` helper — `FromJson<'a>` for
    /// `Cow<'a, str>` ties the output lifetime to the input buffer, which
    /// the HRTB the helper requires can't satisfy. Test the wire shape
    /// inline instead.
    #[test]
    fn cow_roundtrips() {
        use std::borrow::Cow;
        for src in ["owned", "with\nescape", ""] {
            let v = Cow::<str>::Owned(String::from(src));
            let s = to_string(&v).unwrap();
            let back: Cow<'_, str> = parse_str(&s).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn char_roundtrips() {
        for c in ['a', 'Z', '中', '😀', '\n', '\t', '"', '\\'] {
            rt(c);
        }
    }

    #[test]
    fn option_roundtrips() {
        rt(Option::<u32>::None);
        rt(Some(42_u32));
        rt(Option::<String>::None);
        rt(Some(String::from("x")));
    }

    /// Reference impls forward through the inner value — verify the
    /// blanket `&T` impl produces the same bytes as the owned form.
    #[test]
    fn reference_forwarding() {
        let v: u32 = 7;
        let owned = to_string(&v).unwrap();
        let by_ref = to_string(&&v).unwrap();
        assert_eq!(owned, by_ref);
    }

    /// Wrapper types are transparent — round-trip through the wrapper
    /// must produce the same bytes as the inner value.
    #[test]
    fn wrapper_types_transparent() {
        use std::rc::Rc;
        use std::sync::Arc;
        let v: u32 = 9;
        assert_eq!(to_string(&v).unwrap(), to_string(&Box::new(v)).unwrap());
        assert_eq!(to_string(&v).unwrap(), to_string(&Rc::new(v)).unwrap());
        assert_eq!(to_string(&v).unwrap(), to_string(&Arc::new(v)).unwrap());
    }

    /// Pin the wire shape of a few primitives so any accidental
    /// formatter change (digit grouping, capitalization, exponent
    /// notation) shows up as a diff here, not as a downstream failure.
    #[test]
    fn pinned_wire_format() {
        assert_eq!(to_string(&true).unwrap(), "true");
        assert_eq!(to_string(&false).unwrap(), "false");
        assert_eq!(to_string(&()).unwrap(), "null");
        assert_eq!(to_string(&0_i64).unwrap(), "0");
        assert_eq!(to_string(&-1_i64).unwrap(), "-1");
        assert_eq!(to_string(&i64::MIN).unwrap(), "-9223372036854775808");
        assert_eq!(to_string(&u64::MAX).unwrap(), "18446744073709551615");
        assert_eq!(to_string(&"hi").unwrap(), "\"hi\"");
        // Control char inside a string → \u00XX.
        assert_eq!(to_string(&"\x01").unwrap(), "\"\\u0001\"");
    }

    #[test]
    fn vec_roundtrips() {
        rt(Vec::<i32>::new());
        rt(vec![1_i32, 2, 3]);
        rt(vec![String::from("a"), String::from("b")]);
        rt(vec![Some(1_u32), None, Some(3)]);
    }

    #[test]
    fn nested_vec_roundtrips() {
        rt(vec![vec![1_i32, 2], vec![3], Vec::new()]);
    }

    #[test]
    fn fixed_array_roundtrips() {
        rt([1_i32, 2, 3]);
        rt([true, false, true]);
        let zero: [i32; 0] = [];
        rt(zero);
    }

    #[test]
    fn tuple_roundtrips() {
        rt((1_i32, String::from("hi"), true));
        rt((1_u8, 2_u16, 3_u32, 4_u64));
    }

    #[test]
    fn slice_pinned() {
        // Slice has no FromJson impl (you parse into Vec), so it can't
        // round-trip — pin its wire shape directly instead.
        let s: &[i32] = &[10, 20, 30];
        assert_eq!(to_string(&s).unwrap(), "[10,20,30]");
        let empty: &[u8] = &[];
        assert_eq!(to_string(&empty).unwrap(), "[]");
    }

    #[test]
    fn btreemap_roundtrips() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert(String::from("a"), 1_i32);
        m.insert(String::from("b"), 2);
        rt(m);
    }

    #[test]
    fn btreemap_with_escape_in_key() {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert(String::from("a\nb"), 1_i32);
        rt(m);
    }

    #[test]
    fn btreeset_roundtrips() {
        use std::collections::BTreeSet;
        let mut s = BTreeSet::new();
        s.insert(1_i32);
        s.insert(2);
        s.insert(3);
        rt(s);
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashmap_roundtrips() {
        // HashMap iteration order isn't stable, so don't rt() — instead
        // serialize, parse back into HashMap, and compare maps directly.
        use std::collections::HashMap;
        let mut m = HashMap::new();
        m.insert(String::from("alpha"), 1_i32);
        m.insert(String::from("beta"), 2);
        let s = to_string(&m).unwrap();
        let back: HashMap<String, i32> = parse_str(&s).unwrap();
        assert_eq!(back, m);
    }

    #[cfg(feature = "std")]
    #[test]
    fn hashset_roundtrips() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(1_i32);
        s.insert(2);
        s.insert(3);
        let json = to_string(&s).unwrap();
        let back: HashSet<i32> = parse_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[cfg(feature = "std")]
    #[test]
    fn ip_addrs_roundtrip() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
        rt(Ipv4Addr::LOCALHOST);
        rt(Ipv6Addr::LOCALHOST);
        rt(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        rt(SocketAddr::from(([127, 0, 0, 1], 8080)));
    }

    #[cfg(feature = "std")]
    #[test]
    fn pathbuf_roundtrips() {
        use std::path::PathBuf;
        rt(PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn empty_collections_pinned() {
        use std::collections::BTreeMap;
        assert_eq!(to_string(&Vec::<i32>::new()).unwrap(), "[]");
        let empty: [i32; 0] = [];
        assert_eq!(to_string(&empty).unwrap(), "[]");
        let m: BTreeMap<String, i32> = BTreeMap::new();
        assert_eq!(to_string(&m).unwrap(), "{}");
    }

    /// Floats via the production path (`write!`-based today, ryu later).
    /// `rt()` works because `f64` parses back losslessly when serialized
    /// via shortest-round-trip — that's the contract the `write!` impl
    /// inherits from libstd's `Display` (which uses ryu internally).
    #[test]
    fn finite_f64_roundtrips() {
        for v in [
            0.0_f64,
            -0.0,
            1.0,
            -1.0,
            1.5,
            -1.5,
            1.5e2,
            1.5e-2,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            std::f64::consts::PI,
        ] {
            rt(v);
        }
    }

    #[test]
    fn finite_f32_roundtrips() {
        for v in [0.0_f32, 1.5, -1.5, f32::MIN_POSITIVE, f32::MAX] {
            rt(v);
        }
    }

    #[test]
    fn nonfinite_f64_rejected() {
        use crate::ErrorKind;
        let r = to_string(&f64::INFINITY);
        assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
        let r = to_string(&f64::NEG_INFINITY);
        assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
        let r = to_string(&f64::NAN);
        assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
    }

    #[cfg(feature = "std")]
    #[test]
    fn duration_roundtrips() {
        use std::time::Duration;
        rt(Duration::from_secs(0));
        rt(Duration::from_millis(1500));
        rt(Duration::new(42, 750_000_000));
    }
}

/// `to_json!` macro tests. Named-struct arms (PR 4 first slice).
/// Each test pairs a `to_json!`-defined type against a manually-defined
/// `from_json!` mirror so the round-trip exercises both macros on
/// equivalent shapes.
#[cfg(all(test, feature = "alloc"))]
mod to_json_macro_tests {
    use super::{parse_str, to_string};

    // The simplest possible shape: plain struct, no attrs.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Plain {
            id: u32,
            name: String,
        }
    }

    crate::from_json! {
        #[derive(Debug, PartialEq)]
        struct PlainParse {
            id: u32,
            name: String,
        }
    }

    #[test]
    fn plain_struct_emits_object() {
        let v = Plain { id: 7, name: String::from("alice") };
        let s = to_string(&v).unwrap();
        // Field order matches declaration order.
        assert_eq!(s, r#"{"id":7,"name":"alice"}"#);
        // Parse back through the equivalent FromJson type.
        let back: PlainParse = parse_str(&s).unwrap();
        assert_eq!(back, PlainParse { id: 7, name: String::from("alice") });
    }

    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Borrowed<'input> {
            tag: &'input str,
            count: u32,
        }
    }

    #[test]
    fn struct_with_lifetime() {
        let v = Borrowed { tag: "hi", count: 3 };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"tag":"hi","count":3}"#);
    }

    // rename, skip, skip_if_none.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Decorated {
            #[bourne(rename = "user-id")]
            user_id: u32,
            #[bourne(skip)]
            cached: u32,
            #[bourne(skip_if_none)]
            note: Option<String>,
            value: u32,
        }
    }

    #[test]
    fn rename_emits_new_key() {
        let v = Decorated { user_id: 1, cached: 99, note: None, value: 42 };
        let s = to_string(&v).unwrap();
        // user-id renamed; cached omitted; note omitted (None); value present.
        assert_eq!(s, r#"{"user-id":1,"value":42}"#);
    }

    #[test]
    fn skip_if_none_emits_when_some() {
        let v = Decorated {
            user_id: 1,
            cached: 0,
            note: Some(String::from("hi")),
            value: 7,
        };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"user-id":1,"note":"hi","value":7}"#);
    }

    // Empty struct edge case.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Empty {}
    }

    #[test]
    fn empty_struct_emits_empty_object() {
        assert_eq!(to_string(&Empty {}).unwrap(), "{}");
    }

    // String escaping inside emitted values (sanity — should already
    // work via the ToJson<String> impl, but the macro shouldn't
    // double-escape or corrupt the output).
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct WithEscape {
            text: String,
        }
    }

    #[test]
    fn macro_passes_strings_to_escape_path() {
        let v = WithEscape { text: String::from("a\nb\"c") };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"text":"a\nb\"c"}"#);
    }

    // Newtype tuple struct — emits the inner value bare.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct UserId(u64);
    }

    #[test]
    fn newtype_emits_bare_value() {
        let v = UserId(42);
        assert_eq!(to_string(&v).unwrap(), "42");
    }

    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct BorrowedTag<'input>(&'input str);
    }

    #[test]
    fn newtype_with_lifetime() {
        let v = BorrowedTag("hello");
        assert_eq!(to_string(&v).unwrap(), r#""hello""#);
    }

    // Multi-field tuple struct — emits a JSON array.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Point(i32, i32);
    }

    #[test]
    fn tuple_struct_emits_array() {
        let v = Point(3, -7);
        assert_eq!(to_string(&v).unwrap(), "[3,-7]");
    }

    crate::to_json! {
        #[derive(Debug, PartialEq)]
        struct Triple(i32, String, bool);
    }

    #[test]
    fn three_field_tuple_struct() {
        let v = Triple(1, String::from("hi"), true);
        assert_eq!(to_string(&v).unwrap(), r#"[1,"hi",true]"#);
    }

    // Externally-tagged enum — the default encoding.
    crate::to_json! {
        #[derive(Debug, PartialEq)]
        enum Shape {
            Circle,
            Wrapper(u32),
            Pair(u32, String),
            Box { w: u32, h: u32 },
            #[bourne(rename = "tri")]
            Triangle,
        }
    }

    #[test]
    fn enum_unit_emits_string() {
        assert_eq!(to_string(&Shape::Circle).unwrap(), r#""Circle""#);
    }

    #[test]
    fn enum_renamed_unit() {
        assert_eq!(to_string(&Shape::Triangle).unwrap(), r#""tri""#);
    }

    #[test]
    fn enum_newtype_emits_object() {
        assert_eq!(to_string(&Shape::Wrapper(7)).unwrap(), r#"{"Wrapper":7}"#);
    }

    #[test]
    fn enum_tuple_emits_object_with_array() {
        let v = Shape::Pair(1, String::from("x"));
        assert_eq!(to_string(&v).unwrap(), r#"{"Pair":[1,"x"]}"#);
    }

    #[test]
    fn enum_struct_variant_emits_nested_object() {
        let v = Shape::Box { w: 10, h: 20 };
        assert_eq!(to_string(&v).unwrap(), r#"{"Box":{"w":10,"h":20}}"#);
    }
}
