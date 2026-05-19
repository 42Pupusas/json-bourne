//! Type-driven serialization.
//!
//! Mirror image of [`crate::de`]. Each type knows how to write itself to a
//! [`JsonWrite`] sink; the top-level entry point [`to_string`] (and
//! [`to_vec`]) drives a `StringSink` and returns the resulting buffer.
//!
//! The sink is not [`core::fmt::Write`] for two reasons: (1) `fmt::Write`
//! returns an opaque `fmt::Error` and forces every numeric write through
//! the `Formatter` machinery, which is ~3× slower than direct push for
//! integers; (2) we need a typed error path for non-finite floats
//! ([`ErrorKind::NonFiniteFloat`]). The sink trait exposes per-primitive
//! writers so impls can bypass formatting entirely.
//!
//! Float impls are not included here — they live in a follow-up that ports
//! the ryu shortest-round-trip algorithm. Non-float primitives, composites,
//! and the std/alloc adapters are all in scope.

use bourne_core::{Error, ErrorKind, Position};

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::string::String;

// ---------------------------------------------------------------------------
// Sink trait + StringSink
// ---------------------------------------------------------------------------

/// Output sink for [`ToJson`] impls.
///
/// Implementors expose per-primitive writers so the trait impls below can
/// avoid the [`core::fmt`] machinery. The default integer/float methods
/// land on `write_str_raw` after formatting into a small stack buffer, but
/// implementations that can write directly (e.g. `StringSink`'s integer
/// LUT) override them.
pub trait JsonWrite {
    /// Sink-specific error type. `StringSink` uses [`core::convert::Infallible`]
    /// for the byte/string writes; the typed-level [`to_string`] entry point
    /// widens to [`bourne_core::Error`] so non-finite floats can surface.
    type Error;

    /// Hint that at least `additional` more bytes will be written. Sinks
    /// backed by a growable buffer (like [`ByteSink`]) can amortize
    /// capacity growth across a known-size sequence; sinks without a
    /// reservation concept treat this as a no-op.
    ///
    /// Hot path: the array writer in this module calls this once at the
    /// start of a slice/Vec serialization so per-element `reserve` calls
    /// become predictable no-ops.
    #[inline]
    fn reserve_hint(&mut self, _additional: usize) {}

    /// Append a single ASCII byte. Used for structural punctuation
    /// (`{`, `}`, `[`, `]`, `,`, `:`, `"`).
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error>;

    /// Append a single ASCII byte WITHOUT a per-write capacity check.
    ///
    /// # Safety
    ///
    /// The caller MUST have previously called [`Self::reserve_hint`] with
    /// at least enough bytes to cover this write plus all subsequent
    /// writes up to the next `reserve_hint` (or the end of writing).
    /// Sinks that don't track a tail capacity (e.g. `fmt::Write`-backed)
    /// fall back to the safe `write_byte`.
    ///
    /// Why this exists: under random `f64` inputs the per-element
    /// capacity-check branch in `Vec::push` at the comma site mispredicts
    /// at ~13% in `perf record -e branch-misses` — the single largest
    /// mispredict source in the array hot loop. Hoisting the check via
    /// `reserve_hint` and emitting the byte unchecked removes that
    /// branch entirely from the inner loop.
    ///
    /// Default impl forwards to `write_byte` (safe but checked) so
    /// non-`ByteSink` sinks aren't required to opt in.
    ///
    /// # Safety contract example (matches what `write_array` does)
    /// ```ignore
    /// w.reserve_hint(n * MAX + brackets_and_commas);
    /// for _ in 0..count {
    ///     // SAFETY: reserve_hint covered all of these.
    ///     unsafe { w.write_byte_unchecked(b','); }
    ///     w.write_int_u64(...);  // also covered by reserve_hint
    /// }
    /// ```
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_byte_unchecked(&mut self, b: u8) -> Result<(), Self::Error> {
        self.write_byte(b)
    }

    /// Append a `&str` verbatim. Caller is responsible for any escaping
    /// — this is the structural / pre-escaped path. For user string
    /// payloads, call [`Self::write_escaped_str`] instead.
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error>;

    /// Append a JSON-quoted, escaped string (including the surrounding
    /// `"` characters). The default impl escapes one byte at a time
    /// through `write_byte`; sinks with bulk-write capability (like
    /// [`StringSink`]) override with a literal-run fast path.
    #[inline]
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        self.write_byte(b'"')?;
        for &b in s.as_bytes() {
            write_escape_byte(self, b)?;
        }
        self.write_byte(b'"')
    }

    /// Append a byte slice whose contents are known-valid UTF-8.
    ///
    /// Used by macro-generated code to fuse adjacent structural literals
    /// (e.g. `b"{\"id\":"`) into a single write. The default routes
    /// through [`Self::write_str_raw`]; byte-oriented sinks like
    /// [`ByteSink`] override to avoid the `&str` conversion.
    #[inline]
    fn write_raw_bytes(&mut self, b: &[u8]) -> Result<(), Self::Error> {
        // SAFETY: callers guarantee valid UTF-8 — in practice these are
        // compile-time byte-string literals from `concat!` / `b"..."`.
        #[allow(unsafe_code)]
        self.write_str_raw(unsafe { core::str::from_utf8_unchecked(b) })
    }

    /// Write a signed 64-bit integer as a JSON number.
    #[inline]
    fn write_int_i64(&mut self, n: i64) -> Result<(), Self::Error> {
        let mut buf = [0u8; 20];
        let s = format_i64(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write an unsigned 64-bit integer as a JSON number.
    #[inline]
    fn write_int_u64(&mut self, n: u64) -> Result<(), Self::Error> {
        let mut buf = [0u8; 20];
        let s = format_u64(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write a signed 128-bit integer as a JSON number.
    #[inline]
    fn write_int_i128(&mut self, n: i128) -> Result<(), Self::Error> {
        let mut buf = [0u8; 40];
        let s = format_i128(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write an unsigned 128-bit integer as a JSON number.
    #[inline]
    fn write_int_u128(&mut self, n: u128) -> Result<(), Self::Error> {
        let mut buf = [0u8; 40];
        let s = format_u128(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write a finite `f64` as a JSON number. Non-finite inputs (`inf`,
    /// `-inf`, `NaN`) have no JSON representation; sinks reject them
    /// via their `Self::Error` type.
    ///
    /// The default routes through `core::fmt::Write` after stack-buffer
    /// formatting via `format!` — this is the "good enough" path that
    /// works for any sink. Sinks that want shortest-round-trip output
    /// without fmt overhead override with a direct ryu call.
    #[cfg(feature = "alloc")]
    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error>;

    /// Write a finite `f64` ASSUMING the caller has reserved at least
    /// `f64::MAX_SERIALIZED_LEN` (32) bytes of sink capacity. Skips the
    /// per-call cap check in `Vec::extend_from_slice` — the second-largest
    /// mispredict source after the comma `Vec::push`.
    ///
    /// Default impl forwards to `write_float_f64`. `ByteSink` overrides
    /// with a raw-pointer write directly into the Vec's reserved tail.
    ///
    /// # Safety
    /// `f64::MAX_SERIALIZED_LEN` (32) bytes of sink capacity must be
    /// guaranteed available before this call. Used by `[f64]::
    /// write_json_in_reserved` after `[T]::write_json`'s `reserve_hint`.
    #[cfg(feature = "alloc")]
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_float_f64_unchecked(&mut self, f: f64) -> Result<(), Self::Error> {
        self.write_float_f64(f)
    }

    /// Like `write_float_f64_unchecked`, but additionally requires the
    /// caller to have validated that `f` is finite. The per-element
    /// finiteness branch in the bench's hot loop was the dominant
    /// remaining mispredict source after the cap-check elimination;
    /// hoisting validation to a one-shot pre-scan over the whole slice
    /// removes that branch from the inner loop entirely.
    ///
    /// Default impl forwards to `write_float_f64_unchecked`. `ByteSink`
    /// overrides with the finite-known float path.
    ///
    /// # Safety
    /// All preconditions of `write_float_f64_unchecked`, *plus* `f`
    /// must be finite. Used after a successful `[f64]::pre_validate_slice`.
    #[cfg(feature = "alloc")]
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_float_f64_unchecked_finite(
        &mut self,
        f: f64,
    ) -> Result<(), Self::Error> {
        // SAFETY: weaker contract subsumes ours.
        unsafe { self.write_float_f64_unchecked(f) }
    }
}

/// Write one byte of a string body, applying JSON escape rules.
fn write_escape_byte<W: JsonWrite + ?Sized>(w: &mut W, b: u8) -> Result<(), W::Error> {
    match b {
        b'"' => w.write_str_raw("\\\""),
        b'\\' => w.write_str_raw("\\\\"),
        b'\n' => w.write_str_raw("\\n"),
        b'\r' => w.write_str_raw("\\r"),
        b'\t' => w.write_str_raw("\\t"),
        0x08 => w.write_str_raw("\\b"),
        0x0C => w.write_str_raw("\\f"),
        // Other control bytes → \u00XX. Non-control bytes (including
        // multi-byte UTF-8 continuation bytes) pass through verbatim.
        0x00..=0x1F => {
            let hi = HEX_LOWER[(b >> 4) as usize];
            let lo = HEX_LOWER[(b & 0x0F) as usize];
            w.write_byte(b'\\')?;
            w.write_byte(b'u')?;
            w.write_byte(b'0')?;
            w.write_byte(b'0')?;
            w.write_byte(hi)?;
            w.write_byte(lo)
        }
        _ => w.write_byte(b),
    }
}

const HEX_LOWER: [u8; 16] = *b"0123456789abcdef";

/// Returns true for bytes that need an escape sequence inside a JSON string
/// body (quote, backslash, or any control byte `< 0x20`). Everything else —
/// including high-bit UTF-8 continuation bytes — is safe to write verbatim.
#[inline]
const fn needs_escape(b: u8) -> bool {
    b == b'"' || b == b'\\' || b < 0x20
}

/// `JsonWrite` sink that appends to a `String`. Infallible.
#[cfg(feature = "alloc")]
#[derive(Debug)]
pub struct StringSink<'a> {
    out: &'a mut String,
}

#[cfg(feature = "alloc")]
impl<'a> StringSink<'a> {
    #[must_use]
    pub const fn new(out: &'a mut String) -> Self {
        Self { out }
    }
}

#[cfg(feature = "alloc")]
impl JsonWrite for StringSink<'_> {
    /// `StringSink` widens its error to [`Error`] (rather than
    /// [`core::convert::Infallible`]) so the float path has a typed
    /// variant for non-finite inputs. The byte/string/integer writes
    /// never fail in practice; only `write_float_f64` produces an `Err`.
    type Error = Error;

    #[inline]
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        // SAFETY would be required for `as_mut_vec`; instead push as a char.
        // Single-byte ASCII pushes go through `String::push` which is
        // already an inlined `Vec::push` — no formatting overhead.
        self.out.push(b as char);
        Ok(())
    }

    #[inline]
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.push_str(s);
        Ok(())
    }

    /// Literal-run fast path: scan for the next byte that needs escaping,
    /// bulk-append the safe stretch, then emit one escape and resume.
    /// Mirrors the `decode_escapes` strategy in `de.rs` in reverse.
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.push('"');
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut start = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if needs_escape(b) {
                if start < i {
                    // Safe stretch: every byte in [start..i] is either ASCII
                    // non-special or part of a valid UTF-8 multi-byte run
                    // (since `s` is `&str`, the input is valid UTF-8 and
                    // the only single-byte stop conditions are the ones
                    // `needs_escape` flags).
                    self.out.push_str(&s[start..i]);
                }
                write_escape_byte(self, b)?;
                start = i + 1;
            }
            i += 1;
        }
        if start < bytes.len() {
            self.out.push_str(&s[start..]);
        }
        self.out.push('"');
        Ok(())
    }

    /// Production float path. Currently dispatches to the `write!`-based
    /// formatter — see the `float` module below for the alternate ryu
    /// path and the bench that picks between them. Both reject non-finite
    /// inputs with `ErrorKind::NonFiniteFloat`.
    #[inline]
    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        float::format_f64_write(f, self.out)
    }
}

// ---------------------------------------------------------------------------
// ByteSink — fast path for to_string / to_vec
// ---------------------------------------------------------------------------

/// `JsonWrite` sink that appends to a `Vec<u8>`.
///
/// Bypasses `String`'s UTF-8 invariant maintenance — every byte
/// written is known-valid by construction, so the caller can convert
/// to `String` via `from_utf8_unchecked` after serialization completes.
#[cfg(feature = "alloc")]
#[derive(Debug)]
pub struct ByteSink<'a> {
    out: &'a mut alloc::vec::Vec<u8>,
}

#[cfg(feature = "alloc")]
impl<'a> ByteSink<'a> {
    #[must_use]
    pub const fn new(out: &'a mut alloc::vec::Vec<u8>) -> Self {
        Self { out }
    }
}

#[cfg(feature = "alloc")]
impl JsonWrite for ByteSink<'_> {
    type Error = Error;

    #[inline]
    fn reserve_hint(&mut self, additional: usize) {
        self.out.reserve(additional);
    }

    #[inline]
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        self.out.push(b);
        Ok(())
    }

    /// Branchless byte append. Skips `Vec::push`'s capacity check; relies
    /// on the caller having reserved enough via `reserve_hint`.
    ///
    /// # Safety
    /// `self.out.len() < self.out.capacity()` must hold. The caller's
    /// `reserve_hint` is what makes this safe in `write_array` for
    /// primitive slices.
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_byte_unchecked(&mut self, b: u8) -> Result<(), Self::Error> {
        // SAFETY: caller-asserted len < capacity. Equivalent to
        // `Vec::push` minus the cap check + grow path. Writing directly
        // to the tail and bumping len keeps the per-call cost to two
        // memory ops with no branches.
        unsafe {
            let len = self.out.len();
            let ptr = self.out.as_mut_ptr().add(len);
            core::ptr::write(ptr, b);
            self.out.set_len(len + 1);
        }
        Ok(())
    }

    #[inline]
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }

    #[inline]
    fn write_raw_bytes(&mut self, b: &[u8]) -> Result<(), Self::Error> {
        self.out.extend_from_slice(b);
        Ok(())
    }

    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.push(b'"');
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut start = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if needs_escape(b) {
                if start < i {
                    self.out.extend_from_slice(&bytes[start..i]);
                }
                write_escape_byte(self, b)?;
                start = i + 1;
            }
            i += 1;
        }
        if start < bytes.len() {
            self.out.extend_from_slice(&bytes[start..]);
        }
        self.out.push(b'"');
        Ok(())
    }

    #[inline]
    fn write_int_i64(&mut self, n: i64) -> Result<(), Self::Error> {
        if n >= 0 {
            #[allow(clippy::cast_sign_loss)]
            return self.write_int_u64(n as u64);
        }
        self.out.push(b'-');
        #[allow(clippy::cast_possible_truncation)]
        let mag = i128::from(n).unsigned_abs() as u64;
        self.write_int_u64(mag)
    }

    #[inline]
    fn write_int_u64(&mut self, n: u64) -> Result<(), Self::Error> {
        format_u64_direct(n, self.out);
        Ok(())
    }

    #[inline]
    fn write_int_i128(&mut self, n: i128) -> Result<(), Self::Error> {
        let mut buf = [0u8; 40];
        let s = format_i128(n, &mut buf);
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }

    #[inline]
    fn write_int_u128(&mut self, n: u128) -> Result<(), Self::Error> {
        let mut buf = [0u8; 40];
        let s = format_u128(n, &mut buf);
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }

    #[inline]
    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        // `format_finite_to_vec` returns false for non-finite inputs (after
        // doing its own bit-pattern check); folding the check in there keeps
        // `f` in `xmm0` across the call and avoids the spill/reload LLVM
        // produced when the check sat at this call site.
        if crate::float::format_finite_to_vec(f, self.out) {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::NonFiniteFloat, Position::START))
        }
    }

    /// SAFETY-relying override: caller must have reserved ≥ 32 bytes via
    /// `reserve_hint`. Skips `Vec::extend_from_slice`'s per-call cap
    /// check that the per-element float loop otherwise pays.
    ///
    /// The non-finite arm is outlined via `#[cold]` so the branch
    /// predictor gets a strong static hint that the error path is rare.
    /// On `perf record -e branch-misses` the inlined `Err::new(...)`
    /// path inflated the finiteness `jg`'s mispredict rate; outlining
    /// it pushed the cold-arm PCs out of the hot loop's history.
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_float_f64_unchecked(&mut self, f: f64) -> Result<(), Self::Error> {
        // SAFETY: caller-guaranteed capacity (≥ 32) is exactly what
        // `format_finite_to_vec_unchecked` requires.
        if unsafe { crate::float::format_finite_to_vec_unchecked(f, self.out) } {
            Ok(())
        } else {
            cold_nonfinite_byte_sink()
        }
    }

    /// Finite-known override: skip both the cap check AND the finiteness
    /// check. The slice writer guarantees both preconditions via a
    /// one-shot pre-scan + `reserve_hint`. This removes the last
    /// data-dependent branch from the bench's per-element loop.
    ///
    /// # Safety
    /// `cap - len ≥ 32` AND `f.is_finite()`.
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_float_f64_unchecked_finite(
        &mut self,
        f: f64,
    ) -> Result<(), Self::Error> {
        // SAFETY: contract forwarded.
        unsafe { crate::float::format_finite_to_vec_unchecked_finite(f, self.out) };
        Ok(())
    }
}

/// Outlined non-finite error path for `ByteSink`. `#[cold]` biases the
/// branch predictor's static hint toward not-taken (the always-correct
/// guess for well-formed input) and keeps the error-construction code
/// out of the hot icache footprint.
#[cfg(feature = "alloc")]
#[cold]
#[inline(never)]
const fn cold_nonfinite_byte_sink() -> Result<(), Error> {
    Err(Error::new(ErrorKind::NonFiniteFloat, Position::START))
}

// ---------------------------------------------------------------------------
// Integer formatters — forward-write with digit-count precomputation.
// ---------------------------------------------------------------------------
//
// `format!("{n}")` routes through `core::fmt::Formatter`, which on profile
// is the dominant cost when a payload is integer-heavy. Hand-rolled
// two-digit-at-a-time formatting using a 200-byte LUT runs ~4× faster on
// a `Vec<i64>` workload and produces byte-for-byte identical output.
//
// The digit count is computed upfront via `leading_zeros()` + a lookup
// table (Champagne-Gareau & Lemire, SPE 2026), then digits are written
// forward from the end of the buffer toward the front. This eliminates
// the backward-fill + `ptr::copy` shift the previous implementation paid.

const DIGIT_LUT: &[u8; 200] = b"\
0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

/// Branchless digit count for a `u64`.
///
/// Uses `leading_zeros()` to index into a table of candidate digit counts,
/// then one comparison resolves the off-by-one boundary. The table stores
/// `(candidate_digit_count, threshold)` pairs: if `n > threshold` the true
/// count is `candidate + 1`, otherwise it's `candidate`. This runs in ~3
/// cycles on x86-64 (`lzcnt` + table load + compare).
#[inline]
#[allow(clippy::cast_possible_truncation)]
fn fast_digit_count(n: u64) -> usize {
    // Table indexed by `leading_zeros(n | 1)`. Each entry is the largest
    // value with `digit_count` digits; if `n` exceeds it, the true count
    // is one more. Entry 63 covers n=0 and n=1 (lzcnt=63).
    //
    // Built from: for each lzcnt value z, the candidate digit count is
    // floor(log10(2^(63-z))) + 1 when the range is unambiguous, with the
    // threshold being 10^candidate - 1.
    // Each entry is `(candidate, threshold)` where `candidate` is the
    // digit count when `n <= threshold`, and `candidate + 1` when
    // `n > threshold`. For lzcnt buckets where all values share the
    // same digit count, threshold is set to the bucket's max so the
    // `+1` never fires.
    static TABLE: [(u8, u64); 64] = [
        (19, 9_999_999_999_999_999_999), // lzcnt  0: 19 or 20 digits
        (19, 9_223_372_036_854_775_807), // lzcnt  1: always 19
        (19, 4_611_686_018_427_387_903), // lzcnt  2: always 19
        (19, 2_305_843_009_213_693_951), // lzcnt  3: always 19
        (18,   999_999_999_999_999_999), // lzcnt  4: 18 or 19
        (18,   576_460_752_303_423_487), // lzcnt  5: always 18
        (18,   288_230_376_151_711_743), // lzcnt  6: always 18
        (17,    99_999_999_999_999_999), // lzcnt  7: 17 or 18
        (17,    72_057_594_037_927_935), // lzcnt  8: always 17
        (17,    36_028_797_018_963_967), // lzcnt  9: always 17
        (16,     9_999_999_999_999_999), // lzcnt 10: 16 or 17
        (16,     9_007_199_254_740_991), // lzcnt 11: always 16
        (16,     4_503_599_627_370_495), // lzcnt 12: always 16
        (16,     2_251_799_813_685_247), // lzcnt 13: always 16
        (15,       999_999_999_999_999), // lzcnt 14: 15 or 16
        (15,       562_949_953_421_311), // lzcnt 15: always 15
        (15,       281_474_976_710_655), // lzcnt 16: always 15
        (14,        99_999_999_999_999), // lzcnt 17: 14 or 15
        (14,        70_368_744_177_663), // lzcnt 18: always 14
        (14,        35_184_372_088_831), // lzcnt 19: always 14
        (13,         9_999_999_999_999), // lzcnt 20: 13 or 14
        (13,         8_796_093_022_207), // lzcnt 21: always 13
        (13,         4_398_046_511_103), // lzcnt 22: always 13
        (13,         2_199_023_255_551), // lzcnt 23: always 13
        (12,           999_999_999_999), // lzcnt 24: 12 or 13
        (12,           549_755_813_887), // lzcnt 25: always 12
        (12,           274_877_906_943), // lzcnt 26: always 12
        (11,            99_999_999_999), // lzcnt 27: 11 or 12
        (11,            68_719_476_735), // lzcnt 28: always 11
        (11,            34_359_738_367), // lzcnt 29: always 11
        (10,             9_999_999_999), // lzcnt 30: 10 or 11
        (10,             8_589_934_591), // lzcnt 31: always 10
        (10,             4_294_967_295), // lzcnt 32: always 10
        (10,             2_147_483_647), // lzcnt 33: always 10
        ( 9,               999_999_999), // lzcnt 34: 9 or 10
        ( 9,               536_870_911), // lzcnt 35: always 9
        ( 9,               268_435_455), // lzcnt 36: always 9
        ( 8,                99_999_999), // lzcnt 37: 8 or 9
        ( 8,                67_108_863), // lzcnt 38: always 8
        ( 8,                33_554_431), // lzcnt 39: always 8
        ( 7,                 9_999_999), // lzcnt 40: 7 or 8
        ( 7,                 8_388_607), // lzcnt 41: always 7
        ( 7,                 4_194_303), // lzcnt 42: always 7
        ( 7,                 2_097_151), // lzcnt 43: always 7
        ( 6,                   999_999), // lzcnt 44: 6 or 7
        ( 6,                   524_287), // lzcnt 45: always 6
        ( 6,                   262_143), // lzcnt 46: always 6
        ( 5,                    99_999), // lzcnt 47: 5 or 6
        ( 5,                    65_535), // lzcnt 48: always 5
        ( 5,                    32_767), // lzcnt 49: always 5
        ( 4,                     9_999), // lzcnt 50: 4 or 5
        ( 4,                     8_191), // lzcnt 51: always 4
        ( 4,                     4_095), // lzcnt 52: always 4
        ( 4,                     2_047), // lzcnt 53: always 4
        ( 3,                       999), // lzcnt 54: 3 or 4
        ( 3,                       511), // lzcnt 55: always 3
        ( 3,                       255), // lzcnt 56: always 3
        ( 2,                        99), // lzcnt 57: 2 or 3
        ( 2,                        63), // lzcnt 58: always 2
        ( 2,                        31), // lzcnt 59: always 2
        ( 1,                         9), // lzcnt 60: 1 or 2
        ( 1,                         7), // lzcnt 61: always 1
        ( 1,                         3), // lzcnt 62: always 1
        ( 1,                         1), // lzcnt 63: always 1
    ];
    let lz = (n | 1).leading_zeros() as usize;
    let (candidate, threshold) = TABLE[lz];
    candidate as usize + usize::from(n > threshold)
}

/// Write `n` as decimal digits into `buf[0..end]`, filling from the tail
/// toward index 0. Caller must ensure `buf` points to at least `end`
/// writable bytes and that `end == fast_digit_count(n)`.
#[inline]
#[allow(clippy::cast_possible_truncation, unsafe_code)]
fn write_digits_backward(mut n: u64, buf: *mut u8, end: usize) {
    let mut pos = end;
    while n >= 100 {
        let r = (n % 100) as usize;
        n /= 100;
        pos -= 2;
        // SAFETY: `pos` decreases in steps of 2 from `end` (which equals
        // the digit count of the original `n`). The loop exits before
        // `pos` underflows because each iteration consumes two decimal
        // digits. Caller guarantees `buf[0..end]` is writable.
        unsafe {
            *buf.add(pos) = DIGIT_LUT[r * 2];
            *buf.add(pos + 1) = DIGIT_LUT[r * 2 + 1];
        }
    }
    if n >= 10 {
        let r = n as usize;
        pos -= 2;
        unsafe {
            *buf.add(pos) = DIGIT_LUT[r * 2];
            *buf.add(pos + 1) = DIGIT_LUT[r * 2 + 1];
        }
    } else {
        pos -= 1;
        unsafe {
            *buf.add(pos) = b'0' + n as u8;
        }
    }
}

/// Format a `u64` directly into a `Vec<u8>`. Precomputes digit count so
/// digits land at their final position — no post-copy shift needed.
#[cfg(feature = "alloc")]
#[allow(clippy::cast_possible_truncation)]
fn format_u64_direct(n: u64, out: &mut alloc::vec::Vec<u8>) {
    let digits = fast_digit_count(n);
    out.reserve(digits);
    let old_len = out.len();
    #[allow(unsafe_code)]
    unsafe {
        let base = out.as_mut_ptr().add(old_len);
        write_digits_backward(n, base, digits);
        out.set_len(old_len + digits);
    }
}

/// Format a `u64` into `buf` (big-endian text). Returns the slice of
/// `buf` that holds the digits.
#[allow(clippy::cast_possible_truncation)]
fn format_u64(n: u64, buf: &mut [u8; 20]) -> &str {
    let digits = fast_digit_count(n);
    #[allow(unsafe_code)]
    unsafe {
        write_digits_backward(n, buf.as_mut_ptr(), digits);
        core::str::from_utf8_unchecked(&buf[..digits])
    }
}

fn format_i64(n: i64, buf: &mut [u8; 20]) -> &str {
    if n >= 0 {
        #[allow(clippy::cast_sign_loss)]
        return format_u64(n as u64, buf);
    }
    #[allow(clippy::cast_possible_truncation)]
    let mag = i128::from(n).unsigned_abs() as u64;
    let digits = fast_digit_count(mag);
    buf[0] = b'-';
    write_digits_backward(mag, buf[1..].as_mut_ptr(), digits);
    let total = 1 + digits;
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[..total])
    }
}

#[allow(clippy::cast_possible_truncation)]
fn format_u128(mut n: u128, buf: &mut [u8; 40]) -> &str {
    let mut pos = buf.len();
    while n >= 100 {
        let r = (n % 100) as usize;
        n /= 100;
        pos -= 2;
        buf[pos] = DIGIT_LUT[r * 2];
        buf[pos + 1] = DIGIT_LUT[r * 2 + 1];
    }
    if n >= 10 {
        let r = n as usize;
        pos -= 2;
        buf[pos] = DIGIT_LUT[r * 2];
        buf[pos + 1] = DIGIT_LUT[r * 2 + 1];
    } else {
        pos -= 1;
        buf[pos] = b'0' + n as u8;
    }
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[pos..])
    }
}

fn format_i128(n: i128, buf: &mut [u8; 40]) -> &str {
    if n >= 0 {
        #[allow(clippy::cast_sign_loss)]
        return format_u128(n as u128, buf);
    }
    let mag = n.unsigned_abs();
    let mut tmp = [0u8; 40];
    let s = format_u128(mag, &mut tmp);
    let len = s.len();
    buf[0] = b'-';
    buf[1..=len].copy_from_slice(s.as_bytes());
    let total = 1 + len;
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[..total])
    }
}

// ---------------------------------------------------------------------------
// Float formatting.
//
// `format_f64_write` dispatches to the in-tree Grisu3 formatter
// (`crate::float::format_finite`), which falls back to libstd's
// `Display for f64` on the ~0.5% of inputs where Grisu3 cannot prove
// its output is the shortest round-trip representation.
//
// The bench in `bourne-bench/floats` pins this entry point by name.
// ---------------------------------------------------------------------------

#[cfg(feature = "alloc")]
pub mod float {
    //! Public so the head-to-head bench in `bourne-bench` can pin the
    //! production formatter by name; not part of the documented API
    //! surface.

    use super::{Error, ErrorKind, Position};
    use alloc::string::String;

    /// Reject `inf` / `-inf` / `NaN` with a typed error. Position is
    /// `START` because serializer errors don't have an input byte to
    /// point at — symmetric with how the parse side reports
    /// "byte offset" errors.
    #[inline]
    const fn reject_non_finite(f: f64) -> Result<(), Error> {
        if f.is_finite() {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::NonFiniteFloat, Position::START))
        }
    }

    /// Production float formatter. Delegates correctness to the
    /// in-tree Grisu3 implementation (`crate::float`). Non-finite
    /// inputs are rejected before any digit work happens.
    #[inline]
    pub fn format_f64_write(f: f64, out: &mut String) -> Result<(), Error> {
        reject_non_finite(f)?;
        crate::float::format_finite(f, out);
        Ok(())
    }

}

// ---------------------------------------------------------------------------
// ToJson trait + entry points
// ---------------------------------------------------------------------------

/// Types that know how to serialize themselves to a [`JsonWrite`] sink.
///
/// This is the dual of [`crate::FromJson`]. Implementors call sink methods
/// directly — there is no intermediate `Value` representation.
pub trait ToJson {
    /// Lower bound on the number of bytes `write_json` will emit.
    ///
    /// Used by [`to_vec`] / [`to_string`] to size the initial allocation.
    /// Defaults to `0` so existing impls aren't forced to provide it.
    const MIN_SERIALIZED_LEN: usize = 0;

    /// Upper bound on the bytes a single `write_json` call emits, used by
    /// sequence impls to pre-reserve buffer capacity. `0` means "no useful
    /// upper bound" — sequence impls skip the reservation in that case.
    ///
    /// Primitives with a known maximum output length (floats, ints, bools)
    /// override this. Variable-length types (`str`, `Vec<T>`, structs) keep
    /// the default, since their per-element size depends on payload.
    const MAX_SERIALIZED_LEN: usize = 0;

    /// Serialize `self` into `w`.
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error>;

    /// Serialize `self` into `w` ASSUMING the caller has already reserved
    /// at least `MAX_SERIALIZED_LEN` writable bytes in the sink. Used by
    /// [`[T]::write_json`] after its `reserve_hint` call so the per-element
    /// path skips its own cap checks.
    ///
    /// Default impl forwards to `write_json` (safe + checked). Primitives
    /// override with sink-specific unchecked variants. For `MAX_SERIALIZED_LEN
    /// == 0` types this method is never called.
    ///
    /// # Safety
    /// `Self::MAX_SERIALIZED_LEN` bytes of sink capacity must be guaranteed
    /// available before this call. For types where
    /// [`Self::NEEDS_VALIDATION`] is `true`, [`Self::pre_validate_slice`]
    /// must have been called on the enclosing slice (and returned `Ok`)
    /// before invoking this method, so the per-element write can assume
    /// the type's validity precondition (e.g. finiteness for floats).
    #[inline]
    #[allow(unsafe_code)]
    unsafe fn write_json_in_reserved<W: JsonWrite + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<(), W::Error> {
        self.write_json(w)
    }

    /// Whether this type requires a pre-scan validation pass before
    /// the per-element fast path can be entered. Defaults to `false`.
    ///
    /// `f64` / `f32` set this to `true` so the slice writer scans the
    /// whole slice once for non-finite values and rejects up front.
    /// The inner element-by-element loop then runs branchless w.r.t.
    /// finiteness — measured 13.43% of all mispredicts before the
    /// pre-scan was added.
    const NEEDS_VALIDATION: bool = false;

    /// Validate that every element of `slice` satisfies the type's
    /// per-element precondition (e.g. finiteness). Called by the slice
    /// writer ONCE before the per-element loop when `NEEDS_VALIDATION`
    /// is `true`.
    ///
    /// Default impl is a no-op. The `Result` carries `bourne_core::Error`
    /// directly because all current validators reject with that type;
    /// callers that want a different sink error must map.
    ///
    /// `slice` is `&[Self]` rather than `&[T]` because the validator
    /// always sees a homogeneous slice (it's invoked from the slice
    /// impl). On non-slice paths this method is never called.
    #[inline]
    fn pre_validate_slice(slice: &[Self]) -> Result<(), Error>
    where
        Self: Sized,
    {
        let _ = slice;
        Ok(())
    }
}

/// Serialize `value` into a fresh `String`.
///
/// Returns [`Error`] (rather than `Infallible`) so non-finite floats and
/// other typed-level failures have a place to surface.
///
/// Internally serializes into a `Vec<u8>` via [`ByteSink`] (avoiding
/// per-byte UTF-8 invariant checks), then converts to `String` in one
/// step. The resulting bytes are guaranteed valid UTF-8 because every
/// `JsonWrite` method only emits ASCII structural bytes, `&str` slices
/// (valid by construction), ASCII escape sequences, and ASCII digit
/// sequences from the integer/float formatters.
#[cfg(feature = "alloc")]
pub fn to_string<T: ToJson + ?Sized>(value: &T) -> Result<String, Error> {
    let bytes = to_vec(value)?;
    // SAFETY: every code path through `ByteSink`'s `JsonWrite` impl
    // emits only valid UTF-8:
    //   - `write_byte`: called with ASCII structural punctuation
    //   - `write_str_raw`: takes `&str`, valid UTF-8 by definition
    //   - `write_escaped_str`: copies `&str` bytes verbatim (valid
    //     UTF-8) plus ASCII escape sequences
    //   - `write_int_*`: ASCII digits from the LUT formatter
    //   - `write_float_f64`: ASCII digits from Grisu3 / libstd
    #[allow(unsafe_code)]
    Ok(unsafe { String::from_utf8_unchecked(bytes) })
}

/// Serialize `value` into a fresh `Vec<u8>`.
///
/// This is the primary fast path: writes directly into `Vec<u8>` via
/// [`ByteSink`], bypassing `String`'s per-write UTF-8 invariant checks.
#[cfg(feature = "alloc")]
pub fn to_vec<T: ToJson + ?Sized>(value: &T) -> Result<alloc::vec::Vec<u8>, Error> {
    let cap = if T::MIN_SERIALIZED_LEN > 128 {
        T::MIN_SERIALIZED_LEN
    } else {
        128
    };
    let mut out = alloc::vec::Vec::with_capacity(cap);
    let mut sink = ByteSink::new(&mut out);
    value.write_json(&mut sink)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// fmt::Write sink
// ---------------------------------------------------------------------------

/// `JsonWrite` sink that forwards to any `core::fmt::Write` implementor.
///
/// Useful when the destination is something other than a `String` —
/// `&mut String` is the obvious case, but any `fmt::Write` works (a
/// `Formatter`, a custom buffered writer, a tracing-style accumulator).
///
/// The error type is [`core::fmt::Error`] for byte/string writes and
/// [`Error`] for the float path; the unified sink-level error is
/// [`Error`], with `core::fmt::Error` mapped to a generic write
/// failure.
#[cfg(feature = "alloc")]
#[derive(Debug)]
pub struct FmtWriteSink<'a, W: ?Sized> {
    out: &'a mut W,
}

#[cfg(feature = "alloc")]
impl<'a, W: core::fmt::Write + ?Sized> FmtWriteSink<'a, W> {
    pub const fn new(out: &'a mut W) -> Self {
        Self { out }
    }
}

#[cfg(feature = "alloc")]
impl<W: core::fmt::Write + ?Sized> JsonWrite for FmtWriteSink<'_, W> {
    type Error = Error;

    #[inline]
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        self.out
            .write_char(b as char)
            .map_err(|_| fmt_write_error())
    }

    #[inline]
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.write_str(s).map_err(|_| fmt_write_error())
    }

    #[inline]
    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        if !f.is_finite() {
            return Err(Error::new(ErrorKind::NonFiniteFloat, Position::START));
        }
        crate::float::format_finite_fmt(f, self.out).map_err(|_| fmt_write_error())
    }
}

/// `fmt::Write` errors don't carry detail. Map to a typed parse-style
/// error so callers can distinguish the failure mode without losing
/// the trait's error contract.
#[cfg(feature = "alloc")]
#[inline]
const fn fmt_write_error() -> Error {
    Error::new(ErrorKind::TypeMismatch, Position::START)
}

// ---------------------------------------------------------------------------
// io::Write sink (std-only)
// ---------------------------------------------------------------------------

/// `JsonWrite` sink that forwards to any `std::io::Write` implementor.
///
/// The natural target for serializing JSON to a file, socket, or other
/// byte stream. Errors propagate through the sink's `Self::Error`,
/// which is [`std::io::Error`].
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct IoWriteSink<'a, W: ?Sized> {
    out: &'a mut W,
}

#[cfg(feature = "std")]
impl<'a, W: std::io::Write + ?Sized> IoWriteSink<'a, W> {
    pub const fn new(out: &'a mut W) -> Self {
        Self { out }
    }
}

/// Adapter that lets `core::fmt::Write` write into an `io::Write` sink.
/// `format_finite_fmt` only needs `fmt::Write`; this carries the
/// underlying I/O error out so the float path's failures don't get
/// flattened into a generic "fmt failed".
#[cfg(feature = "std")]
struct IoFmtAdapter<'a, W: std::io::Write + ?Sized> {
    inner: &'a mut W,
    err: Option<std::io::Error>,
}

#[cfg(feature = "std")]
impl<W: std::io::Write + ?Sized> core::fmt::Write for IoFmtAdapter<'_, W> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        match self.inner.write_all(s.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.err = Some(e);
                Err(core::fmt::Error)
            }
        }
    }
}

#[cfg(feature = "std")]
impl<W: std::io::Write + ?Sized> JsonWrite for IoWriteSink<'_, W> {
    type Error = std::io::Error;

    #[inline]
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        self.out.write_all(&[b])
    }

    #[inline]
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.write_all(s.as_bytes())
    }

    #[inline]
    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        if !f.is_finite() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-finite float not representable in JSON",
            ));
        }
        let mut adapter = IoFmtAdapter {
            inner: self.out,
            err: None,
        };
        match crate::float::format_finite_fmt(f, &mut adapter) {
            Ok(()) => Ok(()),
            Err(_) => Err(adapter.err.unwrap_or_else(|| {
                std::io::Error::other("fmt error in float formatter")
            })),
        }
    }
}

/// Serialize `value` directly into a [`std::io::Write`] sink.
///
/// The standard "stream JSON to a file/socket" entry point. For an
/// in-memory build use [`to_string`] / [`to_vec`].
#[cfg(feature = "std")]
pub fn to_writer<T: ToJson + ?Sized, W: std::io::Write>(
    value: &T,
    writer: &mut W,
) -> Result<(), std::io::Error> {
    let mut sink = IoWriteSink::new(writer);
    value.write_json(&mut sink)
}

/// Serialize `value` into any [`core::fmt::Write`] sink.
///
/// Returns the underlying [`Error`] (typed) on failure — including
/// non-finite floats. For `String` targets prefer [`to_string`]; this
/// entry point is for arbitrary `fmt::Write` consumers.
#[cfg(feature = "alloc")]
pub fn to_fmt<T: ToJson + ?Sized, W: core::fmt::Write + ?Sized>(
    value: &T,
    writer: &mut W,
) -> Result<(), Error> {
    let mut sink = FmtWriteSink::new(writer);
    value.write_json(&mut sink)
}

// ---------------------------------------------------------------------------
// Pretty-print sink
// ---------------------------------------------------------------------------

/// `JsonWrite` sink that emits indented, multi-line JSON.
///
/// Wraps a `String` and tracks container depth + a one-byte lookahead
/// (`pending_open`). When an opener (`[` / `{`) is followed immediately
/// by the matching closer (no contents), the sink emits the compact
/// form `[]` / `{}`. Otherwise it inserts a newline plus the current
/// indent before each element and before the closing bracket.
///
/// Indent unit defaults to two spaces; configure via [`Self::with_indent`].
#[cfg(feature = "alloc")]
#[derive(Debug)]
pub struct PrettyStringSink<'a> {
    out: &'a mut String,
    indent: &'static str,
    depth: usize,
    /// `Some(b)` when we wrote `[` or `{` and haven't yet decided
    /// whether the container is empty. The next structural byte
    /// resolves it: a matching close → emit `[]` / `{}`; anything
    /// else → flush the open + newline + indent for the first
    /// element, then continue.
    pending_open: Option<u8>,
}

#[cfg(feature = "alloc")]
impl<'a> PrettyStringSink<'a> {
    /// Build a pretty sink writing to `out` with the default 2-space
    /// indent.
    #[must_use]
    pub const fn new(out: &'a mut String) -> Self {
        Self {
            out,
            indent: "  ",
            depth: 0,
            pending_open: None,
        }
    }

    /// Build a pretty sink with a custom indent string. Pass `"\t"`
    /// for tabs, `"    "` for four spaces, etc. The indent must be
    /// pure whitespace — JSON doesn't validate it on the wire, but
    /// emitting non-whitespace would corrupt the output.
    #[must_use]
    pub const fn with_indent(out: &'a mut String, indent: &'static str) -> Self {
        Self {
            out,
            indent,
            depth: 0,
            pending_open: None,
        }
    }

    /// Resolve any pending open by emitting it and dropping the
    /// pending state. Used before writing any non-structural byte
    /// (a value's first byte).
    fn flush_pending_open(&mut self) {
        if let Some(b) = self.pending_open.take() {
            self.out.push(b as char);
            self.depth += 1;
            self.newline_and_indent();
        }
    }

    fn newline_and_indent(&mut self) {
        self.out.push('\n');
        for _ in 0..self.depth {
            self.out.push_str(self.indent);
        }
    }
}

#[cfg(feature = "alloc")]
impl JsonWrite for PrettyStringSink<'_> {
    type Error = Error;

    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        match b {
            b'[' | b'{' => {
                // Resolve any prior pending open: the prior container
                // is non-empty, so emit it + indent for our position.
                self.flush_pending_open();
                // Defer this open — we don't know yet if it's empty.
                self.pending_open = Some(b);
                Ok(())
            }
            b']' | b'}' => {
                if let Some(open) = self.pending_open.take() {
                    // Empty container: write the open and matching
                    // close back-to-back with no whitespace.
                    self.out.push(open as char);
                    self.out.push(b as char);
                    return Ok(());
                }
                self.depth -= 1;
                self.newline_and_indent();
                self.out.push(b as char);
                Ok(())
            }
            b',' => {
                // After a value inside a container: newline + indent
                // before the next element. The pending_open state
                // can't be live here — we must have written at least
                // one value to be at a comma.
                debug_assert!(self.pending_open.is_none());
                self.out.push(',');
                self.newline_and_indent();
                Ok(())
            }
            b':' => {
                // Object key/value separator. JSON-pretty convention
                // is `key: value` (one space after the colon, none
                // before).
                self.out.push_str(": ");
                Ok(())
            }
            _ => {
                // Any other single byte (rare via this entry — most
                // bulk text comes through write_str_raw or
                // write_escaped_str).
                self.flush_pending_open();
                self.out.push(b as char);
                Ok(())
            }
        }
    }

    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        if s.is_empty() {
            return Ok(());
        }
        self.flush_pending_open();
        self.out.push_str(s);
        Ok(())
    }

    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        self.flush_pending_open();
        // Reuse the StringSink escape walk by constructing one
        // transiently. The borrow lasts only for this call.
        let mut inner = StringSink::new(self.out);
        inner.write_escaped_str(s)
    }

    fn write_float_f64(&mut self, f: f64) -> Result<(), Self::Error> {
        self.flush_pending_open();
        float::format_f64_write(f, self.out)
    }
}

/// Pretty-printed equivalent of [`to_string`]. Two-space indent, one
/// space after `:`, newline between every element. Empty containers
/// are kept compact (`[]` / `{}`).
#[cfg(feature = "alloc")]
pub fn to_string_pretty<T: ToJson + ?Sized>(value: &T) -> Result<String, Error> {
    let mut out = String::with_capacity(256);
    let mut sink = PrettyStringSink::new(&mut out);
    value.write_json(&mut sink)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

impl ToJson for bool {
    const MIN_SERIALIZED_LEN: usize = 4; // "true"
    const MAX_SERIALIZED_LEN: usize = 5; // "false"
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_str_raw(if *self { "true" } else { "false" })
    }
}

impl ToJson for () {
    const MIN_SERIALIZED_LEN: usize = 4; // "null"
    const MAX_SERIALIZED_LEN: usize = 4;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_str_raw("null")
    }
}

impl ToJson for str {
    const MIN_SERIALIZED_LEN: usize = 2; // "\"\""
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_escaped_str(self)
    }
}

// `&str` is the form callers actually hold. The blanket `&T` impl below
// covers it via the `str` impl above — no explicit `&str` arm needed.

macro_rules! impl_int_signed {
    ($($t:ty),* $(,)?) => {
        $(
            impl ToJson for $t {
                const MIN_SERIALIZED_LEN: usize = 1;
                // sign + digits. i64::MIN is "-9223372036854775808" = 20 chars.
                const MAX_SERIALIZED_LEN: usize = 20;
                #[inline]
                fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
                    w.write_int_i64(i64::from(*self))
                }
            }
        )*
    };
}

macro_rules! impl_int_unsigned {
    ($($t:ty),* $(,)?) => {
        $(
            impl ToJson for $t {
                const MIN_SERIALIZED_LEN: usize = 1;
                // u64::MAX = "18446744073709551615" = 20 chars.
                const MAX_SERIALIZED_LEN: usize = 20;
                #[inline]
                fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
                    w.write_int_u64(u64::from(*self))
                }
            }
        )*
    };
}

impl_int_signed!(i8, i16, i32, i64);
impl_int_unsigned!(u8, u16, u32, u64);

// `isize` / `usize` widen to 64-bit on the platforms bourne supports.
// `as` is acceptable here: the cast is the documented platform widening,
// not a truncation.
#[allow(clippy::cast_possible_wrap, clippy::cast_lossless, clippy::use_self)]
impl ToJson for isize {
    const MIN_SERIALIZED_LEN: usize = 1;
    const MAX_SERIALIZED_LEN: usize = 20;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_i64(*self as i64)
    }
}

#[allow(clippy::cast_lossless, clippy::use_self)]
impl ToJson for usize {
    const MIN_SERIALIZED_LEN: usize = 1;
    const MAX_SERIALIZED_LEN: usize = 20;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_u64(*self as u64)
    }
}

impl ToJson for i128 {
    const MIN_SERIALIZED_LEN: usize = 1;
    // sign + 39 digits for i128::MIN.
    const MAX_SERIALIZED_LEN: usize = 40;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_i128(*self)
    }
}

impl ToJson for u128 {
    const MIN_SERIALIZED_LEN: usize = 1;
    // u128::MAX has 39 digits.
    const MAX_SERIALIZED_LEN: usize = 39;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_u128(*self)
    }
}

// Composite: Option<T> writes `null` for None or the inner value for Some.
// Mirrors the FromJson direction.
impl<T: ToJson> ToJson for Option<T> {
    const MIN_SERIALIZED_LEN: usize = 4; // "null"
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        match self {
            None => w.write_str_raw("null"),
            Some(v) => v.write_json(w),
        }
    }
}

// References pass through. `&T: ToJson` whenever `T: ToJson` lets callers
// pass `&value` or `&&value` indifferently.
impl<T: ToJson + ?Sized> ToJson for &T {
    const MIN_SERIALIZED_LEN: usize = T::MIN_SERIALIZED_LEN;
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        (*self).write_json(w)
    }
}

// Slices and fixed-size arrays serialize as JSON arrays. The shared
// helper writes the bracketed comma-separated body so impls for Vec,
// slice, and [T; N] don't drift on punctuation.
//
// `vec_write_json` is the symmetric hook to FromJson's `vec_from_lex`:
// types like `i64` / `&str` whose per-element write goes through method
// dispatch can override it to write directly through the sink. Default
// loops `T::write_json` per element.
fn write_array<T: ToJson, I: IntoIterator<Item = T>, W: JsonWrite + ?Sized>(
    iter: I,
    w: &mut W,
) -> Result<(), W::Error> {
    w.write_byte(b'[')?;
    // Peel the first element so the inner loop never re-evaluates a
    // `first` flag — every subsequent element unconditionally writes
    // `,` then itself. Saves one branch per element on hot Vec/slice
    // paths where the iterator is `ExactSizeIterator` and the compiler
    // can hoist the check out of the loop.
    let mut iter = iter.into_iter();
    if let Some(v) = iter.next() {
        v.write_json(w)?;
        for v in iter {
            w.write_byte(b',')?;
            v.write_json(w)?;
        }
    }
    w.write_byte(b']')
}

/// Like `write_array`, but emits the brackets and commas via
/// `write_byte_unchecked`. Callable only after `reserve_hint` has been
/// invoked with at least `2 + len + len * MAX_SERIALIZED_LEN` bytes.
///
/// Why this exists: `perf record -e branch-misses` on the n=10000
/// float-array workload identified the comma capacity-check branch
/// (`cmp len, cap; jne grow`) as the single largest mispredict source —
/// 12.96% of all mispredicts in `to_string`. Hoisting the capacity check
/// via `reserve_hint` and emitting the comma unchecked removes that
/// branch from the inner loop entirely.
///
/// # Safety
/// Caller must have reserved enough sink capacity to cover `2 + len +
/// per_elem_max * len` bytes after the call to `reserve_hint`.
#[allow(unsafe_code)]
unsafe fn write_array_reserved<T: ToJson, W: JsonWrite + ?Sized>(
    slice: &[T],
    w: &mut W,
) -> Result<(), W::Error> {
    // SAFETY: brackets fit in the reserved space (we accounted for 2
    // bracket bytes in the hint). Per-element writes also fit because
    // each is bounded by `MAX_SERIALIZED_LEN + 1` (the +1 covers the
    // comma).
    unsafe {
        w.write_byte_unchecked(b'[')?;
    }
    if let Some((first, rest)) = slice.split_first() {
        // SAFETY: `MAX_SERIALIZED_LEN` was included in the caller's
        // `reserve_hint`, so each element write fits in the reserved
        // tail.
        unsafe { first.write_json_in_reserved(w) }?;
        for v in rest {
            // SAFETY: see function-level safety contract.
            unsafe { w.write_byte_unchecked(b',') }?;
            unsafe { v.write_json_in_reserved(w) }?;
        }
    }
    // SAFETY: same as the opening bracket.
    unsafe { w.write_byte_unchecked(b']') }
}

impl<T: ToJson> ToJson for [T] {
    const MIN_SERIALIZED_LEN: usize = 2; // "[]"
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        // Pre-reserve when the element type has a known upper bound. The
        // hint covers brackets (`[`, `]`), commas (1 per element), and
        // the per-element max payload. After this, *every* byte the
        // primitive path writes fits in the reserved tail.
        if T::MAX_SERIALIZED_LEN != 0 {
            // Pre-scan validation for types that need it (currently
            // `f64`/`f32`'s finiteness check). Doing the scan once here
            // — at predictable, branchless speed — removes the equivalent
            // per-element branch from the inner loop, which dominated
            // remaining mispredicts under random input.
            if T::NEEDS_VALIDATION {
                if let Err(_e) = T::pre_validate_slice(self) {
                    // The error type returned by `pre_validate_slice`
                    // is the typed `bourne_core::Error`. Sinks have
                    // their own `Self::Error`. We rely on the fact that
                    // every sink that calls this path uses `Error` as
                    // `Self::Error` (the only sinks with non-trivial
                    // float handling). Force the conversion via
                    // `write_float_f64` which already produces the
                    // sink's error from a non-finite input — calling
                    // it with `f64::NAN` yields the right typed error
                    // through the sink's natural channel.
                    //
                    // Concretely: when `pre_validate_slice` rejects, we
                    // re-issue the rejection through the sink so the
                    // caller's `W::Error` flows naturally.
                    return w.write_float_f64(f64::NAN);
                }
            }
            let hint = self
                .len()
                .saturating_mul(T::MAX_SERIALIZED_LEN.saturating_add(1))
                .saturating_add(2);
            w.reserve_hint(hint);
            // SAFETY: hint above covers brackets + per-element max +
            // commas. The sink's `write_byte_unchecked` skips the
            // capacity check, eliminating the mispredict-heavy
            // `Vec::push` cap branch. For types where
            // `NEEDS_VALIDATION` is true, the pre-scan above has
            // already rejected any non-finite inputs, so the
            // per-element finite-known path is safe.
            #[allow(unsafe_code)]
            return unsafe { write_array_reserved(self, w) };
        }
        write_array(self.iter(), w)
    }
}

impl<T: ToJson, const N: usize> ToJson for [T; N] {
    const MIN_SERIALIZED_LEN: usize = 2; // "[]"
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        (self as &[T]).write_json(w)
    }
}

// Tuples serialize as fixed-length heterogeneous arrays. Mirrors the
// FromJson side which accepts `[T, U, V]` for `(T, U, V)`.
//
// The macro takes the *first* element separately from the rest so the
// comma placement is unambiguous: emit the first, then for each rest
// element emit `,` followed by it. No trailing comma, no double-walk.
macro_rules! impl_tuple_to_json {
    ($first_idx:tt: $First:ident $(, $idx:tt: $T:ident)* $(,)?) => {
        impl<$First: ToJson $(, $T: ToJson)*> ToJson for ($First, $($T,)*) {
            fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
                w.write_byte(b'[')?;
                self.$first_idx.write_json(w)?;
                $(
                    w.write_byte(b',')?;
                    self.$idx.write_json(w)?;
                )*
                w.write_byte(b']')
            }
        }
    };
}

impl_tuple_to_json!(0: A);
impl_tuple_to_json!(0: A, 1: B);
impl_tuple_to_json!(0: A, 1: B, 2: C);
impl_tuple_to_json!(0: A, 1: B, 2: C, 3: D);
impl_tuple_to_json!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_tuple_to_json!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);

// ---------------------------------------------------------------------------
// alloc-gated impls
// ---------------------------------------------------------------------------

#[cfg(feature = "alloc")]
mod alloc_impls {
    use super::{Error, ErrorKind, JsonWrite, Position, ToJson};
    use alloc::borrow::Cow;
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::string::String;
    use alloc::sync::Arc;

    impl ToJson for String {
        const MIN_SERIALIZED_LEN: usize = 2; // "\"\""
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(self.as_str())
        }
    }

    impl ToJson for Cow<'_, str> {
        const MIN_SERIALIZED_LEN: usize = 2; // "\"\""
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(self.as_ref())
        }
    }

    impl ToJson for f64 {
        const MIN_SERIALIZED_LEN: usize = 1;
        // Worst-case f64 string: sign + 17 digits + '.' + 'e' + sign +
        // 3-digit exponent = 25 bytes. Round to 32 to match the
        // formatter's stack-buffer size.
        const MAX_SERIALIZED_LEN: usize = 32;
        // The slice writer calls `pre_validate_slice` to reject any
        // non-finite up front; the per-element path below then assumes
        // finiteness and skips a hot-loop branch.
        const NEEDS_VALIDATION: bool = true;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_float_f64(*self)
        }
        /// SAFETY: caller (`write_array_reserved`) has both reserved
        /// `MAX_SERIALIZED_LEN` bytes via `reserve_hint` AND already
        /// invoked `pre_validate_slice`, so `*self` is finite.
        #[inline]
        #[allow(unsafe_code)]
        unsafe fn write_json_in_reserved<W: JsonWrite + ?Sized>(
            &self,
            w: &mut W,
        ) -> Result<(), W::Error> {
            // SAFETY: forwarded to the sink's finite-known unchecked path.
            unsafe { w.write_float_f64_unchecked_finite(*self) }
        }
        /// One-shot finiteness scan over the slice. Rejects up front if
        /// any element has the all-ones IEEE exponent (inf or NaN).
        /// The loop body is branchless: each iteration ORs the bits
        /// of `EXP_MASK ^ (bits & EXP_MASK)` — zero iff the element is
        /// non-finite — into an accumulator, then a single branch at
        /// the end checks the accumulator. Auto-vectorisable by LLVM.
        #[inline]
        fn pre_validate_slice(slice: &[Self]) -> Result<(), Error> {
            const EXP_MASK: u64 = 0x7ff0_0000_0000_0000;
            // `acc` accumulates a 0 bit (XOR == 0) for any non-finite
            // input. If acc remains all-ones at the end, every element
            // was finite. We use a u64 to carry the OR-fold; LLVM tends
            // to autovec this with `por` on x86_64.
            let mut all_finite_mask = u64::MAX;
            for &f in slice {
                let bits = f.to_bits();
                // `(bits & EXP_MASK) ^ EXP_MASK` is 0 iff non-finite, else
                // some non-zero pattern. Bitwise-AND into the accumulator
                // turns it to 0 the first time we see a non-finite input.
                let elem_finite_bits = (bits & EXP_MASK) ^ EXP_MASK;
                // Spread elem_finite_bits's non-zero-ness to all bits of
                // the accumulator: if elem_finite_bits == 0, OR keeps acc
                // unchanged but we still want to mark it. Instead, AND
                // with a "is finite" mask derived branchlessly.
                let is_finite_mask = ((elem_finite_bits | elem_finite_bits.wrapping_neg()) >> 63).wrapping_neg();
                // is_finite_mask == u64::MAX when elem_finite_bits != 0
                // (finite), 0 when == 0 (non-finite).
                all_finite_mask &= is_finite_mask;
            }
            if all_finite_mask == u64::MAX {
                Ok(())
            } else {
                Err(Error::new(ErrorKind::NonFiniteFloat, Position::START))
            }
        }
    }

    /// `f32` widens losslessly to `f64` for serialization. The decoded
    /// form on the parse side narrows via `as f32`, mirroring this.
    impl ToJson for f32 {
        const MIN_SERIALIZED_LEN: usize = 1;
        const MAX_SERIALIZED_LEN: usize = 32;
        const NEEDS_VALIDATION: bool = true;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_float_f64(f64::from(*self))
        }
        /// SAFETY: see `f64::write_json_in_reserved`.
        #[inline]
        #[allow(unsafe_code)]
        unsafe fn write_json_in_reserved<W: JsonWrite + ?Sized>(
            &self,
            w: &mut W,
        ) -> Result<(), W::Error> {
            // SAFETY: forwarded to the sink's finite-known unchecked path.
            unsafe { w.write_float_f64_unchecked_finite(f64::from(*self)) }
        }
        #[inline]
        fn pre_validate_slice(slice: &[Self]) -> Result<(), Error> {
            const EXP_MASK: u32 = 0x7f80_0000;
            let mut all_finite_mask = u32::MAX;
            for &f in slice {
                let bits = f.to_bits();
                let elem_finite_bits = (bits & EXP_MASK) ^ EXP_MASK;
                let is_finite_mask = ((elem_finite_bits | elem_finite_bits.wrapping_neg()) >> 31).wrapping_neg();
                all_finite_mask &= is_finite_mask;
            }
            if all_finite_mask == u32::MAX {
                Ok(())
            } else {
                Err(Error::new(ErrorKind::NonFiniteFloat, Position::START))
            }
        }
    }

    /// JSON has no `char` primitive — encode as a one-character string,
    /// matching the `FromJson` direction.
    impl ToJson for char {
        const MIN_SERIALIZED_LEN: usize = 3; // "\"x\""
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            let mut buf = [0u8; 4];
            let s: &str = self.encode_utf8(&mut buf);
            w.write_escaped_str(s)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Box<T> {
        const MIN_SERIALIZED_LEN: usize = T::MIN_SERIALIZED_LEN;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Rc<T> {
        const MIN_SERIALIZED_LEN: usize = T::MIN_SERIALIZED_LEN;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Arc<T> {
        const MIN_SERIALIZED_LEN: usize = T::MIN_SERIALIZED_LEN;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson> ToJson for alloc::vec::Vec<T> {
        const MIN_SERIALIZED_LEN: usize = 2; // "[]"
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            // Forward to the slice impl in the parent module.
            self.as_slice().write_json(w)
        }
    }

    // -----------------------------------------------------------------
    // Map and set collections.
    //
    // Mirror image of `de.rs`: keys are written via `MapKeyOut`, which
    // exposes the canonical `&str` form regardless of whether the
    // underlying type is `String`, `&str`, or `Cow<'_, str>`. Values
    // serialize via their own `ToJson` impl. Sets serialize as arrays
    // in iteration order — for `HashSet` that's hash-bucket order,
    // matching the convention serde_json uses.
    // -----------------------------------------------------------------

    /// Sealed adapter from a map's key type to the borrowed `&str` form.
    ///
    /// JSON object keys are always strings, so any map serialized via
    /// `ToJson` needs its key type to expose a `&str` view. Implemented
    /// for `String`, `&str`, and `Cow<'_, str>` — the same set the
    /// parse side supports via `MapKey`.
    pub trait MapKeyOut {
        fn as_str(&self) -> &str;
    }

    impl MapKeyOut for String {
        #[inline]
        fn as_str(&self) -> &str {
            self
        }
    }

    impl MapKeyOut for &str {
        #[inline]
        fn as_str(&self) -> &str {
            self
        }
    }

    impl MapKeyOut for Cow<'_, str> {
        #[inline]
        fn as_str(&self) -> &str {
            self.as_ref()
        }
    }

    /// Shared object-writing helper. Mirrors `write_array` for arrays.
    /// Pulled out so `BTreeMap` and `HashMap` (and any future map type)
    /// cannot drift on punctuation or empty-object handling.
    fn write_object<'a, K, V, I, W>(iter: I, w: &mut W) -> Result<(), W::Error>
    where
        K: MapKeyOut + 'a,
        V: ToJson + 'a,
        I: IntoIterator<Item = (&'a K, &'a V)>,
        W: JsonWrite + ?Sized,
    {
        w.write_byte(b'{')?;
        let mut first = true;
        for (k, v) in iter {
            if !first {
                w.write_byte(b',')?;
            }
            w.write_escaped_str(k.as_str())?;
            w.write_byte(b':')?;
            v.write_json(w)?;
            first = false;
        }
        w.write_byte(b'}')
    }

    impl<K: MapKeyOut, V: ToJson> ToJson for alloc::collections::BTreeMap<K, V> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_object(self.iter(), w)
        }
    }

    impl<T: ToJson> ToJson for alloc::collections::BTreeSet<T> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            super::write_array(self.iter(), w)
        }
    }

    #[cfg(feature = "std")]
    impl<K, V, S> ToJson for std::collections::HashMap<K, V, S>
    where
        K: MapKeyOut + ::core::hash::Hash + Eq,
        V: ToJson,
        S: ::core::hash::BuildHasher,
    {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_object(self.iter(), w)
        }
    }

    #[cfg(feature = "std")]
    impl<T, S> ToJson for std::collections::HashSet<T, S>
    where
        T: ToJson + ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher,
    {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            super::write_array(self.iter(), w)
        }
    }

    /// Encode `SystemTime` as fractional seconds since `UNIX_EPOCH`.
    /// Times before the epoch serialize as negative numbers; the
    /// parse side accepts the same shape.
    #[cfg(feature = "std")]
    impl ToJson for std::time::SystemTime {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            // `duration_since(UNIX_EPOCH)` returns Err with the negated
            // duration when self < UNIX_EPOCH. Encode the sign back.
            let secs = match self.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_secs_f64(),
                Err(e) => -e.duration().as_secs_f64(),
            };
            w.write_float_f64(secs)
        }
    }

    /// Encode `Duration` as fractional seconds, mirroring the parse-side
    /// `from_secs_f64` adapter. Negative durations are unrepresentable
    /// (`Duration` is unsigned), and the float impl already rejects
    /// non-finite output, so this never errors for valid inputs.
    #[cfg(feature = "std")]
    impl ToJson for std::time::Duration {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_float_f64(self.as_secs_f64())
        }
    }

    // -----------------------------------------------------------------
    // std::net and std::path adapters.
    //
    // Each writes its canonical Display form as a quoted JSON string,
    // matching the FromJson `parse_from_str` adapter on the parse side.
    // -----------------------------------------------------------------

    /// Display the value into a small scratch buffer, then write it as
    /// a JSON string. The `fmt::Write` trait fills our scratch `String`;
    /// from there we reuse `write_escaped_str` even though these types'
    /// canonical text never contains characters that need escaping —
    /// the cost is one SIMD scan that exits immediately, and using the
    /// escape path keeps a single string-writing entry point.
    #[cfg(feature = "std")]
    fn write_display<T: ::core::fmt::Display, W: JsonWrite + ?Sized>(
        v: &T,
        w: &mut W,
    ) -> Result<(), W::Error> {
        use ::core::fmt::Write as _;
        let mut buf = String::new();
        // fmt::Write into a String is infallible.
        let _ = write!(&mut buf, "{v}");
        w.write_escaped_str(&buf)
    }

    #[cfg(feature = "std")]
    impl ToJson for std::net::IpAddr {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_display(self, w)
        }
    }

    #[cfg(feature = "std")]
    impl ToJson for std::net::Ipv4Addr {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_display(self, w)
        }
    }

    #[cfg(feature = "std")]
    impl ToJson for std::net::Ipv6Addr {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_display(self, w)
        }
    }

    #[cfg(feature = "std")]
    impl ToJson for std::net::SocketAddr {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_display(self, w)
        }
    }

    /// `PathBuf` round-trips only for paths whose bytes are valid UTF-8.
    /// `Path::display()` lossily replaces invalid bytes — we want a
    /// clean error in that case, but JSON has no lossless path encoding
    /// anyway, so the convention matches the parse side: assume UTF-8.
    /// `to_string_lossy` here is the symmetric move.
    #[cfg(feature = "std")]
    impl ToJson for std::path::PathBuf {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(&self.to_string_lossy())
        }
    }

    #[cfg(feature = "std")]
    impl ToJson for std::path::Path {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(&self.to_string_lossy())
        }
    }

    // -----------------------------------------------------------------
    // IndexMap / IndexSet (optional `indexmap` feature).
    //
    // Insertion-order iteration is the differentiator from
    // HashMap/BTreeMap; the wire shape is the same.
    // -----------------------------------------------------------------

    #[cfg(feature = "indexmap")]
    impl<K, V, S> ToJson for indexmap::IndexMap<K, V, S>
    where
        K: super::MapKeyOut + ::core::hash::Hash + Eq,
        V: ToJson,
        S: ::core::hash::BuildHasher,
    {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            write_object(self.iter(), w)
        }
    }

    #[cfg(feature = "indexmap")]
    impl<T, S> ToJson for indexmap::IndexSet<T, S>
    where
        T: ToJson + ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher,
    {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            super::write_array(self.iter(), w)
        }
    }
}

#[cfg(feature = "alloc")]
pub use alloc_impls::MapKeyOut;

// Keep `Position` / `ErrorKind` imports live so PR 2's float path can lean
// on them without re-importing. The float impls return
// `Error::new(ErrorKind::NonFiniteFloat, Position::START)` since serializer
// errors don't have an input-byte position to point at.
#[allow(dead_code)]
const _: fn() -> Error = || Error::new(ErrorKind::NonFiniteFloat, Position::START);
