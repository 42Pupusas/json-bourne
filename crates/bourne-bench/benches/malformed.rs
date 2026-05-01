//! Malformed-input rejection latency.
//!
//! For each entry in `malformed::CORPUS`, measure how long the parser
//! takes to return `Err`. Slow rejection is a DoS surface — a malicious
//! payload that takes 100× longer to reject than a valid one to accept
//! is itself a vulnerability.
//!
//! Each iteration of each benchmark also asserts:
//!   1. The parser returns `Err` (never panics, never returns `Ok`).
//!   2. The error kind matches `expected.kind` if the fixture pins one.
//! So this file doubles as a regression suite for the parser's error
//! paths — drift in error reporting trips the assertion.

use bourne_bench::malformed::{Bad, CORPUS};
use bourne_core::Parser;
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

/// Drain the parser, returning the first `Err`. We use the streaming API
/// because it surfaces the lexer's error directly without the typed
/// dispatch wrapping it.
fn drive_until_err(input: &[u8]) -> bourne_core::Error {
    let mut p: Parser<'_> = Parser::new(input);
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("expected error, got end-of-input"),
            Err(e) => return e,
        }
    }
}

fn check(b: &Bad) -> bourne_core::Error {
    let err = drive_until_err(b.bytes);
    if let Some(expected) = b.kind {
        assert_eq!(
            err.kind, expected,
            "fixture {:?}: expected {expected:?}, got {:?}",
            b.name, err.kind,
        );
    }
    err
}

fn bench_malformed(c: &mut Criterion) {
    let mut group = c.benchmark_group("malformed");

    for bad in CORPUS {
        // Throughput in bytes so criterion can express MiB/s — useful for
        // spotting a malformed input whose rejection time scales with input
        // size much worse than a valid parse would (the DoS warning sign).
        group.throughput(Throughput::Bytes(bad.bytes.len() as u64));
        group.bench_function(bad.name, |b| {
            b.iter(|| {
                let e = check(black_box(bad));
                black_box(e);
            });
        });
    }

    // Head-to-head against serde_json on a representative subset. Picks
    // span the failure-mode classes:
    //   - truncated_string  : truncation mid-token
    //   - invalid_escape    : escape error
    //   - utf8_overlong_2byte : UTF-8 violation
    //   - control_char_in_string : control byte in string
    //   - depth_bomb_129    : depth-limit attack
    //
    // Both libs must reject without panicking; we don't compare error
    // kinds across libraries (different taxonomies) but we do assert
    // both return `Err`. The throughput numbers will be tiny (these are
    // small inputs) — what matters is the relative rejection latency.
    const COMPARE_SUBSET: &[&str] = &[
        "truncated_string",
        "invalid_escape",
        "utf8_overlong_2byte",
        "control_char_in_string",
        "depth_bomb_129",
    ];
    for &name in COMPARE_SUBSET {
        let bad = CORPUS
            .iter()
            .find(|b| b.name == name)
            .expect("compare-subset name must exist in CORPUS");
        group.throughput(Throughput::Bytes(bad.bytes.len() as u64));
        group.bench_function(format!("{name}/serde_json"), |b| {
            b.iter(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture {name:?}",
                );
                let _ = black_box(r);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_malformed);
criterion_main!(benches);
