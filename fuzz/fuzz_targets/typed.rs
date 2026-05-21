#![no_main]

//! Typed parse into a representative shape must never panic.
//!
//! Run: `cargo +nightly fuzz run typed`

use json_bourne::parse;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // A handful of typed parses against the same input. Any panic is a bug;
    // any Result is fine. Aggregating multiple types per input multiplies
    // fuzz coverage without multiplying corpus size.
    let _ = parse::<i64>(data);
    let _ = parse::<f64>(data);
    let _ = parse::<&str>(data);
    let _ = parse::<bool>(data);
    let _ = parse::<Option<i64>>(data);
    let _ = parse::<Vec<i64>>(data);
    let _ = parse::<Vec<&str>>(data);
    let _ = parse::<(i64, &str, bool)>(data);
});
