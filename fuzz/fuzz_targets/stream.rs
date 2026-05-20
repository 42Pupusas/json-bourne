#![no_main]

//! Streaming parser must never panic, never UB, never loop forever.
//! libFuzzer's per-input timeout handles the infinite-loop case for free.
//!
//! Run: `cargo +nightly fuzz run stream`

use bourne::Parser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut p: Parser<'_> = Parser::new(data);
    while let Ok(Some(_)) = p.next_event() {}
});
