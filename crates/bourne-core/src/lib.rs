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
        assert_eq!(
            collect("{}").unwrap(),
            [Event::StartObject, Event::EndObject]
        );
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

    // -----------------------------------------------------------------
    // Unsafe-boundary tests — focus on the `from_utf8_unchecked` sites in
    // `JsonStr::as_str`, `JsonNum::as_str`, and the lexer's borrow paths
    // (`parse_str_value`, `parse_f64_value`). Each must hand out only
    // byte ranges that the inline UTF-8 walk has already validated as
    // RFC 3629–compliant. Designed to run under miri (CI gates SIMD off
    // via `--cfg bourne_no_simd`, so the scalar path is what miri sees).
    // -----------------------------------------------------------------

    /// 2-byte UTF-8 (Latin Supplement: é = c3 a9) inside a borrowed
    /// string. Exercises `consume_utf8_multibyte`'s 2-byte arm and the
    /// `from_utf8_unchecked` in `JsonStr::as_str`.
    #[test]
    fn utf8_two_byte_in_borrowed_string() {
        let input = "\"café\"".as_bytes();
        let evs = collect("\"café\"").expect("parse");
        match &evs[..] {
            [Event::String(s)] => {
                assert_eq!(s.as_str(input).unwrap(), "café");
                assert!(!s.has_escapes());
            }
            _ => panic!("{evs:?}"),
        }
    }

    /// 3-byte UTF-8 (Currency Symbol: € = e2 82 ac).
    #[test]
    fn utf8_three_byte_in_borrowed_string() {
        let input = "\"price 5€\"".as_bytes();
        let evs = collect("\"price 5€\"").expect("parse");
        match &evs[..] {
            [Event::String(s)] => assert_eq!(s.as_str(input).unwrap(), "price 5€"),
            _ => panic!("{evs:?}"),
        }
    }

    /// 4-byte UTF-8 (Musical Symbol: 𝄞 = f0 9d 84 9e). The 4-byte arm of
    /// `consume_utf8_multibyte` is otherwise unreachable.
    #[test]
    fn utf8_four_byte_in_borrowed_string() {
        let input = "\"note 𝄞\"".as_bytes();
        let evs = collect("\"note 𝄞\"").expect("parse");
        match &evs[..] {
            [Event::String(s)] => assert_eq!(s.as_str(input).unwrap(), "note 𝄞"),
            _ => panic!("{evs:?}"),
        }
    }

    /// Multi-byte char straddling the 16-byte SIMD chunk boundary —
    /// position the multibyte sequence so its first byte lands at offset
    /// 14, 15, 16, 17 (under SIMD this triggers the boundary fall-through
    /// to scalar; under miri it walks scalar all the way). The borrowed
    /// `&str` must still be valid UTF-8.
    #[test]
    fn utf8_multibyte_at_simd_chunk_boundary() {
        for pad_len in [13usize, 14, 15, 16, 17, 30, 31, 32] {
            let prefix: alloc::string::String = "a".repeat(pad_len);
            // Push a 3-byte char immediately after the pad.
            let body = alloc::format!("{prefix}€");
            let input = alloc::format!("\"{body}\"");
            let evs = collect(&input).expect("parse");
            match &evs[..] {
                [Event::String(s)] => {
                    assert_eq!(s.as_str(input.as_bytes()).unwrap(), body, "pad={pad_len}");
                    assert!(!s.has_escapes(), "pad={pad_len}");
                }
                _ => panic!("pad={pad_len}: {evs:?}"),
            }
        }
    }

    /// `JsonStr::as_str` on a string that DOES have escapes — must return
    /// None and never invoke the `from_utf8_unchecked`. Catches a regression
    /// where the `has_escapes()` early-return is dropped.
    #[test]
    fn jsonstr_as_str_returns_none_when_escapes_present() {
        let input = br#""a\nb""#;
        let evs = collect(r#""a\nb""#).expect("parse");
        match &evs[..] {
            [Event::String(s)] => {
                assert!(s.has_escapes());
                assert_eq!(s.as_str(input), None);
            }
            _ => panic!("{evs:?}"),
        }
    }

    /// `JsonStr::raw_bytes` returns None when input slice doesn't cover
    /// the recorded range. Tests `input.get(start..end)`'s safety check.
    #[test]
    fn jsonstr_raw_bytes_handles_short_input() {
        // Parse against the real input, then query against a shorter slice.
        let real = br#""hello""#;
        let evs = collect(r#""hello""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert!(s.raw_bytes(real).is_some());
                // Truncated input — range goes out of bounds.
                let short: &[u8] = &real[..2];
                assert!(s.raw_bytes(short).is_none(), "should not OOB");
                // Also as_str on truncated input.
                assert!(s.as_str(short).is_none());
            }
            _ => panic!("{evs:?}"),
        }
    }

    /// `JsonNum::as_str` for a numeric literal — ASCII-only by lexer
    /// construction. Cover positive, negative, decimal, exponent forms.
    #[test]
    fn jsonnum_as_str_covers_grammar() {
        for lit in ["0", "1", "-1", "1.5", "-1.5", "1e10", "-1.5e-3", "0.0001"] {
            let input = lit.as_bytes();
            let evs = collect(lit).expect("parse");
            match &evs[..] {
                [Event::Number(n)] => {
                    assert_eq!(n.as_str(input), lit, "literal={lit}");
                    // raw_bytes covers the same range — exercise its
                    // input.get path.
                    assert_eq!(n.raw_bytes(input).unwrap(), lit.as_bytes());
                }
                _ => panic!("literal={lit}: {evs:?}"),
            }
        }
    }

    /// `parse_str_value` (lexer.rs:737) borrow path — same `from_utf8_unchecked`
    /// invariant. Exercised here by string round-trips with multibyte content.
    #[test]
    fn parse_str_value_returns_borrowed_multibyte() {
        let input = b"\"caf\xc3\xa9\""; // "café"
        let mut p: Parser<'_> = Parser::new(input);
        let lex = p.lexer();
        // Drive directly through parse_str_value
        let s = lex.parse_str_value().expect("parse");
        assert_eq!(s, "café");
    }

    /// `parse_f64_value` (lexer.rs:704) borrow path — the `read_number` walk
    /// only emits ASCII bytes from the number grammar. Verify under miri.
    #[test]
    fn parse_f64_value_handles_full_grammar() {
        for lit in [
            "0",
            "1.0",
            "-1.5",
            "1e10",
            "-1.5e-3",
            "1.7976931348623157e308",
        ] {
            let input = lit.as_bytes();
            let mut p: Parser<'_> = Parser::new(input);
            let lex = p.lexer();
            let v = lex.parse_f64_value().expect(lit);
            let expected: f64 = lit.parse().unwrap();
            // Compare bit patterns for exact equality (both go through IEEE 754
            // round-to-nearest and should produce identical encodings). Using
            // bits also makes NaN compare equal to NaN of the same encoding,
            // which `==` does not.
            assert_eq!(
                v.to_bits(),
                expected.to_bits(),
                "literal={lit}: got {v}, want {expected}"
            );
        }
    }

    /// Reject invalid UTF-8 in a string — the lexer must error before any
    /// `from_utf8_unchecked` runs on the bad bytes.
    #[test]
    fn rejects_invalid_utf8_in_string() {
        // 0xC0 is an invalid lead byte (overlong encoding). Must error
        // rather than borrow.
        let bad = b"\"a\xc0b\"";
        let mut p: Parser<'_> = Parser::new(bad);
        assert!(p.next_event().is_err(), "expected error for invalid UTF-8");

        // 0xE0 0x80 (overlong NUL) — invalid continuation.
        let bad2 = b"\"a\xe0\x80\xa9b\"";
        let mut p2: Parser<'_> = Parser::new(bad2);
        assert!(p2.next_event().is_err(), "expected error for overlong");

        // 0xED 0xA0 (UTF-16 surrogate range — forbidden).
        let bad3 = b"\"\xed\xa0\x80\"";
        let mut p3: Parser<'_> = Parser::new(bad3);
        assert!(p3.next_event().is_err(), "expected error for surrogate");
    }

    // -----------------------------------------------------------------
    // Lexer::parse_i64_value — overflow, sign, EOF, and fractional /
    // exponent rejection. Covers the branches inside the digit-
    // accumulation loop and `i64_from_unsigned_magnitude`.
    // -----------------------------------------------------------------

    #[test]
    fn parse_i64_value_handles_zero_and_signed_small() {
        // Leading-zero short path.
        let mut p: Parser<'_> = Parser::new(b"0");
        assert_eq!(p.parse_i64_value().unwrap(), 0);

        // Single negative digit (1..9 + minus branch).
        let mut p: Parser<'_> = Parser::new(b"-7");
        assert_eq!(p.parse_i64_value().unwrap(), -7);

        // Multi-digit positive (the count<19 fast arm).
        let mut p: Parser<'_> = Parser::new(b"1234567890");
        assert_eq!(p.parse_i64_value().unwrap(), 1_234_567_890);
    }

    #[test]
    fn parse_i64_value_handles_i64_min_and_max() {
        // i64::MAX — fits in the fast arm.
        let mut p: Parser<'_> = Parser::new(b"9223372036854775807");
        assert_eq!(p.parse_i64_value().unwrap(), i64::MAX);

        // i64::MIN — sign-aware bounds check accepts the magnitude that
        // would overflow positive i64.
        let mut p: Parser<'_> = Parser::new(b"-9223372036854775808");
        assert_eq!(p.parse_i64_value().unwrap(), i64::MIN);
    }

    #[test]
    fn parse_i64_value_rejects_positive_overflow() {
        // i64::MAX + 1: still 19 digits, fits in u64, but fails the
        // try_from(acc) check.
        let mut p: Parser<'_> = Parser::new(b"9223372036854775808");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::NumberOutOfRange);
    }

    #[test]
    fn parse_i64_value_rejects_u64_overflow_in_fast_arm() {
        // 20 digits — beyond i64::MAX in any sign. Hits the count >= 19
        // checked arithmetic arm with u64 overflow.
        let mut p: Parser<'_> = Parser::new(b"99999999999999999999");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::NumberOutOfRange);

        // Same but negative — exceeds I64_MIN_MAGNITUDE.
        let mut p: Parser<'_> = Parser::new(b"-10000000000000000000");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::NumberOutOfRange);
    }

    #[test]
    fn parse_i64_value_rejects_fractional_and_exponent() {
        // Leading-zero arm followed by '.': must reject.
        let mut p: Parser<'_> = Parser::new(b"0.5");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::ExpectedNumber);

        // Multi-digit arm followed by 'e': must reject.
        let mut p: Parser<'_> = Parser::new(b"123e4");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::ExpectedNumber);

        // Multi-digit arm followed by 'E' (uppercase exponent).
        let mut p: Parser<'_> = Parser::new(b"42E10");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::ExpectedNumber);
    }

    #[test]
    fn parse_i64_value_rejects_bare_minus_and_eof() {
        // Sign with no digit (UnexpectedEof inside the match's None arm).
        let mut p: Parser<'_> = Parser::new(b"-");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::UnexpectedEof);

        // Empty input — EOF.
        let mut p: Parser<'_> = Parser::new(b"");
        let e = p.parse_i64_value().unwrap_err();
        assert_eq!(e.kind, ErrorKind::UnexpectedEof);

        // Non-digit first byte (the catch-all `Some(b)` arm).
        let mut p: Parser<'_> = Parser::new(b"x");
        let e = p.parse_i64_value().unwrap_err();
        assert!(matches!(e.kind, ErrorKind::UnexpectedByte(_)));
    }

    // -----------------------------------------------------------------
    // Cover the cheap getters that no other test exercises directly.
    // The CRAP gate insta-trips any 0%-coverage function, so even
    // one-line accessors need at least one call.
    // -----------------------------------------------------------------

    #[test]
    fn jsonstr_end_and_debug_fmt() {
        use alloc::format;
        let evs = collect(r#""hello""#).unwrap();
        match &evs[..] {
            [Event::String(s)] => {
                assert_eq!(s.end(), 6);
                // Debug formatter
                let d = format!("{s:?}");
                assert!(d.contains("JsonStr"), "got: {d}");
                assert!(d.contains("has_escapes"));
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn jsonnum_start_end_and_debug_fmt() {
        use alloc::format;
        let evs = collect("123").unwrap();
        match &evs[..] {
            [Event::Number(n)] => {
                assert_eq!(n.start(), 0);
                assert_eq!(n.end(), 3);
                let d = format!("{n:?}");
                assert!(d.contains("JsonNum"));
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn jsonnum_as_i128_and_u128_decode() {
        let input = b"123456789012345678901234567890";
        let evs = collect(core::str::from_utf8(input).unwrap()).unwrap();
        match &evs[..] {
            [Event::Number(n)] => {
                let v = n.as_u128(input).expect("fits in u128");
                assert_eq!(v, 123_456_789_012_345_678_901_234_567_890_u128);

                let v = n.as_i128(input).expect("fits in i128");
                assert_eq!(v, 123_456_789_012_345_678_901_234_567_890_i128);

                // Negative for i128
                let input_neg = b"-1";
                let evs_neg = collect("-1").unwrap();
                if let [Event::Number(nn)] = &evs_neg[..] {
                    assert_eq!(nn.as_i128(input_neg).unwrap(), -1_i128);
                }
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn jsonnum_as_i128_rejects_overflow() {
        let input = b"1000000000000000000000000000000000000000"; // > i128::MAX
        let evs = collect(core::str::from_utf8(input).unwrap()).unwrap();
        match &evs[..] {
            [Event::Number(n)] => {
                let r = n.as_i128(input);
                assert!(r.is_err());
                let r = n.as_u128(input);
                assert!(r.is_err());
            }
            _ => panic!("{evs:?}"),
        }
    }

    #[test]
    fn lexer_offset_advances() {
        // Drive the lexer through some bytes and check `offset()` reflects
        // the cursor position.
        let mut p: Parser<'_> = Parser::new(b"  null");
        let lex = p.lexer();
        assert_eq!(lex.offset(), 0);
        lex.skip_whitespace();
        assert_eq!(lex.offset(), 2);
    }

    #[test]
    fn parser_input_and_parse_str_value() {
        // `Parser::input` returns the original byte slice.
        let buf = br#""hi""#;
        let p: Parser<'_> = Parser::new(buf);
        assert_eq!(p.input(), buf as &[u8]);

        // `Parser::parse_str_value` — typed fast path for strings.
        let mut p: Parser<'_> = Parser::new(br#""hello""#);
        let s = p.parse_str_value().expect("parse");
        assert_eq!(s, "hello");
    }

    // -----------------------------------------------------------------
    // Coverage tests for the streaming Parser API. The typed `bourne`
    // crate drives the Lexer directly, so these `Parser` methods
    // (`array_start`, `array_continue`, `object_first_key`,
    // `object_next_key`, plus the `_lex` JsonStr-returning variants for
    // escape-bearing keys) need their own coverage. The `_lex` variants
    // are the only path for callers who need escape support in keys.
    // -----------------------------------------------------------------

    // The typed `Parser::array_start` / `array_continue` etc. compose
    // with the typed `parse_*_value` fast paths, NOT with `next_event`
    // (which is the alternative streaming API). Tests use the typed flow.

    #[test]
    fn parser_array_start_empty_and_nonempty() {
        // Empty array
        let mut p: Parser<'_> = Parser::new(b"[]");
        assert!(p.array_start().expect("ok"), "expected empty");
        assert!(p.next_event().unwrap().is_none(), "post-close is doc-end");

        // Non-empty array
        let mut p: Parser<'_> = Parser::new(b"[1,2,3]");
        assert!(!p.array_start().expect("ok"), "expected non-empty");
        assert_eq!(p.parse_i64_value().unwrap(), 1);
    }

    #[test]
    fn parser_array_continue_closes_and_advances() {
        let input = b"[1,2]";
        let mut p: Parser<'_> = Parser::new(input);
        p.array_start().unwrap();
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(!p.array_continue(b']').unwrap(), "expected another element");
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        assert!(p.array_continue(b']').unwrap(), "expected close");
    }

    #[test]
    fn parser_array_inside_array_state_restore() {
        // Array inside array — the post-close state must be ArrayCommaOrEnd.
        let input = br"[[1],[2]]";
        let mut p: Parser<'_> = Parser::new(input);
        p.array_start().unwrap();
        p.array_start().unwrap();
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        p.array_continue(b']').unwrap(); // close inner
        // Outer not yet closed — array_continue with end_byte `]` should
        // return false (another inner array follows after a comma).
        assert!(!p.array_continue(b']').unwrap(), "expected another inner");
        p.array_start().unwrap();
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        p.array_continue(b']').unwrap(); // close inner
        assert!(p.array_continue(b']').unwrap(), "expected outer close");
    }

    #[test]
    fn parser_object_first_and_next_key_loop() {
        let input = br#"{"a":1,"b":2,"c":3}"#;
        let mut p: Parser<'_> = Parser::new(input);
        // Open object via next_event (start of doc), then drive keys.
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        let k1 = p.object_first_key().unwrap();
        assert_eq!(k1, Some("a"));
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        let k2 = p.object_next_key().unwrap();
        assert_eq!(k2, Some("b"));
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        let k3 = p.object_next_key().unwrap();
        assert_eq!(k3, Some("c"));
        assert_eq!(p.parse_i64_value().unwrap(), 3);
        let k4 = p.object_next_key().unwrap();
        assert!(k4.is_none());
    }

    #[test]
    fn parser_object_first_key_empty_object() {
        let mut p: Parser<'_> = Parser::new(b"{}");
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        let k = p.object_first_key().unwrap();
        assert!(k.is_none(), "empty object has no first key");
    }

    /// `object_first_key_lex` / `object_next_key_lex` — the JsonStr-returning
    /// variants used for escape-bearing keys.
    #[test]
    fn parser_object_first_and_next_key_lex_with_escapes() {
        let input = br#"{"esc\nkey":1,"plain":2}"#;
        let mut p: Parser<'_> = Parser::new(input);
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        let k1 = p
            .object_first_key_lex()
            .unwrap()
            .expect("first key present");
        assert!(k1.has_escapes());
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        let k2 = p.object_next_key_lex().unwrap().expect("second key");
        assert!(!k2.has_escapes());
        assert_eq!(k2.as_str(input), Some("plain"));
        assert_eq!(p.parse_i64_value().unwrap(), 2);
        assert!(p.object_next_key_lex().unwrap().is_none());
    }

    /// Empty object via the `_lex` variant — `first_key_lex` must return None.
    #[test]
    fn parser_object_first_key_lex_empty() {
        let mut p: Parser<'_> = Parser::new(b"{}");
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        assert!(p.object_first_key_lex().unwrap().is_none());
    }

    /// Object inside object — verify post-close state restore for the
    /// non-empty case in `object_first_key`.
    #[test]
    fn parser_nested_object_state_restore() {
        let input = br#"{"a":{"b":1},"c":2}"#;
        let mut p: Parser<'_> = Parser::new(input);
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        let k1 = p.object_first_key().unwrap();
        assert_eq!(k1, Some("a"));
        // Inner object
        assert!(matches!(p.next_event().unwrap(), Some(Event::StartObject)));
        let k_inner = p.object_first_key().unwrap();
        assert_eq!(k_inner, Some("b"));
        assert_eq!(p.parse_i64_value().unwrap(), 1);
        assert!(p.object_next_key().unwrap().is_none()); // close inner
        // Continue outer
        let k2 = p.object_next_key().unwrap();
        assert_eq!(k2, Some("c"));
    }

    // -----------------------------------------------------------------
    // ErrorKind / Error / Position Display impls — used only via
    // `format!` / `println!` from user code, so they had 0% coverage.
    // -----------------------------------------------------------------

    #[test]
    fn errorkind_display_covers_every_variant() {
        use alloc::format;
        // Every variant of the Display match must produce non-empty text.
        let cases = [
            ErrorKind::UnexpectedByte(b'?'),
            ErrorKind::UnexpectedEof,
            ErrorKind::InvalidEscape,
            ErrorKind::InvalidUnicodeEscape,
            ErrorKind::UnpairedSurrogate,
            ErrorKind::InvalidUtf8,
            ErrorKind::InvalidNumber,
            ErrorKind::NumberOutOfRange,
            ErrorKind::ControlCharInString,
            ErrorKind::TrailingData,
            ErrorKind::DepthLimitExceeded,
            ErrorKind::ExpectedValue,
            ErrorKind::ExpectedString,
            ErrorKind::ExpectedNumber,
            ErrorKind::ExpectedBool,
            ErrorKind::ExpectedNull,
            ErrorKind::ExpectedArray,
            ErrorKind::ExpectedObject,
            ErrorKind::TypeMismatch,
            ErrorKind::DuplicateKey,
            ErrorKind::MissingField,
            ErrorKind::UnknownField,
            ErrorKind::NonFiniteFloat,
        ];
        for kind in cases {
            let s = format!("{kind}");
            assert!(!s.is_empty(), "empty Display for {kind:?}");
        }
        // The unexpected-byte arm formats the byte as hex.
        assert!(format!("{}", ErrorKind::UnexpectedByte(0x7F)).contains("7f"));
    }

    #[test]
    fn error_display_includes_position() {
        use alloc::format;
        let err = Error::new(ErrorKind::UnexpectedEof, Position::START);
        let s = format!("{err}");
        // Format is "<kind> at <position>", so position separator must appear.
        assert!(s.contains("at"), "got: {s}");
        assert!(s.contains("unexpected end of input"), "got: {s}");
    }

    extern crate alloc;
}
