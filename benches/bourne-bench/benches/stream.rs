//! Streaming-only throughput: `Parser::next_event` until exhausted, no decoding.
//!
//! This is the floor — anything the typed layer adds shows up as the delta
//! between this bench and the equivalent `typed` bench.

use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use json_bourne::Parser;

fn main() {
    divan::main();
}

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        divan::black_box(ev);
    }
}

#[divan::bench]
fn small_object(bencher: divan::Bencher) {
    let small = SMALL_OBJECT.as_bytes();
    bencher
        .counter(divan::counter::BytesCount::new(small.len()))
        .bench(|| {
            drain(divan::black_box(small));
        });
}

#[divan::bench(args = [10, 1_000, 100_000])]
fn int_array_bench(bencher: divan::Bencher, n: usize) {
    let s = int_array(n);
    bencher
        .counter(divan::counter::BytesCount::new(s.len()))
        .bench(|| {
            drain(divan::black_box(s.as_bytes()));
        });
}

#[divan::bench(args = [10, 1_000, 100_000])]
fn string_array_bench(bencher: divan::Bencher, n: usize) {
    let s = string_array(n);
    bencher
        .counter(divan::counter::BytesCount::new(s.len()))
        .bench(|| {
            drain(divan::black_box(s.as_bytes()));
        });
}
