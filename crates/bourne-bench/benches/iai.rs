//! Instruction-count benchmarks via iai-callgrind.
//!
//! Complements `compare.rs` (criterion, wall-clock). Where criterion measures
//! "how fast on this machine right now," iai counts retired instructions
//! deterministically — same input always produces the same number, so a CI
//! regression of even 1% is a real signal instead of measurement noise.
//!
//! Run:
//!   cargo bench -p bourne-bench --bench iai
//!
//! Requires `valgrind` installed on the host machine.

use bourne::parse;
use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use bourne_core::Parser;
use iai_callgrind::{black_box, library_benchmark, library_benchmark_group, main};
use std::sync::OnceLock;

// Inputs are built once and held in a static. iai measures only the body of
// the bench function; pre-building the input keeps construction cost out of
// the count.
const fn small_object() -> &'static [u8] {
    SMALL_OBJECT.as_bytes()
}

fn ints_10k() -> &'static [u8] {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| int_array(10_000)).as_bytes()
}

fn strings_10k() -> &'static [u8] {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| string_array(10_000)).as_bytes()
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

#[library_benchmark]
fn stream_small() {
    drain(black_box(small_object()));
}

#[library_benchmark]
fn stream_ints_10k() {
    drain(black_box(ints_10k()));
}

#[library_benchmark]
fn stream_strings_10k() {
    drain(black_box(strings_10k()));
}

// ---------------------------------------------------------------------------
// Typed
// ---------------------------------------------------------------------------

#[library_benchmark]
fn typed_vec_i64_10k() {
    let v: Vec<i64> = parse(black_box(ints_10k())).unwrap();
    black_box(v);
}

#[library_benchmark]
fn typed_vec_borrowed_str_10k() {
    let v: Vec<&str> = parse(black_box(strings_10k())).unwrap();
    black_box(v);
}

#[library_benchmark]
fn typed_vec_string_10k() {
    let v: Vec<String> = parse(black_box(strings_10k())).unwrap();
    black_box(v);
}

library_benchmark_group!(
    name = streaming;
    benchmarks = stream_small, stream_ints_10k, stream_strings_10k
);

library_benchmark_group!(
    name = typed;
    benchmarks = typed_vec_i64_10k, typed_vec_borrowed_str_10k, typed_vec_string_10k
);

main!(library_benchmark_groups = streaming, typed);
