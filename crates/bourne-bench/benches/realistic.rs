//! Realistic JSON throughput.
//!
//! Each input mimics a real production shape: GitHub events, log lines,
//! geo coordinates, JWT-id-heavy lists, varied-length / unicode / escape-
//! heavy strings. Streaming + typed where typed has a sensible target.

use bourne::parse;
use bourne_bench::realistic::{
    escape_heavy_string_array, geo_array, github_event_array, jwt_id_array, log_line_array,
    mixed_length_string_array, unicode_string_array,
};
use bourne_core::Parser;
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

fn drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

fn bench_realistic(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic");

    // GitHub event-shaped — heterogeneous structs with long URLs.
    let gh = github_event_array(500);
    group.throughput(Throughput::Bytes(gh.len() as u64));
    group.bench_function("github_events/500/stream", |b| {
        b.iter(|| drain(black_box(gh.as_bytes())));
    });

    // Log lines — varied message lengths.
    let logs = log_line_array(500);
    group.throughput(Throughput::Bytes(logs.len() as u64));
    group.bench_function("log_lines/500/stream", |b| {
        b.iter(|| drain(black_box(logs.as_bytes())));
    });

    // Geo — pairs of f64. Hits the float decode path with magnitude variety.
    let geo = geo_array(2_000);
    group.throughput(Throughput::Bytes(geo.len() as u64));
    group.bench_function("geo/2000/stream", |b| {
        b.iter(|| drain(black_box(geo.as_bytes())));
    });
    group.bench_function("geo/2000/typed_f64_pairs", |b| {
        b.iter(|| {
            let v: Vec<(f64, f64)> = parse(black_box(geo.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // JWT-shaped: 13-19 digit IDs. The corpus that should expose SIMD-digit
    // wins (or losses) directly — most ints are wide enough to fill a chunk.
    let jwts = jwt_id_array(1_000);
    group.throughput(Throughput::Bytes(jwts.len() as u64));
    group.bench_function("jwt_ids/1000/stream", |b| {
        b.iter(|| drain(black_box(jwts.as_bytes())));
    });

    // Mixed-length strings — 50/30/15/5 distribution.
    let mixed = mixed_length_string_array(1_000);
    group.throughput(Throughput::Bytes(mixed.len() as u64));
    group.bench_function("mixed_length_strings/1000/stream", |b| {
        b.iter(|| drain(black_box(mixed.as_bytes())));
    });
    group.bench_function("mixed_length_strings/1000/typed_borrowed", |b| {
        b.iter(|| {
            let v: Vec<&str> = parse(black_box(mixed.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Unicode strings — exercises consume_utf8_multibyte across 2/3/4-byte
    // sequences. No existing bench touches this path.
    let uni = unicode_string_array(1_000);
    group.throughput(Throughput::Bytes(uni.len() as u64));
    group.bench_function("unicode_strings/1000/stream", |b| {
        b.iter(|| drain(black_box(uni.as_bytes())));
    });

    // Escape-heavy strings — 10 escapes per string. Stresses validate_escapes.
    let esc = escape_heavy_string_array(1_000);
    group.throughput(Throughput::Bytes(esc.len() as u64));
    group.bench_function("escape_heavy_strings/1000/stream", |b| {
        b.iter(|| drain(black_box(esc.as_bytes())));
    });

    group.finish();
}

criterion_group!(benches, bench_realistic);
criterion_main!(benches);
