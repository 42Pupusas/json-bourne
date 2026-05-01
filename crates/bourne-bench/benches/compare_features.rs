//! Head-to-head benches for the type/macro surface added in the
//! "feature parity" pass: `i128`/`u128`, map collections, `Duration`,
//! and escaped object keys.
//!
//! Each criterion group has a `bourne` and a `serde_json` function over
//! the same input so the report shows them side by side. The escaped-
//! key group additionally pairs an escape-free input against an
//! escape-bearing one to surface the per-key decode cost on the new
//! `_lex` dispatch path.

use bourne::parse;
use bourne_bench::{
    duration_seconds_array, i128_array, small_keyed_object, small_object_escaped_keys,
};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::collections::HashMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// 1. i128/u128 — wide-integer decode.
// ---------------------------------------------------------------------------
//
// serde_json by default decodes JSON numbers via its internal `Number`
// type and only widens to 128-bit when the destination asks for it.
// Both libs hit the same str→i128 path internally; the harness
// mirrors a real "ingest a wide ledger column" workload.

fn bench_i128_array(c: &mut Criterion) {
    const N: usize = 1024;
    let input = i128_array(N);
    let bytes = input.as_bytes();

    let mut g = c.benchmark_group("i128_array_1024");
    g.throughput(Throughput::Bytes(bytes.len() as u64));

    g.bench_function("bourne", |b| {
        b.iter(|| {
            let v: Vec<i128> = parse(black_box(bytes)).unwrap();
            black_box(v);
        });
    });

    g.bench_function("serde_json", |b| {
        b.iter(|| {
            let v: Vec<i128> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(v);
        });
    });

    g.finish();
}

// ---------------------------------------------------------------------------
// 2. Maps — `HashMap<String, i64>` build throughput.
// ---------------------------------------------------------------------------
//
// Both libs allocate a fresh `String` per key and hash-insert. This is
// the closest thing to apples-to-apples for map deserialization: the
// per-entry alloc dominates and any parser-level wins show up directly.

fn bench_hashmap_string_keys(c: &mut Criterion) {
    const N: usize = 256;
    let input = small_keyed_object(N);
    let bytes = input.as_bytes();

    let mut g = c.benchmark_group("hashmap_string_keys_256");
    g.throughput(Throughput::Bytes(bytes.len() as u64));

    g.bench_function("bourne", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = parse(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.bench_function("serde_json", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.finish();
}

// ---------------------------------------------------------------------------
// 3. `Duration` decode.
// ---------------------------------------------------------------------------
//
// Both libs decode the same float-seconds shape; bourne's impl is a
// `f64::from_lex` plus a finite-bounded `Duration::new`, serde_json's
// goes through `Deserialize` for `Duration` (also seconds-based when
// the field is typed as `Duration` in the schema). The bench gives
// the gross float-decode-+-construct number on a Vec.

fn bench_duration_array(c: &mut Criterion) {
    const N: usize = 1024;
    let input = duration_seconds_array(N);
    let bytes = input.as_bytes();

    let mut g = c.benchmark_group("duration_array_1024");
    g.throughput(Throughput::Bytes(bytes.len() as u64));

    g.bench_function("bourne", |b| {
        b.iter(|| {
            let v: Vec<Duration> = parse(black_box(bytes)).unwrap();
            black_box(v);
        });
    });

    // Baseline: bourne's `Vec<f64>` parse alone, no Duration construct.
    // Isolates whether the gap below comes from float parsing or from
    // the seconds → Duration conversion step.
    g.bench_function("bourne_f64_baseline", |b| {
        b.iter(|| {
            let v: Vec<f64> = parse(black_box(bytes)).unwrap();
            black_box(v);
        });
    });

    // serde_json's stock `Deserialize` for `Duration` expects an
    // object `{ "secs": u64, "nanos": u32 }` shape, *not* float-
    // seconds. To keep the comparison apples-to-apples, decode the
    // floats into `f64` on the serde side and construct `Duration`
    // afterwards — the same transformation bourne does internally.
    g.bench_function("serde_json", |b| {
        b.iter(|| {
            let v: Vec<f64> = serde_json::from_slice(black_box(bytes)).unwrap();
            let out: Vec<Duration> = v.into_iter().map(Duration::from_secs_f64).collect();
            black_box(out);
        });
    });

    // serde_json's `Vec<f64>` parse alone — paired with the bourne
    // baseline above to isolate float-parse overhead from the
    // Duration-construct step.
    g.bench_function("serde_json_f64_baseline", |b| {
        b.iter(|| {
            let v: Vec<f64> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(v);
        });
    });

    g.finish();
}

// ---------------------------------------------------------------------------
// 4. Escaped object keys.
// ---------------------------------------------------------------------------
//
// Two paired groups:
//   - `keys_no_escape`: the new `object_first_key_lex` path on the
//     escape-free fast case. This guards against regression — the old
//     `&str`-returning fast path is what we used to use, and the new
//     code must not be measurably slower when no escapes are present.
//   - `keys_escaped`: every key carries a `\n` escape, exercising
//     `key_to_cow`'s decode arm. Quantifies the per-key decode tax.

fn bench_keys_no_escape(c: &mut Criterion) {
    const N: usize = 256;
    let input = small_keyed_object(N);
    let bytes = input.as_bytes();

    let mut g = c.benchmark_group("hashmap_keys_no_escape_256");
    g.throughput(Throughput::Bytes(bytes.len() as u64));

    g.bench_function("bourne", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = parse(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.bench_function("serde_json", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.finish();
}

fn bench_keys_escaped(c: &mut Criterion) {
    const N: usize = 256;
    let input = small_object_escaped_keys(N);
    let bytes = input.as_bytes();

    let mut g = c.benchmark_group("hashmap_keys_escaped_256");
    g.throughput(Throughput::Bytes(bytes.len() as u64));

    g.bench_function("bourne", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = parse(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.bench_function("serde_json", |b| {
        b.iter(|| {
            let m: HashMap<String, i64> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(m);
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_i128_array,
    bench_hashmap_string_keys,
    bench_duration_array,
    bench_keys_no_escape,
    bench_keys_escaped,
);
criterion_main!(benches);
