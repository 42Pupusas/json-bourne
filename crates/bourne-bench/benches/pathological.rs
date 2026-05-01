//! Pathological-but-legal JSON throughput.
//!
//! Each input is well-formed; we just probe edge shapes the uniform
//! fixtures miss. The wide-int benches are the most important: they
//! cover the regime where SIMD digit scanning starts paying off, which
//! the original `int_array(N)` corpus (1-5 digit ints) cannot reach.

use bourne::parse;
use bourne_bench::pathological::{
    empty_object_array, huge_int_literal, longest_legal_i64_array, max_depth_legal,
    mixed_width_int_array, null_array, wide_int_array,
};
use bourne_core::Parser;
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

fn bench_pathological(c: &mut Criterion) {
    let mut group = c.benchmark_group("pathological");

    // Wide ints — 13-19 digit values. Streaming + typed paths.
    for &n in &[1_000usize, 10_000] {
        let s = wide_int_array(n);
        group.throughput(Throughput::Bytes(s.len() as u64));
        group.bench_function(format!("wide_int_array/{n}/stream"), |b| {
            b.iter(|| drain(black_box(s.as_bytes())));
        });
        group.bench_function(format!("wide_int_array/{n}/typed_i64"), |b| {
            b.iter(|| {
                let v: Vec<i64> = parse(black_box(s.as_bytes())).unwrap();
                black_box(v);
            });
        });
    }

    // Mixed-width ints — same array contains 1, 10, 19-digit values.
    let mixed = mixed_width_int_array(3000);
    group.throughput(Throughput::Bytes(mixed.len() as u64));
    group.bench_function("mixed_width_int_array/typed_i64", |b| {
        b.iter(|| {
            let v: Vec<i64> = parse(black_box(mixed.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Single huge integer literal — 1000 digits. Tests `read_number`'s
    // digit walk in the worst case. Streaming only (typed `i64` would
    // reject as out-of-range, which is itself a separate measurement).
    let huge = huge_int_literal(1_000);
    group.throughput(Throughput::Bytes(huge.len() as u64));
    group.bench_function("huge_int_literal/1000/stream", |b| {
        b.iter(|| drain(black_box(huge.as_bytes())));
    });

    // Empty objects — pure dispatch, no value content.
    let empties = empty_object_array(10_000);
    group.throughput(Throughput::Bytes(empties.len() as u64));
    group.bench_function("empty_object_array/10000/stream", |b| {
        b.iter(|| drain(black_box(empties.as_bytes())));
    });

    // Null array — even cheaper than empty objects (no frame push/pop).
    let nulls = null_array(10_000);
    group.throughput(Throughput::Bytes(nulls.len() as u64));
    group.bench_function("null_array/10000/stream", |b| {
        b.iter(|| drain(black_box(nulls.as_bytes())));
    });

    // Maximum legal depth — `[[…[1]…]]` 128 levels deep, one off the limit.
    let deep = max_depth_legal();
    group.throughput(Throughput::Bytes(deep.len() as u64));
    group.bench_function("max_depth_legal/stream", |b| {
        b.iter(|| drain(black_box(deep.as_bytes())));
    });

    // Longest legal i64 (i64::MIN as text). Forces the slow-path
    // overflow-checked accumulator in parse_i64_value.
    let min_i64 = longest_legal_i64_array();
    group.bench_function("longest_legal_i64/typed_i64", |b| {
        b.iter(|| {
            let v: Vec<i64> = parse(black_box(min_i64.as_bytes())).unwrap();
            black_box(v);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_pathological);
criterion_main!(benches);
