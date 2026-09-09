//! Property-based tests for parser invariants.
//!
//! Fuzzing finds crashes; properties find *behavioural* bugs — the parser
//! disagreeing with the spec in a way that doesn't panic.

#![cfg(feature = "std")]

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use json_bourne::{ErrorKind, Parser, parse_str, to_string};
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
    /// teju-jagua formatter — the failure mode it catches is "teju
    /// emitted shorter-than-shortest output that rounds wrong on
    /// parse-back". The previous property went through `f64::to_string`
    /// (libstd's ryu/dragon4), which would mask any regression in our
    /// own formatter.
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
// String round-trip properties (audit 2026-09 F5.1).
//
// Escape handling is where the last two live bugs lived (the variant
// `skip_if_none` divergence and the `char` scratch overflow), so this is
// the property the audit asked for by name. The synthesised-JSON
// generator below deliberately restricts strings to simple ASCII — it
// keeps shrink tractable — so these properties are the ones actually
// leaning on the harder cases.
// ---------------------------------------------------------------------------

/// Strings drawn from every escape-relevant character class: the quote
/// and backslash fast paths, C0 controls (`\u00XX` output), DEL, 2- and
/// 3-byte non-ASCII scalars, and 4-byte non-BMP scalars.
fn arb_escape_string() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            1 => Just('"'),
            1 => Just('\\'),
            1 => Just('\u{7f}'),
            2 => proptest::char::range('\u{0}', '\u{1f}'),
            2 => proptest::char::range('\u{80}', '\u{7ff}'),
            2 => proptest::char::range('\u{800}', '\u{ffff}'),
            2 => proptest::char::range('\u{10000}', '\u{10ffff}'),
            6 => proptest::char::any(),
        ],
        0..32,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    /// bourne's serializer output for any `&str` must parse back to the
    /// bit-exact string — via the borrowed path when there is nothing to
    /// decode and the escape-decoding path otherwise. Going through
    /// `to_string` (not `format!("{:?}")`) keeps the serializer itself
    /// under test.
    #[test]
    fn bourne_serialized_str_round_trips(s in arb_escape_string()) {
        let json = to_string(&s).expect("serializing a &str cannot fail");
        let parsed: Cow<'_, str> = parse_str(&json).expect("bourne string output parses back");
        prop_assert_eq!(&*parsed, s.as_str(), "round-trip failed via {:?}", json);
    }

    /// The same escape machinery reached through the map-key path: a key
    /// that required escapes must survive object parsing and re-key the
    /// value correctly.
    #[test]
    fn bourne_serialized_map_key_round_trips(
        k in arb_escape_string(),
        v in any::<u64>(),
    ) {
        let m: BTreeMap<String, u64> = std::iter::once((k.clone(), v)).collect();
        let json = to_string(&m).expect("serializing a BTreeMap cannot fail");
        let back: BTreeMap<String, u64> = parse_str(&json).expect("bourne map output parses back");
        prop_assert_eq!(back.get(&k), Some(&v), "round-trip failed via {:?}", json);
    }
}

// ---------------------------------------------------------------------------
// Derive-side escaped-key round-trip (audit A1).
//
// The map-key property above covers the `BTreeMap` path, which was never
// broken. The derive path had its own key walk, where the first key
// skipped the decode retry — so bourne's own output failed to re-parse
// whenever the escape-bearing field came first. A derived key is fixed at
// expansion time, so instead of generating the key we generate the
// *document order*: whichever field leads must parse the same.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, json_bourne::FromJson, json_bourne::ToJson)]
struct EscapedKeys {
    #[bourne(rename = "q\"uote")]
    quote: u64,
    #[bourne(rename = "nl\nline")]
    newline: u64,
    plain: u64,
    #[bourne(rename = "caf\u{00e9}")]
    accent: u64,
}

impl EscapedKeys {
    const KEYS: [&'static str; 4] = ["q\"uote", "nl\nline", "plain", "caf\u{00e9}"];

    fn from_order(order: &[usize; 4]) -> (Self, String) {
        let values = [11_u64, 22, 33, 44];
        let mut json = String::from("{");
        for (n, &idx) in order.iter().enumerate() {
            if n > 0 {
                json.push(',');
            }
            json.push_str(&to_string(&Self::KEYS[idx]).expect("key serializes"));
            json.push(':');
            json.push_str(&values[idx].to_string());
        }
        json.push('}');
        let expected = Self {
            quote: values[0],
            newline: values[1],
            plain: values[2],
            accent: values[3],
        };
        (expected, json)
    }
}

proptest! {
    /// Any permutation of the same object's keys must parse identically —
    /// position must never decide whether an escaped key is accepted.
    #[test]
    fn derived_escaped_keys_parse_in_any_order(
        order in Just([0_usize, 1, 2, 3]).prop_shuffle(),
    ) {
        let order: [usize; 4] = order.as_slice().try_into().expect("shuffle preserves length");
        let (expected, json) = EscapedKeys::from_order(&order);
        let parsed: EscapedKeys = parse_str(&json)
            .unwrap_or_else(|e| panic!("derived reader rejected {json:?}: {e}"));
        prop_assert_eq!(parsed, expected, "round-trip failed via {:?}", json);
    }
}

/// The property's fixed point: bourne's own serializer output for a type
/// whose escape-bearing field is declared *first* must re-parse.
#[test]
fn derived_escaped_key_output_reparses() {
    let original = EscapedKeys {
        quote: 1,
        newline: 2,
        plain: 3,
        accent: 4,
    };
    let json = to_string(&original).expect("serializing cannot fail");
    let back: EscapedKeys = parse_str(&json)
        .unwrap_or_else(|e| panic!("bourne emitted JSON it cannot read back: {json:?}: {e}"));
    assert_eq!(back, original);
}

// ---------------------------------------------------------------------------
// Wide-integer properties (audit 2026-09 F5.2). The table tests pin the
// i64/u64 boundaries exhaustively; these do the same job for i128/u128,
// whose fused paths are separate code.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn i128_round_trips(
        x in prop_oneof![
            1 => Just(i128::MIN),
            1 => Just(i128::MAX),
            1 => Just(i128::from(i64::MIN) - 1),
            1 => Just(i128::from(i64::MAX) + 1),
            6 => any::<i128>(),
        ],
    ) {
        let s = to_string(&x).expect("serializing an i128 cannot fail");
        let parsed: i128 = parse_str(&s).expect("formatted i128 must parse");
        prop_assert_eq!(parsed, x);
    }

    #[test]
    fn u128_round_trips(
        x in prop_oneof![
            1 => Just(u128::MAX),
            1 => Just(u128::from(u64::MAX) + 1),
            6 => any::<u128>(),
        ],
    ) {
        let s = to_string(&x).expect("serializing a u128 cannot fail");
        let parsed: u128 = parse_str(&s).expect("formatted u128 must parse");
        prop_assert_eq!(parsed, x);
    }
}

// ---------------------------------------------------------------------------
// Map duplicate-key property (audit 2026-09 F5.3). Map FromJson impls own
// their own duplicate detection (the DupMap path), separate from the
// derive's; any synthesised object with a repeated key must be a
// DuplicateKey error rather than a silent last-wins overwrite, in both
// map flavours.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn map_duplicate_keys_error(
        key in "[a-zA-Z]{1,6}",
        a in any::<u64>(),
        b in any::<u64>(),
    ) {
        // Equal values would make a silent overwrite indistinguishable
        // from correct rejection, so keep them apart.
        prop_assume!(a != b);
        let json = format!(r#"{{"{key}":{a},"{key}":{b}}}"#);

        let err = parse_str::<BTreeMap<String, u64>>(&json)
            .expect_err("duplicate key must error in BTreeMap");
        prop_assert_eq!(err.kind, ErrorKind::DuplicateKey);

        let err = parse_str::<HashMap<String, u64>>(&json)
            .expect_err("duplicate key must error in HashMap");
        prop_assert_eq!(err.kind, ErrorKind::DuplicateKey);
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
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
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
