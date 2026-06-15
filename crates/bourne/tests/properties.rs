//! Property-based tests for parser invariants.
//!
//! Fuzzing finds crashes; properties find *behavioural* bugs — the parser
//! disagreeing with the spec in a way that doesn't panic.

#![cfg(feature = "std")]

use json_bourne::Parser;
use json_bourne::{parse_str, to_string};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Round-trip properties
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn i64_round_trips(x: i64) {
        let s = x.to_string();
        let parsed: i64 = parse_str(&s).expect("formatted i64 must parse");
        prop_assert_eq!(parsed, x);
    }

    #[test]
    fn u64_round_trips(x: u64) {
        let s = x.to_string();
        let parsed: u64 = parse_str(&s).expect("formatted u64 must parse");
        prop_assert_eq!(parsed, x);
    }

    #[test]
    fn finite_f64_round_trips(x in proptest::num::f64::NORMAL | proptest::num::f64::POSITIVE | proptest::num::f64::NEGATIVE | proptest::num::f64::ZERO) {
        // f64::to_string produces a shortest-roundtrip representation, so
        // exact equality is the right assertion (not approximate).
        let s = x.to_string();
        let parsed: f64 = parse_str(&s).expect("formatted finite f64 must parse");
        prop_assert_eq!(parsed.to_bits(), x.to_bits());
    }

    /// Stronger property: every finite f64 that survives `json_bourne::to_string`
    /// must parse back bit-identically. This exercises the in-tree
    /// Grisu3 path *and* its libstd fallback — the failure mode it
    /// catches is "Grisu3 emitted shorter-than-shortest output that
    /// rounds wrong on parse-back". The previous property went through
    /// `f64::to_string` (libstd's ryu/dragon4), which would mask any
    /// Grisu3 regression.
    #[test]
    fn bourne_serialized_finite_f64_round_trips(
        x in proptest::num::f64::NORMAL | proptest::num::f64::POSITIVE | proptest::num::f64::NEGATIVE | proptest::num::f64::ZERO,
    ) {
        let s = to_string(&x).expect("json-bourne serializes finite f64");
        let parsed: f64 = parse_str(&s).expect("json-bourne output parses back");
        prop_assert_eq!(
            parsed.to_bits(), x.to_bits(),
            "round-trip failed via {:?} for f={:e} (bits=0x{:016x})",
            s, x, x.to_bits(),
        );
    }
}

// ---------------------------------------------------------------------------
// Robustness properties — for ANY input, parser must not panic and any
// reported error position must lie within the input.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn streaming_never_panics(bytes: Vec<u8>) {
        let mut p: Parser<'_> = Parser::new(&bytes);
        for _ in 0..10_000 {
            match p.next_event() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
    }

    #[test]
    fn error_position_in_bounds(bytes: Vec<u8>) {
        let mut p: Parser<'_> = Parser::new(&bytes);
        loop {
            match p.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    prop_assert!(
                        (e.position.offset as usize) <= bytes.len(),
                        "error position {} out of bounds for input of {} bytes",
                        e.position.offset, bytes.len(),
                    );
                    break;
                }
            }
        }
    }

    #[test]
    fn typed_parse_never_panics(bytes: Vec<u8>) {
        let _ = parse_str::<i64>(&String::from_utf8_lossy(&bytes));
        let _ = parse_str::<f64>(&String::from_utf8_lossy(&bytes));
        let _ = parse_str::<bool>(&String::from_utf8_lossy(&bytes));
    }
}

// ---------------------------------------------------------------------------
// Constructive property: synthesised valid JSON must always parse.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum JsonValue {
    Null,
    Bool(bool),
    Int(i32),
    Str(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    fn render(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(n) => out.push_str(&n.to_string()),
            Self::Str(s) => render_string(s, out),
            Self::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.render(out);
                }
                out.push(']');
            }
            Self::Object(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    render_string(k, out);
                    out.push(':');
                    v.render(out);
                }
                out.push('}');
            }
        }
    }
}

fn render_string(s: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn arb_json() -> impl Strategy<Value = JsonValue> {
    let leaf = prop_oneof![
        Just(JsonValue::Null),
        any::<bool>().prop_map(JsonValue::Bool),
        any::<i32>().prop_map(JsonValue::Int),
        // Restrict strings to BMP non-control chars to keep generation fast;
        // the conformance corpus + fuzzing exercises the harder cases.
        "[a-zA-Z0-9 ]{0,10}".prop_map(JsonValue::Str),
    ];
    leaf.prop_recursive(
        4,  // up to 4 levels
        32, // max ~32 nodes total
        6,  // each container has up to 6 children
        |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(JsonValue::Array),
                prop::collection::vec(("[a-zA-Z]{1,6}", inner), 0..6).prop_map(JsonValue::Object),
            ]
        },
    )
}

proptest! {
    #[test]
    fn synthesised_json_parses(v in arb_json()) {
        let mut s = String::new();
        v.render(&mut s);
        let mut p: Parser<'_> = Parser::new(s.as_bytes());
        let outcome = loop {
            match p.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        prop_assert!(
            outcome.is_ok(),
            "synthesised JSON failed to parse: {:?}\ninput: {s}",
            outcome.err(),
        );
    }
}
