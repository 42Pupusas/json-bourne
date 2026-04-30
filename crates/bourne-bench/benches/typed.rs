//! Typed deserialization throughput. Compare against `stream` benches for the
//! same input to see the cost of `FromJson` on top of the streaming parser.

use bourne::parse;
use bourne_bench::{int_array, string_array};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

fn bench_typed(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed");

    for &n in &[10usize, 1_000, 100_000] {
        let s = int_array(n);
        let bytes = s.as_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("vec_i64/{n}"), |b| {
            b.iter(|| {
                let v: Vec<i64> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
    }

    for &n in &[10usize, 1_000, 100_000] {
        let s = string_array(n);
        let bytes = s.as_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        // Borrowed: should not allocate per element (only the outer Vec).
        group.bench_function(format!("vec_borrowed_str/{n}"), |b| {
            b.iter(|| {
                let v: Vec<&str> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
        // Owned: forces a String allocation per element.
        group.bench_function(format!("vec_string/{n}"), |b| {
            b.iter(|| {
                let v: Vec<String> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_typed);
criterion_main!(benches);
