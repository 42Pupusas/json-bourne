//! Public-API tests — the `mod tests` block that lived in `lib.rs`
//! (audit 2026-09 F2), moved unchanged; it touches only the public
//! surface, so running it as an external crate pins that surface.
#![cfg(feature = "std")]

extern crate alloc;

use json_bourne::*;

mod tests {
    use crate::*;

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
    #[cfg(feature = "alloc")]
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
    fn system_time_round_trips_around_epoch() {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        // Exact epoch.
        let t: SystemTime = parse_str("0").unwrap();
        assert_eq!(t, UNIX_EPOCH);
        // Positive: 1.5s after epoch.
        let t: SystemTime = parse_str("1.5").unwrap();
        assert_eq!(t, UNIX_EPOCH + Duration::new(1, 500_000_000));
        // Negative: 2s before epoch — the SystemTime model supports
        // pre-epoch timestamps.
        let t: SystemTime = parse_str("-2").unwrap();
        assert_eq!(t, UNIX_EPOCH - Duration::from_secs(2));
        // Wire round-trip.
        let s = to_string(&(UNIX_EPOCH + Duration::from_secs(42))).unwrap();
        let back: SystemTime = parse_str(&s).unwrap();
        assert_eq!(back, UNIX_EPOCH + Duration::from_secs(42));
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
    fn f32_rejects_overflow_to_infinity() {
        // 1e40 fits f64 but exceeds f32::MAX (~3.4e38), so the
        // narrowing cast would round to ±inf. The impl rejects.
        let r = parse_str::<f32>("1e40");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
        let r = parse_str::<f32>("-1e40");
        assert_eq!(r.unwrap_err().kind, ErrorKind::NumberOutOfRange);
        // f32::MAX itself round-trips.
        let v: f32 = parse_str("3.4028235e38").unwrap();
        assert!(v.is_finite());
    }

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

    // -----------------------------------------------------------------
    // Coverage: Parser::object_first_key_lex / object_next_key_lex
    // -----------------------------------------------------------------

    #[test]
    fn parser_object_key_lex_with_escapes() {
        let input = br#"{"a\nb":1,"c":2}"#;
        let mut p: Parser<'_> = Parser::new(input);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let k1 = p.object_first_key_lex().unwrap().unwrap();
        assert!(k1.has_escapes());
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        let k2 = p.object_next_key_lex().unwrap().unwrap();
        assert!(!k2.has_escapes());
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        assert!(p.object_next_key_lex().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_object_key_lex_empty() {
        let mut p: Parser<'_> = Parser::new(b"{}");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(p.object_first_key_lex().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    // -----------------------------------------------------------------
    // Coverage: Parser::array_start / array_continue
    // -----------------------------------------------------------------

    #[test]
    fn parser_array_start_continue() {
        let mut p: Parser<'_> = Parser::new(b"[1,2,3]");
        assert!(!p.array_start().unwrap());
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(!p.array_continue(b']').unwrap());
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        assert!(!p.array_continue(b']').unwrap());
        assert_eq!(p.parse_i64_value().unwrap(), 3);
        assert!(p.array_continue(b']').unwrap());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_array_start_empty() {
        let mut p: Parser<'_> = Parser::new(b"[]");
        assert!(p.array_start().unwrap());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_array_nested_in_object() {
        let mut p: Parser<'_> = Parser::new(br#"{"v":[1,2]}"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let key = p.object_first_key().unwrap().unwrap();
        assert_eq!(key, "v");
        assert!(!p.array_start().unwrap());
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(!p.array_continue(b']').unwrap());
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        assert!(p.array_continue(b']').unwrap());
        assert!(p.object_next_key().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    // -----------------------------------------------------------------
    // Coverage: ErrorKind::static_msg
    // -----------------------------------------------------------------

    #[test]
    fn error_kind_display_covers_all_variants() {
        use alloc::format;
        let cases: &[(ErrorKind, &str)] = &[
            (ErrorKind::UnexpectedEof, "unexpected end of input"),
            (ErrorKind::InvalidEscape, "invalid string escape"),
            (
                ErrorKind::BorrowedKeyNeedsDecode,
                "key contains an escape: borrow-only key type cannot represent it; use String or Cow<str>",
            ),
            (ErrorKind::InvalidUnicodeEscape, "invalid \\u escape"),
            (
                ErrorKind::UnpairedSurrogate,
                "unpaired UTF-16 surrogate in \\u escape",
            ),
            (ErrorKind::InvalidUtf8, "invalid UTF-8"),
            (ErrorKind::InvalidNumber, "invalid number literal"),
            (
                ErrorKind::NumberOutOfRange,
                "number does not fit target type",
            ),
            (
                ErrorKind::ControlCharInString,
                "control character in string literal",
            ),
            (ErrorKind::TrailingData, "trailing data after JSON value"),
            (
                ErrorKind::DepthLimitExceeded,
                "nesting depth limit exceeded",
            ),
            (ErrorKind::ExpectedValue, "expected JSON value"),
            (ErrorKind::ExpectedString, "expected string"),
            (ErrorKind::ExpectedNumber, "expected number"),
            (ErrorKind::ExpectedBool, "expected boolean"),
            (ErrorKind::ExpectedNull, "expected null"),
            (ErrorKind::ExpectedArray, "expected array"),
            (ErrorKind::ExpectedObject, "expected object"),
            (ErrorKind::TypeMismatch, "type mismatch"),
            (ErrorKind::DuplicateKey, "duplicate object key"),
            (ErrorKind::MissingField, "missing required field"),
            (ErrorKind::UnknownField, "unknown field"),
            (
                ErrorKind::NonFiniteFloat,
                "non-finite float not representable in JSON",
            ),
            (ErrorKind::UnexpectedByte(0x7B), "unexpected byte 0x7b"),
        ];
        for (kind, expected) in cases {
            assert_eq!(format!("{kind}"), *expected, "mismatch for {kind:?}");
        }
    }

    // -----------------------------------------------------------------
    // Coverage: Lexer::consume_utf8_multibyte — all leading-byte ranges
    // -----------------------------------------------------------------

    #[test]
    fn utf8_f4_leading_byte_valid() {
        let s = "\"\u{100000}\"";
        let v: &str = parse_str(s).unwrap();
        assert_eq!(v, "\u{100000}");
    }

    #[test]
    fn utf8_f4_leading_byte_invalid_continuation() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xF4);
        bytes.push(0x90); // too high for F4 (must be 0x80..0x8F)
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(b'"');
        let r = parse::<&str>(&bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn utf8_multibyte_all_leading_ranges() {
        let cases: &[&str] = &[
            "\"\u{0080}\"",   // C2: 2-byte
            "\"\u{0800}\"",   // E0 A0: 3-byte low
            "\"\u{1000}\"",   // E1: 3-byte mid
            "\"\u{D7FF}\"",   // ED 9F: 3-byte just below surrogates
            "\"\u{E000}\"",   // EE: 3-byte private use
            "\"\u{10000}\"",  // F0 90: 4-byte low
            "\"\u{40000}\"",  // F1: 4-byte mid
            "\"\u{100000}\"", // F4 80: 4-byte high
        ];
        for input in cases {
            let v: &str = parse_str(input).unwrap();
            let expected = &input[1..input.len() - 1];
            assert_eq!(v, expected);
        }
    }

    // -----------------------------------------------------------------
    // Coverage: write_escape_byte — all control bytes
    // -----------------------------------------------------------------

    #[test]
    fn serialize_all_control_bytes() {
        for b in 0x00_u8..=0x1F {
            let s = alloc::string::String::from(b as char);
            let json = to_string(&s).unwrap();
            assert!(json.starts_with('"') && json.ends_with('"'));
            let inner = &json[1..json.len() - 1];
            match b {
                b'"' | b'\\' => unreachable!(),
                b'\n' => assert_eq!(inner, "\\n"),
                b'\r' => assert_eq!(inner, "\\r"),
                b'\t' => assert_eq!(inner, "\\t"),
                0x08 => assert_eq!(inner, "\\b"),
                0x0C => assert_eq!(inner, "\\f"),
                _ => {
                    let expected = alloc::format!("\\u00{b:02x}");
                    assert_eq!(inner, expected, "byte 0x{b:02x}");
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Coverage: Parser::next_event — error-path branches
    // -----------------------------------------------------------------

    #[test]
    fn next_event_rejects_trailing_comma_in_array() {
        let mut p: Parser<'_> = Parser::new(b"[1,]");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        let _ = p.next_event().unwrap().unwrap(); // Int(1)
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b']'));
    }

    #[test]
    fn next_event_rejects_trailing_comma_in_object() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":1,}"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_)));
        let _ = p.next_event().unwrap().unwrap(); // Int(1)
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'}'));
    }

    #[test]
    fn next_event_object_colon_eof() {
        let mut p: Parser<'_> = Parser::new(br#"{"a""#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_)));
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_object_colon_wrong_byte() {
        let mut p: Parser<'_> = Parser::new(br#"{"a";"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_)));
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn next_event_empty_input() {
        let mut p: Parser<'_> = Parser::new(b"");
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_eof_inside_array() {
        let mut p: Parser<'_> = Parser::new(b"[");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_eof_after_array_comma() {
        let mut p: Parser<'_> = Parser::new(b"[1,");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        let _ = p.next_event().unwrap().unwrap(); // 1
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_bad_byte_after_array_value() {
        let mut p: Parser<'_> = Parser::new(b"[1;");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        let _ = p.next_event().unwrap().unwrap(); // 1
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn next_event_eof_in_object_key_position() {
        let mut p: Parser<'_> = Parser::new(b"{");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_bad_byte_in_object_key_position() {
        let mut p: Parser<'_> = Parser::new(b"{1");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'1'));
    }

    #[test]
    fn next_event_eof_after_object_value() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":1"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_)));
        let _ = p.next_event().unwrap().unwrap(); // 1
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_event_bad_byte_after_object_value() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":1;"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_)));
        let _ = p.next_event().unwrap().unwrap(); // 1
        let err = p.next_event().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn next_event_object_value_state_via_fast_path() {
        let mut p: Parser<'_> = Parser::new(br#"{"x":42,"y":99}"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let k1 = p.object_first_key().unwrap().unwrap();
        assert_eq!(k1, "x");
        let v1 = p.next_event().unwrap().unwrap();
        assert!(matches!(v1, Event::Number(_)));
        let k2 = p.object_next_key().unwrap().unwrap();
        assert_eq!(k2, "y");
        let v2 = p.next_event().unwrap().unwrap();
        assert!(matches!(v2, Event::Number(_)));
        assert!(p.object_next_key().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn next_event_nested_containers_close_correctly() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":[1],"b":{"c":2}}"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_))); // "a"
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Number(_)));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_))); // "b"
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Key(_))); // "c"
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Number(_)));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndObject));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndObject));
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn next_event_nested_array_in_array() {
        let input = b"[[],[1,2]]";
        let mut p: Parser<'_> = Parser::new(input);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Number(_)));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::Number(_)));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn serialize_string_with_quote_and_backslash() {
        let s = "a\"b\\c";
        let json = to_string(&s).unwrap();
        assert_eq!(json, r#""a\"b\\c""#);
    }

    #[test]
    fn serialize_string_with_all_named_escapes() {
        let s = "\x08\x0C\n\r\t";
        let json = to_string(&s).unwrap();
        assert_eq!(json, r#""\b\f\n\r\t""#);
    }

    // -----------------------------------------------------------------
    // Coverage: Parser::object_first_key — nested container state paths
    // -----------------------------------------------------------------

    #[test]
    fn parser_object_first_key_empty_inside_array() {
        let mut p: Parser<'_> = Parser::new(b"[{}]");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(p.object_first_key().unwrap().is_none());
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_object_first_key_empty_inside_object() {
        let mut p: Parser<'_> = Parser::new(br#"{"a":{}}"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let k = p.object_first_key().unwrap().unwrap();
        assert_eq!(k, "a");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(p.object_first_key().unwrap().is_none());
        assert!(p.object_next_key().unwrap().is_none());
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_object_first_key_lex_empty_inside_array() {
        let mut p: Parser<'_> = Parser::new(b"[{}]");
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        assert!(p.object_first_key_lex().unwrap().is_none());
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_object_next_key_close_inside_array() {
        let mut p: Parser<'_> = Parser::new(br#"[{"a":1}]"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let k = p.object_first_key().unwrap().unwrap();
        assert_eq!(k, "a");
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(p.object_next_key().unwrap().is_none());
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(p.next_event().unwrap().is_none());
    }

    #[test]
    fn parser_object_next_key_lex_close_inside_array() {
        let mut p: Parser<'_> = Parser::new(br#"[{"a":1}]"#);
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartArray
        ));
        assert!(matches!(
            p.next_event().unwrap().unwrap(),
            Event::StartObject
        ));
        let k = p.object_first_key_lex().unwrap().unwrap();
        assert!(!k.has_escapes());
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(p.object_next_key_lex().unwrap().is_none());
        assert!(matches!(p.next_event().unwrap().unwrap(), Event::EndArray));
        assert!(p.next_event().unwrap().is_none());
    }

    // -----------------------------------------------------------------
    // Coverage: Lexer object_* — error paths via raw Lexer API
    // -----------------------------------------------------------------

    #[test]
    fn lexer_object_first_key_bad_byte() {
        let mut lex: Lexer<'_> = Lexer::new(b"{1");
        lex.object_start().unwrap();
        let err = lex.object_first_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'1'));
    }

    #[test]
    fn lexer_object_first_key_eof() {
        let mut lex: Lexer<'_> = Lexer::new(b"{");
        lex.object_start().unwrap();
        let err = lex.object_first_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn lexer_object_first_key_lex_bad_byte() {
        let mut lex: Lexer<'_> = Lexer::new(b"{1");
        lex.object_start().unwrap();
        let err = lex.object_first_key_lex().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'1'));
    }

    #[test]
    fn lexer_object_first_key_lex_eof() {
        let mut lex: Lexer<'_> = Lexer::new(b"{");
        lex.object_start().unwrap();
        let err = lex.object_first_key_lex().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn lexer_object_next_key_bad_byte() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1;"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn lexer_object_next_key_eof() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn lexer_object_next_key_bad_byte_after_comma() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1,2"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'2'));
    }

    #[test]
    fn lexer_object_next_key_lex_bad_byte() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1;"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key_lex().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key_lex().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn lexer_object_next_key_lex_eof() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key_lex().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key_lex().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn lexer_object_next_key_lex_bad_byte_after_comma() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a":1,2"#);
        lex.object_start().unwrap();
        let _ = lex.object_first_key_lex().unwrap();
        let _ = lex.parse_i64_value().unwrap();
        let err = lex.object_next_key_lex().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b'2'));
    }

    #[test]
    fn lexer_expect_colon_bad_byte() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a";"#);
        lex.object_start().unwrap();
        let err = lex.object_first_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedByte(b';'));
    }

    #[test]
    fn lexer_expect_colon_eof() {
        let mut lex: Lexer<'_> = Lexer::new(br#"{"a""#);
        lex.object_start().unwrap();
        let err = lex.object_first_key().unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    // -----------------------------------------------------------------
    // Coverage: [T; N]::from_lex — empty array with N=0, closed early
    // -----------------------------------------------------------------

    #[test]
    fn fixed_array_zero_length() {
        let arr: [i32; 0] = parse_str("[]").unwrap();
        assert_eq!(arr, [0_i32; 0]);
    }

    #[test]
    fn fixed_array_empty_but_expected_nonempty() {
        let r = parse_str::<[i32; 3]>("[]");
        assert!(r.is_err());
    }

    #[test]
    fn fixed_array_closed_early() {
        let r = parse_str::<[i32; 3]>("[1]");
        assert!(r.is_err());
    }

    // -----------------------------------------------------------------
    // Coverage: ErrorKind::static_msg — cross-category variants
    // -----------------------------------------------------------------

    #[test]
    fn static_msg_cross_category() {
        use alloc::format;
        let kind = ErrorKind::UnexpectedEof;
        let msg = format!("{kind}");
        assert_eq!(msg, "unexpected end of input");

        let kind = ErrorKind::ExpectedNull;
        let msg = format!("{kind}");
        assert_eq!(msg, "expected null");
    }

    // -----------------------------------------------------------------
    // Coverage: write_escape_byte — quote and backslash arms
    // -----------------------------------------------------------------

    #[test]
    fn serialize_string_containing_only_quote() {
        let s = "\"";
        let json = to_string(&s).unwrap();
        assert_eq!(json, r#""\"""#);
    }

    #[test]
    fn serialize_string_containing_only_backslash() {
        let s = "\\";
        let json = to_string(&s).unwrap();
        assert_eq!(json, r#""\\""#);
    }

    // -----------------------------------------------------------------
    // Coverage: consume_utf8_multibyte — error paths
    // -----------------------------------------------------------------

    #[test]
    fn utf8_invalid_leading_byte() {
        let bytes = b"\"\xFF\"";
        let r = parse::<&str>(bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn utf8_truncated_3byte_sequence() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xE1);
        bytes.push(0x80);
        // missing third continuation byte — EOF
        let r = parse::<&str>(&bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn utf8_bad_third_continuation_byte() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xE1);
        bytes.push(0x80);
        bytes.push(0x00); // not a continuation byte
        let r = parse::<&str>(&bytes);
        assert!(r.is_err());
    }

    #[test]
    fn utf8_4byte_bad_third_continuation() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xF0);
        bytes.push(0x90);
        bytes.push(0x80);
        bytes.push(0x00); // bad fourth byte
        let r = parse::<&str>(&bytes);
        assert!(r.is_err());
    }

    #[test]
    fn utf8_e0_bad_second_byte() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xE0);
        bytes.push(0x80); // too low for E0 (must be 0xA0..0xBF)
        bytes.push(0x80);
        bytes.push(b'"');
        let r = parse::<&str>(&bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn utf8_ed_bad_second_byte() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xED);
        bytes.push(0xA0); // too high for ED (must be 0x80..0x9F) — surrogate range
        bytes.push(0x80);
        bytes.push(b'"');
        let r = parse::<&str>(&bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    #[test]
    fn utf8_f0_bad_second_byte() {
        let mut bytes = b"\"".to_vec();
        bytes.push(0xF0);
        bytes.push(0x80); // too low for F0 (must be 0x90..0xBF)
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(b'"');
        let r = parse::<&str>(&bytes);
        assert_eq!(r.unwrap_err().kind, ErrorKind::InvalidUtf8);
    }

    // -----------------------------------------------------------------
    // Coverage: [T; N]::from_lex — line 295 too-long path
    // -----------------------------------------------------------------

    #[test]
    fn fixed_array_non_array_input() {
        let r = parse_str::<[i32; 2]>("42");
        assert!(r.is_err());
    }
}
