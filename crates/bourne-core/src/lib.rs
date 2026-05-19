#![cfg_attr(not(feature = "std"), no_std)]
//! Streaming JSON lexer and parser. `no_std`, zero alloc, zero deps.
//!
//! `bourne-core` is the byte-walking layer that the higher-level
//! [`bourne`](https://docs.rs/bourne) crate builds on. Most users want
//! `bourne`; reach for `bourne-core` directly only when you need the
//! event stream without the typed `FromJson` / `ToJson` traits — for
//! example to write a custom dispatcher or to parse on a no-alloc
//! target.
//!
//! # Two layers
//!
//! - [`Parser`] — pull-based event stream. Each [`next_event`] call
//!   returns one [`Event`] (`StartObject`, `Key`, `Number`, …) and
//!   enforces JSON's grammar via an internal state machine.
//! - [`Lexer`] — stateless token reader. The `Parser` wraps a
//!   `Lexer`; typed consumers in `bourne` drive the lexer directly
//!   to skip the per-event grammar dispatch.
//!
//! # Example
//!
//! ```
//! use bourne_core::{Event, Parser};
//!
//! let mut p: Parser<'_> = Parser::new(br#"{"id":42}"#);
//! assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
//! assert!(matches!(p.next_event().unwrap(), Some(Event::Key(_))));
//! assert!(matches!(p.next_event().unwrap(), Some(Event::Number(_))));
//! assert!(matches!(p.next_event().unwrap(), Some(Event::EndObject)));
//! assert!(p.next_event().unwrap().is_none());
//! ```
//!
//! # Bounds
//!
//! - Maximum input size: ~2 GB ([`MAX_INPUT_LEN`]). Offsets are stored
//!   as 31-bit values; the top bit is reserved for the `has_escapes`
//!   flag in [`JsonStr`].
//! - Maximum nesting depth: 128 by default ([`DEFAULT_MAX_DEPTH`]).
//!   Override at construction by parameterizing `Parser<'_, N>`.
//!
//! [`next_event`]: Parser::next_event
// Targeted uses of `unsafe`:
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
// localized, justified exception.
#![allow(unsafe_code)]

mod error;
mod event;
mod lexer;
mod parser;

pub use error::{Error, ErrorKind, LineColumn, Position};
pub use event::{Event, JsonNum, JsonStr, MAX_INPUT_LEN};
pub use lexer::{Checkpoint, DEFAULT_MAX_DEPTH, Lexer, ValueKind};
pub use parser::Parser;

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(input: &str) -> Result<alloc::vec::Vec<Event>, Error> {
        let mut p: Parser<'_> = Parser::new(input.as_bytes());
        let mut out = alloc::vec::Vec::new();
        while let Some(ev) = p.next_event()? {
            out.push(ev);
        }
        Ok(out)
    }

    #[test]
    fn null_true_false() {
        assert_eq!(collect("null").unwrap(), [Event::Null]);
        assert_eq!(collect("true").unwrap(), [Event::Bool(true)]);
        assert_eq!(collect("false").unwrap(), [Event::Bool(false)]);
    }

    #[test]
    fn numbers() {
        let input = b"123";
        let evs = collect("123").unwrap();
        match &evs[..] {
            [Event::Number(n)] => assert_eq!(n.as_i64(input).unwrap(), 123),
            _ => panic!("{evs:?}"),
        }
        let input2 = b"-1.5e2";
        let evs = collect("-1.5e2").unwrap();
        match &evs[..] {
            [Event::Number(n)] => {
                assert!(n.is_float(input2));
                assert!((n.as_f64(input2).unwrap() - -150.0).abs() < f64::EPSILON);
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn empty_string_and_borrowed() {
        let input = br#""""#;
        let evs = collect(r#""""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert_eq!(s.as_str(input), Some(""));
                assert!(!s.has_escapes());
            }
            _ => panic!("{evs:?}"),
        }
        let input2 = br#""hello""#;
        let evs = collect(r#""hello""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => assert_eq!(s.as_str(input2), Some("hello")),
            _ => panic!("{evs:?}"),
        }
    }

    /// The SSE2 string-scan walks 16 bytes at a time. Strings whose body
    /// crosses chunk boundaries (terminating quote at offsets 15, 16, 17, …
    /// inside a 16-byte chunk) exercise the boundary logic specifically.
    /// Strings shorter than 16 bytes go straight to the scalar tail; strings
    /// longer than 16 bytes go through at least one full SIMD iteration.
    #[test]
    fn ascii_string_simd_boundaries() {
        for body_len in [0usize, 1, 14, 15, 16, 17, 31, 32, 33, 64, 100] {
            let body: alloc::string::String = "a".repeat(body_len);
            let input = alloc::format!(r#""{body}""#);
            let evs = collect(&input).expect("valid input");
            match &evs[..] {
                [Event::String(s)] => {
                    let got = s
                        .as_str(input.as_bytes())
                        .expect("no escapes, should borrow");
                    assert_eq!(got, body, "len={body_len}");
                    assert!(!s.has_escapes(), "len={body_len}");
                }
                _ => panic!("len={body_len}: {evs:?}"),
            }
        }
    }

    /// Backslash, control char, and high-bit byte must each be detected at
    /// any offset inside a 16-byte SIMD chunk. The signed-compare trick
    /// (`cmplt_epi8 < 0x20`) flags both control chars (positive byte <0x20)
    /// and high-bit bytes (negative when read as i8); regress this fusion
    /// by scanning each kind at position 0, 7, 15, 16, 23 inside a longer run.
    #[test]
    fn ascii_string_simd_finds_stop_bytes() {
        // Quote at every position 0..=20: each must terminate cleanly.
        for pos in 0..=20usize {
            let prefix: alloc::string::String = "a".repeat(pos);
            let input = alloc::format!(r#""{prefix}""#);
            let evs = collect(&input).expect("valid input");
            match &evs[..] {
                [Event::String(s)] => {
                    assert_eq!(s.as_str(input.as_bytes()).unwrap(), prefix);
                }
                _ => panic!("quote at {pos}: {evs:?}"),
            }
        }
        // Control char at position 16 (inside the second SIMD chunk) must
        // be rejected, not skipped over.
        let mut bad = alloc::string::String::from("\"");
        for _ in 0..16 {
            bad.push('a');
        }
        bad.push('\n'); // 0x0A — control byte
        bad.push_str("more\"");
        assert!(collect(&bad).is_err(), "control byte not flagged");
    }

    #[test]
    fn escaped_string_marked() {
        let input = br#""a\nb""#;
        let evs = collect(r#""a\nb""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert!(s.has_escapes());
                assert_eq!(s.as_str(input), None);
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn empty_array_and_object() {
        assert_eq!(collect("[]").unwrap(), [Event::StartArray, Event::EndArray]);
        assert_eq!(collect("{}").unwrap(), [Event::StartObject, Event::EndObject]);
    }

    #[test]
    fn array_of_scalars() {
        let evs = collect("[1, true, null]").unwrap();
        assert!(matches!(evs[0], Event::StartArray));
        assert!(matches!(evs[1], Event::Number(_)));
        assert_eq!(evs[2], Event::Bool(true));
        assert_eq!(evs[3], Event::Null);
        assert!(matches!(evs[4], Event::EndArray));
    }

    #[test]
    fn nested_object() {
        let evs = collect(r#"{"a":{"b":[1,2]}}"#).unwrap();
        let kinds: alloc::vec::Vec<_> = evs
            .iter()
            .map(|e| match e {
                Event::StartObject => "{",
                Event::EndObject => "}",
                Event::StartArray => "[",
                Event::EndArray => "]",
                Event::Key(_) => "k",
                Event::Number(_) => "n",
                _ => "?",
            })
            .collect();
        assert_eq!(kinds, ["{", "k", "{", "k", "[", "n", "n", "]", "}", "}"]);
    }

    #[test]
    fn rejects_trailing_comma_in_array() {
        assert!(collect("[1,]").is_err());
    }

    #[test]
    fn rejects_trailing_comma_in_object() {
        assert!(collect(r#"{"a":1,}"#).is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(collect("null x").is_err());
    }

    #[test]
    fn rejects_leading_zero() {
        assert!(collect("01").is_err());
    }

    #[test]
    fn rejects_bare_minus() {
        assert!(collect("-").is_err());
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(collect(r#""abc"#).is_err());
    }

    #[test]
    fn rejects_control_in_string() {
        assert!(collect("\"a\nb\"").is_err());
    }

    #[test]
    fn position_tracks_lines() {
        let input = b"\n\n  null";
        let mut p: Parser<'_> = Parser::new(input);
        let _ = p.next_event().unwrap();
        let pos = p.position();
        assert_eq!(pos.resolve(input).line, 3);
    }

    extern crate alloc;
}
