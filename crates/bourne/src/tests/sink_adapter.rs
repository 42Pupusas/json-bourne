//! Sink-adapter tests: `to_writer` (`io::Write`) and `to_fmt` (`fmt::Write`)
//! must produce identical bytes to the canonical `to_string` path, and the
//! pretty sink's indentation is pinned here.

use crate::{to_fmt, to_string, to_string_pretty};

#[test]
fn fmt_sink_matches_to_string_for_struct() {
    let m = vec![("a", 1_i32), ("b", 2)]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let canonical = to_string(&m).unwrap();
    let mut out = String::new();
    to_fmt(&m, &mut out).unwrap();
    assert_eq!(out, canonical);
}

#[test]
fn fmt_sink_handles_floats_with_grisu3() {
    let canonical = to_string(&1.5_f64).unwrap();
    let mut out = String::new();
    to_fmt(&1.5_f64, &mut out).unwrap();
    assert_eq!(out, canonical);
}

#[test]
fn fmt_sink_rejects_non_finite() {
    let mut out = String::new();
    let err = to_fmt(&f64::INFINITY, &mut out).unwrap_err();
    assert_eq!(err.kind, crate::ErrorKind::NonFiniteFloat);
}

#[cfg(feature = "std")]
#[test]
fn io_writer_matches_to_string_for_struct() {
    use crate::to_writer;
    let v = vec![1_i32, 2, 3];
    let canonical = to_string(&v).unwrap();
    let mut buf = Vec::<u8>::new();
    to_writer(&v, &mut buf).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), canonical);
}

#[cfg(feature = "std")]
#[test]
fn io_writer_handles_floats() {
    use crate::to_writer;
    let canonical = to_string(&-2.7e-5_f64).unwrap();
    let mut buf = Vec::<u8>::new();
    to_writer(&-2.7e-5_f64, &mut buf).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), canonical);
}

#[cfg(feature = "std")]
#[test]
fn io_writer_rejects_non_finite() {
    use crate::to_writer;
    let mut buf = Vec::<u8>::new();
    let err = to_writer(&f64::NAN, &mut buf).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn pretty_empty_object_compact() {
    let m: std::collections::BTreeMap<String, i32> = std::collections::BTreeMap::new();
    assert_eq!(to_string_pretty(&m).unwrap(), "{}");
}

#[test]
fn pretty_empty_array_compact() {
    let v: Vec<i32> = Vec::new();
    assert_eq!(to_string_pretty(&v).unwrap(), "[]");
}

#[test]
fn pretty_array_indents_two_spaces() {
    let v = vec![1_i32, 2, 3];
    assert_eq!(to_string_pretty(&v).unwrap(), "[\n  1,\n  2,\n  3\n]");
}

#[test]
fn pretty_object_keys_have_one_space_after_colon() {
    let m: std::collections::BTreeMap<&str, i32> = [("a", 1), ("b", 2)].into_iter().collect();
    assert_eq!(
        to_string_pretty(&m).unwrap(),
        "{\n  \"a\": 1,\n  \"b\": 2\n}",
    );
}

#[test]
fn pretty_nested_indents_proportionally() {
    let v: Vec<Vec<i32>> = vec![vec![1, 2], vec![3]];
    assert_eq!(
        to_string_pretty(&v).unwrap(),
        "[\n  [\n    1,\n    2\n  ],\n  [\n    3\n  ]\n]",
    );
}

#[test]
fn pretty_round_trips_through_compact_parse() {
    use crate::parse_str;
    let m: std::collections::BTreeMap<String, Vec<i32>> = [
        (String::from("a"), vec![1, 2]),
        (String::from("b"), vec![3]),
    ]
    .into_iter()
    .collect();
    let pretty = to_string_pretty(&m).unwrap();
    // Whitespace is irrelevant to the parser; the pretty form must
    // re-parse to the same map.
    let back: std::collections::BTreeMap<String, Vec<i32>> = parse_str(&pretty).unwrap();
    assert_eq!(back, m);
}

#[cfg(feature = "std")]
#[test]
fn io_writer_propagates_underlying_error() {
    use crate::to_writer;
    // A writer that always errors: assert the io::Error reaches us
    // unmolested instead of getting flattened to a generic kind.
    struct FailingWriter;
    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = FailingWriter;
    let err = to_writer(&"hi", &mut w).unwrap_err();
    assert_eq!(err.to_string(), "disk full");
}
