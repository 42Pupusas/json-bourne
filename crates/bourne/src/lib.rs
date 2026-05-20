#![cfg_attr(not(feature = "std"), no_std)]

//! Type-driven JSON: parse straight into the caller's chosen type and
//! serialize from it, with no generic `Value` middle layer.
//!
//! `bourne` skips the dynamic-tree intermediate that crates like
//! `serde_json` use. Each type knows how to deserialize itself from a
//! [`Lexer`] (via [`FromJson`](trait@FromJson)) or write itself to a [`JsonWrite`]
//! sink (via [`ToJson`]). The typed structure already enforces JSON's
//! grammar, so the per-event state machine is pure overhead for typed
//! consumers — skipping it makes the typed path ~2× faster on
//! integer / string-heavy payloads.
//!
//! - **No proc-macros.** [`from_json!`], [`to_json!`], and [`json!`]
//!   are declarative `macro_rules!`. Empty dependency graph.
//! - **`no_std` everywhere.** `bourne-core` is `no_std` always; this
//!   crate is `no_std + alloc` with optional `std` for `HashMap` /
//!   `std::net` / `std::path` / `std::io` adapters.
//! - **Borrowed strings by default.** `&'input str` and `Cow<'input,
//!   str>` parse zero-copy when the input contains no escapes.
//! - **Bounded by construction.** Container nesting is depth-limited
//!   (default 128, const-generic). The streaming parser is a state
//!   machine with no recursion.
//!
//! # Quick start
//!
//! Parse a primitive directly into a Rust type:
//!
//! ```
//! use bourne::parse_str;
//! let n: u32 = parse_str("42").unwrap();
//! assert_eq!(n, 42);
//! ```
//!
//! Parse a struct (no proc-macro — `from_json!` is declarative):
//!
//! ```
//! use bourne::{from_json, parse_str};
//!
//! from_json! {
//!     #[derive(Debug, PartialEq)]
//!     struct User<'input> {
//!         id: u64,
//!         name: &'input str,
//!         active: bool,
//!     }
//! }
//!
//! let u: User<'_> = parse_str(r#"{"id":1,"name":"alice","active":true}"#).unwrap();
//! assert_eq!(u.name, "alice");
//! ```
//!
//! Serialize back out:
//!
//! ```
//! use bourne::{to_json, to_string};
//!
//! to_json! {
//!     struct Point { x: i32, y: i32 }
//! }
//!
//! let s = to_string(&Point { x: 3, y: -7 }).unwrap();
//! assert_eq!(s, r#"{"x":3,"y":-7}"#);
//! ```
//!
//! # Output destinations
//!
//! | Sink                       | Entry point                 |
//! |----------------------------|-----------------------------|
//! | `String`                   | [`to_string`]               |
//! | `Vec<u8>`                  | [`to_vec`]                  |
//! | `String` (pretty-printed)  | [`to_string_pretty`]        |
//! | any [`std::io::Write`]     | [`to_writer`] (std only)    |
//! | any [`core::fmt::Write`]   | [`to_fmt`]                  |
//!
//! Custom sinks implement [`JsonWrite`] directly.
//!
//! # Features
//!
//! | Feature     | Default | Purpose                                     |
//! |-------------|---------|---------------------------------------------|
//! | `std`       | yes     | `HashMap`, `std::net`, `std::path`, `to_writer` |
//! | `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, escape decoding |
//! | `indexmap`  | no      | `IndexMap` / `IndexSet` (insertion order)   |
//!
//! `default-features = false` plus `["alloc"]` gives a `no_std + alloc`
//! build. For pure `no_std` use the `bourne-core` crate directly.

#[cfg(feature = "alloc")]
extern crate alloc;

mod de;
#[cfg(feature = "alloc")]
mod float;
mod ser;
#[cfg(test)]
mod teju_gen;

pub use bourne_core::{
    Checkpoint, Error, ErrorKind, Event, JsonNum, JsonStr, Lexer, Parser, Position, ValueKind,
};
pub use de::{FromJson, parse, parse_str};
#[cfg(feature = "alloc")]
pub use de::{MapKey, key_to_cow};
#[cfg(feature = "alloc")]
pub use ser::{
    ByteSink, FmtWriteSink, MapKeyOut, PrettyStringSink, StringSink, to_fmt, to_string,
    to_string_pretty, to_vec,
};
#[cfg(feature = "std")]
pub use ser::{IoWriteSink, to_writer};
pub use ser::{JsonWrite, ToJson};

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
        let v = Plain {
            id: 7,
            name: String::from("alice"),
        };
        let s = to_string(&v).unwrap();
        // Field order matches declaration order.
        assert_eq!(s, r#"{"id":7,"name":"alice"}"#);
        // Parse back through the equivalent FromJson type.
        let back: PlainParse = parse_str(&s).unwrap();
        assert_eq!(
            back,
            PlainParse {
                id: 7,
                name: String::from("alice")
            }
        );
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
        let v = Borrowed {
            tag: "hi",
            count: 3,
        };
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
        let v = Decorated {
            user_id: 1,
            cached: 99,
            note: None,
            value: 42,
        };
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
        let v = WithEscape {
            text: String::from("a\nb\"c"),
        };
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

    // Internally-tagged enum.
    crate::to_json! {
        #[bourne(tag = "type")]
        #[derive(Debug, PartialEq)]
        enum Event {
            Heartbeat,
            #[bourne(rename = "click")]
            Click { x: u32, y: u32 },
        }
    }

    #[test]
    fn internal_tag_unit_emits_object_with_tag() {
        assert_eq!(
            to_string(&Event::Heartbeat).unwrap(),
            r#"{"type":"Heartbeat"}"#
        );
    }

    #[test]
    fn internal_tag_struct_inlines_fields() {
        let v = Event::Click { x: 1, y: 2 };
        assert_eq!(to_string(&v).unwrap(), r#"{"type":"click","x":1,"y":2}"#);
    }

    // Adjacently-tagged enum.
    crate::to_json! {
        #[bourne(tag = "t", content = "c")]
        #[derive(Debug, PartialEq)]
        enum Msg {
            Ping,
            Echo(String),
            Pair(u32, u32),
            Body { text: String },
        }
    }

    #[test]
    fn adjacent_unit_emits_only_tag() {
        assert_eq!(to_string(&Msg::Ping).unwrap(), r#"{"t":"Ping"}"#);
    }

    #[test]
    fn adjacent_newtype_emits_content() {
        let v = Msg::Echo(String::from("hi"));
        assert_eq!(to_string(&v).unwrap(), r#"{"t":"Echo","c":"hi"}"#);
    }

    #[test]
    fn adjacent_tuple_emits_content_array() {
        let v = Msg::Pair(1, 2);
        assert_eq!(to_string(&v).unwrap(), r#"{"t":"Pair","c":[1,2]}"#);
    }

    #[test]
    fn adjacent_struct_emits_content_object() {
        let v = Msg::Body {
            text: String::from("ok"),
        };
        assert_eq!(to_string(&v).unwrap(), r#"{"t":"Body","c":{"text":"ok"}}"#);
    }

    // Untagged enum.
    crate::to_json! {
        #[bourne(untagged)]
        #[derive(Debug, PartialEq)]
        enum Mixed {
            Nothing,
            One(u32),
            Two(u32, u32),
            Body { name: String },
        }
    }

    #[test]
    fn untagged_unit_emits_null() {
        assert_eq!(to_string(&Mixed::Nothing).unwrap(), "null");
    }

    #[test]
    fn untagged_newtype_emits_inner() {
        assert_eq!(to_string(&Mixed::One(42)).unwrap(), "42");
    }

    #[test]
    fn untagged_tuple_emits_array() {
        assert_eq!(to_string(&Mixed::Two(1, 2)).unwrap(), "[1,2]");
    }

    #[test]
    fn untagged_struct_emits_object() {
        let v = Mixed::Body {
            name: String::from("x"),
        };
        assert_eq!(to_string(&v).unwrap(), r#"{"name":"x"}"#);
    }
}

/// Sink-adapter tests: `to_writer` (`io::Write`) and `to_fmt` (`fmt::Write`)
/// must produce identical bytes to the canonical `to_string` path.
#[cfg(all(test, feature = "alloc"))]
mod sink_adapter_tests {
    use super::{to_fmt, to_string};

    #[test]
    fn fmt_sink_matches_to_string_for_struct() {
        let m = vec![("a", 1_i32), ("b", 2)]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        let canonical = to_string(&m).unwrap();
        let mut out = String::new();
        to_fmt(&m, &mut out).unwrap();
        assert_eq!(out, canonical);
    }

    #[test]
    fn fmt_sink_handles_floats_with_grisu3() {
        let canonical = to_string(&1.5_f64).unwrap();
        let mut out = String::new();
        to_fmt(&1.5_f64, &mut out).unwrap();
        assert_eq!(out, canonical);
    }

    #[test]
    fn fmt_sink_rejects_non_finite() {
        let mut out = String::new();
        let err = to_fmt(&f64::INFINITY, &mut out).unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::NonFiniteFloat);
    }

    #[cfg(feature = "std")]
    #[test]
    fn io_writer_matches_to_string_for_struct() {
        use super::to_writer;
        let v = vec![1_i32, 2, 3];
        let canonical = to_string(&v).unwrap();
        let mut buf = Vec::<u8>::new();
        to_writer(&v, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), canonical);
    }

    #[cfg(feature = "std")]
    #[test]
    fn io_writer_handles_floats() {
        use super::to_writer;
        let canonical = to_string(&-2.7e-5_f64).unwrap();
        let mut buf = Vec::<u8>::new();
        to_writer(&-2.7e-5_f64, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), canonical);
    }

    #[cfg(feature = "std")]
    #[test]
    fn io_writer_rejects_non_finite() {
        use super::to_writer;
        let mut buf = Vec::<u8>::new();
        let err = to_writer(&f64::NAN, &mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn pretty_empty_object_compact() {
        use super::to_string_pretty;
        let m: std::collections::BTreeMap<String, i32> = std::collections::BTreeMap::new();
        assert_eq!(to_string_pretty(&m).unwrap(), "{}");
    }

    #[test]
    fn pretty_empty_array_compact() {
        use super::to_string_pretty;
        let v: Vec<i32> = Vec::new();
        assert_eq!(to_string_pretty(&v).unwrap(), "[]");
    }

    #[test]
    fn pretty_array_indents_two_spaces() {
        use super::to_string_pretty;
        let v = vec![1_i32, 2, 3];
        assert_eq!(to_string_pretty(&v).unwrap(), "[\n  1,\n  2,\n  3\n]");
    }

    #[test]
    fn pretty_object_keys_have_one_space_after_colon() {
        use super::to_string_pretty;
        let m: std::collections::BTreeMap<&str, i32> = [("a", 1), ("b", 2)].into_iter().collect();
        assert_eq!(
            to_string_pretty(&m).unwrap(),
            "{\n  \"a\": 1,\n  \"b\": 2\n}",
        );
    }

    #[test]
    fn pretty_nested_indents_proportionally() {
        use super::to_string_pretty;
        let v: Vec<Vec<i32>> = vec![vec![1, 2], vec![3]];
        assert_eq!(
            to_string_pretty(&v).unwrap(),
            "[\n  [\n    1,\n    2\n  ],\n  [\n    3\n  ]\n]",
        );
    }

    #[cfg(feature = "indexmap")]
    #[test]
    fn indexmap_preserves_insertion_order_on_parse() {
        use crate::parse_str;
        // Distinct, non-alphabetical order so a hash-bucket walk
        // would visibly reshuffle. IndexMap must yield the keys in
        // the order they appeared in the JSON.
        let json = r#"{"zebra":1,"alpha":2,"mango":3}"#;
        let m: indexmap::IndexMap<String, i32> = parse_str(json).unwrap();
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        assert_eq!(keys, ["zebra", "alpha", "mango"]);
        assert_eq!(m["alpha"], 2);
    }

    #[cfg(feature = "indexmap")]
    #[test]
    fn indexmap_round_trips_preserving_order() {
        use super::to_string;
        use crate::parse_str;
        let mut m = indexmap::IndexMap::<String, i32>::new();
        m.insert("z".into(), 1);
        m.insert("a".into(), 2);
        m.insert("m".into(), 3);
        let s = to_string(&m).unwrap();
        // Wire shape should preserve declaration order.
        assert_eq!(s, r#"{"z":1,"a":2,"m":3}"#);
        // Round-trip back into IndexMap must keep that order.
        let back: indexmap::IndexMap<String, i32> = parse_str(&s).unwrap();
        assert_eq!(
            back.keys().map(String::as_str).collect::<Vec<_>>(),
            ["z", "a", "m"],
        );
    }

    #[cfg(feature = "indexmap")]
    #[test]
    fn indexmap_rejects_duplicate_keys() {
        use crate::parse_str;
        let r: Result<indexmap::IndexMap<String, i32>, _> = parse_str(r#"{"a":1,"a":2}"#);
        assert_eq!(r.unwrap_err().kind, crate::ErrorKind::DuplicateKey);
    }

    #[cfg(feature = "indexmap")]
    #[test]
    fn indexset_round_trips() {
        use super::to_string;
        use crate::parse_str;
        let mut s = indexmap::IndexSet::<i32>::new();
        s.insert(3);
        s.insert(1);
        s.insert(2);
        let json = to_string(&s).unwrap();
        assert_eq!(json, "[3,1,2]");
        let back: indexmap::IndexSet<i32> = parse_str(&json).unwrap();
        assert_eq!(back.iter().copied().collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn pretty_round_trips_through_compact_parse() {
        use super::{parse_str, to_string_pretty};
        let m: std::collections::BTreeMap<String, Vec<i32>> = [
            (String::from("a"), vec![1, 2]),
            (String::from("b"), vec![3]),
        ]
        .into_iter()
        .collect();
        let pretty = to_string_pretty(&m).unwrap();
        // Whitespace is irrelevant to the parser; the pretty form must
        // re-parse to the same map.
        let back: std::collections::BTreeMap<String, Vec<i32>> = parse_str(&pretty).unwrap();
        assert_eq!(back, m);
    }

    #[cfg(feature = "std")]
    #[test]
    fn io_writer_propagates_underlying_error() {
        use super::to_writer;
        // A writer that always errors: assert the io::Error reaches us
        // unmolested instead of getting flattened to a generic kind.
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = FailingWriter;
        let err = to_writer(&"hi", &mut w).unwrap_err();
        assert_eq!(err.to_string(), "disk full");
    }
}

/// `json!` combined macro tests. Each type gets both `FromJson` and
/// `ToJson` from a single invocation — the struct/enum is emitted once.
#[cfg(all(test, feature = "alloc"))]
mod json_macro_tests {
    use super::{parse_str, to_string};

    crate::json! {
        #[derive(Debug, PartialEq)]
        struct Plain {
            id: u32,
            name: String,
        }
    }

    #[test]
    fn plain_struct_round_trips() {
        let v = Plain {
            id: 7,
            name: String::from("alice"),
        };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"id":7,"name":"alice"}"#);
        let back: Plain = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    crate::json! {
        #[derive(Debug, PartialEq)]
        struct Borrowed<'input> {
            tag: &'input str,
            count: u32,
        }
    }

    #[test]
    fn struct_with_lifetime_round_trips() {
        let json = r#"{"tag":"hi","count":3}"#;
        let v: Borrowed<'_> = parse_str(json).unwrap();
        assert_eq!(
            v,
            Borrowed {
                tag: "hi",
                count: 3
            }
        );
        assert_eq!(to_string(&v).unwrap(), json);
    }

    crate::json! {
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
    fn rename_skip_skip_if_none() {
        let v = Decorated {
            user_id: 1,
            cached: 99,
            note: None,
            value: 42,
        };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"user-id":1,"value":42}"#);
        let back: Decorated = parse_str(&s).unwrap();
        assert_eq!(back.user_id, 1);
        assert_eq!(back.value, 42);
    }

    crate::json! {
        #[derive(Debug, PartialEq)]
        struct UserId(u64);
    }

    #[test]
    fn newtype_round_trips() {
        let v = UserId(42);
        let s = to_string(&v).unwrap();
        assert_eq!(s, "42");
        let back: UserId = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    crate::json! {
        #[derive(Debug, PartialEq)]
        struct Pair(i32, i32);
    }

    #[test]
    fn tuple_struct_round_trips() {
        let v = Pair(3, -7);
        let s = to_string(&v).unwrap();
        assert_eq!(s, "[3,-7]");
        let back: Pair = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    crate::json! {
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
    fn externally_tagged_enum_round_trips() {
        let cases: Vec<(Shape, &str)> = vec![
            (Shape::Circle, r#""Circle""#),
            (Shape::Wrapper(7), r#"{"Wrapper":7}"#),
            (Shape::Pair(1, String::from("x")), r#"{"Pair":[1,"x"]}"#),
            (Shape::Box { w: 10, h: 20 }, r#"{"Box":{"w":10,"h":20}}"#),
            (Shape::Triangle, r#""tri""#),
        ];
        for (val, expected) in cases {
            let s = to_string(&val).unwrap();
            assert_eq!(s, expected);
            let back: Shape = parse_str(&s).unwrap();
            assert_eq!(back, val);
        }
    }

    crate::json! {
        #[bourne(tag = "type")]
        #[derive(Debug, PartialEq)]
        enum Event {
            Heartbeat,
            #[bourne(rename = "click")]
            Click { x: u32, y: u32 },
        }
    }

    #[test]
    fn internally_tagged_enum_round_trips() {
        let hb = Event::Heartbeat;
        let s = to_string(&hb).unwrap();
        assert_eq!(s, r#"{"type":"Heartbeat"}"#);
        let back: Event = parse_str(&s).unwrap();
        assert_eq!(back, hb);

        let click = Event::Click { x: 1, y: 2 };
        let s = to_string(&click).unwrap();
        assert_eq!(s, r#"{"type":"click","x":1,"y":2}"#);
        let back: Event = parse_str(&s).unwrap();
        assert_eq!(back, click);
    }

    crate::json! {
        #[bourne(tag = "t", content = "c")]
        #[derive(Debug, PartialEq)]
        enum Msg {
            Ping,
            Echo(String),
            Pair(u32, u32),
            Body { text: String },
        }
    }

    #[test]
    fn adjacently_tagged_enum_round_trips() {
        let cases: Vec<(Msg, &str)> = vec![
            (Msg::Ping, r#"{"t":"Ping"}"#),
            (Msg::Echo(String::from("hi")), r#"{"t":"Echo","c":"hi"}"#),
            (Msg::Pair(1, 2), r#"{"t":"Pair","c":[1,2]}"#),
            (
                Msg::Body {
                    text: String::from("ok"),
                },
                r#"{"t":"Body","c":{"text":"ok"}}"#,
            ),
        ];
        for (val, expected) in cases {
            let s = to_string(&val).unwrap();
            assert_eq!(s, expected);
            let back: Msg = parse_str(&s).unwrap();
            assert_eq!(back, val);
        }
    }

    crate::json! {
        #[bourne(untagged)]
        #[derive(Debug, PartialEq)]
        enum Mixed {
            Nothing,
            One(u32),
            Two(u32, u32),
            Body { name: String },
        }
    }

    crate::json! {
        #[derive(Debug, PartialEq, Eq)]
        pub struct PubFields {
            pub id: u32,
            pub(crate) name: String,
            value: u32,
        }
    }

    #[test]
    fn pub_fields_round_trip() {
        let v = PubFields {
            id: 1,
            name: String::from("hi"),
            value: 2,
        };
        let s = to_string(&v).unwrap();
        assert_eq!(s, r#"{"id":1,"name":"hi","value":2}"#);
        let back: PubFields = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn untagged_enum_serializes() {
        assert_eq!(to_string(&Mixed::Nothing).unwrap(), "null");
        assert_eq!(to_string(&Mixed::One(42)).unwrap(), "42");
        assert_eq!(to_string(&Mixed::Two(1, 2)).unwrap(), "[1,2]");
        assert_eq!(
            to_string(&Mixed::Body {
                name: String::from("x")
            })
            .unwrap(),
            r#"{"name":"x"}"#,
        );
    }
}

/// Unsafe-boundary tests for the public surface — pinned at the safety
/// contracts of the `_unchecked` Vec-tail writers in `ByteSink` and the
/// `from_utf8_unchecked` site in `decode_escapes`. These are designed to
/// run under miri (CI: `MIRIFLAGS=-Zmiri-disable-isolation
/// RUSTFLAGS=--cfg bourne_no_simd cargo +nightly miri test -p bourne --lib`).
/// Each one targets a specific invariant; if a future refactor breaks the
/// caller-side capacity reservation or the UTF-8 boundary, miri here trips
/// on the precise unsafe before any user code does.
#[cfg(all(test, feature = "alloc"))]
mod unsafe_boundary_tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Slice-of-floats ser at the exact-capacity boundary. The slice
    /// writer's `reserve_hint` computes
    ///   `2 + n * (MAX_SERIALIZED_LEN + 1) = 2 + n * 33`.
    /// We pre-reserve exactly that, so the per-element
    /// `write_float_f64_taint` (which assumes ≥ 32 bytes headroom)
    /// runs against the tightest legal Vec capacity.
    #[test]
    fn bytesink_slice_floats_exact_capacity() {
        for n in [0usize, 1, 2, 3, 7, 16, 17, 32, 33] {
            #[allow(clippy::cast_precision_loss)]
            let data: Vec<f64> = (0..n).map(|i| i as f64 + 0.5).collect();
            let mut out: Vec<u8> = Vec::with_capacity(2 + n * 33);
            let mut sink = ByteSink::new(&mut out);
            (data.as_slice()).write_json(&mut sink).expect("ser");
            let s = core::str::from_utf8(&out).expect("ASCII");
            // Sanity: parse back as Vec<f64>.
            let back: Vec<f64> = parse_str(s).expect("parse back");
            assert_eq!(back.len(), n, "n={n}");
            #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
            for (i, &v) in back.iter().enumerate() {
                assert_eq!(v, i as f64 + 0.5, "n={n} i={i}");
            }
        }
    }

    /// Same as above but for `Vec<f32>`. f32 widens to f64 in the writer
    /// but uses the same taint path, so the capacity boundary is identical.
    #[test]
    fn bytesink_slice_f32_exact_capacity() {
        for n in [0usize, 1, 2, 16, 17] {
            #[allow(clippy::cast_precision_loss)]
            let data: Vec<f32> = (0..n).map(|i| i as f32 + 0.25).collect();
            let mut out: Vec<u8> = Vec::with_capacity(2 + n * 33);
            let mut sink = ByteSink::new(&mut out);
            (data.as_slice()).write_json(&mut sink).expect("ser");
            let s = core::str::from_utf8(&out).expect("ASCII");
            let back: Vec<f32> = parse_str(s).expect("parse back");
            assert_eq!(back.len(), n);
            #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
            for (i, &v) in back.iter().enumerate() {
                assert_eq!(v, i as f32 + 0.25, "n={n} i={i}");
            }
        }
    }

    /// Slice of f64 containing non-finite values — exercises the taint
    /// accumulator path and verifies the slice writer surfaces a
    /// `NonFiniteFloat` error rather than emitting garbage bytes.
    #[test]
    fn bytesink_slice_nonfinite_taint_path() {
        // The non-finite path writes substitute bytes into the Vec before
        // reporting the error. Miri ensures those substitute writes stay
        // within the reserved capacity.
        for (name, bad_idx, bad_val) in [
            ("inf at 0", 0usize, f64::INFINITY),
            ("nan at end", 4usize, f64::NAN),
            ("neg_inf middle", 2usize, f64::NEG_INFINITY),
        ] {
            let mut data: Vec<f64> = (0..5).map(f64::from).collect();
            data[bad_idx] = bad_val;
            let r = to_string(data.as_slice());
            assert!(r.is_err(), "{name}: expected non-finite error");
        }
    }

    /// Slice of i64 — exercises `write_byte_unchecked` for both `[`/`,`/`]`
    /// at exact capacity. Integers don't use the float taint path but they
    /// do use `write_array_reserved`'s `write_byte_unchecked` for delimiters.
    #[test]
    fn bytesink_slice_ints_exact_capacity() {
        // MAX_SERIALIZED_LEN for i64 = 20 ("-9223372036854775808"), so
        // hint = 2 + n * 21.
        for n in [0usize, 1, 5, 16, 17] {
            #[allow(clippy::cast_possible_wrap)]
            let n_i64 = n as i64;
            #[allow(clippy::cast_possible_wrap)]
            let data: Vec<i64> = (0..n).map(|i| (i as i64) - n_i64 / 2).collect();
            let mut out: Vec<u8> = Vec::with_capacity(2 + n * 21);
            let mut sink = ByteSink::new(&mut out);
            (data.as_slice()).write_json(&mut sink).expect("ser");
            let s = core::str::from_utf8(&out).expect("ASCII");
            let back: Vec<i64> = parse_str(s).expect("parse back");
            assert_eq!(back, data, "n={n}");
        }
    }

    /// `decode_escapes`' literal-byte run uses `from_utf8_unchecked` on
    /// the stretch between escapes. The lexer's UTF-8 invariant must hold
    /// across multi-byte sequences. Test escapes interleaved with 2/3/4-byte
    /// UTF-8 chars so the literal chunk passed to the unsafe spans
    /// multibyte data.
    #[test]
    fn decode_escapes_multibyte_in_literal_chunk() {
        // 2-byte (é = c3 a9), 3-byte (€ = e2 82 ac), 4-byte (𝄞 = f0 9d 84 9e)
        // chars surrounding `\n` escapes. The literal chunks bracket the
        // escape and must contain valid UTF-8.
        let cases: &[(&str, &str)] = &[
            (r#""café\nfin""#, "café\nfin"),
            (r#""price: 5€\tea""#, "price: 5€\tea"),
            (r#""note 𝄞\nplayed""#, "note 𝄞\nplayed"),
            // Many escapes between multibyte stretches.
            (r#""éé\nçç\tüü""#, "éé\nçç\tüü"),
            // Escape at start / end with multibyte in middle.
            (r#""\n𝄞café\t""#, "\n𝄞café\t"),
            // Long literal run before single escape (exercises the SIMD
            // scan tail).
            (
                r#""aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaéé\n""#,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaéé\n",
            ),
        ];
        for (json, expected) in cases {
            let s: String = parse_str(json).expect("parse");
            assert_eq!(s, *expected, "case {json}");
        }
    }

    /// Empty slice / array — `write_array_reserved`'s `split_first` is None,
    /// no `write_byte_unchecked` for elements. Just `[` and `]`.
    #[test]
    fn bytesink_empty_slice_writes_brackets_only() {
        let empty: Vec<i64> = Vec::new();
        let s = to_string(empty.as_slice()).expect("ser");
        assert_eq!(s, "[]");

        let empty: Vec<f64> = Vec::new();
        let s = to_string(empty.as_slice()).expect("ser");
        assert_eq!(s, "[]");
    }

    /// Single-element slice — `write_array_reserved`'s `split_first` returns
    /// `(first, [])` so we skip the inner loop. Verify the byte boundary.
    #[test]
    fn bytesink_single_element_slice() {
        let one = [42_i64];
        assert_eq!(to_string(one.as_slice()).unwrap(), "[42]");
        let one = [1.5_f64];
        assert_eq!(to_string(one.as_slice()).unwrap(), "[1.5]");
        let one = [1.5_f32];
        assert_eq!(to_string(one.as_slice()).unwrap(), "[1.5]");
    }

    /// Empty string — `decode_escapes` is called with `raw=&[]`. The
    /// `from_utf8_unchecked(&raw[start..i])` slice is empty, which is
    /// a degenerate edge case that must not OOB.
    #[test]
    fn decode_escapes_empty_string() {
        let s: String = parse_str(r#""""#).expect("parse");
        assert_eq!(s, "");
    }

    /// Pure-ASCII without any escape — the literal-byte run covers the
    /// whole input and `find_backslash` returns None. The unsafe slice
    /// is `&raw[0..raw.len()]`.
    #[test]
    fn decode_escapes_no_escapes_pure_ascii() {
        // Use `Vec<String>` to force the owned-decode path even though
        // the input has no escapes (the `&str` impl would borrow).
        let v: Vec<String> = parse_str(r#"["hello world","no escapes here"]"#).expect("parse");
        assert_eq!(
            v,
            [String::from("hello world"), String::from("no escapes here")]
        );
    }

    /// Decode an escape immediately at the start — the first literal-byte
    /// run is empty, so `from_utf8_unchecked(&raw[0..0])` is called.
    #[test]
    fn decode_escapes_escape_at_start() {
        let v: Vec<String> = parse_str(r#"["\nhello","\tworld"]"#).expect("parse");
        assert_eq!(v, [String::from("\nhello"), String::from("\tworld")]);
    }

    /// Decode an escape immediately at the end — after the escape there is
    /// no literal-byte run.
    #[test]
    fn decode_escapes_escape_at_end() {
        let v: Vec<String> = parse_str(r#"["hello\n","world\t"]"#).expect("parse");
        assert_eq!(v, [String::from("hello\n"), String::from("world\t")]);
    }

    /// Back-to-back escapes — multiple zero-length literal chunks.
    #[test]
    fn decode_escapes_consecutive_escapes() {
        let v: Vec<String> = parse_str(r#"["\n\t\r\\","\"\"\""]"#).expect("parse");
        assert_eq!(v, [String::from("\n\t\r\\"), String::from(r#"""""#)]);
    }

    // -----------------------------------------------------------------
    // Vec<T>::vec_from_lex fast-path coverage. The default
    // `FromJson::vec_from_lex` impl drives `from_lex` per element; the
    // primitive types (`&str`, integers via macros, `Duration`) override
    // it to skip the per-element Event detour. These overrides have
    // independent code paths from the default and need their own tests.
    // -----------------------------------------------------------------

    /// `Vec<&str>::vec_from_lex` — borrowed strings, fused fast path.
    #[test]
    fn vec_borrowed_str_fast_path() {
        // Empty array — early-return branch.
        let v: Vec<&str> = parse_str("[]").expect("empty");
        assert_eq!(v, Vec::<&str>::new());

        // Single element — first push only, no while loop.
        let v: Vec<&str> = parse_str(r#"["only"]"#).expect("single");
        assert_eq!(v, ["only"]);

        // Many elements — exercises the while-loop body.
        let v: Vec<&str> = parse_str(r#"["a","b","c","d","e"]"#).expect("many");
        assert_eq!(v, ["a", "b", "c", "d", "e"]);

        // Reject escape-bearing string (the borrowed path requires no escapes).
        let r: Result<Vec<&str>, _> = parse_str(r#"["plain","esc\nbad"]"#);
        assert!(r.is_err(), "borrowed path must reject escape");
    }

    /// `Vec<Duration>::vec_from_lex` — fused, with non-finite / negative rejection.
    #[cfg(feature = "std")]
    #[test]
    fn vec_duration_fast_path() {
        use std::time::Duration;

        // Empty — early return.
        let v: Vec<Duration> = parse_str("[]").expect("empty");
        assert!(v.is_empty());

        // Single — first push.
        let v: Vec<Duration> = parse_str("[1.5]").expect("single");
        assert_eq!(v, [Duration::from_secs_f64(1.5)]);

        // Many — loop body.
        let v: Vec<Duration> = parse_str("[0.0,1.0,1.5,2.25,100.125]").expect("many");
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], Duration::ZERO);
        assert_eq!(v[4], Duration::from_secs_f64(100.125));

        // Negative — rejected.
        let r: Result<Vec<Duration>, _> = parse_str("[1.0,-2.0]");
        assert!(r.is_err());
    }

    /// `Vec<i64>::vec_from_lex` — exercises the macro-generated override
    /// in `de.rs::impl_int!`, including the bounds check on the empty branch.
    #[test]
    fn vec_i64_fast_path() {
        let v: Vec<i64> = parse_str("[]").expect("empty");
        assert!(v.is_empty());

        let v: Vec<i64> = parse_str("[42]").expect("single");
        assert_eq!(v, [42]);

        let v: Vec<i64> =
            parse_str("[1,-1,9223372036854775807,-9223372036854775808]").expect("many");
        assert_eq!(v, [1, -1, i64::MAX, i64::MIN]);

        // Out-of-range for u32 should error.
        let r: Result<Vec<u32>, _> = parse_str("[1,99999999999]");
        assert!(r.is_err(), "u32 should overflow");
    }

    /// `Vec<u128>::vec_from_lex` — wide-int macro path.
    #[test]
    fn vec_u128_fast_path() {
        let v: Vec<u128> = parse_str("[]").expect("empty");
        assert!(v.is_empty());

        let v: Vec<u128> = parse_str("[0,170141183460469231731687303715884105727]").expect("many");
        assert_eq!(v, [0u128, i128::MAX as u128]);
    }

    // -----------------------------------------------------------------
    // `decode_escapes` — full branch coverage. The match arms for `\b`,
    // `\f`, `\/`, and the various surrogate-pair error paths weren't
    // exercised by the existing tests.
    // -----------------------------------------------------------------

    #[test]
    fn decode_all_simple_escapes() {
        // Each backslash escape variant — covers every match arm in
        // `decode_escapes`.
        let cases: &[(&str, &str)] = &[
            (r#""\b""#, "\u{0008}"), // backspace
            (r#""\f""#, "\u{000C}"), // form feed
            (r#""\/""#, "/"),        // solidus
            (r#""\\""#, "\\"),       // backslash
            (r#""\"""#, "\""),       // quote
            (r#""\n""#, "\n"),
            (r#""\r""#, "\r"),
            (r#""\t""#, "\t"),
        ];
        for &(json, expected) in cases {
            let s: String = parse_str(json).expect(json);
            assert_eq!(s, expected, "case {json}");
        }
    }

    #[test]
    fn decode_unknown_escape_errors() {
        // `\x` is not a recognized escape — last match arm.
        let r: Result<String, _> = parse_str(r#""\x""#);
        assert!(r.is_err(), "unknown escape should error");

        // Backslash at end-of-input — `i >= raw.len()` branch.
        let r: Result<String, _> = parse_str("\"\\\"");
        assert!(r.is_err(), "lone trailing backslash should error");
    }

    #[test]
    fn decode_unicode_escape_short_input_errors() {
        // `\u` followed by < 4 hex digits — `i + 5 > raw.len()` branch.
        let r: Result<String, _> = parse_str(r#""\u00""#);
        assert!(r.is_err(), "short \\u should error");
        let r: Result<String, _> = parse_str(r#""\u""#);
        assert!(r.is_err());
    }

    #[test]
    fn decode_unicode_lone_low_surrogate_errors() {
        // `\uDC00` standalone is a lone low surrogate — second
        // `0xDC00..=0xDFFF` branch.
        let r: Result<String, _> = parse_str(r#""\uDC00""#);
        assert!(r.is_err(), "lone low surrogate should error");
    }

    #[test]
    fn decode_unicode_high_surrogate_then_invalid_low_errors() {
        // High surrogate \uD800 followed by another `\u` escape whose
        // codepoint is OUTSIDE the low-surrogate range — exercises the
        // explicit range-check arm (raw[i+1]==`\\` AND raw[i+2]==`u` but
        // the parsed low_value isn't a low surrogate).
        let json = "\"\\uD800\\u0041\""; // high then 'A' as A
        let r: Result<String, _> = parse_str(json);
        assert!(
            r.is_err(),
            "high surrogate then non-low-surrogate \\u must error"
        );
    }

    /// High surrogate followed by `\uXXXX` where XXXX is a valid low
    /// surrogate, but the four hex digits are at end-of-input. Covers
    /// the `i + 7 > raw.len()` short-buffer guard.
    #[test]
    fn decode_unicode_high_surrogate_then_short_second_escape_errors() {
        let json = r#""\uD800\u""#;
        let r: Result<String, _> = parse_str(json);
        assert!(r.is_err(), "high surrogate then truncated \\u must error");
    }

    /// Invalid hex inside the second \u of a surrogate pair.
    #[test]
    fn decode_unicode_high_surrogate_then_invalid_hex_errors() {
        let json = "\"\\uD800\\uZZZZ\"";
        let r: Result<String, _> = parse_str(json);
        assert!(r.is_err(), "high surrogate then bad hex must error");
    }

    #[test]
    fn decode_unicode_high_surrogate_then_non_u_escape_errors() {
        // `\uD800` followed by `\n` (not a `\u` escape).
        let r: Result<String, _> = parse_str(r#""\uD800\n""#);
        assert!(
            r.is_err(),
            "high surrogate not followed by \\u should error"
        );

        // `\uD800` followed by non-backslash byte (EOF or literal).
        let r: Result<String, _> = parse_str(r#""\uD800A""#);
        assert!(
            r.is_err(),
            "high surrogate not followed by escape should error"
        );
    }

    #[test]
    fn decode_unicode_surrogate_pair_round_trips() {
        // Valid surrogate pair for U+1F600 (GRINNING FACE): high=D83D
        // low=DE00. Encoded as `\u` escapes (not the literal emoji) so
        // decode_escapes' surrogate arm runs — literal multi-byte UTF-8
        // goes through the lexer's `consume_utf8_multibyte` instead.
        let json = "\"\\uD83D\\uDE00\"";
        let s: String = parse_str(json).expect("parse");
        assert_eq!(s, "\u{1F600}");

        // Surrogate pair adjacent to literal ASCII / other escapes.
        let json = "\"a\\uD83D\\uDE00b\\uD83D\\uDE01c\"";
        let s: String = parse_str(json).expect("parse");
        assert_eq!(s, "a\u{1F600}b\u{1F601}c");
    }

    #[test]
    fn decode_unicode_bmp_escape_round_trips() {
        // Standard BMP unicode escape — non-surrogate path. U+00E9 ('é')
        // encoded as `é` so decode_escapes' \u arm runs (a literal
        // "é" goes through consume_utf8_multibyte instead).
        let json = "\"\\u00E9\"";
        let s: String = parse_str(json).expect("parse");
        assert_eq!(s, "é");

        // BMP escapes at various code points.
        let json = "\"A\\u00A3\\u20AC\""; // A, £, €
        let s: String = parse_str(json).expect("parse");
        assert_eq!(s, "A£€");
    }

    #[test]
    fn decode_invalid_hex_in_u_escape_errors() {
        // `\u00ZX` — invalid hex.
        let r: Result<String, _> = parse_str(r#""\u00ZX""#);
        assert!(r.is_err(), "invalid hex should error");
    }

    // -----------------------------------------------------------------
    // `to_decimal_uncentred` — the float path for powers of 2 (mantissa
    // == 1 << 52). Hit when the IEEE 754 mantissa lands exactly on the
    // uncentred boundary. These values are rare in random samples, so
    // need explicit tests.
    // -----------------------------------------------------------------

    /// Powers of 2 hit the uncentred decompose path. Each gets a
    /// shortest-roundtrip rendering through `to_decimal_uncentred`.
    #[test]
    fn to_decimal_uncentred_powers_of_two_round_trip() {
        // f64 values where mantissa == 1 << 52 and exponent != EXPONENT_MIN:
        // these are 2.0, 4.0, 8.0, 16.0, ..., up to 2^1023.
        let cases = [
            2.0_f64,
            4.0,
            8.0,
            16.0,
            32.0,
            64.0,
            128.0,
            256.0,
            512.0,
            1024.0,
            2.0_f64.powi(20),
            2.0_f64.powi(50),
            2.0_f64.powi(100),
            2.0_f64.powi(500),
            2.0_f64.powi(1023), // largest finite power of 2
            // Negative powers of 2 — half exponent.
            2.0_f64.powi(-1), // 0.5
            2.0_f64.powi(-2), // 0.25
            2.0_f64.powi(-10),
            2.0_f64.powi(-50),
            2.0_f64.powi(-100),
            2.0_f64.powi(-1000),
        ];
        for &v in &cases {
            let s = to_string(&v).expect("ser");
            let back: f64 = parse_str(&s).expect("parse back");
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
            }
        }
    }

    /// Negative powers of 2 — sign path through `to_decimal_uncentred`.
    #[test]
    fn to_decimal_uncentred_negative_powers_round_trip() {
        for k in [-30, -10, -1, 1, 10, 30, 100, 500] {
            let v = -(2.0_f64.powi(k));
            let s = to_string(&v).expect("ser");
            let back: f64 = parse_str(&s).expect("parse back");
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
            }
        }
    }

    /// Subnormal f64 — minimum positive denormal, uses uncentred path
    /// at a different boundary.
    #[test]
    fn to_decimal_uncentred_subnormals_round_trip() {
        let cases = [
            5e-324_f64,        // smallest subnormal
            f64::MIN_POSITIVE, // smallest normal
            -f64::MIN_POSITIVE,
            f64::MAX,
            -f64::MAX,
        ];
        for &v in &cases {
            let s = to_string(&v).expect("ser");
            let back: f64 = parse_str(&s).expect("parse back");
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
            }
        }
    }

    // -----------------------------------------------------------------
    // Direct-sink coverage. Every public sink type (StringSink,
    // PrettyStringSink, ByteSink unchecked methods) needs at least one
    // call so the CRAP gate's zero-coverage trigger doesn't fire.
    // -----------------------------------------------------------------

    /// `StringSink` — the `&mut String` sink. Exercises `write_byte`,
    /// `write_str_raw`, and `write_float_f64` directly.
    #[test]
    fn string_sink_writes_directly() {
        let mut out = String::new();
        let mut sink = StringSink::new(&mut out);
        // write_byte and write_str_raw via the trait
        sink.write_byte(b'[').unwrap();
        sink.write_str_raw("1").unwrap();
        sink.write_byte(b',').unwrap();
        // write_float_f64 (StringSink override)
        sink.write_float_f64(2.5).unwrap();
        sink.write_byte(b']').unwrap();
        assert_eq!(out, "[1,2.5]");

        // Non-finite rejection through the StringSink path.
        let mut out = String::new();
        let mut sink = StringSink::new(&mut out);
        assert!(sink.write_float_f64(f64::NAN).is_err());
    }

    /// `PrettyStringSink::with_indent` — custom indent variant.
    /// Existing pretty tests only cover the default-indent constructor.
    #[test]
    fn pretty_sink_with_indent_uses_custom_indent() {
        let mut out = String::new();
        let mut sink = PrettyStringSink::with_indent(&mut out, "\t");
        let data = (1_i32, 2_i32, 3_i32);
        data.write_json(&mut sink).unwrap();
        // Tab indent → contains a `\n\t` sequence.
        assert!(out.contains("\n\t"), "got: {out:?}");
    }

    /// `PrettyStringSink::write_float_f64` — covers the pretty-sink
    /// float arm (existing tests use the simple-byte and string paths).
    #[test]
    fn pretty_sink_handles_floats() {
        let v: alloc::vec::Vec<f64> = alloc::vec![1.5, 2.5, 3.0];
        let s = to_string_pretty(v.as_slice()).unwrap();
        assert!(s.contains("1.5"));
        assert!(s.contains("3.0"));
    }

    /// `ByteSink::write_float_f64_unchecked` — non-finite triggers the
    /// outlined `cold_nonfinite_byte_sink` error. Exercises the cold path.
    #[test]
    fn bytesink_unchecked_float_nonfinite_returns_error() {
        let mut out: Vec<u8> = Vec::with_capacity(64);
        let mut sink = ByteSink::new(&mut out);
        // SAFETY: ample reserved capacity (≥32 bytes).
        #[allow(unsafe_code)]
        let r = unsafe { sink.write_float_f64_unchecked(f64::INFINITY) };
        assert!(r.is_err(), "non-finite must error through cold arm");
    }

    /// `ByteSink::write_float_f64_unchecked_finite` — direct call with
    /// a finite value. The slice fast path calls this internally.
    #[test]
    fn bytesink_unchecked_finite_writes_value() {
        let mut out: Vec<u8> = Vec::with_capacity(64);
        let mut sink = ByteSink::new(&mut out);
        // Pick a finite value that round-trips exactly through the formatter
        // and `f64::parse`. 2.5 is exact in binary; avoiding 3.14 also dodges
        // clippy's approx_constant warning about PI.
        let value = 2.5_f64;
        // SAFETY: reserved 64 bytes, value is finite.
        #[allow(unsafe_code)]
        unsafe {
            sink.write_float_f64_unchecked_finite(value).unwrap();
        }
        let s = core::str::from_utf8(&out).unwrap();
        let parsed: f64 = s.parse().unwrap();
        // Bit-pattern compare: write→parse must round-trip exactly.
        assert_eq!(parsed.to_bits(), value.to_bits());
    }

    /// `ToJson for Path` (std-only) — covers `Path::write_json`.
    #[cfg(feature = "std")]
    #[test]
    fn path_serializes_via_to_string() {
        use std::path::Path;
        let p = Path::new("/tmp/file.txt");
        let s = to_string(p).unwrap();
        assert_eq!(s, "\"/tmp/file.txt\"");
    }

    /// `MapKeyOut for Cow<'_, str>` — used when a HashMap/BTreeMap
    /// key type is `Cow<'_, str>`. Exercises `Cow::as_str`.
    #[test]
    fn map_with_cow_str_keys_serializes() {
        use alloc::borrow::Cow;
        use alloc::collections::BTreeMap;
        let mut m: BTreeMap<Cow<'_, str>, i32> = BTreeMap::new();
        m.insert(Cow::Borrowed("a"), 1);
        m.insert(Cow::Owned(String::from("b")), 2);
        let s = to_string(&m).unwrap();
        assert!(s.contains(r#""a":1"#));
        assert!(s.contains(r#""b":2"#));
    }

    /// `f64::pre_validate_slice` — the post-taint stub returns Ok(()).
    /// Direct call to ensure the function is exercised.
    #[test]
    fn f64_pre_validate_slice_is_noop() {
        let slice: &[f64] = &[1.0, 2.0, f64::INFINITY];
        // The fn body just returns Ok; calling it through the trait
        // exercises both the dispatch and the body.
        let r = <f64 as ToJson>::pre_validate_slice(slice);
        assert!(r.is_ok());
    }

    /// `crate::ser::float::format_f64_write` and `reject_non_finite`
    /// are exercised by `StringSink::write_float_f64` (via the `float::`
    /// module). Existing tests cover that, but cover them explicitly
    /// for the non-finite branch through the public `to_fmt` entry
    /// point which uses `FmtWriteSink` (a different code path).
    #[test]
    fn to_fmt_writes_finite_and_rejects_nonfinite() {
        let mut s = String::new();
        to_fmt(&1.5_f64, &mut s).unwrap();
        assert_eq!(s, "1.5");

        let mut s = String::new();
        assert!(to_fmt(&f64::INFINITY, &mut s).is_err());
    }

    /// `fmt_write_error` is invoked when the underlying `core::fmt::Write`
    /// impl returns an error. Construct a writer that always fails and
    /// verify `to_fmt` propagates a typed `Error` (rather than the
    /// detail-less `fmt::Error`).
    #[test]
    fn to_fmt_propagates_underlying_write_failure() {
        use core::fmt;

        /// Writer that errors on every call — drives `fmt_write_error`.
        struct AlwaysFail;
        impl fmt::Write for AlwaysFail {
            fn write_str(&mut self, _s: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }

        // Any non-empty serializable value flushes at least one byte
        // through `write_str` / `write_char`, which the writer rejects.
        let mut sink = AlwaysFail;
        let r = to_fmt(&42_i64, &mut sink);
        assert!(r.is_err(), "expected propagated error from failing writer");

        // Also exercise the float path through FmtWriteSink, which uses
        // a separate `fmt_write_error()` call site.
        let mut sink = AlwaysFail;
        let r = to_fmt(&1.5_f64, &mut sink);
        assert!(r.is_err());
    }
}
