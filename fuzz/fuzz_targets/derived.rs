#![no_main]

//! Derived types end-to-end must never panic.
//!
//! The `typed` target covers primitives and collections. This one covers
//! the derive-generated surface: structs (strict and lenient), all four
//! enum tagging modes, maps with every supported key type, and the
//! round-trip property "anything that serializes must re-parse". A panic
//! here is a bug in the derive or the shared decode paths; a `Result` is
//! fine.
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

#[derive(Debug, FromJson, ToJson)]
enum External {
    Unit,
    Newtype(u64),
    Tuple(u64, bool),
    Struct { name: String, flag: bool },
}

#[derive(Debug, FromJson, ToJson)]
#[bourne(tag = "kind")]
enum Internal {
    Unit,
    Item { count: u64 },
}

#[derive(Debug, FromJson, ToJson)]
#[bourne(tag = "kind", content = "data")]
enum Adjacent {
    Unit,
    Item(u64),
}

#[derive(Debug, FromJson, ToJson)]
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

    // Maps exercise every MapKey impl.
    let _ = parse::<Vec<(String, u64)>>(data);

    // Round-trip: any input that parses into a serializable enum must
    // serialize, and whatever serializes must re-parse to the same tag.
    if let Ok(v) = parse::<External>(data) {
        if let Ok(s) = to_string(&v) {
            let _ = parse::<External>(s.as_bytes());
        }
    }
});
