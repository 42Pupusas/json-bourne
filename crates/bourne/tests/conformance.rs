//! `JSONTestSuite` conformance.
//!
//! Walks `tests/corpus/JSONTestSuite/` and asserts that every file's accept/reject
//! outcome matches its filename prefix:
//!   - `y_*`: must parse (any valid JSON value).
//!   - `n_*`: must fail.
//!   - `i_*`: implementation-defined — we record but do not assert.
//!
//! We "parse" by draining the streaming parser to EOF. The typed layer's value
//! choice is irrelevant; the question this corpus answers is whether the lexer
//! and parser accept exactly the strings RFC 8259 allows.

#![cfg(feature = "std")]

use json_bourne::Parser;
use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/JSONTestSuite")
}

fn parses(input: &[u8]) -> bool {
    let mut p: Parser<'_> = Parser::new(input);
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
}

#[derive(Default)]
struct Report {
    failures: Vec<String>,
    i_accepted: usize,
    i_rejected: usize,
    y_count: usize,
    n_count: usize,
}

#[test]
fn jsontestsuite_conformance() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "corpus missing at {} — did you forget to vendor JSONTestSuite?",
        dir.display(),
    );

    let mut report = Report::default();

    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();

    assert!(
        entries.len() >= 250,
        "expected ~318 corpus files, found {}",
        entries.len(),
    );

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(&path).unwrap();
        let accepted = parses(&bytes);

        match name.as_bytes()[0] {
            b'y' => {
                report.y_count += 1;
                if !accepted {
                    report
                        .failures
                        .push(format!("{name}: expected accept, got reject"));
                }
            }
            b'n' => {
                report.n_count += 1;
                if accepted {
                    report
                        .failures
                        .push(format!("{name}: expected reject, got accept"));
                }
            }
            b'i' => {
                if accepted {
                    report.i_accepted += 1;
                } else {
                    report.i_rejected += 1;
                }
            }
            _ => panic!("unexpected corpus filename prefix: {name}"),
        }
    }

    eprintln!(
        "JSONTestSuite: y={} n={} i_accepted={} i_rejected={} failures={}",
        report.y_count,
        report.n_count,
        report.i_accepted,
        report.i_rejected,
        report.failures.len(),
    );

    if !report.failures.is_empty() {
        for f in &report.failures {
            eprintln!("  FAIL: {f}");
        }
        panic!("{} conformance failures", report.failures.len());
    }
}
