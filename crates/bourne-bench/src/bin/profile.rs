//! Long-running, single-workload binary for use under `perf` / flamegraph.
//!
//! Each workload is a tight loop sized to take roughly five seconds on a
//! modern machine, so a `perf record` capture has enough samples to draw
//! meaningful conclusions without producing absurdly large profiles.
//!
//! Run:
//!   cargo build --profile profiling --features profile --bin profile
//!   perf record -g --call-graph=dwarf \
//!       ./target/profiling/profile vec_i64_10k
//!   perf report
//!
//! See `PROFILING.md` for the full runbook and notes on flamegraphs.

use bourne::{FromJson, parse};
use bourne_bench::realistic::unicode_string_array;
use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use bourne_core::{Error, ErrorKind, Lexer, Parser};
use std::hint::black_box;

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

/// A single workload's tight loop. Each `run_*` accepts a pre-built input
/// slice (so input construction isn't part of the profile) and an iteration
/// count tuned so the whole call takes ~5s on a modern x86_64.
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

// ---------------------------------------------------------------------------
// Workload registry
// ---------------------------------------------------------------------------

fn workloads() -> &'static [&'static str] {
    &[
        "stream_small",
        "stream_ints_10k",
        "stream_strings_10k",
        "typed_struct",
        "vec_i64_10k",
        "vec_borrowed_str_10k",
        "vec_string_10k",
        "vec_borrowed_unicode_10k",
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
    run(&workload);
}
