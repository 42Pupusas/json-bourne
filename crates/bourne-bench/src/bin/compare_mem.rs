//! Memory comparison: bourne vs `serde_json`.
//!
//! Each workload runs once through bourne and once through `serde_json` with
//! a process-wide counting allocator installed. We report:
//!   - allocs: number of trips to the allocator
//!   - total : cumulative bytes requested across the parse
//!   - peak  : maximum bytes live at any point during the parse
//!
//! Throughput is criterion's job (`benches/compare.rs`); this is the
//! complementary memory story. A library can win one and lose the other.
//!
//! Run:
//!   cargo run --release --features compare-mem --bin `compare_mem`

use bourne::{FromJson, parse};
use bourne_alloctest::measure;
use bourne_bench::realistic::{
    giant_geojson_doc, metric_event_array, mixed_length_string_array_with_escapes,
    nested_config_doc, wide_key_object,
};
use bourne_bench::{SMALL_OBJECT, float_array, int_array, string_array};
use bourne_core::{Error, ErrorKind, Lexer, Parser};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Struct shapes — same fields on both sides so the comparison is fair.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct UserBourne<'input> {
    id: u64,
    name: &'input str,
    verified: bool,
    followers: u32,
    bio: Option<&'input str>,
    links: Vec<&'input str>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UserSerde<'a> {
    id: u64,
    #[serde(borrow)]
    name: &'a str,
    verified: bool,
    followers: u32,
    #[serde(borrow)]
    bio: Option<&'a str>,
    #[serde(borrow)]
    links: Vec<&'a str>,
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
                "id" => id = Some(u64::from_lex(lex)?),
                "name" => name = Some(<&str>::from_lex(lex)?),
                "verified" => verified = Some(bool::from_lex(lex)?),
                "followers" => followers = Some(u32::from_lex(lex)?),
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

// Realistic-sized struct shape, mirroring the one in benches/compare.rs.
// 8 fields, mixed int/string/f64 — closer to what real telemetry payloads
// look like than the 6-field SMALL_OBJECT user record.

#[derive(Debug)]
#[allow(dead_code)]
struct MetricEventBourne<'input> {
    ts: u64,
    host: &'input str,
    metric: &'input str,
    count: u64,
    bytes: u64,
    latency_ms: f64,
    cpu: f64,
    throughput_rps: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MetricEventSerde<'a> {
    ts: u64,
    #[serde(borrow)]
    host: &'a str,
    #[serde(borrow)]
    metric: &'a str,
    count: u64,
    bytes: u64,
    latency_ms: f64,
    cpu: f64,
    throughput_rps: f64,
}

impl<'input> FromJson<'input> for MetricEventBourne<'input> {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        lex.object_start()?;
        let mut ts: Option<u64> = None;
        let mut host: Option<&'input str> = None;
        let mut metric: Option<&'input str> = None;
        let mut count: Option<u64> = None;
        let mut bytes: Option<u64> = None;
        let mut latency_ms: Option<f64> = None;
        let mut cpu: Option<f64> = None;
        let mut throughput_rps: Option<f64> = None;
        let mut maybe_key = lex.object_first_key()?;
        while let Some(key) = maybe_key {
            match key {
                "ts" => ts = Some(u64::from_lex(lex)?),
                "host" => host = Some(<&str>::from_lex(lex)?),
                "metric" => metric = Some(<&str>::from_lex(lex)?),
                "count" => count = Some(u64::from_lex(lex)?),
                "bytes" => bytes = Some(u64::from_lex(lex)?),
                "latency_ms" => latency_ms = Some(f64::from_lex(lex)?),
                "cpu" => cpu = Some(f64::from_lex(lex)?),
                "throughput_rps" => throughput_rps = Some(f64::from_lex(lex)?),
                _ => return Err(Error::new(ErrorKind::UnknownField, lex.position())),
            }
            maybe_key = lex.object_next_key()?;
        }
        Ok(Self {
            ts: ts.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            host: host.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            metric: metric
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            count: count
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            bytes: bytes
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            latency_ms: latency_ms
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            cpu: cpu.ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
            throughput_rps: throughput_rps
                .ok_or_else(|| Error::new(ErrorKind::MissingField, lex.position()))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct Report {
    allocs: usize,
    bytes: usize,
}

fn run_bourne_drain(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let mut p: Parser<'_> = Parser::new(input);
        let mut count = 0usize;
        while p.next_event().expect("valid input").is_some() {
            count += 1;
        }
        count
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_value(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: serde_json::Value = serde_json::from_slice(input).expect("valid input");
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_struct(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let u: UserBourne<'_> = parse(input).unwrap();
        u
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_struct(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let u: UserSerde<'_> = serde_json::from_slice(input).unwrap();
        u
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_i64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<i64> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_i64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<i64> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_f64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<f64> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_f64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<f64> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_str<'input>(input: &'input [u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<&'input str> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_str<'input>(input: &'input [u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<&'input str> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_string(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<String> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_string(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<String> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_metric(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<MetricEventBourne<'_>> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_metric(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<MetricEventSerde<'_>> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn print_row(workload: &str, b: Report, s: Report) {
    let ratio_allocs = if s.allocs == 0 {
        "—".to_string()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let r = (b.allocs as f64 / s.allocs as f64) * 100.0;
        format!("{r:>5.0}%")
    };
    let ratio_bytes = if s.bytes == 0 {
        "—".to_string()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let r = (b.bytes as f64 / s.bytes as f64) * 100.0;
        format!("{r:>5.0}%")
    };
    println!(
        "  {:<32}  {:>6}  {:>9}  |  {:>6}  {:>9}  |  {:>6}  {:>6}",
        workload,
        format!("{}", b.allocs),
        format!("{} B", b.bytes),
        format!("{}", s.allocs),
        format!("{} B", s.bytes),
        ratio_allocs,
        ratio_bytes,
    );
}

fn header() {
    println!();
    println!(
        "  {:<32}  {:>18}  |  {:>18}  |  {:>15}",
        "workload", "bourne", "serde_json", "ratio (b/s)",
    );
    println!(
        "  {:<32}  {:>6}  {:>9}  |  {:>6}  {:>9}  |  {:>6}  {:>6}",
        "", "allocs", "bytes", "allocs", "bytes", "allocs", "bytes",
    );
    println!("  {}", "-".repeat(89));
}

// bench main is a sequential walk; extracting helpers would obscure the runbook nature of the file
#[allow(clippy::too_many_lines)]
fn main() {
    // Touch the allocator a couple of times before measuring; the very first
    // `format!` / `String::new` call in a process can pull in lazy-init
    // pages that would otherwise be charged to the first measured workload.
    let _warm = format!("{}", std::process::id());

    println!("Memory: bourne vs serde_json (single parse, no warmup)");
    header();

    // 1. stream-vs-DOM. bourne is streaming (zero alloc); serde builds Value.
    let small = SMALL_OBJECT.as_bytes();
    print_row(
        "stream small_object",
        run_bourne_drain(small),
        run_serde_value(small),
    );
    let big_ints = int_array(10_000);
    print_row(
        "stream ints/10000",
        run_bourne_drain(big_ints.as_bytes()),
        run_serde_value(big_ints.as_bytes()),
    );
    let big_strs = string_array(10_000);
    print_row(
        "stream strings/10000",
        run_bourne_drain(big_strs.as_bytes()),
        run_serde_value(big_strs.as_bytes()),
    );

    // 2. Typed struct.
    print_row(
        "typed_struct small",
        run_bourne_struct(small),
        run_serde_struct(small),
    );

    // 3. Vec<i64>.
    let s = int_array(100);
    print_row(
        "vec_i64/100",
        run_bourne_vec_i64(s.as_bytes()),
        run_serde_vec_i64(s.as_bytes()),
    );
    let s = int_array(10_000);
    print_row(
        "vec_i64/10000",
        run_bourne_vec_i64(s.as_bytes()),
        run_serde_vec_i64(s.as_bytes()),
    );

    // 3b. Vec<f64> — paired with vec_i64 above so the report shows the
    // fast-path-vs-fast-path memory story for both number kinds. After
    // the `parse_f64_value` rewrite these should match `vec_i64`'s
    // alloc shape exactly: just Vec growth, no per-element churn.
    let s = float_array(100);
    print_row(
        "vec_f64/100",
        run_bourne_vec_f64(s.as_bytes()),
        run_serde_vec_f64(s.as_bytes()),
    );
    let s = float_array(10_000);
    print_row(
        "vec_f64/10000",
        run_bourne_vec_f64(s.as_bytes()),
        run_serde_vec_f64(s.as_bytes()),
    );

    // 4. Vec<&str> — borrowed both sides.
    let s = string_array(100);
    print_row(
        "vec_borrowed_str/100",
        run_bourne_vec_str(s.as_bytes()),
        run_serde_vec_str(s.as_bytes()),
    );
    let s = string_array(10_000);
    print_row(
        "vec_borrowed_str/10000",
        run_bourne_vec_str(s.as_bytes()),
        run_serde_vec_str(s.as_bytes()),
    );

    // 5. Vec<String> — owned both sides.
    let s = string_array(100);
    print_row(
        "vec_string/100",
        run_bourne_vec_string(s.as_bytes()),
        run_serde_vec_string(s.as_bytes()),
    );
    let s = string_array(10_000);
    print_row(
        "vec_string/10000",
        run_bourne_vec_string(s.as_bytes()),
        run_serde_vec_string(s.as_bytes()),
    );

    // -----------------------------------------------------------------
    // Realistic corpora — same shapes the benches use, so the memory
    // numbers tell the same story as the throughput numbers about the
    // documents callers actually parse. The original section above is
    // all uniform fixtures (`int_array`, `string_array`); these match
    // the structural shapes the realistic-corpus benches added.
    // -----------------------------------------------------------------
    println!();
    println!("  realistic corpora");
    println!("  {}", "-".repeat(89));

    // Stream-vs-Value on each realistic shape. bourne emits Events, serde
    // builds a Value tree — the alloc story the benches pair with their
    // throughput numbers.
    let cfg = nested_config_doc(200);
    print_row(
        "stream nested_config/200",
        run_bourne_drain(cfg.as_bytes()),
        run_serde_value(cfg.as_bytes()),
    );
    let wide = wide_key_object(500);
    print_row(
        "stream wide_key_object/500",
        run_bourne_drain(wide.as_bytes()),
        run_serde_value(wide.as_bytes()),
    );
    let geo = giant_geojson_doc(25_000);
    print_row(
        "stream giant_geojson/25000",
        run_bourne_drain(geo.as_bytes()),
        run_serde_value(geo.as_bytes()),
    );
    let metrics = metric_event_array(1_000);
    print_row(
        "stream metric_events/1000",
        run_bourne_drain(metrics.as_bytes()),
        run_serde_value(metrics.as_bytes()),
    );

    // Typed struct on the realistic shape: Vec<MetricEvent> over 1000
    // records. The SMALL_OBJECT typed_struct row is dominated by per-call
    // overhead; this row is dominated by per-record dispatch + decode.
    print_row(
        "typed_struct metric_events/1000",
        run_bourne_vec_metric(metrics.as_bytes()),
        run_serde_vec_metric(metrics.as_bytes()),
    );

    // Vec<String> with escapes — the case bourne couldn't decode at all
    // before the escape-decoder landed. Both sides allocate one String
    // per element and own the result, so this is the apples-to-apples
    // memory story for escaped string fields.
    let strs_esc = mixed_length_string_array_with_escapes(1_000);
    print_row(
        "vec_string mixed_esc/1000",
        run_bourne_vec_string(strs_esc.as_bytes()),
        run_serde_vec_string(strs_esc.as_bytes()),
    );

    println!();
    println!("ratio < 100% means bourne uses less.");
    println!();
}
