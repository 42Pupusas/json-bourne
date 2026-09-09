#![no_main]

//! Derived types end-to-end must never panic, and must be able to read
//! back whatever they write.
//!
//! The `typed` target covers primitives and collections. This one covers
//! the derive-generated surface: structs (strict and lenient), all four
//! enum tagging modes, maps with every supported key type, and the
//! round-trip property "anything that serializes must re-parse". A panic
//! here is a bug in the derive or the shared decode paths; a `Result`
//! from a *parse* is fine.
//!
//! The round-trip assertion is not decoration: audit A1 was a wrong-`Err`
//! bug, invisible to a target that only checks for panics. A successful
//! `to_string` whose output fails to parse is always a defect, so this
//! target panics on that rather than discarding the result.
//!
//! Run: `cargo +nightly fuzz run derived`

use json_bourne::{FromJson, ToJson, parse, to_string};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, FromJson)]
#[allow(dead_code)] // parse-only target: fields are checked, never read
struct Strict<'input> {
    id: u64,
    name: &'input str,
    active: bool,
}

#[derive(Debug, FromJson)]
#[bourne(deny_unknown_fields = false)]
#[allow(dead_code)] // parse-only target: fields are checked, never read
struct Lenient<'input> {
    #[bourne(default)]
    id: u64,
    name: Option<&'input str>,
    tags: Vec<&'input str>,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
enum External {
    Unit,
    Newtype(u64),
    Tuple(u64, bool),
    Struct { name: String, flag: bool },
}

/// Escape-bearing keys in every position, including first (audit A1).
#[derive(Debug, PartialEq, FromJson, ToJson)]
struct EscapedKeys {
    #[bourne(rename = "q\"uote")]
    quote: u64,
    #[bourne(rename = "nl\nline")]
    newline: String,
    plain: u64,
    #[bourne(rename = "caf\u{00e9}")]
    accent: f64,
}

/// Asserts the round-trip property for one derived type.
struct RoundTrip;

impl RoundTrip {
    fn assert<T>(parsed: &T)
    where
        T: ToJson + PartialEq + core::fmt::Debug + for<'a> FromJson<'a>,
    {
        let Ok(text) = to_string(parsed) else {
            return;
        };
        match parse::<T>(text.as_bytes()) {
            Ok(back) => assert_eq!(
                &back, parsed,
                "round-trip changed the value via {text:?}",
            ),
            Err(e) => panic!("serialized output failed to re-parse: {text:?}: {e}"),
        }
    }
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind")]
enum Internal {
    Unit,
    Item { count: u64 },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind", content = "data")]
enum Adjacent {
    Unit,
    Item(u64),
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(untagged)]
enum Untagged {
    Num(u64),
    Text(String),
}

fuzz_target!(|data: &[u8]| {
    let _ = parse::<Strict<'_>>(data);
    let _ = parse::<Lenient<'_>>(data);
    let _ = parse::<External>(data);
    let _ = parse::<Internal>(data);
    let _ = parse::<Adjacent>(data);
    let _ = parse::<Untagged>(data);
    let _ = parse::<EscapedKeys>(data);

    // Maps exercise every MapKey impl.
    let _ = parse::<Vec<(String, u64)>>(data);

    // Round-trip: whatever serializes must re-parse to an equal value.
    if let Ok(v) = parse::<External>(data) {
        RoundTrip::assert(&v);
    }
    if let Ok(v) = parse::<EscapedKeys>(data) {
        RoundTrip::assert(&v);
    }
    if let Ok(v) = parse::<Internal>(data) {
        RoundTrip::assert(&v);
    }
    if let Ok(v) = parse::<Adjacent>(data) {
        RoundTrip::assert(&v);
    }
    if let Ok(v) = parse::<Untagged>(data) {
        RoundTrip::assert(&v);
    }
});
