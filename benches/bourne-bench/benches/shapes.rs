//! Per-shape micro-benches: numbers, escaped strings, deep nesting.
//!
//! These exist to catch perf regressions in specific lexer paths. If the
//! number-parsing fast path slows down by 10%, this is where we'll see it
//! before the broader stream/typed benches surface it.

use bourne_bench::{deep_nesting, escaped_string_array, float_array, int_array};
use json_bourne::Parser;
use json_bourne::parse;

fn main() {
    divan::main();
}

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        divan::black_box(ev);
    }
}

// Number lexer hot path — pure integer parsing, no object overhead.
// 20k ints in one valid array (was previously 1000 concatenated arrays
// which produced trailing-data errors).
#[divan::bench]
fn ints_lex(bencher: divan::Bencher) {
    let ints = int_array(20_000);
    bencher
        .counter(divan::counter::BytesCount::new(ints.len()))
        .bench(|| {
            drain(divan::black_box(ints.as_bytes()));
        });
}

// Floats: fraction + exponent — slowest number path. 5000 mixed-form
// floats in a single valid array.
#[divan::bench]
fn floats_lex(bencher: divan::Bencher) {
    let floats = float_array(5_000);
    bencher
        .counter(divan::counter::BytesCount::new(floats.len()))
        .bench(|| {
            drain(divan::black_box(floats.as_bytes()));
        });
}

// Floats decoded via typed parse — pays for f64::from_str.
#[divan::bench]
fn floats_decode(bencher: divan::Bencher) {
    let floats = float_array(5_000);
    bencher
        .counter(divan::counter::BytesCount::new(floats.len()))
        .bench(|| {
            let v: Vec<f64> = parse(divan::black_box(floats.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Plain decimal floats decoded via typed parse — the fast path's target
// shape: no exponent, short fraction, exactly representable mantissa.
#[divan::bench]
fn plain_floats_decode(bencher: divan::Bencher) {
    let floats = bourne_bench::plain_float_array(5_000);
    bencher
        .counter(divan::counter::BytesCount::new(floats.len()))
        .bench(|| {
            let v: Vec<f64> = parse(divan::black_box(floats.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Escaped strings — exercises the validate_escapes path.
#[divan::bench]
fn escaped_strings_lex(bencher: divan::Bencher) {
    let escaped = escaped_string_array(1000);
    bencher
        .counter(divan::counter::BytesCount::new(escaped.len()))
        .bench(|| {
            drain(divan::black_box(escaped.as_bytes()));
        });
}

// Deep nesting — pure stack push/pop, no values.
#[divan::bench(args = [16, 64, 128])]
fn nesting(bencher: divan::Bencher, depth: usize) {
    let s = deep_nesting(depth);
    bencher
        .counter(divan::counter::BytesCount::new(s.len()))
        .bench(|| {
            drain(divan::black_box(s.as_bytes()));
        });
}
