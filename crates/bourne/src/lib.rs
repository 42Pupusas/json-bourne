#![cfg_attr(not(feature = "std"), no_std)]

//! Type-driven JSON: parse straight into the caller's chosen type and
//! serialize from it, with no generic `Value` middle layer.
//!
//! `json-bourne` skips the dynamic-tree intermediate that crates like
//! `serde_json` use. Each type knows how to deserialize itself from a
//! [`Lexer`] (via [`FromJson`](trait@FromJson)) or write itself to a [`JsonWrite`]
//! sink (via [`ToJson`]). The typed structure already enforces JSON's
//! grammar, so the per-event state machine is pure overhead for typed
//! consumers — skipping it makes the typed path ~2× faster on
//! integer / string-heavy payloads.
//!
//! - **Derive-driven.** `#[derive(FromJson, ToJson)]` (the `derive`
//!   feature, on by default) generates the typed impls. The generated
//!   code is itself `no_std`; only the compile-time derive pulls in the
//!   proc-macro stack.
//! - **`no_std` everywhere.** The streaming [`Lexer`] / [`Parser`]
//!   layer is `no_std` always; with the default `std` feature off
//!   the crate is `no_std + alloc`, and with `alloc` off too it is
//!   pure `no_std`.
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
//! use json_bourne::parse_str;
//! let n: u32 = parse_str("42").unwrap();
//! assert_eq!(n, 42);
//! ```
//!
//! Parse a struct with `#[derive(FromJson)]`:
//!
//! ```
//! use json_bourne::{FromJson, parse_str};
//!
//! #[derive(Debug, PartialEq, FromJson)]
//! struct User<'input> {
//!     id: u64,
//!     name: &'input str,
//!     active: bool,
//! }
//!
//! let u: User<'_> = parse_str(r#"{"id":1,"name":"alice","active":true}"#).unwrap();
//! assert_eq!(u.name, "alice");
//! ```
//!
//! Serialize back out with `#[derive(ToJson)]`:
//!
//! ```
//! use json_bourne::{ToJson, to_string};
//!
//! #[derive(ToJson)]
//! struct Point { x: i32, y: i32 }
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
//! | `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, escape decoding, `to_string`/`to_vec` |
//! | `derive`    | yes     | `#[derive(FromJson, ToJson)]` via `bourne-derive` |
//!
//! `default-features = false` plus `["alloc"]` gives a `no_std + alloc`
//! build. `default-features = false` alone gives pure `no_std`: the
//! streaming [`Lexer`] / [`Parser`], typed APIs that don't need a heap,
//! and float serialization through [`to_fmt`] / [`FmtWriteSink`] are
//! available.
// Targeted uses of `unsafe` inside the streaming layer (lexer.rs / event.rs):
//   1. `from_utf8_unchecked` after the lexer has validated every byte against
//      the RFC 3629 byte ranges inline. The safe alternative re-walks the
//      entire string per call and was 48% of `vec_borrowed_str` runtime.
//   2. `core::arch::x86_64` SSE2 intrinsics in `scan_ascii_string_run_simd`.
//      SSE2 is part of the x86_64 ABI baseline, so the `#[target_feature]`
//      precondition is statically guaranteed on x86_64 — the unsafe is
//      mechanical (intrinsics are unsafe by signature), not a memory-safety
//      escape hatch. Non-x86_64 targets compile to the scalar path.
//
// Workspace lint is `deny` (not `forbid`) for exactly this kind of
// localized, justified exception — granted per-site, never crate-wide.

#[cfg(feature = "alloc")]
extern crate alloc;

// The derive macros emit `::json_bourne::…` paths that must resolve even
// when the derive is used *inside* this crate (tests, doctests). Alias
// the crate to its own name so those absolute paths bind here too. This
// is the same self-aliasing trick `serde_derive` relies on.
#[cfg(feature = "derive")]
extern crate self as json_bourne;

mod casing;
mod de;
#[cfg(feature = "std")]
pub(crate) mod display_scratch;
mod error;
mod escape;
mod event;
mod float;
mod lexer;
mod parser;
mod ser;
#[cfg(all(test, feature = "std"))]
mod teju_gen;

#[doc(hidden)]
pub use casing::{Casing as __Casing, Renamed as __Renamed};
pub use de::{FromJson, parse, parse_str};
#[cfg(feature = "alloc")]
pub use de::{KeyCow, MapKey, key_to_cow};
pub use error::{Error, ErrorKind, LineColumn, Position};
pub use event::{Event, JsonNum, JsonStr, MAX_INPUT_LEN};
pub use lexer::{Checkpoint, DEFAULT_MAX_DEPTH, Lexer, ValueKind};
pub use parser::Parser;
#[cfg(feature = "alloc")]
pub use ser::{
    ByteSink, MapKeyOut, PrettyStringSink, StringSink, to_string, to_string_pretty, to_vec,
};
pub use ser::{FmtWriteSink, to_fmt};
#[cfg(feature = "std")]
pub use ser::{IoWriteSink, to_writer};
pub use ser::{JsonWrite, ToJson};

/// Derive `FromJson` / `ToJson` for your own structs and enums.
///
/// Requires the `derive` feature (on by default). The derives are
/// implemented by the companion `bourne-derive` proc-macro crate and
/// re-exported here so you only ever depend on and import `json_bourne`.
///
/// ```
/// use json_bourne::{FromJson, ToJson, parse_str, to_string};
///
/// #[derive(FromJson, ToJson)]
/// struct Point { x: i32, y: i32 }
///
/// let p: Point = parse_str(r#"{"x":3,"y":-7}"#).unwrap();
/// assert_eq!(to_string(&p).unwrap(), r#"{"x":3,"y":-7}"#);
/// ```
#[cfg(feature = "derive")]
pub use bourne_derive::{FromJson, ToJson};

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    mod float_fast_path;
    mod stack_frames;

    mod integer_paths {
        use super::*;

        const fn lex(input: &[u8]) -> Lexer<'_> {
            Lexer::new(input)
        }

        #[test]
        fn i64_value_matches_str_parse_chunked() {
            for n in [1u64, 9, 12_345_678, 99_999_999, 100_000_000, 123_456_789] {
                let s = format!("{n}");
                let want = i64::try_from(n).unwrap();
                assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), want);
                let s = format!("-{n}");
                assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), -want);
            }
        }

        #[test]
        fn i64_chunk_boundary_lengths() {
            // 7- and 8-digit literals sit at the length where a wider
            // (SIMD/SWAR) digit reader would start paying; everything
            // shorter and longer must agree with `str::parse` too.
            for s in [
                "1234567",
                "12345678",
                "123456789",
                "123456789012345",
                "1234567890123456",
                "12345678901234567",
            ] {
                let want: i64 = s.parse().unwrap();
                assert_eq!(lex(s.as_bytes()).parse_i64_value().unwrap(), want, "{s}");
            }
        }

        #[test]
        fn i64_stops_at_first_non_digit() {
            // The parser must stop at the first non-digit and leave the
            // cursor there.
            let mut lx = lex(b"12345678,9");
            assert_eq!(lx.parse_i64_value().unwrap(), 12_345_678);
            assert_eq!(lx.position().offset, 8);
        }

        #[test]
        fn i64_trailing_fraction_rejected_after_chunk() {
            let mut lx = lex(b"12345678.5");
            assert_eq!(
                lx.parse_i64_value().unwrap_err().kind,
                ErrorKind::ExpectedNumber
            );
        }

        #[test]
        fn i64_nineteen_digits_ok_twenty_rejected() {
            let ok = "9223372036854775807"; // i64::MAX, 19 digits
            assert_eq!(lex(ok.as_bytes()).parse_i64_value().unwrap(), i64::MAX);
            let over = "9223372036854775808"; // 19 digits + 1
            assert_eq!(
                lex(over.as_bytes()).parse_i64_value().unwrap_err().kind,
                ErrorKind::NumberOutOfRange
            );
            let long = "12345678901234567890"; // 20 digits, out of range
            assert_eq!(
                lex(long.as_bytes()).parse_i64_value().unwrap_err().kind,
                ErrorKind::NumberOutOfRange
            );
        }

        #[test]
        fn u64_value_matches_str_parse_chunked() {
            for n in [
                1u64,
                9,
                12_345_678,
                99_999_999,
                100_000_000,
                u64::MAX,
                u64::MAX - 1,
            ] {
                let s = format!("{n}");
                assert_eq!(lex(s.as_bytes()).parse_u64_value().unwrap(), n);
            }
        }

        #[test]
        fn i128_value_matches_str_parse_chunked() {
            for n in [
                1u128,
                12_345_678,
                100_000_000,
                170_141_183_460_469_231_731_687_303_715_884_105_727, // i128::MAX
            ] {
                let s = format!("{n}");
                let want = i128::try_from(n).unwrap();
                assert_eq!(lex(s.as_bytes()).parse_i128_value().unwrap(), want);
            }
            let neg = "-170141183460469231731687303715884105728"; // i128::MIN
            assert_eq!(lex(neg.as_bytes()).parse_i128_value().unwrap(), i128::MIN);
        }

        #[test]
        fn scan_digit_run_advances_to_first_non_digit() {
            let mut lx = lex(b"12345678901234567890abc");
            lx.read_number().unwrap();
            assert_eq!(lx.position().offset, 20);
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

/// Round-trip tests for [`crate::ToJson`] primitives. Pairs each impl
/// against the existing `FromJson` impl: serialize a value, parse it
/// back, assert equality. This is the contract that the two sides agree
/// on the wire format — if a primitive's encoding ever drifts, one of
/// these tests breaks before any user code does.
#[cfg(all(test, feature = "std"))]
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
/// Each test pairs a `ToJson`-derived type against a manually-defined
/// `FromJson` mirror so the round-trip exercises both derives on
/// equivalent shapes.
#[cfg(all(test, feature = "std"))]
mod to_json_macro_tests {
    use super::{parse_str, to_string};
    use crate::{FromJson, ToJson};

    // The simplest possible shape: plain struct, no attrs.
    #[derive(Debug, PartialEq, ToJson)]
    struct Plain {
        id: u32,
        name: String,
    }

    #[derive(Debug, PartialEq, FromJson)]
    struct PlainParse {
        id: u32,
        name: String,
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

    #[derive(Debug, PartialEq, ToJson)]
    struct Borrowed<'input> {
        tag: &'input str,
        count: u32,
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
    #[derive(Debug, PartialEq, ToJson)]
    struct Decorated {
        #[bourne(rename = "user-id")]
        user_id: u32,
        #[bourne(skip)]
        cached: u32,
        #[bourne(skip_if_none)]
        note: Option<String>,
        value: u32,
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

    // Regression: a *plain* (un-renamed) field followed by a skipped
    // `skip_if_none` field, then a present field. The plain fast path folds
    // the comma+key into a compile-time literal; it must still record that
    // output was committed so the later runtime comma (gated on `!__first`)
    // is emitted. Prior to the fix this produced `.."b""c":..` (missing
    // comma) whenever the optional middle field was None.
    #[derive(Debug, PartialEq, ToJson)]
    struct PlainThenSkip {
        a: u32,
        b: u32,
        #[bourne(skip_if_none)]
        mid: Option<u32>,
        c: u32,
    }

    #[test]
    fn plain_field_before_skipped_option_keeps_comma() {
        // mid = None -> the fast-path plain fields must still separate from `c`.
        let v = PlainThenSkip {
            a: 1,
            b: 2,
            mid: None,
            c: 3,
        };
        assert_eq!(to_string(&v).unwrap(), r#"{"a":1,"b":2,"c":3}"#);
        // mid = Some -> comma on both sides of the optional.
        let v = PlainThenSkip {
            a: 1,
            b: 2,
            mid: Some(9),
            c: 3,
        };
        assert_eq!(to_string(&v).unwrap(), r#"{"a":1,"b":2,"mid":9,"c":3}"#);
    }

    // Empty struct edge case.
    #[derive(Debug, PartialEq, ToJson)]
    struct Empty {}

    #[test]
    fn empty_struct_emits_empty_object() {
        assert_eq!(to_string(&Empty {}).unwrap(), "{}");
    }

    // String escaping inside emitted values (sanity — should already
    // work via the ToJson<String> impl, but the macro shouldn't
    // double-escape or corrupt the output).
    #[derive(Debug, PartialEq, ToJson)]
    struct WithEscape {
        text: String,
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
    #[derive(Debug, PartialEq, ToJson)]
    struct UserId(u64);

    #[test]
    fn newtype_emits_bare_value() {
        let v = UserId(42);
        assert_eq!(to_string(&v).unwrap(), "42");
    }

    #[derive(Debug, PartialEq, ToJson)]
    struct BorrowedTag<'input>(&'input str);

    #[test]
    fn newtype_with_lifetime() {
        let v = BorrowedTag("hello");
        assert_eq!(to_string(&v).unwrap(), r#""hello""#);
    }

    // Multi-field tuple struct — emits a JSON array.
    #[derive(Debug, PartialEq, ToJson)]
    struct Point(i32, i32);

    #[test]
    fn tuple_struct_emits_array() {
        let v = Point(3, -7);
        assert_eq!(to_string(&v).unwrap(), "[3,-7]");
    }

    #[derive(Debug, PartialEq, ToJson)]
    struct Triple(i32, String, bool);

    #[test]
    fn three_field_tuple_struct() {
        let v = Triple(1, String::from("hi"), true);
        assert_eq!(to_string(&v).unwrap(), r#"[1,"hi",true]"#);
    }

    // Externally-tagged enum — the default encoding.
    #[derive(Debug, PartialEq, ToJson)]
    enum Shape {
        Circle,
        Wrapper(u32),
        Pair(u32, String),
        Box {
            w: u32,
            h: u32,
        },
        #[bourne(rename = "tri")]
        Triangle,
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
    #[derive(Debug, PartialEq, ToJson)]
    #[bourne(tag = "type")]
    enum Event {
        Heartbeat,
        #[bourne(rename = "click")]
        Click {
            x: u32,
            y: u32,
        },
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
    #[derive(Debug, PartialEq, ToJson)]
    #[bourne(tag = "t", content = "c")]
    enum Msg {
        Ping,
        Echo(String),
        Pair(u32, u32),
        Body { text: String },
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
    #[derive(Debug, PartialEq, ToJson)]
    #[bourne(untagged)]
    enum Mixed {
        Nothing,
        One(u32),
        Two(u32, u32),
        Body { name: String },
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
#[cfg(all(test, feature = "std"))]
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

/// Combined derive tests. Each type gets both `FromJson` and `ToJson`
/// from one `#[derive(...)]`.
#[cfg(all(test, feature = "std"))]
mod json_macro_tests {
    use super::{parse_str, to_string};
    use crate::{FromJson, ToJson};

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    struct Plain {
        id: u32,
        name: String,
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    struct Borrowed<'input> {
        tag: &'input str,
        count: u32,
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    struct Decorated {
        #[bourne(rename = "user-id")]
        user_id: u32,
        #[bourne(skip)]
        cached: u32,
        #[bourne(skip_if_none)]
        note: Option<String>,
        value: u32,
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    struct UserId(u64);

    #[test]
    fn newtype_round_trips() {
        let v = UserId(42);
        let s = to_string(&v).unwrap();
        assert_eq!(s, "42");
        let back: UserId = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    struct Pair(i32, i32);

    #[test]
    fn tuple_struct_round_trips() {
        let v = Pair(3, -7);
        let s = to_string(&v).unwrap();
        assert_eq!(s, "[3,-7]");
        let back: Pair = parse_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    enum Shape {
        Circle,
        Wrapper(u32),
        Pair(u32, String),
        Box {
            w: u32,
            h: u32,
        },
        #[bourne(rename = "tri")]
        Triangle,
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    #[bourne(tag = "type")]
    enum Event {
        Heartbeat,
        #[bourne(rename = "click")]
        Click {
            x: u32,
            y: u32,
        },
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    #[bourne(tag = "t", content = "c")]
    enum Msg {
        Ping,
        Echo(String),
        Pair(u32, u32),
        Body { text: String },
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

    #[derive(Debug, PartialEq, FromJson, ToJson)]
    #[bourne(untagged)]
    enum Mixed {
        Nothing,
        One(u32),
        Two(u32, u32),
        Body { name: String },
    }

    #[derive(Debug, PartialEq, Eq, FromJson, ToJson)]
    pub struct PubFields {
        pub id: u32,
        pub(crate) name: String,
        value: u32,
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

/// Boundary tests for the public surface — pinned at the raw-tail write
/// in `ByteSink` (guarded by a live capacity check) and the
/// `from_utf8_unchecked` site in `decode_escapes`. These are designed to
/// run under miri (CI: `MIRIFLAGS=-Zmiri-disable-isolation
/// RUSTFLAGS=--cfg bourne_no_simd cargo +nightly miri test -p json-bourne --lib`).
/// Each one targets a specific invariant; if a future refactor breaks the
/// capacity check or the UTF-8 boundary, miri here trips
/// on the precise unsafe before any user code does.
#[cfg(all(test, feature = "std"))]
mod unsafe_boundary_tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Slice-of-floats ser at the exact-capacity boundary. The slice
    /// writer's `reserve_hint` computes
    ///   `2 + n * (MAX_SERIALIZED_LEN + 1) = 2 + n * 33`.
    /// We pre-reserve exactly that, so the per-element
    /// `write_float_f64_hinted` tail-write path (which needs ≥ 32 spare
    /// bytes) runs against the tightest legal Vec capacity — under-reserving
    /// by even one byte would degrade to the checked path, which these
    /// round-trips would not detect, so the boundary itself is what miri
    /// verifies.
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
    /// and shares the hinted tail-write path, so the capacity boundary is
    /// identical.
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

    /// Slice of f64 containing non-finite values — verifies the per-element
    /// hinted float write surfaces `NonFiniteFloat` through the sink and
    /// emits nothing for the offending element.
    #[test]
    fn bytesink_slice_nonfinite_errors_without_partial_element() {
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

    /// Slice of i64 — exercises `write_byte_hinted` for `[`/`,`/`]` at
    /// exact capacity. The hint math is the same as the float case; this
    /// pins the integer shape (20-byte worst case) of the same path.
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

    /// Empty slice / array — `write_array_hinted`'s `split_first` is None,
    /// no per-element writes. Just `[` and `]`.
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

    /// Regression for the §3.1 audit finding: the slice writer used to
    /// take `T::MAX_SERIALIZED_LEN` (a safe const any impl could lie
    /// about) as a hard precondition for raw unchecked tail writes, so a
    /// lying impl caused a heap overflow. The hinted path now falls back
    /// to checked writes when the tail is exhausted — a wrong bound costs
    /// a reallocation, never memory safety.
    #[test]
    fn slice_writer_survives_lying_max_serialized_len() {
        #[derive(Debug, PartialEq)]
        struct Liar(i64);
        impl ToJson for Liar {
            const MIN_SERIALIZED_LEN: usize = 1;
            // Deliberately wrong: elements emit up to 20 bytes.
            const MAX_SERIALIZED_LEN: usize = 1;
            fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
                self.0.write_json(w)
            }
        }
        let data: Vec<Liar> = (0..500)
            .map(|i| Liar(i64::from(i) * 1_000_000_000 - 250_000_000_000))
            .collect();
        let s = to_string(data.as_slice()).expect("serialize despite lying bound");
        let back: Vec<i64> = parse_str(&s).expect("round-trip");
        let expected: Vec<i64> = (0..500)
            .map(|i| i64::from(i) * 1_000_000_000 - 250_000_000_000)
            .collect();
        assert_eq!(back, expected);
    }

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
    // PrettyStringSink, ByteSink hinted methods) needs at least one
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

    /// Audit 3.14: a custom sink whose `write_float_f64` tolerates NaN
    /// must never receive a fabricated `NaN` from the `f64`/`f32`
    /// `write_json` error paths — it receives the actual value, which
    /// it may serialize (this sink does), but never a placeholder.
    struct NaNTolerantSink {
        out: String,
        saw_bytes: bool,
    }

    impl JsonWrite for NaNTolerantSink {
        type Error = crate::Error;

        fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
            self.out.push(b as char);
            Ok(())
        }

        fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
            self.out.push_str(s);
            Ok(())
        }

        fn on_invalid_utf8(&mut self) -> Self::Error {
            crate::Error::new(crate::ErrorKind::InvalidUtf8, crate::Position::START)
        }

        fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
            if f.is_nan() {
                // Tolerant: record the sentinel so the test can tell a
                // fabricated NaN from an honest copy of the input.
                self.saw_bytes = true;
                self.out.push_str("nan-sentinel");
                return Ok(());
            }
            if f.is_infinite() {
                // The in-tree formatter asserts finiteness; render the
                // infinity directly to observe what arrived.
                self.out.push_str(if f > 0.0 { "inf" } else { "-inf" });
                return Ok(());
            }
            crate::float::format_finite(f, &mut self.out);
            Ok(())
        }
    }

    #[test]
    fn non_finite_error_delivers_actual_value_not_fabricated_nan() {
        let mut sink = NaNTolerantSink {
            out: String::new(),
            saw_bytes: false,
        };
        // -inf, not NaN: a fabricated placeholder would surface as the
        // `nan-sentinel` bytes; the honest path hands over -inf.
        let f = f64::NEG_INFINITY;
        f.write_json(&mut sink).unwrap();
        assert!(!sink.saw_bytes, "fabricated NaN reached the sink");
        assert_eq!(sink.out, "-inf");

        // NaN itself round-trips through the tolerant sink as NaN.
        let mut sink = NaNTolerantSink {
            out: String::new(),
            saw_bytes: false,
        };
        f64::NAN.write_json(&mut sink).unwrap();
        assert!(sink.saw_bytes);
    }

    /// Audit 3.15: `write_raw_bytes` with non-UTF-8 input returns `Err`
    /// instead of panicking — a custom `ToJson` impl feeding arbitrary
    /// bytes must not be able to panic the serializer.
    #[test]
    fn write_raw_bytes_rejects_non_utf8_with_err() {
        struct BareSink {
            out: String,
        }
        impl JsonWrite for BareSink {
            type Error = crate::Error;

            fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
                self.out.push(b as char);
                Ok(())
            }
            fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
                self.out.push_str(s);
                Ok(())
            }
            fn on_invalid_utf8(&mut self) -> Self::Error {
                crate::Error::new(crate::ErrorKind::InvalidUtf8, crate::Position::START)
            }
            fn write_float_f64(&mut self, _f: f64) -> Result<(), Self::Error> {
                unimplemented!()
            }
        }

        let mut sink = BareSink { out: String::new() };
        // 0x80 alone is never valid UTF-8.
        let err = sink.write_raw_bytes(&[b'h', 0x80, b'i']).unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::InvalidUtf8);
        // Valid input still writes through.
        sink.write_raw_bytes(b"ok").unwrap();
        assert_eq!(sink.out, "ok");
    }

    /// Audit 4.4.5: `ByteSink` does not validate `write_raw_bytes`, so a
    /// hand-written `ToJson` impl can put non-UTF-8 bytes into `to_vec`.
    /// `to_string` — the one API promising a `String` — must report that
    /// as `Err(InvalidUtf8)` instead of panicking on the `expect`.
    #[test]
    fn to_string_reports_non_utf8_from_user_impl_as_err() {
        struct RawBytes(u8);
        impl ToJson for RawBytes {
            const MIN_SERIALIZED_LEN: usize = 3;
            fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
                w.write_raw_bytes(&[b'"', self.0, b'"'])
            }
        }

        let err = to_string(&RawBytes(0x80)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidUtf8);
        // The bytes are still intact in to_vec for byte-oriented callers.
        assert_eq!(to_vec(&RawBytes(b'x')).unwrap(), b"\"x\"");
    }

    /// Audit 3.15: the `InputTooLarge` kind carries a helpful message,
    /// and `try_new` accepts normal inputs through the `Result` API.
    #[test]
    fn oversized_input_api_returns_err_not_panic() {
        let lex: Lexer<'_> = Lexer::try_new(b"[1]").expect("small input parses");
        assert_eq!(lex.input().len(), 3);
        assert_eq!(
            ErrorKind::InputTooLarge.to_string(),
            "input exceeds the maximum supported JSON document size"
        );
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

    /// `ByteSink::write_float_f64` — non-finite returns the typed error
    /// without touching the buffer.
    #[test]
    fn bytesink_float_nonfinite_returns_error() {
        let mut out: Vec<u8> = Vec::with_capacity(64);
        let mut sink = ByteSink::new(&mut out);
        let r = sink.write_float_f64(f64::INFINITY);
        assert!(r.is_err(), "non-finite must error");
        assert!(out.is_empty(), "failed write must not emit bytes");
    }

    /// `JsonWrite::write_float_f64_hinted` — with reserved capacity the
    /// write goes through the hinted path and round-trips exactly.
    #[test]
    fn bytesink_hinted_finite_writes_value() {
        let mut out: Vec<u8> = Vec::with_capacity(64);
        let mut sink = ByteSink::new(&mut out);
        // Pick a finite value that round-trips exactly through the formatter
        // and `f64::parse`. 2.5 is exact in binary; avoiding 3.14 also dodges
        // clippy's approx_constant warning about PI.
        let value = 2.5_f64;
        sink.reserve_hint(32);
        assert!(sink.write_float_f64_hinted(value).unwrap());
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

    /// The f64 `NEEDS_VALIDATION` machinery was removed with the reserved-
    /// write path; keep a marker test so the behavior stays pinned: slice
    /// writes succeed for finite input and error (not corrupt) on non-finite.
    #[test]
    fn f64_slice_writes_and_errors_on_nonfinite() {
        let slice: &[f64] = &[1.0, 2.0, 3.0];
        assert_eq!(to_string(slice).unwrap(), "[1.0,2.0,3.0]");
        let bad: &[f64] = &[1.0, f64::INFINITY];
        assert!(to_string(bad).is_err());
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

    /// Regression for audit 3.9: the default `write_escaped_str` pushed
    /// every byte through `write_byte`, and `FmtWriteSink::write_byte`
    /// is char-oriented (`write_char(b as char)`), so UTF-8 continuation
    /// bytes became Latin-1 mojibake (`é` → `Ã©`). All sinks must agree
    /// byte-for-byte on strings that mix escapes with non-ASCII.
    #[test]
    fn all_sinks_agree_on_non_ascii_and_escapes() {
        let cases = [
            "plain ascii",
            "café ñ€𝄞",
            "tab\tnewline\nquote\"backslash\\",
            "é\"é\\é\né",
            "100% ctrl-less unicode ✓ ✓ ✓",
            "\u{7f}\u{1f}\u{0}",
        ];
        for case in cases {
            let expected = to_string(case).expect("StringSink");
            assert_eq!(expected, format!("\"{}\"", escaped_debug(case)));

            let mut fmt_out = String::new();
            to_fmt(case, &mut fmt_out).expect("FmtWriteSink");
            assert_eq!(fmt_out, expected, "FmtWriteSink diverged for {case:?}");

            let mut byte_out: Vec<u8> = Vec::new();
            let mut sink = ByteSink::new(&mut byte_out);
            case.write_json(&mut sink).expect("ByteSink");
            assert_eq!(
                core::str::from_utf8(&byte_out).unwrap(),
                expected,
                "ByteSink diverged for {case:?}"
            );

            #[cfg(feature = "std")]
            {
                let mut io_out = Vec::<u8>::new();
                to_writer(case, &mut io_out).expect("IoWriteSink");
                assert_eq!(
                    String::from_utf8(io_out).unwrap(),
                    expected,
                    "IoWriteSink diverged for {case:?}"
                );
            }

            let mut pretty_out = String::new();
            let mut sink = PrettyStringSink::new(&mut pretty_out);
            case.write_json(&mut sink).expect("PrettyStringSink");
            assert_eq!(
                pretty_out, expected,
                "PrettyStringSink diverged for {case:?}"
            );
        }
    }

    /// Build the expected JSON string body the same way the doc example
    /// does — via an independent escape walk over chars, not via the
    /// code under test.
    fn escaped_debug(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => {
                    use core::fmt::Write as _;
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out
    }
}
