//! Exercises the bounded reserve-window arm of `<[T] as ToJson>::write_json`
//! (audit 4.5.2's fix): arrays whose upper-bound estimate exceeds
//! `RESERVE_WINDOW` must chunk their hints and stay byte-identical to the
//! small-array output. First flagged by the CRAP gate (audit F4).

use crate::to_string;

fn expect_array_json(v: &[i64]) {
    let got = to_string(v).expect("serialize");

    let mut want = String::from("[");
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            want.push(',');
        }
        want.push_str(&n.to_string());
    }
    want.push(']');
    assert_eq!(got, want);
}

/// Just under the window boundary: single-hint fast path.
#[test]
fn array_just_under_reserve_window_matches_plain_output() {
    let v: Vec<i64> = (0..40_000).map(i64::from).collect();
    expect_array_json(&v);
}

/// Just past the boundary: the chunked-hint cold arm, plus a multi-window
/// case that loops it.
#[test]
fn array_over_reserve_window_chunks_hints_and_stays_correct() {
    let v: Vec<i64> = (0..40_002).map(i64::from).collect();
    expect_array_json(&v);
}

/// A long enough run loops the chunked path several times.
#[test]
fn array_multi_window_loops_the_chunked_path() {
    let v: Vec<i64> = (0..80_004).map(i64::from).collect();
    expect_array_json(&v);
}

/// Wide elements cross the window in a handful of chunks.
#[test]
fn array_of_wide_elements_crosses_window_in_few_chunks() {
    let v: Vec<f64> = (0..8_000).map(|i| f64::from(i) + 0.5).collect();
    let got = to_string(&v).expect("serialize");
    assert_eq!(got.chars().next(), Some('['));
    assert_eq!(got.chars().last(), Some(']'));
    assert_eq!(got.matches(',').count(), v.len() - 1);
}
