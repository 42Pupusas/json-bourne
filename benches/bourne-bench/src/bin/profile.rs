//! Long-running, single-workload binary for use under `perf` / flamegraph.
//!
//! Each workload is a tight loop sized to take roughly five seconds on a
//! modern machine, so a `perf record` capture has enough samples to draw
//! meaningful conclusions without producing absurdly large profiles.
//!
//! Run:
//!   cargo build --profile profiling --features profile --bin profile
//!   perf record -g --call-graph=dwarf \
//!       ./target/profiling/profile `vec_i64_10k`
//!   perf report
//!
//! See `PROFILING.md` for the full runbook and notes on flamegraphs.

use bourne_bench::realistic::{mixed_length_string_array_with_escapes, unicode_string_array};
use bourne_bench::{
    SMALL_OBJECT, duration_seconds_array, float_array, i128_array, int_array, small_keyed_object,
    small_object_escaped_keys, string_array,
};
use json_bourne::{Error, ErrorKind, Lexer, Parser};
use json_bourne::{FromJson, parse, to_json, to_string};
use std::collections::HashMap;
use std::hint::black_box;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Struct shape — kept identical to compare.rs so profiles cross-reference.
// ---------------------------------------------------------------------------

#[allow(dead_code)] // Fields used only by FromJson; profiler doesn't read them.
#[derive(Debug)]
struct UserBourne<'input> {
    id: u64,
    name: &'input str,
    verified: bool,
    followers: u32,
    bio: Option<&'input str>,
    links: Vec<&'input str>,
}

impl<'input> FromJson<'input> for UserBourne<'input> {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        lex.object_start()?;
        let mut id: Option<u64> = None;
        let mut name: Option<&'input str> = None;
        let mut verified: Option<bool> = None;
        let mut followers: Option<u32> = None;
        let mut bio: Option<&'input str> = None;
        let mut links: Option<Vec<&'input str>> = None;
        let mut maybe_key = lex.object_first_key()?;
        while let Some(key) = maybe_key {
            match key {
                "id" => {
                    id =
                        Some(u64::try_from(lex.parse_i64_value()?).map_err(|_| {
                            Error::new(ErrorKind::NumberOutOfRange, lex.position())
                        })?);
                }
                "name" => name = Some(lex.parse_str_value()?),
                "verified" => verified = Some(bool::from_lex(lex)?),
                "followers" => {
                    followers =
                        Some(u32::try_from(lex.parse_i64_value()?).map_err(|_| {
                            Error::new(ErrorKind::NumberOutOfRange, lex.position())
                        })?);
                }
                "bio" => bio = Option::<&str>::from_lex(lex)?,
                "links" => links = Some(Vec::<&str>::from_lex(lex)?),
                _ => return Err(Error::new(ErrorKind::UnknownField, lex.position())),
            }
            maybe_key = lex.object_next_key()?;
        }
        Ok(Self {
            id: id.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            name: name.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            verified: verified
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            followers: followers
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            bio,
            links: links.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Workloads
// ---------------------------------------------------------------------------

/// A single workload's tight loop. Each `run_*` accepts a pre-built input
/// slice (so input construction isn't part of the profile) and an iteration
/// count tuned so the whole call takes ~5s on a modern `x86_64`.
fn run_stream(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let mut p: Parser<'_> = Parser::new(black_box(input));
        while let Some(ev) = p.next_event().expect("valid input") {
            black_box(ev);
        }
    }
}

fn run_typed_struct(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let u: UserBourne<'_> = parse(black_box(input)).unwrap();
        black_box(u);
    }
}

fn run_vec_i64(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<i64> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_borrowed_str(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<&str> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_string(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<String> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_borrowed_unicode(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<&str> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_f64(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<f64> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_i128(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<i128> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_vec_duration(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let v: Vec<Duration> = parse(black_box(input)).unwrap();
        black_box(v);
    }
}

fn run_hashmap_string_keys(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let m: HashMap<String, i64> = parse(black_box(input)).unwrap();
        black_box(m);
    }
}

fn run_hashmap_keys_escaped(input: &[u8], iters: u64) {
    for _ in 0..iters {
        let m: HashMap<String, i64> = parse(black_box(input)).unwrap();
        black_box(m);
    }
}

// ---------------------------------------------------------------------------
// Serialization workloads
// ---------------------------------------------------------------------------

to_json! {
    #[derive(Debug)]
    struct MetricEventSer<'input> {
        ts: u64,
        host: &'input str,
        metric: &'input str,
        count: u64,
        bytes: u64,
        latency_ms: f64,
        cpu: f64,
        throughput_rps: f64,
    }
}

const SER_N: usize = 1_000;
const HOSTS: &[&str] = &[
    "node-0", "node-1", "node-2", "node-3", "node-4", "node-5", "node-6", "node-7",
];

// Bench-fixture conversions. `0..SER_N` with `SER_N = 1_000` is well within
// u16 / u32 / i32 / f64-mantissa range; these keep call sites lint-clean
// without bare `as` casts.
fn as_u16(i: usize) -> u16 {
    u16::try_from(i).expect("loop index < SER_N fits u16")
}
fn as_u32(i: usize) -> u32 {
    u32::try_from(i).expect("loop index SER_N=1_000 fits u32")
}
fn as_i32(i: usize) -> i32 {
    i32::try_from(i).expect("loop index SER_N=1_000 fits i32")
}
fn as_f64(i: usize) -> f64 {
    // u32 widens losslessly to f64; the i->u32 step is the bounded one.
    f64::from(as_u32(i))
}

fn metric_ser_vec() -> Vec<MetricEventSer<'static>> {
    (0..SER_N)
        .map(|i| MetricEventSer {
            ts: 1_700_000_000_000 + i as u64,
            host: HOSTS[i % HOSTS.len()],
            metric: "req.latency",
            count: i as u64 % 10_000,
            bytes: 1024 * (i as u64 % 1_000_000),
            latency_ms: as_f64(i % 500) + 0.125,
            cpu: as_f64(i % 100) / 100.0,
            throughput_rps: as_f64(i) * 12.345,
        })
        .collect()
}

fn run_to_json_metric(data: &[MetricEventSer<'_>], iters: u64) {
    for _ in 0..iters {
        let out = to_string(black_box(data)).unwrap();
        black_box(out);
    }
}

to_json! {
    #[derive(Debug)]
    struct IntStruct {
        id: u64,
        count: u64,
        flags: u32,
        status: i32,
        version: u16,
    }
}

fn int_struct_vec() -> Vec<IntStruct> {
    (0..SER_N)
        .map(|i| IntStruct {
            id: 1_700_000_000_000 + i as u64,
            count: i as u64 * 37,
            flags: as_u32(i) | 0xFF00,
            status: if i % 3 == 0 { -as_i32(i) } else { as_i32(i) },
            version: as_u16(i % 256),
        })
        .collect()
}

fn run_to_json_int_struct(data: &[IntStruct], iters: u64) {
    for _ in 0..iters {
        let out = to_string(black_box(data)).unwrap();
        black_box(out);
    }
}

// Mirrors `benches/floats.rs::make_floats` so the pprof flamegraph
// reflects exactly what the divan bench measures: a `Vec<f64>` mixing
// small (1e-6), large (1e6), and mid-range (~[-100, 100)) magnitudes.
#[allow(clippy::cast_precision_loss)]
fn float_ser_vec(n: usize) -> Vec<f64> {
    let mut state: u64 = 0xCAFE_BABE_DEAD_BEEF;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mantissa = (state >> 11) as f64 / (1u64 << 53) as f64;
        let signed = mantissa.mul_add(2.0, -1.0);
        let v = match i % 10 {
            0 => signed * 1e-6,
            1 => signed * 1e6,
            _ => signed * 100.0,
        };
        out.push(v);
    }
    out
}

fn run_to_json_floats(data: &[f64], iters: u64) {
    for _ in 0..iters {
        let out = to_string(black_box(data)).unwrap();
        black_box(out);
    }
}

// Head-to-head counterpart of `run_to_json_floats`. Same input, same call
// shape (`to_string(&Vec<f64>)`) but via `serde_json` instead of `bourne`.
// Produces a flamegraph we can diff against bourne's `to_json_floats_10k`
// to spot what serde_json is *not* doing that bourne is.
fn run_serde_to_json_floats(data: &[f64], iters: u64) {
    for _ in 0..iters {
        let out = serde_json::to_string(black_box(data)).unwrap();
        black_box(out);
    }
}

// ---------------------------------------------------------------------------
// Workload registry
// ---------------------------------------------------------------------------

const fn workloads() -> &'static [&'static str] {
    &[
        "stream_small",
        "stream_ints_10k",
        "stream_strings_10k",
        "typed_struct",
        "vec_i64_10k",
        "vec_borrowed_str_10k",
        "vec_string_10k",
        "vec_borrowed_unicode_10k",
        "vec_string_escaped_1k",
        "vec_f64_10k",
        "vec_i128_10k",
        "vec_duration_10k",
        "hashmap_string_keys_1k",
        "hashmap_keys_escaped_1k",
        "to_json_metric",
        "to_json_int_struct",
        "to_json_floats_10k",
        "serde_to_json_floats_10k",
    ]
}

fn run(name: &str) {
    // Iteration counts hand-picked so each workload takes ~5s in release mode
    // on a modern x86_64. They don't have to be exact — `perf record` cares
    // about sample count, not wall time, but ~5s is a comfortable middle.
    match name {
        "stream_small" => {
            let input = SMALL_OBJECT.as_bytes();
            run_stream(input, 8_000_000);
        }
        "stream_ints_10k" => {
            let input = int_array(10_000);
            run_stream(input.as_bytes(), 12_000);
        }
        "stream_strings_10k" => {
            let input = string_array(10_000);
            run_stream(input.as_bytes(), 50_000);
        }
        "typed_struct" => {
            let input = SMALL_OBJECT.as_bytes();
            run_typed_struct(input, 5_000_000);
        }
        "vec_i64_10k" => {
            let input = int_array(10_000);
            run_vec_i64(input.as_bytes(), 60_000);
        }
        "vec_borrowed_str_10k" => {
            let input = string_array(10_000);
            run_vec_borrowed_str(input.as_bytes(), 50_000);
        }
        "vec_string_10k" => {
            let input = string_array(10_000);
            run_vec_string(input.as_bytes(), 30_000);
        }
        "vec_borrowed_unicode_10k" => {
            let input = unicode_string_array(10_000);
            run_vec_borrowed_unicode(input.as_bytes(), 50_000);
        }
        "vec_string_escaped_1k" => {
            // The case the bench surfaced: bourne is ~2.8x slower than
            // serde_json on Vec<String> when the input has escapes. This
            // workload is shaped to exercise the decoder hot path.
            let input = mixed_length_string_array_with_escapes(1_000);
            run_vec_string(input.as_bytes(), 8_000);
        }
        "vec_f64_10k" => {
            // The new fused `parse_f64_value` path. Bench measured this
            // at ~17.85 µs / 1024 elems = ~175 ns / 10k. Iter count
            // chosen for ~5s.
            let input = float_array(10_000);
            run_vec_f64(input.as_bytes(), 30_000);
        }
        "vec_i128_10k" => {
            // 128-bit integer decode via JsonNum::as_i128 (str::parse).
            // Measured ~440 µs / 10k → ~12k iters for ~5s.
            let input = i128_array(10_000);
            run_vec_i128(input.as_bytes(), 12_000);
        }
        "vec_duration_10k" => {
            // f64 decode + finite/non-negative/range check + libstd's
            // `Duration::from_secs_f64`. Measured ~265 µs / 10k.
            let input = duration_seconds_array(10_000);
            run_vec_duration(input.as_bytes(), 18_000);
        }
        "hashmap_string_keys_1k" => {
            // Object → HashMap<String, i64> on the new `_lex` key path.
            // Measured ~110 µs / 1k → ~45k iters for ~5s.
            let input = small_keyed_object(1_000);
            run_hashmap_string_keys(input.as_bytes(), 45_000);
        }
        "hashmap_keys_escaped_1k" => {
            // Same shape but every key carries a `\n` escape, exercising
            // `key_to_cow`'s decode arm. Measured ~125 µs / 1k.
            let input = small_object_escaped_keys(1_000);
            run_hashmap_keys_escaped(input.as_bytes(), 40_000);
        }
        "to_json_metric" => {
            let data = metric_ser_vec();
            run_to_json_metric(&data, 25_000);
        }
        "to_json_int_struct" => {
            let data = int_struct_vec();
            run_to_json_int_struct(&data, 100_000);
        }
        "to_json_floats_10k" => {
            // Mirrors `benches/floats.rs::bourne_write::bench(10_000)`.
            // Divan measured ~720 µs/iter, so ~7k iters ≈ 5s.
            let data = float_ser_vec(10_000);
            run_to_json_floats(&data, 7_000);
        }
        "serde_to_json_floats_10k" => {
            // Mirrors `benches/floats.rs::serde_json::bench(10_000)`.
            // Divan measured ~217 µs/iter, so ~23k iters ≈ 5s.
            let data = float_ser_vec(10_000);
            run_serde_to_json_floats(&data, 23_000);
        }
        other => {
            eprintln!("unknown workload: {other}");
            eprintln!("known workloads:");
            for w in workloads() {
                eprintln!("  {w}");
            }
            std::process::exit(2);
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let workload = args.next().unwrap_or_else(|| {
        eprintln!("usage: profile <workload>");
        eprintln!("workloads:");
        for w in workloads() {
            eprintln!("  {w}");
        }
        std::process::exit(2);
    });

    #[cfg(feature = "pprof")]
    {
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(1000)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .expect("failed to start pprof profiler");

        run(&workload);

        let report = guard
            .report()
            .build()
            .expect("failed to build pprof report");
        let out_path = format!("{workload}.svg");
        let file = std::fs::File::create(&out_path).expect("failed to create SVG file");
        report.flamegraph(file).expect("failed to write flamegraph");
        eprintln!("flamegraph written to {out_path}");
    }

    #[cfg(not(feature = "pprof"))]
    {
        run(&workload);
    }
}
