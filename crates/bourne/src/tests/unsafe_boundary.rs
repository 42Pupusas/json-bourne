//! Boundary tests for the public surface — pinned at the raw-tail write
//! in `ByteSink` (guarded by a live capacity check) and the
//! `from_utf8_unchecked` site in `decode_escapes`. These are designed to
//! run under miri (CI: `MIRIFLAGS=-Zmiri-disable-isolation
//! RUSTFLAGS=--cfg bourne_no_simd cargo +nightly miri test -p json-bourne --lib`).
//! Each one targets a specific invariant; if a future refactor breaks the
//! capacity check or the UTF-8 boundary, miri here trips
//! on the precise unsafe before any user code does.

use crate::{ByteSink, ErrorKind, JsonWrite, Lexer, ToJson, parse_str, to_string, to_vec};
use alloc::string::String;
use alloc::vec::Vec;

/// Slice-of-floats ser at the exact-capacity boundary. The slice
/// writer's `reserve_hint` computes
///   `2 + n * (MAX_SERIALIZED_LEN + 1) = 2 + n * 33`.
/// We pre-reserve exactly that, so the per-element
/// `write_float_f64_hinted` tail-write path (which needs ≥ 32 spare
/// bytes) runs against the tightest legal Vec capacity — under-reserving
/// by even one byte would degrade to the checked path, which these
/// round-trips would not detect, so the boundary itself is what miri
/// verifies.
#[test]
fn bytesink_slice_floats_exact_capacity() {
    for n in [0usize, 1, 2, 3, 7, 16, 17, 32, 33] {
        #[allow(clippy::cast_precision_loss)]
        let data: Vec<f64> = (0..n).map(|i| i as f64 + 0.5).collect();
        let mut out: Vec<u8> = Vec::with_capacity(2 + n * 33);
        let mut sink = ByteSink::new(&mut out);
        (data.as_slice()).write_json(&mut sink).expect("ser");
        let s = core::str::from_utf8(&out).expect("ASCII");
        // Sanity: parse back as Vec<f64>.
        let back: Vec<f64> = parse_str(s).expect("parse back");
        assert_eq!(back.len(), n, "n={n}");
        #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
        for (i, &v) in back.iter().enumerate() {
            assert_eq!(v, i as f64 + 0.5, "n={n} i={i}");
        }
    }
}

/// Same as above but for `Vec<f32>`. f32 widens to f64 in the writer
/// and shares the hinted tail-write path, so the capacity boundary is
/// identical.
#[test]
fn bytesink_slice_f32_exact_capacity() {
    for n in [0usize, 1, 2, 16, 17] {
        #[allow(clippy::cast_precision_loss)]
        let data: Vec<f32> = (0..n).map(|i| i as f32 + 0.25).collect();
        let mut out: Vec<u8> = Vec::with_capacity(2 + n * 33);
        let mut sink = ByteSink::new(&mut out);
        (data.as_slice()).write_json(&mut sink).expect("ser");
        let s = core::str::from_utf8(&out).expect("ASCII");
        let back: Vec<f32> = parse_str(s).expect("parse back");
        assert_eq!(back.len(), n);
        #[allow(clippy::cast_precision_loss, clippy::float_cmp)]
        for (i, &v) in back.iter().enumerate() {
            assert_eq!(v, i as f32 + 0.25, "n={n} i={i}");
        }
    }
}

/// Slice of f64 containing non-finite values — verifies the per-element
/// hinted float write surfaces `NonFiniteFloat` through the sink and
/// emits nothing for the offending element.
#[test]
fn bytesink_slice_nonfinite_errors_without_partial_element() {
    // The non-finite path writes substitute bytes into the Vec before
    // reporting the error. Miri ensures those substitute writes stay
    // within the reserved capacity.
    for (name, bad_idx, bad_val) in [
        ("inf at 0", 0usize, f64::INFINITY),
        ("nan at end", 4usize, f64::NAN),
        ("neg_inf middle", 2usize, f64::NEG_INFINITY),
    ] {
        let mut data: Vec<f64> = (0..5).map(f64::from).collect();
        data[bad_idx] = bad_val;
        let r = to_string(data.as_slice());
        assert!(r.is_err(), "{name}: expected non-finite error");
    }
}

/// Slice of i64 — exercises `write_byte_hinted` for `[`/`,`/`]` at
/// exact capacity. The hint math is the same as the float case; this
/// pins the integer shape (20-byte worst case) of the same path.
#[test]
fn bytesink_slice_ints_exact_capacity() {
    // MAX_SERIALIZED_LEN for i64 = 20 ("-9223372036854775808"), so
    // hint = 2 + n * 21.
    for n in [0usize, 1, 5, 16, 17] {
        #[allow(clippy::cast_possible_wrap)]
        let n_i64 = n as i64;
        #[allow(clippy::cast_possible_wrap)]
        let data: Vec<i64> = (0..n).map(|i| (i as i64) - n_i64 / 2).collect();
        let mut out: Vec<u8> = Vec::with_capacity(2 + n * 21);
        let mut sink = ByteSink::new(&mut out);
        (data.as_slice()).write_json(&mut sink).expect("ser");
        let s = core::str::from_utf8(&out).expect("ASCII");
        let back: Vec<i64> = parse_str(s).expect("parse back");
        assert_eq!(back, data, "n={n}");
    }
}

/// `decode_escapes`' literal-byte run uses `from_utf8_unchecked` on
/// the stretch between escapes. The lexer's UTF-8 invariant must hold
/// across multi-byte sequences. Test escapes interleaved with 2/3/4-byte
/// UTF-8 chars so the literal chunk passed to the unsafe spans
/// multibyte data.
#[test]
fn decode_escapes_multibyte_in_literal_chunk() {
    // 2-byte (é = c3 a9), 3-byte (€ = e2 82 ac), 4-byte (𝄞 = f0 9d 84 9e)
    // chars surrounding `\n` escapes. The literal chunks bracket the
    // escape and must contain valid UTF-8.
    let cases: &[(&str, &str)] = &[
        (r#""café\nfin""#, "café\nfin"),
        (r#""price: 5€\tea""#, "price: 5€\tea"),
        (r#""note 𝄞\nplayed""#, "note 𝄞\nplayed"),
        // Many escapes between multibyte stretches.
        (r#""éé\nçç\tüü""#, "éé\nçç\tüü"),
        // Escape at start / end with multibyte in middle.
        (r#""\n𝄞café\t""#, "\n𝄞café\t"),
        // Long literal run before single escape (exercises the SIMD
        // scan tail).
        (
            r#""aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaéé\n""#,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaéé\n",
        ),
    ];
    for (json, expected) in cases {
        let s: String = parse_str(json).expect("parse");
        assert_eq!(s, *expected, "case {json}");
    }
}

/// Empty slice / array — `write_array_hinted`'s `split_first` is None,
/// no per-element writes. Just `[` and `]`.
#[test]
fn bytesink_empty_slice_writes_brackets_only() {
    let empty: Vec<i64> = Vec::new();
    let s = to_string(empty.as_slice()).expect("ser");
    assert_eq!(s, "[]");

    let empty: Vec<f64> = Vec::new();
    let s = to_string(empty.as_slice()).expect("ser");
    assert_eq!(s, "[]");
}

/// Single-element slice — `write_array_reserved`'s `split_first` returns
/// `(first, [])` so we skip the inner loop. Verify the byte boundary.
#[test]
fn bytesink_single_element_slice() {
    let one = [42_i64];
    assert_eq!(to_string(one.as_slice()).unwrap(), "[42]");
    let one = [1.5_f64];
    assert_eq!(to_string(one.as_slice()).unwrap(), "[1.5]");
    let one = [1.5_f32];
    assert_eq!(to_string(one.as_slice()).unwrap(), "[1.5]");
}

/// Empty string — `decode_escapes` is called with `raw=&[]`. The
/// `from_utf8_unchecked(&raw[start..i])` slice is empty, which is
/// a degenerate edge case that must not OOB.
#[test]
fn decode_escapes_empty_string() {
    let s: String = parse_str(r#""""#).expect("parse");
    assert_eq!(s, "");
}

/// Pure-ASCII without any escape — the literal-byte run covers the
/// whole input and `find_backslash` returns None. The unsafe slice
/// is `&raw[0..raw.len()]`.
#[test]
fn decode_escapes_no_escapes_pure_ascii() {
    // Use `Vec<String>` to force the owned-decode path even though
    // the input has no escapes (the `&str` impl would borrow).
    let v: Vec<String> = parse_str(r#"["hello world","no escapes here"]"#).expect("parse");
    assert_eq!(
        v,
        [String::from("hello world"), String::from("no escapes here")]
    );
}

/// Decode an escape immediately at the start — the first literal-byte
/// run is empty, so `from_utf8_unchecked(&raw[0..0])` is called.
#[test]
fn decode_escapes_escape_at_start() {
    let v: Vec<String> = parse_str(r#"["\nhello","\tworld"]"#).expect("parse");
    assert_eq!(v, [String::from("\nhello"), String::from("\tworld")]);
}

/// Decode an escape immediately at the end — after the escape there is
/// no literal-byte run.
#[test]
fn decode_escapes_escape_at_end() {
    let v: Vec<String> = parse_str(r#"["hello\n","world\t"]"#).expect("parse");
    assert_eq!(v, [String::from("hello\n"), String::from("world\t")]);
}

/// Back-to-back escapes — multiple zero-length literal chunks.
#[test]
fn decode_escapes_consecutive_escapes() {
    let v: Vec<String> = parse_str(r#"["\n\t\r\\","\"\"\""]"#).expect("parse");
    assert_eq!(v, [String::from("\n\t\r\\"), String::from(r#"""""#)]);
}

/// Regression for the §3.1 audit finding: the slice writer used to
/// take `T::MAX_SERIALIZED_LEN` (a safe const any impl could lie
/// about) as a hard precondition for raw unchecked tail writes, so a
/// lying impl caused a heap overflow. The hinted path now falls back
/// to checked writes when the tail is exhausted — a wrong bound costs
/// a reallocation, never memory safety.
#[test]
fn slice_writer_survives_lying_max_serialized_len() {
    #[derive(Debug, PartialEq)]
    struct Liar(i64);
    impl ToJson for Liar {
        const MIN_SERIALIZED_LEN: usize = 1;
        // Deliberately wrong: elements emit up to 20 bytes.
        const MAX_SERIALIZED_LEN: usize = 1;
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            self.0.write_json(w)
        }
    }
    let data: Vec<Liar> = (0..500)
        .map(|i| Liar(i64::from(i) * 1_000_000_000 - 250_000_000_000))
        .collect();
    let s = to_string(data.as_slice()).expect("serialize despite lying bound");
    let back: Vec<i64> = parse_str(&s).expect("round-trip");
    let expected: Vec<i64> = (0..500)
        .map(|i| i64::from(i) * 1_000_000_000 - 250_000_000_000)
        .collect();
    assert_eq!(back, expected);
}

/// Audit 4.4.5: `ByteSink` does not validate `write_raw_bytes`, so a
/// hand-written `ToJson` impl can put non-UTF-8 bytes into `to_vec`.
/// `to_string` — the one API promising a `String` — must report that
/// as `Err(InvalidUtf8)` instead of panicking on the `expect`.
#[test]
fn to_string_reports_non_utf8_from_user_impl_as_err() {
    struct RawBytes(u8);
    impl ToJson for RawBytes {
        const MIN_SERIALIZED_LEN: usize = 3;
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_raw_bytes(&[b'"', self.0, b'"'])
        }
    }

    let err = to_string(&RawBytes(0x80)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidUtf8);
    // The bytes are still intact in to_vec for byte-oriented callers.
    assert_eq!(to_vec(&RawBytes(b'x')).unwrap(), b"\"x\"");
}

/// Audit 3.15: the `InputTooLarge` kind carries a helpful message,
/// and `try_new` accepts normal inputs through the `Result` API.
#[test]
fn oversized_input_api_returns_err_not_panic() {
    let lex: Lexer<'_> = Lexer::try_new(b"[1]").expect("small input parses");
    assert_eq!(lex.input().len(), 3);
    assert_eq!(
        ErrorKind::InputTooLarge.to_string(),
        "input exceeds the maximum supported JSON document size"
    );
}
