//! Pathological-but-legal JSON throughput.
//!
//! Each input is well-formed; we just probe edge shapes the uniform
//! fixtures miss. The wide-int benches are the most important: they
//! cover the regime where SIMD digit scanning starts paying off, which
//! the original `int_array(N)` corpus (1-5 digit ints) cannot reach.

use bourne_bench::pathological::{
    empty_object_array, huge_int_literal, longest_legal_i64_array, max_depth_legal,
    mixed_width_int_array, null_array, wide_int_array,
};
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

mod wide_int_array {
    use super::{drain, parse, wide_int_array};

    // Wide ints — 13-19 digit values. Streaming + typed paths.

    #[divan::bench(args = [1_000, 10_000])]
    fn stream(bencher: divan::Bencher, n: usize) {
        let s = wide_int_array(n);
        bencher
            .counter(divan::counter::BytesCount::new(s.len()))
            .bench(|| drain(divan::black_box(s.as_bytes())));
    }

    #[divan::bench(args = [1_000, 10_000])]
    fn typed_i64(bencher: divan::Bencher, n: usize) {
        let s = wide_int_array(n);
        bencher
            .counter(divan::counter::BytesCount::new(s.len()))
            .bench(|| {
                let v: Vec<i64> = parse(divan::black_box(s.as_bytes())).unwrap();
                divan::black_box(v);
            });
    }

    // Head-to-head: same input bytes, same output type. The reason this
    // category exists at all is that the original int corpus had no wide
    // values; a fair-fight number is what tells us whether `parse_i64_value`'s
    // 18-digit-fast / 20-digit-checked split lands.
    #[divan::bench(args = [1_000, 10_000])]
    fn typed_i64_serde_json(bencher: divan::Bencher, n: usize) {
        let s = wide_int_array(n);
        bencher
            .counter(divan::counter::BytesCount::new(s.len()))
            .bench(|| {
                let v: Vec<i64> = serde_json::from_slice(divan::black_box(s.as_bytes())).unwrap();
                divan::black_box(v);
            });
    }
}

// Mixed-width ints — same array contains 1, 10, 19-digit values.
#[divan::bench]
fn mixed_width_int_array_typed_i64(bencher: divan::Bencher) {
    let mixed = mixed_width_int_array(3000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed.len()))
        .bench(|| {
            let v: Vec<i64> = parse(divan::black_box(mixed.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Single huge integer literal — 1000 digits. Tests `read_number`'s
// digit walk in the worst case. Streaming only (typed `i64` would
// reject as out-of-range, which is itself a separate measurement).
#[divan::bench]
fn huge_int_literal_1000_stream(bencher: divan::Bencher) {
    let huge = huge_int_literal(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(huge.len()))
        .bench(|| drain(divan::black_box(huge.as_bytes())));
}

// Head-to-head: serde_json's `arbitrary_precision` feature is on for
// this bench crate, so `Value` accepts the 1000-digit literal and
// stores it as a `Number` containing the original text. Bourne stores
// the same text via `JsonNum`'s offset span — same logical info, both
// libraries parse the same bytes; throughput is comparable.
//
// Worth noting: by default (without `arbitrary_precision`) serde_json
// rejects numbers wider than i64/u64/f64 with `Error("number out of
// range")`. Bourne always accepts; consumers decide whether to
// decode to a primitive. Different defaults, different trade-offs.
#[divan::bench]
fn huge_int_literal_1000_serde_json_value(bencher: divan::Bencher) {
    let huge = huge_int_literal(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(huge.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(huge.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Empty objects — pure dispatch, no value content.
#[divan::bench]
fn empty_object_array_10000_stream(bencher: divan::Bencher) {
    let empties = empty_object_array(10_000);
    bencher
        .counter(divan::counter::BytesCount::new(empties.len()))
        .bench(|| drain(divan::black_box(empties.as_bytes())));
}

// Null array — even cheaper than empty objects (no frame push/pop).
#[divan::bench]
fn null_array_10000_stream(bencher: divan::Bencher) {
    let nulls = null_array(10_000);
    bencher
        .counter(divan::counter::BytesCount::new(nulls.len()))
        .bench(|| drain(divan::black_box(nulls.as_bytes())));
}

// Maximum legal depth — `[[...[1]...]]` 128 levels deep, one off the limit.
#[divan::bench]
fn max_depth_legal_stream(bencher: divan::Bencher) {
    let deep = max_depth_legal();
    bencher
        .counter(divan::counter::BytesCount::new(deep.len()))
        .bench(|| drain(divan::black_box(deep.as_bytes())));
}

// Longest legal i64 (i64::MIN as text). Forces the slow-path
// overflow-checked accumulator in parse_i64_value.
#[divan::bench]
fn longest_legal_i64_typed_i64(bencher: divan::Bencher) {
    let min_i64 = longest_legal_i64_array();
    bencher.bench(|| {
        let v: Vec<i64> = parse(divan::black_box(min_i64.as_bytes())).unwrap();
        divan::black_box(v);
    });
}
