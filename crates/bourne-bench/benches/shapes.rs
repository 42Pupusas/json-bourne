//! Per-shape micro-benches: numbers, escaped strings, deep nesting.
//!
//! These exist to catch perf regressions in specific lexer paths. If the
//! number-parsing fast path slows down by 10%, this is where we'll see it
//! before the broader stream/typed benches surface it.

use bourne::parse;
use bourne_bench::{deep_nesting, escaped_string_array};
use bourne_core::Parser;
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

fn bench_shapes(c: &mut Criterion) {
    let mut group = c.benchmark_group("shapes");

    // Number lexer hot path — pure integer parsing, no object overhead.
    let ints = "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20]".repeat(1000);
    group.throughput(Throughput::Bytes(ints.len() as u64));
    group.bench_function("ints_lex", |b| b.iter(|| drain(black_box(ints.as_bytes()))));

    // Floats: fraction + exponent — slowest number path.
    let floats = "[1.5e10,-2.7e-5,3.14159,0.0,1e100]".repeat(1000);
    group.throughput(Throughput::Bytes(floats.len() as u64));
    group.bench_function("floats_lex", |b| b.iter(|| drain(black_box(floats.as_bytes()))));

    // Floats decoded via typed parse — pays for f64::from_str.
    group.bench_function("floats_decode", |b| {
        b.iter(|| {
            let v: Vec<f64> = parse(black_box(floats.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Escaped strings — exercises the validate_escapes path.
    let escaped = escaped_string_array(1000);
    group.throughput(Throughput::Bytes(escaped.len() as u64));
    group.bench_function("escaped_strings_lex", |b| {
        b.iter(|| drain(black_box(escaped.as_bytes())));
    });

    // Deep nesting — pure stack push/pop, no values.
    for &depth in &[16usize, 64, 128] {
        let s = deep_nesting(depth);
        group.throughput(Throughput::Bytes(s.len() as u64));
        group.bench_function(format!("nesting/{depth}"), |b| {
            b.iter(|| drain(black_box(s.as_bytes())));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_shapes);
criterion_main!(benches);
