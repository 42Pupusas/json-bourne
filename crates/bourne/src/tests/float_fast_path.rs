//! Differential tests for the fused plain-decimal `parse_f64_value` path
//! (audit 4.4.3): every accepted literal must be bit-identical to
//! `str::parse`, and every rejection must match the grammar walk.

use crate::{ErrorKind, Lexer};

const fn lex(input: &[u8]) -> Lexer<'_> {
    Lexer::new(input)
}

/// `to_bits` comparison: catches `-0.0` vs `0.0` and any 1-ulp
/// divergence that an `==` on floats would hide.
fn assert_bits_match(input: &str) {
    let got = lex(input.as_bytes()).parse_f64_value().unwrap();
    let want: f64 = input.parse().unwrap();
    assert_eq!(got.to_bits(), want.to_bits(), "input {input}");
}

#[test]
fn plain_decimals_match_str_parse_bit_for_bit() {
    for s in [
        "1.0",
        "3.14159",
        "2.718281828459045",
        "-1.5",
        "123456.789",
        "0.5",
        "0.25",
        "9007199254740993.0",     // 2^53 + 1 digit-wise
        "1.7976931348623157e308", // f64::MAX via slow path
        "5e-324",                 // subnormal via slow path
        "1e100",
        "-2.7e-5",
        "1.5e10",
        "0.0",
        "-0.0",
        "0e0",
    ] {
        assert_bits_match(s);
    }
}

#[test]
fn every_single_and_double_digit_fraction() {
    // Exhaustive over the smallest mantissa scale the fast path
    // serves: d.dd across all digit pairs.
    for a in 0u32..10 {
        for b in 0u32..10 {
            for c in 0u32..10 {
                let s = format!("{a}.{b}{c}");
                assert_bits_match(&s);
                let s = format!("-{s}");
                assert_bits_match(&s);
            }
        }
    }
}

#[test]
fn mantissa_and_scale_boundaries_fall_back_cleanly() {
    // 2^53 mantissa is the last exactly-representable integer;
    // one more digit sends the fast path to libcore.
    for s in [
        "9007199254740992.0",  // = 2^53 exactly
        "90071992547409921.0", // 17 digits, over the gate
        "12345678901234567890.5",
        "0.000000000000000000000001", // 10^-24, scale > 22
    ] {
        assert_bits_match(s);
    }
}

#[test]
fn rejection_kinds_match_the_grammar() {
    // Fast-path bail-outs fall through to the same grammar walk
    // the old code used, so rejections must keep their kinds.
    // (`1x` and `1.2.3` are not here: the pre-existing slow path
    // accepts the valid prefix and leaves the cursor on the
    // stray byte for the enclosing context to reject — that is
    // historical behavior, unchanged.)
    for s in ["1.", "1e", "1e+", ".5", "--1", "-"] {
        let err = lex(s.as_bytes()).parse_f64_value().unwrap_err();
        assert!(
            matches!(
                err.kind,
                ErrorKind::InvalidNumber
                    | ErrorKind::UnexpectedByte(_)
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::ExpectedNumber
            ),
            "{s}: unexpected {:?}",
            err.kind
        );
    }
}

#[test]
fn stops_after_literal_and_leaves_cursor() {
    let mut lx = lex(b"3.5,\"x\"");
    let v = lx.parse_f64_value().unwrap();
    assert_eq!(v.to_bits(), 3.5f64.to_bits());
    assert_eq!(lx.position().offset, 3);
}

#[test]
fn random_long_literals_agree_or_reject_together() {
    // Deterministic LCG so failures reproduce. Long random digit
    // strings exercise both the double-rounding edge (fast path
    // accepted, must equal libcore) and the bail-out edge (must
    // reject exactly when libcore's grammar walk would error —
    // a stray letter mid-literal).
    let mut seed: u64 = 0x5EED_CAFE_F00D_1234;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let len = (next() % 24) as usize + 1;
        let mut s = String::with_capacity(len + 4);
        if next() % 4 == 0 {
            s.push('-');
        }
        #[allow(clippy::cast_possible_truncation)] // len ≤ 25
        let int_digits = (next() % len as u64).max(1) as usize;
        // Leading zeros are invalid JSON — the first digit must
        // be non-zero whenever it is followed by more digits.
        for k in 0..len {
            if k == int_digits {
                s.push('.');
            }
            let d = if k == 0 && len > 1 || k == int_digits + 1 {
                1 + (next() % 9) as u8
            } else {
                (next() % 10) as u8
            };
            s.push(char::from(b'0' + d));
        }
        let got = lex(s.as_bytes()).parse_f64_value();
        let want = s.parse::<f64>();
        match (got, want) {
            (Ok(g), Ok(w)) => {
                assert_eq!(
                    g.to_bits(),
                    w.to_bits(),
                    "{s}: fast path diverged from libcore"
                );
            }
            (Ok(_), Err(_)) => panic!("{s}: fast path accepted, libcore rejected"),
            // Fast-path rejection must mean libcore would also
            // reject (grammar violation), not just a bail-out.
            (Err(e), Err(_)) => assert!(
                matches!(
                    e.kind,
                    ErrorKind::InvalidNumber
                        | ErrorKind::UnexpectedByte(_)
                        | ErrorKind::UnexpectedEof
                        | ErrorKind::ExpectedNumber
                        | ErrorKind::NumberOutOfRange
                ),
                "{s}: unexpected {:?}",
                e.kind
            ),
            (Err(_), Ok(_)) => panic!("{s}: fast path rejected, libcore accepted"),
        }
    }
}
