//! Shortest-roundtrip `f64` → decimal-string formatter.
//!
//! Direct port of V8's `fast-dtoa.cc` (Grisu3) with libstd fallback
//! for the ~0.5% of inputs where Grisu3 cannot prove its output is
//! shortest. The fallback uses `write!("{f}")`, which routes through
//! libstd's own dragon4-derived shortest formatter — correct, slow,
//! but only fires on the rare cases.
//!
//! No external dependency. Pure-Rust IEEE 754 manipulation.
//!
//! Reference: `google/double-conversion`,
//! `double-conversion/{fast-dtoa.cc, cached-powers.{cc,h}, diy-fp.h}`.
//! BSD-3-Clause licensed; structure preserved here line-by-line for
//! correctness, with Rust-idiomatic naming.

// Numeric algorithm: identifier shapes and constants follow the V8
// source rather than rust-idiomatic conventions; long hex literals are
// IEEE 754 mantissas — neither benefits from style-lint reformatting.
#![allow(
    clippy::many_single_char_names,
    clippy::unreadable_literal,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    clippy::doc_markdown,
    clippy::redundant_pub_crate
)]

extern crate alloc;

use alloc::string::String;
use core::fmt::{self, Write as _};

// ===========================================================================
// DiyFp: 64-bit mantissa with a power-of-2 exponent. value = f * 2^e.
// ===========================================================================

#[derive(Clone, Copy)]
struct DiyFp {
    f: u64,
    e: i32,
}

impl DiyFp {
    /// Width of the mantissa. Both operands of `multiply` must be
    /// normalized to a 1 in this top bit for the rounded-multiply
    /// invariant to hold.
    const SIGNIFICAND_SIZE: i32 = 64;

    fn new(f: u64, e: i32) -> Self {
        Self { f, e }
    }

    /// Subtract `rhs` from `self`. The two operands must share the
    /// same exponent and `self.f >= rhs.f`. The result is **not**
    /// normalized.
    fn minus(self, rhs: Self) -> Self {
        debug_assert!(self.e == rhs.e);
        debug_assert!(self.f >= rhs.f);
        Self::new(self.f - rhs.f, self.e)
    }

    /// 64-bit-rounded multiply. Decomposes both operands into 32-bit
    /// halves and sums the four cross-products into a 128-bit
    /// accumulator (in u64 pairs). Adding `1<<31` to the low half
    /// before truncating realizes round-half-up on the discarded
    /// 64 bits.
    fn times(self, rhs: Self) -> Self {
        const M32: u64 = 0xFFFF_FFFF;
        let a = self.f >> 32;
        let b = self.f & M32;
        let c = rhs.f >> 32;
        let d = rhs.f & M32;
        let ac = a * c;
        let bc = b * c;
        let ad = a * d;
        let bd = b * d;
        let tmp = (bd >> 32) + (ad & M32) + (bc & M32) + (1u64 << 31);
        let f = ac + (ad >> 32) + (bc >> 32) + (tmp >> 32);
        let e = self.e + rhs.e + 64;
        Self::new(f, e)
    }

    /// Shift `f` left until the top bit is set; decrement `e` by the
    /// shift amount.
    fn normalize(self) -> Self {
        debug_assert!(self.f != 0);
        let leading = self.f.leading_zeros();
        Self::new(self.f << leading, self.e - leading as i32)
    }
}

// ===========================================================================
// f64 → DiyFp + boundaries.
// ===========================================================================

const F64_SIG_BITS: i32 = 52;
const F64_HIDDEN_BIT: u64 = 1u64 << F64_SIG_BITS;
const F64_SIG_MASK: u64 = F64_HIDDEN_BIT - 1;
const F64_EXP_MASK: u64 = 0x7FF << F64_SIG_BITS;
const F64_EXP_BIAS: i32 = 0x3FF + F64_SIG_BITS;
const F64_DENORMAL_EXP: i32 = -F64_EXP_BIAS + 1;

/// Decompose `f` into a `DiyFp`. `value = result.f * 2^result.e`.
/// Subnormals get the conventional exponent `1 - bias` (their stored
/// exponent of 0 nominally maps to `2^(-bias)`, but the IEEE 754
/// convention is `2^(1-bias)` to keep the mantissa interpretation
/// continuous across the subnormal/normal boundary).
fn from_f64(value: f64) -> DiyFp {
    let bits = value.to_bits();
    let biased_exp = ((bits & F64_EXP_MASK) >> F64_SIG_BITS) as i32;
    let sig = bits & F64_SIG_MASK;
    if biased_exp == 0 {
        DiyFp::new(sig, F64_DENORMAL_EXP)
    } else {
        DiyFp::new(sig + F64_HIDDEN_BIT, biased_exp - F64_EXP_BIAS)
    }
}

/// Normalized boundaries `(m_minus, m_plus)`. After scaling by 2 (to
/// represent half-ulp distances) and asymmetric handling at the
/// power-of-2 boundary, both are returned with the same exponent
/// `m_plus.e` so subtraction works without renormalization.
///
/// The asymmetric case: when the f64's significand is exactly the
/// hidden bit (`v.f == HIDDEN_BIT`) and we're not at the smallest
/// normal exponent, the lower neighbor sits at half the distance of
/// the upper neighbor — `m_minus` is shifted accordingly.
fn normalized_boundaries(value: f64) -> (DiyFp, DiyFp) {
    let v = from_f64(value);
    let plus = DiyFp::new((v.f << 1) + 1, v.e - 1).normalize();
    let mut minus = if v.f == F64_HIDDEN_BIT && v.e != F64_DENORMAL_EXP {
        DiyFp::new((v.f << 2) - 1, v.e - 2)
    } else {
        DiyFp::new((v.f << 1) - 1, v.e - 1)
    };
    minus.f <<= (minus.e - plus.e) as u32;
    minus.e = plus.e;
    (minus, plus)
}

// ===========================================================================
// Cached powers of 10.
// ===========================================================================
//
// Table copied verbatim from V8's `cached-powers.cc`. Each entry is a
// 64-bit-rounded `DiyFp` representation of `10^decimal_exp`. The step
// between consecutive entries is 8 in decimal exponent, which keeps
// the chosen power's binary exponent within ~26.6 bits of the target —
// inside the `[ALPHA, GAMMA]` window required by digit generation.

#[derive(Clone, Copy)]
struct CachedPower {
    f: u64,
    binary_exp: i16,
    decimal_exp: i16,
}

const KCACHED_POWERS_OFFSET: i32 = 348;
const KDECIMAL_EXPONENT_DISTANCE: i32 = 8;

const KCACHED_POWERS: [CachedPower; 87] = [
    CachedPower { f: 0xfa8fd5a0081c0288, binary_exp: -1220, decimal_exp: -348 },
    CachedPower { f: 0xbaaee17fa23ebf76, binary_exp: -1193, decimal_exp: -340 },
    CachedPower { f: 0x8b16fb203055ac76, binary_exp: -1166, decimal_exp: -332 },
    CachedPower { f: 0xcf42894a5dce35ea, binary_exp: -1140, decimal_exp: -324 },
    CachedPower { f: 0x9a6bb0aa55653b2d, binary_exp: -1113, decimal_exp: -316 },
    CachedPower { f: 0xe61acf033d1a45df, binary_exp: -1087, decimal_exp: -308 },
    CachedPower { f: 0xab70fe17c79ac6ca, binary_exp: -1060, decimal_exp: -300 },
    CachedPower { f: 0xff77b1fcbebcdc4f, binary_exp: -1034, decimal_exp: -292 },
    CachedPower { f: 0xbe5691ef416bd60c, binary_exp: -1007, decimal_exp: -284 },
    CachedPower { f: 0x8dd01fad907ffc3c, binary_exp: -980, decimal_exp: -276 },
    CachedPower { f: 0xd3515c2831559a83, binary_exp: -954, decimal_exp: -268 },
    CachedPower { f: 0x9d71ac8fada6c9b5, binary_exp: -927, decimal_exp: -260 },
    CachedPower { f: 0xea9c227723ee8bcb, binary_exp: -901, decimal_exp: -252 },
    CachedPower { f: 0xaecc49914078536d, binary_exp: -874, decimal_exp: -244 },
    CachedPower { f: 0x823c12795db6ce57, binary_exp: -847, decimal_exp: -236 },
    CachedPower { f: 0xc21094364dfb5637, binary_exp: -821, decimal_exp: -228 },
    CachedPower { f: 0x9096ea6f3848984f, binary_exp: -794, decimal_exp: -220 },
    CachedPower { f: 0xd77485cb25823ac7, binary_exp: -768, decimal_exp: -212 },
    CachedPower { f: 0xa086cfcd97bf97f4, binary_exp: -741, decimal_exp: -204 },
    CachedPower { f: 0xef340a98172aace5, binary_exp: -715, decimal_exp: -196 },
    CachedPower { f: 0xb23867fb2a35b28e, binary_exp: -688, decimal_exp: -188 },
    CachedPower { f: 0x84c8d4dfd2c63f3b, binary_exp: -661, decimal_exp: -180 },
    CachedPower { f: 0xc5dd44271ad3cdba, binary_exp: -635, decimal_exp: -172 },
    CachedPower { f: 0x936b9fcebb25c996, binary_exp: -608, decimal_exp: -164 },
    CachedPower { f: 0xdbac6c247d62a584, binary_exp: -582, decimal_exp: -156 },
    CachedPower { f: 0xa3ab66580d5fdaf6, binary_exp: -555, decimal_exp: -148 },
    CachedPower { f: 0xf3e2f893dec3f126, binary_exp: -529, decimal_exp: -140 },
    CachedPower { f: 0xb5b5ada8aaff80b8, binary_exp: -502, decimal_exp: -132 },
    CachedPower { f: 0x87625f056c7c4a8b, binary_exp: -475, decimal_exp: -124 },
    CachedPower { f: 0xc9bcff6034c13053, binary_exp: -449, decimal_exp: -116 },
    CachedPower { f: 0x964e858c91ba2655, binary_exp: -422, decimal_exp: -108 },
    CachedPower { f: 0xdff9772470297ebd, binary_exp: -396, decimal_exp: -100 },
    CachedPower { f: 0xa6dfbd9fb8e5b88f, binary_exp: -369, decimal_exp: -92 },
    CachedPower { f: 0xf8a95fcf88747d94, binary_exp: -343, decimal_exp: -84 },
    CachedPower { f: 0xb94470938fa89bcf, binary_exp: -316, decimal_exp: -76 },
    CachedPower { f: 0x8a08f0f8bf0f156b, binary_exp: -289, decimal_exp: -68 },
    CachedPower { f: 0xcdb02555653131b6, binary_exp: -263, decimal_exp: -60 },
    CachedPower { f: 0x993fe2c6d07b7fac, binary_exp: -236, decimal_exp: -52 },
    CachedPower { f: 0xe45c10c42a2b3b06, binary_exp: -210, decimal_exp: -44 },
    CachedPower { f: 0xaa242499697392d3, binary_exp: -183, decimal_exp: -36 },
    CachedPower { f: 0xfd87b5f28300ca0e, binary_exp: -157, decimal_exp: -28 },
    CachedPower { f: 0xbce5086492111aeb, binary_exp: -130, decimal_exp: -20 },
    CachedPower { f: 0x8cbccc096f5088cc, binary_exp: -103, decimal_exp: -12 },
    CachedPower { f: 0xd1b71758e219652c, binary_exp: -77, decimal_exp: -4 },
    CachedPower { f: 0x9c40000000000000, binary_exp: -50, decimal_exp: 4 },
    CachedPower { f: 0xe8d4a51000000000, binary_exp: -24, decimal_exp: 12 },
    CachedPower { f: 0xad78ebc5ac620000, binary_exp: 3, decimal_exp: 20 },
    CachedPower { f: 0x813f3978f8940984, binary_exp: 30, decimal_exp: 28 },
    CachedPower { f: 0xc097ce7bc90715b3, binary_exp: 56, decimal_exp: 36 },
    CachedPower { f: 0x8f7e32ce7bea5c70, binary_exp: 83, decimal_exp: 44 },
    CachedPower { f: 0xd5d238a4abe98068, binary_exp: 109, decimal_exp: 52 },
    CachedPower { f: 0x9f4f2726179a2245, binary_exp: 136, decimal_exp: 60 },
    CachedPower { f: 0xed63a231d4c4fb27, binary_exp: 162, decimal_exp: 68 },
    CachedPower { f: 0xb0de65388cc8ada8, binary_exp: 189, decimal_exp: 76 },
    CachedPower { f: 0x83c7088e1aab65db, binary_exp: 216, decimal_exp: 84 },
    CachedPower { f: 0xc45d1df942711d9a, binary_exp: 242, decimal_exp: 92 },
    CachedPower { f: 0x924d692ca61be758, binary_exp: 269, decimal_exp: 100 },
    CachedPower { f: 0xda01ee641a708dea, binary_exp: 295, decimal_exp: 108 },
    CachedPower { f: 0xa26da3999aef774a, binary_exp: 322, decimal_exp: 116 },
    CachedPower { f: 0xf209787bb47d6b85, binary_exp: 348, decimal_exp: 124 },
    CachedPower { f: 0xb454e4a179dd1877, binary_exp: 375, decimal_exp: 132 },
    CachedPower { f: 0x865b86925b9bc5c2, binary_exp: 402, decimal_exp: 140 },
    CachedPower { f: 0xc83553c5c8965d3d, binary_exp: 428, decimal_exp: 148 },
    CachedPower { f: 0x952ab45cfa97a0b3, binary_exp: 455, decimal_exp: 156 },
    CachedPower { f: 0xde469fbd99a05fe3, binary_exp: 481, decimal_exp: 164 },
    CachedPower { f: 0xa59bc234db398c25, binary_exp: 508, decimal_exp: 172 },
    CachedPower { f: 0xf6c69a72a3989f5c, binary_exp: 534, decimal_exp: 180 },
    CachedPower { f: 0xb7dcbf5354e9bece, binary_exp: 561, decimal_exp: 188 },
    CachedPower { f: 0x88fcf317f22241e2, binary_exp: 588, decimal_exp: 196 },
    CachedPower { f: 0xcc20ce9bd35c78a5, binary_exp: 614, decimal_exp: 204 },
    CachedPower { f: 0x98165af37b2153df, binary_exp: 641, decimal_exp: 212 },
    CachedPower { f: 0xe2a0b5dc971f303a, binary_exp: 667, decimal_exp: 220 },
    CachedPower { f: 0xa8d9d1535ce3b396, binary_exp: 694, decimal_exp: 228 },
    CachedPower { f: 0xfb9b7cd9a4a7443c, binary_exp: 720, decimal_exp: 236 },
    CachedPower { f: 0xbb764c4ca7a44410, binary_exp: 747, decimal_exp: 244 },
    CachedPower { f: 0x8bab8eefb6409c1a, binary_exp: 774, decimal_exp: 252 },
    CachedPower { f: 0xd01fef10a657842c, binary_exp: 800, decimal_exp: 260 },
    CachedPower { f: 0x9b10a4e5e9913129, binary_exp: 827, decimal_exp: 268 },
    CachedPower { f: 0xe7109bfba19c0c9d, binary_exp: 853, decimal_exp: 276 },
    CachedPower { f: 0xac2820d9623bf429, binary_exp: 880, decimal_exp: 284 },
    CachedPower { f: 0x80444b5e7aa7cf85, binary_exp: 907, decimal_exp: 292 },
    CachedPower { f: 0xbf21e44003acdd2d, binary_exp: 933, decimal_exp: 300 },
    CachedPower { f: 0x8e679c2f5e44ff8f, binary_exp: 960, decimal_exp: 308 },
    CachedPower { f: 0xd433179d9c8cb841, binary_exp: 986, decimal_exp: 316 },
    CachedPower { f: 0x9e19db92b4e31ba9, binary_exp: 1013, decimal_exp: 324 },
    CachedPower { f: 0xeb96bf6ebadf77d9, binary_exp: 1039, decimal_exp: 332 },
    CachedPower { f: 0xaf87023b9bf0ee6b, binary_exp: 1066, decimal_exp: 340 },
];

/// `1 / log2(10) ≈ 0.30103`. Used to convert a binary exponent target
/// into its decimal-exponent equivalent so we can look up the right
/// table entry.
const KD_1_LOG2_10: f64 = 0.30102999566398114;

/// `ceil(x)` for a finite `f64` whose magnitude fits comfortably in
/// `i32`. Hand-rolled because [`f64::ceil`] is in `std` (it's a libm
/// intrinsic), and `bourne` aspires to build under `no_std + alloc`
/// without an external `libm` dep.
///
/// Strategy: truncate toward zero via `as i64`, then add one when the
/// truncation discarded a positive fractional part. The cast is safe
/// because the only caller below feeds `n * KD_1_LOG2_10` where `n`
/// is bounded in `[-1135, 962]` — magnitudes well under `i64::MAX`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn ceil_f64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if x > 0.0 && x > t {
        t + 1.0
    } else {
        t
    }
}

/// Pick the cached `10^k` entry whose binary exponent satisfies
/// `min_exponent ≤ binary_exp ≤ max_exponent`. The table's step of 8
/// (decimal) ensures exactly one entry lies in any 28-binary-wide
/// window, which is what the `[ALPHA, GAMMA]` digit-gen requirement
/// produces.
fn cached_power_for_binary_exponent_range(
    min_exponent: i32,
    _max_exponent: i32,
) -> (DiyFp, i32) {
    let kq = DiyFp::SIGNIFICAND_SIZE;
    let k = ceil_f64((min_exponent + kq - 1) as f64 * KD_1_LOG2_10);
    let index =
        (KCACHED_POWERS_OFFSET + k as i32 - 1) / KDECIMAL_EXPONENT_DISTANCE + 1;
    let cached = KCACHED_POWERS[index as usize];
    (
        DiyFp::new(cached.f, cached.binary_exp as i32),
        cached.decimal_exp as i32,
    )
}

// ===========================================================================
// Digit generation. Direct port of V8 `DigitGen` / `RoundWeed`.
// ===========================================================================

const KMINIMAL_TARGET_EXPONENT: i32 = -60;
const KMAXIMAL_TARGET_EXPONENT: i32 = -32;

/// Small powers of 10 indexed by `exponent_plus_one`. The leading 0
/// matches V8's table — `kSmallPowersOfTen[0]` is never read but
/// keeps indexing consistent with V8's `BiggestPowerTen` formula.
const KSMALL_POWERS_OF_TEN: [u32; 11] = [
    0, 1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000,
];

/// Returns `(power, exponent_plus_one)` such that
/// `power == 10^(exponent_plus_one - 1)` and `power ≤ number < 10*power`.
/// `number_bits` must be `≤ 32`.
fn biggest_power_ten(number: u32, number_bits: i32) -> (u32, i32) {
    debug_assert!(number_bits <= 32);
    debug_assert!(number_bits == 32 || number < (1u32 << (number_bits + 1)));
    // 1233/4096 ≈ 1/log2(10) — gives us a guess that's within 1 of
    // the true answer.
    let mut exponent_plus_one_guess = ((number_bits + 1) * 1233) >> 12;
    exponent_plus_one_guess += 1;
    if number < KSMALL_POWERS_OF_TEN[exponent_plus_one_guess as usize] {
        exponent_plus_one_guess -= 1;
    }
    let power = KSMALL_POWERS_OF_TEN[exponent_plus_one_guess as usize];
    (power, exponent_plus_one_guess)
}

/// Generate the shortest digit sequence between `low` and `high`,
/// preferring the value closest to `w` on ambiguity. All three inputs
/// must share the same exponent in `[KMINIMAL, KMAXIMAL]`.
///
/// Returns `Some((len, kappa))` on success or `None` to signal Grisu3
/// failure (caller falls back to libstd).
fn digit_gen(low: DiyFp, w: DiyFp, high: DiyFp, buffer: &mut [u8; 24]) -> Option<(usize, i32)> {
    debug_assert!(low.e == w.e && w.e == high.e);
    debug_assert!(low.f < high.f - 1);
    debug_assert!(KMINIMAL_TARGET_EXPONENT <= w.e && w.e <= KMAXIMAL_TARGET_EXPONENT);

    let mut unit: u64 = 1;
    let too_low = DiyFp::new(low.f - unit, low.e);
    let too_high = DiyFp::new(high.f + unit, high.e);
    let mut unsafe_interval = too_high.minus(too_low);
    let one = DiyFp::new(1u64 << (-w.e), w.e);
    let mut integrals = (too_high.f >> (-one.e)) as u32;
    let mut fractionals = too_high.f & (one.f - 1);
    let (mut divisor, divisor_exponent_plus_one) =
        biggest_power_ten(integrals, DiyFp::SIGNIFICAND_SIZE - (-one.e));
    let mut kappa = divisor_exponent_plus_one;
    let mut length: usize = 0;

    // Phase 1: integer digits.
    while kappa > 0 {
        let digit = integrals / divisor;
        debug_assert!(digit <= 9);
        buffer[length] = b'0' + digit as u8;
        length += 1;
        integrals %= divisor;
        kappa -= 1;
        let rest = (u64::from(integrals) << (-one.e)) + fractionals;
        if rest < unsafe_interval.f {
            return round_weed(
                buffer,
                length,
                too_high.minus(w).f,
                unsafe_interval.f,
                rest,
                u64::from(divisor) << (-one.e),
                unit,
            )
            .then_some((length, kappa));
        }
        divisor /= 10;
    }

    // Phase 2: fractional digits.
    debug_assert!(one.e >= -60);
    debug_assert!(fractionals < one.f);
    loop {
        fractionals = fractionals.wrapping_mul(10);
        unit = unit.wrapping_mul(10);
        unsafe_interval.f = unsafe_interval.f.wrapping_mul(10);
        let digit = (fractionals >> (-one.e)) as u8;
        debug_assert!(digit <= 9);
        buffer[length] = b'0' + digit;
        length += 1;
        fractionals &= one.f - 1;
        kappa -= 1;
        if fractionals < unsafe_interval.f {
            return round_weed(
                buffer,
                length,
                too_high.minus(w).f.wrapping_mul(unit),
                unsafe_interval.f,
                fractionals,
                one.f,
                unit,
            )
            .then_some((length, kappa));
        }
        if length >= buffer.len() {
            return None;
        }
    }
}

/// Adjust the last digit toward `w` while staying inside the safe
/// interval, then verify the result is unambiguously the closest
/// representation. Returns `true` on success, `false` to bail.
///
/// All distance arguments are in the same `* unit` scale.
fn round_weed(
    buffer: &mut [u8; 24],
    length: usize,
    distance_too_high_w: u64,
    unsafe_interval: u64,
    mut rest: u64,
    ten_kappa: u64,
    unit: u64,
) -> bool {
    let small_distance = distance_too_high_w - unit;
    let big_distance = distance_too_high_w + unit;
    debug_assert!(rest <= unsafe_interval);
    while rest < small_distance
        && unsafe_interval - rest >= ten_kappa
        && (rest + ten_kappa < small_distance
            || small_distance - rest >= rest + ten_kappa - small_distance)
    {
        buffer[length - 1] -= 1;
        rest += ten_kappa;
    }
    if rest < big_distance
        && unsafe_interval - rest >= ten_kappa
        && (rest + ten_kappa < big_distance
            || big_distance - rest > rest + ten_kappa - big_distance)
    {
        return false;
    }
    2 * unit <= rest && rest <= unsafe_interval - 4 * unit
}

// ===========================================================================
// Top-level Grisu3 + libstd fallback.
// ===========================================================================

struct Grisu3Out {
    buf: [u8; 24],
    len: usize,
    decimal_exponent: i32,
}

/// Run Grisu3 on `value`. Caller must ensure `value > 0` and finite.
fn grisu3(value: f64) -> Option<Grisu3Out> {
    let w = from_f64(value).normalize();
    let (boundary_minus, boundary_plus) = normalized_boundaries(value);
    debug_assert!(boundary_plus.e == w.e);

    let ten_mk_minimal_binary_exponent =
        KMINIMAL_TARGET_EXPONENT - (w.e + DiyFp::SIGNIFICAND_SIZE);
    let ten_mk_maximal_binary_exponent =
        KMAXIMAL_TARGET_EXPONENT - (w.e + DiyFp::SIGNIFICAND_SIZE);
    let (ten_mk, mk) = cached_power_for_binary_exponent_range(
        ten_mk_minimal_binary_exponent,
        ten_mk_maximal_binary_exponent,
    );
    debug_assert!(
        KMINIMAL_TARGET_EXPONENT <= w.e + ten_mk.e + DiyFp::SIGNIFICAND_SIZE
            && KMAXIMAL_TARGET_EXPONENT >= w.e + ten_mk.e + DiyFp::SIGNIFICAND_SIZE
    );

    let scaled_w = w.times(ten_mk);
    let scaled_boundary_minus = boundary_minus.times(ten_mk);
    let scaled_boundary_plus = boundary_plus.times(ten_mk);

    let mut buf = [0u8; 24];
    let (length, kappa) = digit_gen(scaled_boundary_minus, scaled_w, scaled_boundary_plus, &mut buf)?;
    Some(Grisu3Out {
        buf,
        len: length,
        decimal_exponent: -mk + kappa,
    })
}

/// Format the digit string + decimal exponent into JSON-compatible
/// number text. Mirror libstd's `Display for f64` choice between
/// plain decimal and scientific forms. Plain when the magnitude sits
/// in `(-6, 21]`; scientific outside.
///
/// The digit slice is guaranteed ASCII (`'0'..='9'`) by `digit_gen`,
/// so we can transmute to `&str` via `from_utf8_unchecked` and emit
/// it as a single `push_str` rather than per-byte `push`. Same for
/// zero-padding runs — one `push_str` of a `'0'` slice replaces N
/// pushes.
fn write_grisu3(g: &Grisu3Out, negative: bool, out: &mut String) {
    // Digit slice as `&str` without an allocation. SAFETY: `digit_gen`
    // only ever writes `b'0'..=b'9'` to the buffer, so the byte slice
    // is valid UTF-8. Same pattern as the integer formatters above.
    #[allow(unsafe_code)]
    let digits: &str = unsafe { core::str::from_utf8_unchecked(&g.buf[..g.len]) };
    if negative {
        out.push('-');
    }
    let point = g.len as i32 + g.decimal_exponent;
    if (-6..=21).contains(&point) {
        if point <= 0 {
            out.push_str("0.");
            push_zeros(out, -point as usize);
            out.push_str(digits);
        } else if (point as usize) >= digits.len() {
            out.push_str(digits);
            push_zeros(out, point as usize - digits.len());
            out.push_str(".0");
        } else {
            let p = point as usize;
            out.push_str(&digits[..p]);
            out.push('.');
            out.push_str(&digits[p..]);
        }
    } else {
        out.push_str(&digits[..1]);
        if g.len > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let exp = point - 1;
        let _ = write!(out, "{exp}");
    }
}

/// Bulk-push N copies of `'0'`. We slice from a static run so
/// `push_str` does a single `memcpy`. The cap of 21 matches the
/// plain-decimal range; longer zero runs route through scientific
/// notation and don't hit this path.
fn push_zeros(out: &mut String, n: usize) {
    const ZEROS: &str = "00000000000000000000000"; // 23 chars, headroom over 21.
    debug_assert!(n <= ZEROS.len());
    out.push_str(&ZEROS[..n]);
}

/// Format a finite `f64` as JSON-compatible decimal text. Caller is
/// responsible for the finite check; this function will panic on
/// `NaN` / `inf` via the boundary computation.
///
/// The `String`-targeted entry point — the original hot path. Most
/// production serializers go through this. For sinks that don't have
/// a `String` to push into, see [`format_finite_fmt`].
pub(crate) fn format_finite(f: f64, out: &mut String) {
    if f == 0.0 {
        if f.is_sign_negative() {
            out.push_str("-0.0");
        } else {
            out.push_str("0.0");
        }
        return;
    }
    let negative = f.is_sign_negative();
    let abs = f.abs();
    if let Some(g) = grisu3(abs) {
        write_grisu3(&g, negative, out);
    } else {
        // Grisu3 declined to commit to its output (~0.5% of inputs).
        // libstd's `Display` finishes the job.
        let _ = write!(out, "{f}");
    }
}

/// `fmt::Write`-targeted variant of [`format_finite`]. Used by the
/// `FmtWriteSink` and `IoWriteSink` adapters. The `String` path stays
/// on the hand-rolled push routine because it sidesteps the
/// `Formatter` machinery; this generic path is fine for every other
/// sink.
///
/// Errors propagate from the sink — `String` formats infallibly,
/// `io::Write` formats wrap an `io::Error`.
pub(crate) fn format_finite_fmt<W: fmt::Write + ?Sized>(f: f64, out: &mut W) -> fmt::Result {
    if f == 0.0 {
        return out.write_str(if f.is_sign_negative() { "-0.0" } else { "0.0" });
    }
    let negative = f.is_sign_negative();
    let abs = f.abs();
    if let Some(g) = grisu3(abs) {
        // Stage Grisu3's digit emission into a small stack-sized
        // String, then forward in one `write_str`. The Grisu3 output
        // for an f64 fits comfortably in 24 bytes; we reserve more.
        let mut buf = String::with_capacity(32);
        write_grisu3(&g, negative, &mut buf);
        out.write_str(&buf)
    } else {
        write!(out, "{f}")
    }
}

#[cfg(test)]
mod tests {
    //! Diagnostic tests for the Grisu3 hot path.

    use super::grisu3;
    use alloc::format;
    use alloc::string::String;

    /// Grisu3 hit rate across an LCG-generated mix mirroring the
    /// `floats` bench corpus (1e-6 / 1e6 / ±100). Loitsch's published
    /// figure is ~99.5%; we observe ~99.7% on this fixture. The
    /// threshold is set at 95% — well above the rate we'd see if a
    /// regression broke part of the algorithm but below normal
    /// fixture wobble.
    #[test]
    fn grisu3_hit_rate_above_95_percent() {
        let mut state: u64 = 0xCAFE_BABE_DEAD_BEEF;
        let (mut hits, mut misses) = (0usize, 0usize);
        for i in 0..10_000usize {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            #[allow(clippy::cast_precision_loss)]
            let mantissa = (state >> 11) as f64 / (1u64 << 53) as f64;
            let signed = mantissa.mul_add(2.0, -1.0);
            let v = match i % 10 {
                0 => signed * 1e-6,
                1 => signed * 1e6,
                _ => signed * 100.0,
            };
            if !v.is_finite() || v == 0.0 {
                continue;
            }
            if grisu3(v.abs()).is_some() {
                hits += 1;
            } else {
                misses += 1;
            }
        }
        let total = hits + misses;
        let rate = (hits as f64) / (total as f64);
        assert!(
            rate > 0.95,
            "Grisu3 hit rate {rate:.3} below 0.95 — \
             {hits}/{total} hits, {misses} misses"
        );
    }

    /// Every successful Grisu3 output must round-trip to the same
    /// `f64`. A non-roundtrip Some return is a correctness bug.
    //
    // `float_cmp`: bit-exact equality is the literal property under
    // test — the parsed output of Grisu3 must equal the input f64.
    #[test]
    #[allow(clippy::float_cmp)]
    fn grisu3_output_roundtrips_when_returned() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        for _ in 0..2_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let f = f64::from_bits(state);
            if !f.is_finite() || f == 0.0 || f.is_sign_negative() {
                continue;
            }
            if let Some(g) = grisu3(f) {
                let mut out = String::new();
                super::write_grisu3(&g, false, &mut out);
                let parsed: f64 = out.parse().expect("our output parses");
                assert_eq!(
                    parsed, f,
                    "Grisu3 output {out:?} did not round-trip f={f:e} \
                     (bits=0x{:016x})",
                    f.to_bits()
                );
            }
        }
    }

    /// libstd parity on canonical inputs. Compare via the parsed
    /// value (their formatting choices are equivalent up to e-vs-e+
    /// conventions on scientific notation).
    //
    // `float_cmp`: comparing the *parsed* form of two formattings of
    // the same f64 — they must be bit-identical, not approximately
    // equal.
    //
    // `approx_constant`: 3.14159 is a deliberate "round but not exact"
    // input chosen to exercise Grisu3 on a real-world-shaped literal,
    // not a stand-in for `core::f64::consts::PI`.
    #[test]
    #[allow(clippy::float_cmp, clippy::approx_constant)]
    fn grisu3_matches_libstd_display() {
        let cases = [
            1.0_f64,
            1.5,
            0.1,
            0.2,
            0.3,
            100.0,
            12.5,
            1.5e10,
            -2.7e-5,
            3.14159,
            1e100,
            1e-100,
            1.7976931348623157e308,
            5e-324,
        ];
        for &v in &cases {
            if v == 0.0 || !v.is_finite() {
                continue;
            }
            let abs = v.abs();
            if let Some(g) = grisu3(abs) {
                let mut ours = String::new();
                super::write_grisu3(&g, v.is_sign_negative(), &mut ours);
                let libstd = format!("{v}");
                let ours_parsed: f64 = ours.parse().expect("ours parses");
                let libstd_parsed: f64 = libstd.parse().expect("libstd parses");
                assert_eq!(
                    ours_parsed, libstd_parsed,
                    "ours={ours:?} libstd={libstd:?} for v={v:e}",
                );
            }
        }
    }
}
