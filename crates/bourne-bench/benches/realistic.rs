//! Realistic JSON throughput.
//!
//! Each input mimics a real production shape: GitHub events, log lines,
//! geo coordinates, JWT-id-heavy lists, varied-length / unicode / escape-
//! heavy strings. Streaming + typed where typed has a sensible target.

use bourne::parse;
use bourne_bench::realistic::{
    escape_heavy_string_array, geo_array, giant_geojson_doc, github_event_array, jwt_id_array,
    log_line_array, metric_event_array, mixed_length_string_array,
    mixed_length_string_array_with_escapes, nested_config_doc, unicode_string_array,
    wide_key_object,
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
    // Stream-vs-DOM: bourne emits Events, serde_json builds a Value tree.
    // Different output types so this is the cost-to-usable-shape comparison
    // for the rich-struct case. A typed `T: Deserialize` would be apples-to-
    // apples but only for one fixed struct shape; the stream/Value framing
    // captures "what does each library cost to parse arbitrary JSON?"
    group.bench_function("github_events/500/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(gh.as_bytes())).unwrap();
            black_box(v);
        });
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
    // Stream-vs-Value: this is the wide-int corpus with the strongest
    // claim on "where SIMD digit-scan would matter." If serde_json (which
    // doesn't SIMD digits either) is much slower than bourne here, we know
    // the gap isn't from missing SIMD — it's from the Value/event difference.
    group.bench_function("jwt_ids/1000/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value = serde_json::from_slice(black_box(jwts.as_bytes())).unwrap();
            black_box(v);
        });
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
    // Apples-to-apples: same input, same output type. serde_json's borrowed
    // &str path requires `from_slice` (not `from_str`) and the absence of
    // escapes — both true here. This is the headline number for "long
    // string body throughput", which is where SSE2 chunk-scan should pull
    // ahead the most.
    group.bench_function("mixed_length_strings/1000/typed_borrowed/serde_json", |b| {
        b.iter(|| {
            let v: Vec<&str> = serde_json::from_slice(black_box(mixed.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Same length distribution, but with ~1 escape per 50 bytes so neither
    // library can use its borrow-everything fast path. The point is not
    // typed deserialization — `Vec<String>` would need bourne's escape-
    // decoding milestone which is still pending — but to measure raw
    // validation throughput on strings with escapes, which is where most
    // production text fields actually live.
    //
    // Compared against `serde_json::from_slice::<serde_json::Value>` so
    // both sides produce a usable shape. This is the comparison the no-
    // escape `mixed_length_strings` head-to-head deliberately can't make:
    // serde_json's borrowed-`&str` fast path doesn't apply to escaped
    // bodies, so the existing bench understates serde_json's typical cost
    // on real string-heavy JSON. This corpus closes that gap.
    let mixed_esc = mixed_length_string_array_with_escapes(1_000);
    group.throughput(Throughput::Bytes(mixed_esc.len() as u64));
    group.bench_function("mixed_length_strings_with_escapes/1000/stream", |b| {
        b.iter(|| drain(black_box(mixed_esc.as_bytes())));
    });
    group.bench_function(
        "mixed_length_strings_with_escapes/1000/serde_json_value",
        |b| {
            b.iter(|| {
                let v: serde_json::Value =
                    serde_json::from_slice(black_box(mixed_esc.as_bytes())).unwrap();
                black_box(v);
            });
        },
    );

    // Unicode strings — exercises consume_utf8_multibyte across 2/3/4-byte
    // sequences. No existing bench touches this path.
    let uni = unicode_string_array(1_000);
    group.throughput(Throughput::Bytes(uni.len() as u64));
    group.bench_function("unicode_strings/1000/stream", |b| {
        b.iter(|| drain(black_box(uni.as_bytes())));
    });
    // Apples-to-apples: borrowed &str on both sides. The corpus has no
    // escapes, so serde_json's zero-copy path applies. Whether bourne's
    // inline UTF-8 validation beats serde_json's `from_slice` UTF-8 check
    // is the question this answers.
    group.bench_function("unicode_strings/1000/typed_borrowed/serde_json", |b| {
        b.iter(|| {
            let v: Vec<&str> = serde_json::from_slice(black_box(uni.as_bytes())).unwrap();
            black_box(v);
        });
    });
    group.bench_function("unicode_strings/1000/typed_borrowed", |b| {
        b.iter(|| {
            let v: Vec<&str> = parse(black_box(uni.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Escape-heavy strings — 10 escapes per string. Stresses validate_escapes.
    let esc = escape_heavy_string_array(1_000);
    group.throughput(Throughput::Bytes(esc.len() as u64));
    group.bench_function("escape_heavy_strings/1000/stream", |b| {
        b.iter(|| drain(black_box(esc.as_bytes())));
    });

    // Nested config-shaped document — branching object tree, 3-5 levels
    // deep. Every other realistic fixture is a flat array of records; this
    // one stresses the parser's frame stack across nested *objects*.
    let cfg = nested_config_doc(200);
    group.throughput(Throughput::Bytes(cfg.len() as u64));
    group.bench_function("nested_config/200/stream", |b| {
        b.iter(|| drain(black_box(cfg.as_bytes())));
    });
    group.bench_function("nested_config/200/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(cfg.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Wide-key object — single object with hundreds of fields. Mimics
    // protobuf-decoded records and feature-flag bundles, where one object
    // carries many keys rather than many objects each carrying a few.
    // Per-key dispatch dominates parse time at this shape.
    let wide = wide_key_object(500);
    group.throughput(Throughput::Bytes(wide.len() as u64));
    group.bench_function("wide_key_object/500/stream", |b| {
        b.iter(|| drain(black_box(wide.as_bytes())));
    });
    group.bench_function("wide_key_object/500/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(wide.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Giant single document — multi-MB GeoJSON-style. Sustained throughput
    // across one continuous stream rather than dispatch-amortized-over-
    // records. Surfaces any per-byte cost that scales linearly without
    // showing up in the smaller fixtures.
    let geo_doc = giant_geojson_doc(25_000);
    group.throughput(Throughput::Bytes(geo_doc.len() as u64));
    group.bench_function("giant_geojson/25000/stream", |b| {
        b.iter(|| drain(black_box(geo_doc.as_bytes())));
    });
    group.bench_function("giant_geojson/25000/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(geo_doc.as_bytes())).unwrap();
            black_box(v);
        });
    });

    // Metric events — int + float fields per record. Every other realistic
    // corpus is all-int (jwt_ids) or all-float (geo). Mixing per record
    // means the lexer's int/float dispatch can't be amortized away.
    let metrics = metric_event_array(2_000);
    group.throughput(Throughput::Bytes(metrics.len() as u64));
    group.bench_function("metric_events/2000/stream", |b| {
        b.iter(|| drain(black_box(metrics.as_bytes())));
    });
    group.bench_function("metric_events/2000/serde_json_value", |b| {
        b.iter(|| {
            let v: serde_json::Value =
                serde_json::from_slice(black_box(metrics.as_bytes())).unwrap();
            black_box(v);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_realistic);
criterion_main!(benches);
