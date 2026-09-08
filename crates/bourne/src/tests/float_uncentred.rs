//! `to_decimal_uncentred` — the float path for powers of 2 (mantissa
//! == 1 << 52). Hit when the IEEE 754 mantissa lands exactly on the
//! uncentred boundary. These values are rare in random samples, so
//! they need explicit tests.

use crate::{parse_str, to_string};

/// Powers of 2 hit the uncentred decompose path. Each gets a
/// shortest-roundtrip rendering through `to_decimal_uncentred`.
#[test]
fn to_decimal_uncentred_powers_of_two_round_trip() {
    // f64 values where mantissa == 1 << 52 and exponent != EXPONENT_MIN:
    // these are 2.0, 4.0, 8.0, 16.0, ..., up to 2^1023.
    let cases = [
        2.0_f64,
        4.0,
        8.0,
        16.0,
        32.0,
        64.0,
        128.0,
        256.0,
        512.0,
        1024.0,
        2.0_f64.powi(20),
        2.0_f64.powi(50),
        2.0_f64.powi(100),
        2.0_f64.powi(500),
        2.0_f64.powi(1023), // largest finite power of 2
        // Negative powers of 2 — half exponent.
        2.0_f64.powi(-1), // 0.5
        2.0_f64.powi(-2), // 0.25
        2.0_f64.powi(-10),
        2.0_f64.powi(-50),
        2.0_f64.powi(-100),
        2.0_f64.powi(-1000),
    ];
    for &v in &cases {
        let s = to_string(&v).expect("ser");
        let back: f64 = parse_str(&s).expect("parse back");
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
        }
    }
}

/// Negative powers of 2 — sign path through `to_decimal_uncentred`.
#[test]
fn to_decimal_uncentred_negative_powers_round_trip() {
    for k in [-30, -10, -1, 1, 10, 30, 100, 500] {
        let v = -(2.0_f64.powi(k));
        let s = to_string(&v).expect("ser");
        let back: f64 = parse_str(&s).expect("parse back");
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
        }
    }
}

/// Subnormal f64 — minimum positive denormal, uses uncentred path
/// at a different boundary.
#[test]
fn to_decimal_uncentred_subnormals_round_trip() {
    let cases = [
        5e-324_f64,        // smallest subnormal
        f64::MIN_POSITIVE, // smallest normal
        -f64::MIN_POSITIVE,
        f64::MAX,
        -f64::MAX,
    ];
    for &v in &cases {
        let s = to_string(&v).expect("ser");
        let back: f64 = parse_str(&s).expect("parse back");
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(back, v, "round-trip for {v:e}: emitted {s:?}");
        }
    }
}
