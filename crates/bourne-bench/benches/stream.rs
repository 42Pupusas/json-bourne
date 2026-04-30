//! Streaming-only throughput: `Parser::next_event` until exhausted, no decoding.
//!
//! This is the floor — anything the typed layer adds shows up as the delta
//! between this bench and the equivalent `typed` bench.

use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use bourne_core::Parser;
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

fn bench_stream(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream");

    let small = SMALL_OBJECT.as_bytes();
    group.throughput(Throughput::Bytes(small.len() as u64));
    group.bench_function("small_object", |b| b.iter(|| drain(black_box(small))));

    for &n in &[10usize, 1_000, 100_000] {
        let s = int_array(n);
        group.throughput(Throughput::Bytes(s.len() as u64));
        group.bench_function(format!("int_array/{n}"), |b| {
            b.iter(|| drain(black_box(s.as_bytes())));
        });
    }

    for &n in &[10usize, 1_000, 100_000] {
        let s = string_array(n);
        group.throughput(Throughput::Bytes(s.len() as u64));
        group.bench_function(format!("string_array/{n}"), |b| {
            b.iter(|| drain(black_box(s.as_bytes())));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_stream);
criterion_main!(benches);
