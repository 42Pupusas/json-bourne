//! `decode_escapes` — full branch coverage. The match arms for `\b`,
//! `\f`, `\/`, and the various surrogate-pair error paths.

use crate::parse_str;
use alloc::string::String;

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
