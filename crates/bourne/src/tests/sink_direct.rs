//! Direct-sink coverage. Every public sink type (`StringSink`,
//! `PrettyStringSink`, `ByteSink` hinted methods) gets at least one
//! direct call, plus the custom-`JsonWrite`-impl contracts: honest
//! non-finite delivery and `write_raw_bytes` UTF-8 rejection.

use crate::{
    ByteSink, JsonWrite, PrettyStringSink, StringSink, ToJson, to_fmt, to_string, to_string_pretty,
};
use alloc::string::String;
use alloc::vec::Vec;

/// `StringSink` — the `&mut String` sink. Exercises `write_byte`,
/// `write_str_raw`, and `write_float_f64` directly.
#[test]
fn string_sink_writes_directly() {
    let mut out = String::new();
    let mut sink = StringSink::new(&mut out);
    // write_byte and write_str_raw via the trait
    sink.write_byte(b'[').unwrap();
    sink.write_str_raw("1").unwrap();
    sink.write_byte(b',').unwrap();
    // write_float_f64 (StringSink override)
    sink.write_float_f64(2.5).unwrap();
    sink.write_byte(b']').unwrap();
    assert_eq!(out, "[1,2.5]");

    // Non-finite rejection through the StringSink path.
    let mut out = String::new();
    let mut sink = StringSink::new(&mut out);
    assert!(sink.write_float_f64(f64::NAN).is_err());
}

/// `PrettyStringSink::with_indent` — custom indent variant.
/// Existing pretty tests only cover the default-indent constructor.
#[test]
fn pretty_sink_with_indent_uses_custom_indent() {
    let mut out = String::new();
    let mut sink = PrettyStringSink::with_indent(&mut out, "\t");
    let data = (1_i32, 2_i32, 3_i32);
    data.write_json(&mut sink).unwrap();
    // Tab indent → contains a `\n\t` sequence.
    assert!(out.contains("\n\t"), "got: {out:?}");
}

/// Audit 3.14: a custom sink whose `write_float_f64` tolerates NaN
/// must never receive a fabricated `NaN` from the `f64`/`f32`
/// `write_json` error paths — it receives the actual value, which
/// it may serialize (this sink does), but never a placeholder.
struct NaNTolerantSink {
    out: String,
    saw_bytes: bool,
}

impl JsonWrite for NaNTolerantSink {
    type Error = crate::Error;

    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        self.out.push(b as char);
        Ok(())
    }

    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.push_str(s);
        Ok(())
    }

    fn on_invalid_utf8(&mut self) -> Self::Error {
        crate::Error::new(crate::ErrorKind::InvalidUtf8, crate::Position::START)
    }

    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        if f.is_nan() {
            // Tolerant: record the sentinel so the test can tell a
            // fabricated NaN from an honest copy of the input.
            self.saw_bytes = true;
            self.out.push_str("nan-sentinel");
            return Ok(());
        }
        if f.is_infinite() {
            // The in-tree formatter asserts finiteness; render the
            // infinity directly to observe what arrived.
            self.out.push_str(if f > 0.0 { "inf" } else { "-inf" });
            return Ok(());
        }
        crate::float::format_finite(f, &mut self.out);
        Ok(())
    }
}

#[test]
fn non_finite_error_delivers_actual_value_not_fabricated_nan() {
    let mut sink = NaNTolerantSink {
        out: String::new(),
        saw_bytes: false,
    };
    // -inf, not NaN: a fabricated placeholder would surface as the
    // `nan-sentinel` bytes; the honest path hands over -inf.
    let f = f64::NEG_INFINITY;
    f.write_json(&mut sink).unwrap();
    assert!(!sink.saw_bytes, "fabricated NaN reached the sink");
    assert_eq!(sink.out, "-inf");

    // NaN itself round-trips through the tolerant sink as NaN.
    let mut sink = NaNTolerantSink {
        out: String::new(),
        saw_bytes: false,
    };
    f64::NAN.write_json(&mut sink).unwrap();
    assert!(sink.saw_bytes);
}

/// Audit 3.15: `write_raw_bytes` with non-UTF-8 input returns `Err`
/// instead of panicking — a custom `ToJson` impl feeding arbitrary
/// bytes must not be able to panic the serializer.
#[test]
fn write_raw_bytes_rejects_non_utf8_with_err() {
    struct BareSink {
        out: String,
    }
    impl JsonWrite for BareSink {
        type Error = crate::Error;

        fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
            self.out.push(b as char);
            Ok(())
        }
        fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
            self.out.push_str(s);
            Ok(())
        }
        fn on_invalid_utf8(&mut self) -> Self::Error {
            crate::Error::new(crate::ErrorKind::InvalidUtf8, crate::Position::START)
        }
        fn write_float_f64(&mut self, _f: f64) -> Result<(), Self::Error> {
            unimplemented!()
        }
    }

    let mut sink = BareSink { out: String::new() };
    // 0x80 alone is never valid UTF-8.
    let err = sink.write_raw_bytes(&[b'h', 0x80, b'i']).unwrap_err();
    assert_eq!(err.kind, crate::ErrorKind::InvalidUtf8);
    // Valid input still writes through.
    sink.write_raw_bytes(b"ok").unwrap();
    assert_eq!(sink.out, "ok");
}

/// `PrettyStringSink::write_float_f64` — covers the pretty-sink
/// float arm (existing tests use the simple-byte and string paths).
#[test]
fn pretty_sink_handles_floats() {
    let v: alloc::vec::Vec<f64> = alloc::vec![1.5, 2.5, 3.0];
    let s = to_string_pretty(v.as_slice()).unwrap();
    assert!(s.contains("1.5"));
    assert!(s.contains("3.0"));
}

/// `ByteSink::write_float_f64` — non-finite returns the typed error
/// without touching the buffer.
#[test]
fn bytesink_float_nonfinite_returns_error() {
    let mut out: Vec<u8> = Vec::with_capacity(64);
    let mut sink = ByteSink::new(&mut out);
    let r = sink.write_float_f64(f64::INFINITY);
    assert!(r.is_err(), "non-finite must error");
    assert!(out.is_empty(), "failed write must not emit bytes");
}

/// `JsonWrite::write_float_f64_hinted` — with reserved capacity the
/// write goes through the hinted path and round-trips exactly.
#[test]
fn bytesink_hinted_finite_writes_value() {
    let mut out: Vec<u8> = Vec::with_capacity(64);
    let mut sink = ByteSink::new(&mut out);
    // Pick a finite value that round-trips exactly through the formatter
    // and `f64::parse`. 2.5 is exact in binary; avoiding 3.14 also dodges
    // clippy's approx_constant warning about PI.
    let value = 2.5_f64;
    sink.reserve_hint(32);
    assert!(sink.write_float_f64_hinted(value).unwrap());
    let s = core::str::from_utf8(&out).unwrap();
    let parsed: f64 = s.parse().unwrap();
    // Bit-pattern compare: write→parse must round-trip exactly.
    assert_eq!(parsed.to_bits(), value.to_bits());
}

/// `ToJson for Path` (std-only) — covers `Path::write_json`.
#[cfg(feature = "std")]
#[test]
fn path_serializes_via_to_string() {
    use std::path::Path;
    let p = Path::new("/tmp/file.txt");
    let s = to_string(p).unwrap();
    assert_eq!(s, "\"/tmp/file.txt\"");
}

/// `MapKeyOut for Cow<'_, str>` — used when a HashMap/BTreeMap
/// key type is `Cow<'_, str>`. Exercises `Cow::as_str`.
#[test]
fn map_with_cow_str_keys_serializes() {
    use alloc::borrow::Cow;
    use alloc::collections::BTreeMap;
    let mut m: BTreeMap<Cow<'_, str>, i32> = BTreeMap::new();
    m.insert(Cow::Borrowed("a"), 1);
    m.insert(Cow::Owned(String::from("b")), 2);
    let s = to_string(&m).unwrap();
    assert!(s.contains(r#""a":1"#));
    assert!(s.contains(r#""b":2"#));
}

/// The f64 `NEEDS_VALIDATION` machinery was removed with the reserved-
/// write path; keep a marker test so the behavior stays pinned: slice
/// writes succeed for finite input and error (not corrupt) on non-finite.
#[test]
fn f64_slice_writes_and_errors_on_nonfinite() {
    let slice: &[f64] = &[1.0, 2.0, 3.0];
    assert_eq!(to_string(slice).unwrap(), "[1.0,2.0,3.0]");
    let bad: &[f64] = &[1.0, f64::INFINITY];
    assert!(to_string(bad).is_err());
}

/// `crate::ser::float::format_f64_write` and `reject_non_finite`
/// are exercised by `StringSink::write_float_f64` (via the `float::`
/// module). Existing tests cover that, but cover them explicitly
/// for the non-finite branch through the public `to_fmt` entry
/// point which uses `FmtWriteSink` (a different code path).
#[test]
fn to_fmt_writes_finite_and_rejects_nonfinite() {
    let mut s = String::new();
    to_fmt(&1.5_f64, &mut s).unwrap();
    assert_eq!(s, "1.5");

    let mut s = String::new();
    assert!(to_fmt(&f64::INFINITY, &mut s).is_err());
}

/// `fmt_write_error` is invoked when the underlying `core::fmt::Write`
/// impl returns an error. Construct a writer that always fails and
/// verify `to_fmt` propagates a typed `Error` (rather than the
/// detail-less `fmt::Error`).
#[test]
fn to_fmt_propagates_underlying_write_failure() {
    use core::fmt;

    /// Writer that errors on every call — drives `fmt_write_error`.
    struct AlwaysFail;
    impl fmt::Write for AlwaysFail {
        fn write_str(&mut self, _s: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    // Any non-empty serializable value flushes at least one byte
    // through `write_str` / `write_char`, which the writer rejects.
    let mut sink = AlwaysFail;
    let r = to_fmt(&42_i64, &mut sink);
    assert!(r.is_err(), "expected propagated error from failing writer");

    // Also exercise the float path through FmtWriteSink, which uses
    // a separate `fmt_write_error()` call site.
    let mut sink = AlwaysFail;
    let r = to_fmt(&1.5_f64, &mut sink);
    assert!(r.is_err());
}

/// Regression for audit 3.9: the default `write_escaped_str` pushed
/// every byte through `write_byte`, and `FmtWriteSink::write_byte`
/// is char-oriented (`write_char(b as char)`), so UTF-8 continuation
/// bytes became Latin-1 mojibake (`é` → `Ã©`). All sinks must agree
/// byte-for-byte on strings that mix escapes with non-ASCII.
#[test]
fn all_sinks_agree_on_non_ascii_and_escapes() {
    let cases = [
        "plain ascii",
        "café ñ€𝄞",
        "tab\tnewline\nquote\"backslash\\",
        "é\"é\\é\né",
        "100% ctrl-less unicode ✓ ✓ ✓",
        "\u{7f}\u{1f}\u{0}",
    ];
    for case in cases {
        let expected = to_string(case).expect("StringSink");
        assert_eq!(expected, format!("\"{}\"", escaped_debug(case)));

        let mut fmt_out = String::new();
        to_fmt(case, &mut fmt_out).expect("FmtWriteSink");
        assert_eq!(fmt_out, expected, "FmtWriteSink diverged for {case:?}");

        let mut byte_out: Vec<u8> = Vec::new();
        let mut sink = ByteSink::new(&mut byte_out);
        case.write_json(&mut sink).expect("ByteSink");
        assert_eq!(
            core::str::from_utf8(&byte_out).unwrap(),
            expected,
            "ByteSink diverged for {case:?}"
        );

        #[cfg(feature = "std")]
        {
            use crate::to_writer;
            let mut io_out = Vec::<u8>::new();
            to_writer(case, &mut io_out).expect("IoWriteSink");
            assert_eq!(
                String::from_utf8(io_out).unwrap(),
                expected,
                "IoWriteSink diverged for {case:?}"
            );
        }

        let mut pretty_out = String::new();
        let mut sink = PrettyStringSink::new(&mut pretty_out);
        case.write_json(&mut sink).expect("PrettyStringSink");
        assert_eq!(
            pretty_out, expected,
            "PrettyStringSink diverged for {case:?}"
        );
    }
}

/// Build the expected JSON string body the same way the doc example
/// does — via an independent escape walk over chars, not via the
/// code under test.
fn escaped_debug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                use core::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
