//! Malformed-input rejection latency.
//!
//! For each entry in `malformed::CORPUS`, measure how long the parser
//! takes to return `Err`. Slow rejection is a `DoS` surface — a malicious
//! payload that takes 100x longer to reject than a valid one to accept
//! is itself a vulnerability.
//!
//! Each iteration of each benchmark also asserts:
//!   1. The parser returns `Err` (never panics, never returns `Ok`).
//!   2. The error kind matches `expected.kind` if the fixture pins one.
//!      So this file doubles as a regression suite for the parser's error
//!      paths — drift in error reporting trips the assertion.

use bourne_bench::malformed::{Bad, CORPUS};
use json_bourne::Parser;

fn main() {
    divan::main();
}

/// Drain the parser, returning the first `Err`. We use the streaming API
/// because it surfaces the lexer's error directly without the typed
/// dispatch wrapping it.
fn drive_until_err(input: &[u8]) -> json_bourne::Error {
    let mut p: Parser<'_> = Parser::new(input);
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("expected error, got end-of-input"),
            Err(e) => return e,
        }
    }
}

fn check(b: &Bad) -> json_bourne::Error {
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

// ---------------------------------------------------------------------------
// Full CORPUS — aggregate rejection time over all malformed inputs.
// There are too many entries to enumerate individually, so this single
// bench iterates all of them, measuring total rejection throughput.
// ---------------------------------------------------------------------------

#[divan::bench]
fn corpus_all(bencher: divan::Bencher) {
    let total_bytes: usize = CORPUS.iter().map(|b| b.bytes.len()).sum();
    bencher
        .counter(divan::counter::BytesCount::new(total_bytes))
        .bench(|| {
            for bad in CORPUS {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            }
        });
}

// ---------------------------------------------------------------------------
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
// ---------------------------------------------------------------------------

mod compare {
    use super::{Bad, CORPUS, check};

    fn find_bad(name: &str) -> &'static Bad {
        CORPUS
            .iter()
            .find(|b| b.name == name)
            .expect("compare-subset name must exist in CORPUS")
    }

    #[divan::bench]
    fn truncated_string_bourne(bencher: divan::Bencher) {
        let bad = find_bad("truncated_string");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            });
    }

    #[divan::bench]
    fn truncated_string_serde_json(bencher: divan::Bencher) {
        let bad = find_bad("truncated_string");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture \"truncated_string\"",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn invalid_escape_bourne(bencher: divan::Bencher) {
        let bad = find_bad("invalid_escape");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            });
    }

    #[divan::bench]
    fn invalid_escape_serde_json(bencher: divan::Bencher) {
        let bad = find_bad("invalid_escape");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture \"invalid_escape\"",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn utf8_overlong_2byte_bourne(bencher: divan::Bencher) {
        let bad = find_bad("utf8_overlong_2byte");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            });
    }

    #[divan::bench]
    fn utf8_overlong_2byte_serde_json(bencher: divan::Bencher) {
        let bad = find_bad("utf8_overlong_2byte");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture \"utf8_overlong_2byte\"",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn control_char_in_string_bourne(bencher: divan::Bencher) {
        let bad = find_bad("control_char_in_string");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            });
    }

    #[divan::bench]
    fn control_char_in_string_serde_json(bencher: divan::Bencher) {
        let bad = find_bad("control_char_in_string");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture \"control_char_in_string\"",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn depth_bomb_129_bourne(bencher: divan::Bencher) {
        let bad = find_bad("depth_bomb_129");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let e = check(divan::black_box(bad));
                divan::black_box(e);
            });
    }

    #[divan::bench]
    fn depth_bomb_129_serde_json(bencher: divan::Bencher) {
        let bad = find_bad("depth_bomb_129");
        bencher
            .counter(divan::counter::BytesCount::new(bad.bytes.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(bad.bytes));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture \"depth_bomb_129\"",
                );
                let _ = divan::black_box(r);
            });
    }
}

// ---------------------------------------------------------------------------
// Large malformed inputs — the DoS-realistic shape.
//
// Every fixture in CORPUS is under ~200 bytes. Real attacker payloads
// are megabytes: a 10 MB string with a single bad escape at the end,
// a 5 MB number followed by trailing garbage. The bench numbers on
// tiny inputs tell us nothing about whether rejection latency scales
// linearly with input (acceptable) or quadratically (a vulnerability).
// ---------------------------------------------------------------------------

mod large {
    use super::Parser;

    // 10 MB JSON string with a bare backslash at the end and no closing
    // quote. The lexer must walk the entire body before failing on the
    // truncated escape. Linear rejection ~= a few ms; an O(n^2) bug would
    // balloon to seconds.
    fn make_huge_string_bad_tail() -> Vec<u8> {
        let mut buf = Vec::with_capacity(10 * 1024 * 1024 + 4);
        buf.push(b'"');
        buf.resize(10 * 1024 * 1024 + 1, b'a');
        buf.push(b'\\');
        buf
    }

    // 5 MB digit run followed by trailing garbage. Probes the number-
    // parsing path's ability to handle a wide literal *and* still report
    // the trailing-byte error correctly.
    fn make_huge_number_trailing_garbage() -> Vec<u8> {
        let mut buf = Vec::with_capacity(5 * 1024 * 1024 + 8);
        buf.push(b'1');
        buf.resize(5 * 1024 * 1024, b'7');
        buf.extend_from_slice(b" QQ");
        buf
    }

    // 1 M legal escape sequences (`\n`) inside a string, terminated by an
    // illegal escape (`\q`). Forces a walk of the full sequence before
    // hitting the bad one.
    fn make_huge_escape_run_bad_tail() -> Vec<u8> {
        let mut buf = Vec::with_capacity(1024 * 1024 * 2 + 8);
        buf.push(b'"');
        for _ in 0..(1024 * 1024) {
            buf.push(b'\\');
            buf.push(b'n');
        }
        buf.extend_from_slice(b"\\q\"");
        buf
    }

    #[divan::bench]
    fn huge_string_bad_tail_10mb_bourne(bencher: divan::Bencher) {
        let data = make_huge_string_bad_tail();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let bytes = &data[..];
                let mut p: Parser<'_> = Parser::new(divan::black_box(bytes));
                let mut got_err = false;
                loop {
                    match p.next_event() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => {
                            got_err = true;
                            break;
                        }
                    }
                }
                assert!(got_err, "parser accepted invalid input");
            });
    }

    #[divan::bench]
    fn huge_string_bad_tail_10mb_serde_json(bencher: divan::Bencher) {
        let data = make_huge_string_bad_tail();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(&data));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn huge_number_trailing_garbage_5mb_bourne(bencher: divan::Bencher) {
        let data = make_huge_number_trailing_garbage();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let bytes = &data[..];
                let mut p: Parser<'_> = Parser::new(divan::black_box(bytes));
                let mut got_err = false;
                loop {
                    match p.next_event() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => {
                            got_err = true;
                            break;
                        }
                    }
                }
                assert!(got_err, "parser accepted invalid input");
            });
    }

    #[divan::bench]
    fn huge_number_trailing_garbage_5mb_serde_json(bencher: divan::Bencher) {
        let data = make_huge_number_trailing_garbage();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(&data));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture",
                );
                let _ = divan::black_box(r);
            });
    }

    #[divan::bench]
    fn huge_escape_run_bad_tail_1m_escapes_bourne(bencher: divan::Bencher) {
        let data = make_huge_escape_run_bad_tail();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let bytes = &data[..];
                let mut p: Parser<'_> = Parser::new(divan::black_box(bytes));
                let mut got_err = false;
                loop {
                    match p.next_event() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => {
                            got_err = true;
                            break;
                        }
                    }
                }
                assert!(got_err, "parser accepted invalid input");
            });
    }

    #[divan::bench]
    fn huge_escape_run_bad_tail_1m_escapes_serde_json(bencher: divan::Bencher) {
        let data = make_huge_escape_run_bad_tail();
        bencher
            .counter(divan::counter::BytesCount::new(data.len()))
            .bench(|| {
                let r: Result<serde_json::Value, _> =
                    serde_json::from_slice(divan::black_box(&data));
                assert!(
                    r.is_err(),
                    "serde_json unexpectedly accepted malformed fixture",
                );
                let _ = divan::black_box(r);
            });
    }
}
