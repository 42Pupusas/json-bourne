//! Head-to-head: bourne vs `serde_json`.
//!
//! Each criterion group contains one `bourne` and one `serde_json` function
//! over the same input, so the report shows the two bars side by side.
//!
//! Categories:
//!   1. `stream_vs_dom` — bourne's streaming parser vs `serde_json`'s DOM
//!      parse of the same bytes. Not apples-to-apples (different output
//!      types) but the only honest framing for a streaming-first lib: the
//!      question is "what does the user actually pay to get from bytes to
//!      a usable shape." For DOM users, that's `Value`; for us, the events
//!      themselves are usable.
//!   2. `typed_struct` — apples-to-apples. Both libs deserialize the same
//!      JSON into a struct of identical shape. This is the headline number.
//!   3. `vec_*` — Vec<i64>, Vec<&str>, Vec<String>. The first measures
//!      number throughput; the second measures the borrowed-string fast
//!      path (where we should win biggest); the third is fair-fight
//!      allocation-bound territory where the gap should be smaller.

use bourne::{FromJson, parse};
use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use bourne_core::{Error, ErrorKind, Lexer, Parser};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Struct shapes used by typed_struct.
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
struct UserSerde<'a> {
    #[allow(dead_code)]
    id: u64,
    #[serde(borrow)]
    #[allow(dead_code)]
    name: &'a str,
    #[allow(dead_code)]
    verified: bool,
    #[allow(dead_code)]
    followers: u32,
    #[serde(borrow)]
    #[allow(dead_code)]
    bio: Option<&'a str>,
    #[serde(borrow)]
    #[allow(dead_code)]
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

        // Fast path: drive the lexer with `object_first_key` /
        // `object_next_key`, which lex the key as a borrowed `&str` and
        // leave the cursor at the field's value. For each known field we
        // call a typed `parse_*_value` (or the corresponding `from_lex`)
        // directly — there is no streaming-event detour at all anymore.
        let mut maybe_key = lex.object_first_key()?;
        while let Some(key) = maybe_key {
            match key {
                "id" => id = Some(u64::try_from(lex.parse_i64_value()?).map_err(|_| {
                    Error::new(ErrorKind::NumberOutOfRange, lex.position())
                })?),
                "name" => name = Some(lex.parse_str_value()?),
                "verified" => verified = Some(bool::from_lex(lex)?),
                "followers" => followers = Some(u32::try_from(lex.parse_i64_value()?).map_err(|_| {
                    Error::new(ErrorKind::NumberOutOfRange, lex.position())
                })?),
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

fn bourne_drain(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(ev) = p.next_event().expect("valid input") {
        black_box(ev);
    }
}

fn bench_stream_vs_dom(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_vs_dom");

    // Use the existing small/representative object plus one bigger document.
    let small = SMALL_OBJECT.as_bytes();
    let big_ints = int_array(10_000);
    let big_strs = string_array(10_000);

    for (name, bytes) in [
        ("small_object", small),
        ("ints/10000", big_ints.as_bytes()),
        ("strings/10000", big_strs.as_bytes()),
    ] {
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("{name}/bourne_stream"), |b| {
            b.iter(|| bourne_drain(black_box(bytes)));
        });
        group.bench_function(format!("{name}/serde_json_value"), |b| {
            b.iter(|| {
                let v: serde_json::Value =
                    serde_json::from_slice(black_box(bytes)).expect("valid input");
                black_box(v);
            });
        });
    }

    group.finish();
}

fn bench_typed_struct(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed_struct");
    let bytes = SMALL_OBJECT.as_bytes();
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    group.bench_function("bourne", |b| {
        b.iter(|| {
            let u: UserBourne<'_> = parse(black_box(bytes)).unwrap();
            black_box(u);
        });
    });
    group.bench_function("serde_json", |b| {
        b.iter(|| {
            let u: UserSerde<'_> = serde_json::from_slice(black_box(bytes)).unwrap();
            black_box(u);
        });
    });

    group.finish();
}

fn bench_vec_i64(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_i64");
    for &n in &[100usize, 10_000] {
        let s = int_array(n);
        let bytes = s.as_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("{n}/bourne"), |b| {
            b.iter(|| {
                let v: Vec<i64> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
        group.bench_function(format!("{n}/serde_json"), |b| {
            b.iter(|| {
                let v: Vec<i64> = serde_json::from_slice(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
    }
    group.finish();
}

fn bench_vec_borrowed_str(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_borrowed_str");
    for &n in &[100usize, 10_000] {
        let s = string_array(n);
        let bytes = s.as_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("{n}/bourne"), |b| {
            b.iter(|| {
                let v: Vec<&str> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
        group.bench_function(format!("{n}/serde_json"), |b| {
            b.iter(|| {
                // serde_json supports zero-copy borrowed &str via from_slice + #[serde(borrow)],
                // but only when there are no escapes. Our string_array has none.
                let v: Vec<&str> = serde_json::from_slice(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
    }
    group.finish();
}

fn bench_vec_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_string");
    for &n in &[100usize, 10_000] {
        let s = string_array(n);
        let bytes = s.as_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("{n}/bourne"), |b| {
            b.iter(|| {
                let v: Vec<String> = parse(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
        group.bench_function(format!("{n}/serde_json"), |b| {
            b.iter(|| {
                let v: Vec<String> = serde_json::from_slice(black_box(bytes)).unwrap();
                black_box(v);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_stream_vs_dom,
    bench_typed_struct,
    bench_vec_i64,
    bench_vec_borrowed_str,
    bench_vec_string,
);
criterion_main!(benches);
