//! Realistic JSON throughput.
//!
//! Each input mimics a real production shape: GitHub events, log lines,
//! geo coordinates, JWT-id-heavy lists, varied-length / unicode / escape-
//! heavy strings. Streaming + typed where typed has a sensible target.

use bourne_bench::realistic::{
    escape_heavy_string_array, geo_array, giant_geojson_doc, github_event_array, jwt_id_array,
    log_line_array, metric_event_array, mixed_length_string_array,
    mixed_length_string_array_with_escapes, nested_config_doc, unicode_string_array,
    wide_key_object,
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

// GitHub event-shaped — heterogeneous structs with long URLs.
#[divan::bench]
fn github_events_500_stream(bencher: divan::Bencher) {
    let gh = github_event_array(500);
    bencher
        .counter(divan::counter::BytesCount::new(gh.len()))
        .bench(|| drain(divan::black_box(gh.as_bytes())));
}

// Stream-vs-DOM: bourne emits Events, serde_json builds a Value tree.
// Different output types so this is the cost-to-usable-shape comparison
// for the rich-struct case. A typed `T: Deserialize` would be apples-to-
// apples but only for one fixed struct shape; the stream/Value framing
// captures "what does each library cost to parse arbitrary JSON?"
#[divan::bench]
fn github_events_500_serde_json_value(bencher: divan::Bencher) {
    let gh = github_event_array(500);
    bencher
        .counter(divan::counter::BytesCount::new(gh.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(gh.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Log lines — varied message lengths.
#[divan::bench]
fn log_lines_500_stream(bencher: divan::Bencher) {
    let logs = log_line_array(500);
    bencher
        .counter(divan::counter::BytesCount::new(logs.len()))
        .bench(|| drain(divan::black_box(logs.as_bytes())));
}

// Geo — pairs of f64. Hits the float decode path with magnitude variety.
#[divan::bench]
fn geo_2000_stream(bencher: divan::Bencher) {
    let geo = geo_array(2_000);
    bencher
        .counter(divan::counter::BytesCount::new(geo.len()))
        .bench(|| drain(divan::black_box(geo.as_bytes())));
}

#[divan::bench]
fn geo_2000_typed_f64_pairs(bencher: divan::Bencher) {
    let geo = geo_array(2_000);
    bencher
        .counter(divan::counter::BytesCount::new(geo.len()))
        .bench(|| {
            let v: Vec<(f64, f64)> = parse(divan::black_box(geo.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// JWT-shaped: 13-19 digit IDs. The corpus that should expose SIMD-digit
// wins (or losses) directly — most ints are wide enough to fill a chunk.
#[divan::bench]
fn jwt_ids_1000_stream(bencher: divan::Bencher) {
    let jwts = jwt_id_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(jwts.len()))
        .bench(|| drain(divan::black_box(jwts.as_bytes())));
}

// Stream-vs-Value: this is the wide-int corpus with the strongest
// claim on "where SIMD digit-scan would matter." If serde_json (which
// doesn't SIMD digits either) is much slower than bourne here, we know
// the gap isn't from missing SIMD — it's from the Value/event difference.
#[divan::bench]
fn jwt_ids_1000_serde_json_value(bencher: divan::Bencher) {
    let jwts = jwt_id_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(jwts.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(jwts.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Mixed-length strings — 50/30/15/5 distribution.
#[divan::bench]
fn mixed_length_strings_1000_stream(bencher: divan::Bencher) {
    let mixed = mixed_length_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed.len()))
        .bench(|| drain(divan::black_box(mixed.as_bytes())));
}

#[divan::bench]
fn mixed_length_strings_1000_typed_borrowed(bencher: divan::Bencher) {
    let mixed = mixed_length_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed.len()))
        .bench(|| {
            let v: Vec<&str> = parse(divan::black_box(mixed.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Apples-to-apples: same input, same output type. serde_json's borrowed
// &str path requires `from_slice` (not `from_str`) and the absence of
// escapes — both true here. This is the headline number for "long
// string body throughput", which is where SSE2 chunk-scan should pull
// ahead the most.
#[divan::bench]
fn mixed_length_strings_1000_typed_borrowed_serde_json(bencher: divan::Bencher) {
    let mixed = mixed_length_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed.len()))
        .bench(|| {
            let v: Vec<&str> = serde_json::from_slice(divan::black_box(mixed.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Same length distribution, but with ~1 escape per 50 bytes so neither
// library can use its borrow-everything fast path. The headline number
// for the case most production string fields actually hit: strings
// with embedded newlines, quotes, or unicode escapes.
//
// Apples-to-apples: same input, same output type. Both libraries
// allocate one String per element, decode escapes into it, and own
// the result. The no-escape `mixed_length_strings` comparison runs
// serde_json's borrowed-`&str` fast path which doesn't apply once
// there are escapes; this corpus closes that gap.
#[divan::bench]
fn mixed_length_strings_with_escapes_1000_stream(bencher: divan::Bencher) {
    let mixed_esc = mixed_length_string_array_with_escapes(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed_esc.len()))
        .bench(|| drain(divan::black_box(mixed_esc.as_bytes())));
}

#[divan::bench]
fn mixed_length_strings_with_escapes_1000_typed_owned(bencher: divan::Bencher) {
    let mixed_esc = mixed_length_string_array_with_escapes(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed_esc.len()))
        .bench(|| {
            let v: Vec<String> = parse(divan::black_box(mixed_esc.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

#[divan::bench]
fn mixed_length_strings_with_escapes_1000_typed_owned_serde_json(bencher: divan::Bencher) {
    let mixed_esc = mixed_length_string_array_with_escapes(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(mixed_esc.len()))
        .bench(|| {
            let v: Vec<String> =
                serde_json::from_slice(divan::black_box(mixed_esc.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Unicode strings — exercises consume_utf8_multibyte across 2/3/4-byte
// sequences. No existing bench touches this path.
#[divan::bench]
fn unicode_strings_1000_stream(bencher: divan::Bencher) {
    let uni = unicode_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(uni.len()))
        .bench(|| drain(divan::black_box(uni.as_bytes())));
}

// Apples-to-apples: borrowed &str on both sides. The corpus has no
// escapes, so serde_json's zero-copy path applies. Whether bourne's
// inline UTF-8 validation beats serde_json's `from_slice` UTF-8 check
// is the question this answers.
#[divan::bench]
fn unicode_strings_1000_typed_borrowed_serde_json(bencher: divan::Bencher) {
    let uni = unicode_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(uni.len()))
        .bench(|| {
            let v: Vec<&str> = serde_json::from_slice(divan::black_box(uni.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

#[divan::bench]
fn unicode_strings_1000_typed_borrowed(bencher: divan::Bencher) {
    let uni = unicode_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(uni.len()))
        .bench(|| {
            let v: Vec<&str> = parse(divan::black_box(uni.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Escape-heavy strings — 10 escapes per string. Stresses validate_escapes.
#[divan::bench]
fn escape_heavy_strings_1000_stream(bencher: divan::Bencher) {
    let esc = escape_heavy_string_array(1_000);
    bencher
        .counter(divan::counter::BytesCount::new(esc.len()))
        .bench(|| drain(divan::black_box(esc.as_bytes())));
}

// Nested config-shaped document — branching object tree, 3-5 levels
// deep. Every other realistic fixture is a flat array of records; this
// one stresses the parser's frame stack across nested *objects*.
#[divan::bench]
fn nested_config_200_stream(bencher: divan::Bencher) {
    let cfg = nested_config_doc(200);
    bencher
        .counter(divan::counter::BytesCount::new(cfg.len()))
        .bench(|| drain(divan::black_box(cfg.as_bytes())));
}

#[divan::bench]
fn nested_config_200_serde_json_value(bencher: divan::Bencher) {
    let cfg = nested_config_doc(200);
    bencher
        .counter(divan::counter::BytesCount::new(cfg.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(cfg.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Wide-key object — single object with hundreds of fields. Mimics
// protobuf-decoded records and feature-flag bundles, where one object
// carries many keys rather than many objects each carrying a few.
// Per-key dispatch dominates parse time at this shape.
#[divan::bench]
fn wide_key_object_500_stream(bencher: divan::Bencher) {
    let wide = wide_key_object(500);
    bencher
        .counter(divan::counter::BytesCount::new(wide.len()))
        .bench(|| drain(divan::black_box(wide.as_bytes())));
}

#[divan::bench]
fn wide_key_object_500_serde_json_value(bencher: divan::Bencher) {
    let wide = wide_key_object(500);
    bencher
        .counter(divan::counter::BytesCount::new(wide.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(wide.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Giant single document — multi-MB GeoJSON-style. Sustained throughput
// across one continuous stream rather than dispatch-amortized-over-
// records. Surfaces any per-byte cost that scales linearly without
// showing up in the smaller fixtures.
#[divan::bench]
fn giant_geojson_25000_stream(bencher: divan::Bencher) {
    let geo_doc = giant_geojson_doc(25_000);
    bencher
        .counter(divan::counter::BytesCount::new(geo_doc.len()))
        .bench(|| drain(divan::black_box(geo_doc.as_bytes())));
}

#[divan::bench]
fn giant_geojson_25000_serde_json_value(bencher: divan::Bencher) {
    let geo_doc = giant_geojson_doc(25_000);
    bencher
        .counter(divan::counter::BytesCount::new(geo_doc.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(geo_doc.as_bytes())).unwrap();
            divan::black_box(v);
        });
}

// Metric events — int + float fields per record. Every other realistic
// corpus is all-int (jwt_ids) or all-float (geo). Mixing per record
// means the lexer's int/float dispatch can't be amortized away.
#[divan::bench]
fn metric_events_2000_stream(bencher: divan::Bencher) {
    let metrics = metric_event_array(2_000);
    bencher
        .counter(divan::counter::BytesCount::new(metrics.len()))
        .bench(|| drain(divan::black_box(metrics.as_bytes())));
}

#[divan::bench]
fn metric_events_2000_serde_json_value(bencher: divan::Bencher) {
    let metrics = metric_event_array(2_000);
    bencher
        .counter(divan::counter::BytesCount::new(metrics.len()))
        .bench(|| {
            let v: serde_json::Value =
                serde_json::from_slice(divan::black_box(metrics.as_bytes())).unwrap();
            divan::black_box(v);
        });
}
