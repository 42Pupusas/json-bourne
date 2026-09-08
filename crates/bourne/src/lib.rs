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

    mod escape_decode;
    mod float_fast_path;
    mod float_uncentred;
    mod integer_paths;
    mod sink_adapter;
    mod sink_direct;
    mod stack_frames;
    mod unsafe_boundary;
    mod vec_fast_path;

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
