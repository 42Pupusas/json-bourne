#![no_main]

//! Streaming parser must never panic, never UB, never loop forever.
//! libFuzzer's per-input timeout handles the infinite-loop case for free.
//!
//! Every input gets three passes, each over the cursor + frame-stack
//! bookkeeping the parser depends on:
//!
//! 1. `Parser::next_event` — the grammar state machine walk.
//! 2. `Lexer::skip_value` — the lenient skip walk with its own frame
//!    bookkeeping and escape/surrogate validation rules.
//! 3. `checkpoint`/`restore` interleaved with `read_value` — the
//!    speculative-retry pattern that untagged and adjacent enum dispatch
//!    run under, so a restore that leaves the lexer unable to make
//!    progress (or that corrupts the frame stack) surfaces here.
//!
//! Run: `cargo +nightly fuzz run stream`

use json_bourne::{Lexer, Parser};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut p = Parser::new(data);
    while let Ok(Some(_)) = p.next_event() {}

    let Ok(mut lex) = Lexer::try_new(data) else {
        return;
    };
    let _ = lex.skip_value();

    let Ok(mut lex) = Lexer::try_new(data) else {
        return;
    };
    for _ in 0..64 {
        let cp = lex.checkpoint();
        let _ = lex.read_value();
        lex.restore(cp);
        match lex.read_value() {
            Ok(_) => {}
            Err(_) => break,
        }
    }
});
