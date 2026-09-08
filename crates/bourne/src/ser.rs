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

use crate::escape::{write_escaped, write_escaped_body};
use crate::{Error, ErrorKind, Position};

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
    /// widens to [`crate::Error`] so non-finite floats can surface.
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

    /// Append a single ASCII byte, skipping the per-write capacity check
    /// when the sink's spare tail can hold it.
    ///
    /// Paired with [`Self::reserve_hint`]: after a hint, sinks with a
    /// growable tail take the raw-write path and the capacity branch
    /// leaves the hot loop (the per-element `Vec::push` check was the
    /// single largest mispredict source on float-array workloads). When
    /// the tail is exhausted the sink falls back to the checked
    /// [`Self::write_byte`], so a hint that under-reserves costs a
    /// reallocation — never memory safety. Sinks without a tail-capacity
    /// concept (e.g. `fmt::Write`-backed) just forward to `write_byte`.
    #[inline]
    fn write_byte_hinted(&mut self, b: u8) -> Result<(), Self::Error> {
        self.write_byte(b)
    }

    /// Append a `&str` verbatim. Caller is responsible for any escaping
    /// — this is the structural / pre-escaped path. For user string
    /// payloads, call [`Self::write_escaped_str`] instead.
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error>;

    /// Append a JSON-quoted, escaped string (including the surrounding
    /// `"` characters).
    ///
    /// Default: open-quote, shared escape walk, close-quote, through
    /// [`Self::write_byte`]/[`Self::write_str_raw`]. One implementation
    /// ([`crate::escape`]) serves every sink; char-oriented `write_byte`
    /// sinks cannot mangle non-ASCII because multi-byte runs only ever
    /// cross [`Self::write_str_raw`] (audit 3.9). Sinks with a faster
    /// native path override this.
    #[inline]
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        write_escaped(self, s)
    }

    /// Append a byte slice whose contents are known-valid UTF-8.
    ///
    /// Used by macro-generated code to fuse adjacent structural literals
    /// (e.g. `b"{\"id\":"`) into a single write. The default routes
    /// through [`Self::write_str_raw`]; byte-oriented sinks like
    /// [`ByteSink`] override to avoid the `&str` conversion.
    ///
    /// Non-UTF-8 input returns [`ErrorKind::InvalidUtf8`] rather than
    /// panicking — the contents may come from a user-written `ToJson`
    /// impl, which the compiler cannot verify.
    #[inline]
    fn write_raw_bytes(&mut self, b: &[u8]) -> Result<(), Self::Error> {
        match core::str::from_utf8(b) {
            Ok(s) => self.write_str_raw(s),
            Err(_) => Err(self.on_invalid_utf8()),
        }
    }

    /// Error produced when [`Self::write_raw_bytes`] receives non-UTF-8
    /// input — the contents may come from a user-written `ToJson` impl,
    /// which the compiler cannot verify. Required because each sink owns
    /// its error type; map to your [`Self::Error`]'s closest equivalent.
    fn on_invalid_utf8(&mut self) -> Self::Error;

    /// Open a JSON object (`{`).
    ///
    /// Writers must use the structural methods instead of pushing `{` /
    /// `}` / `[` / `]` / `,` / `:` through [`Self::write_byte`] or fusing
    /// them into raw literals: pretty sinks recognize structure only
    /// through these hooks, and raw bytes reach them as opaque text (a
    /// derived struct serialized compactly even under
    /// `to_string_pretty`, audit 3.8).
    #[inline]
    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.write_byte(b'{')
    }

    /// Whether byte runs fed to [`Self::write_raw_bytes`] render
    /// verbatim on this sink, so a writer may fuse structural
    /// punctuation into one literal (e.g. `,"id":`).
    ///
    /// Sinks that treat raw text as opaque content — the pretty sink
    /// needs to see the comma, key, and colon as separate structural
    /// events — set this to `false`, and generated writers take the
    /// structural-method path instead. The flag is an associated
    /// constant, so the branch folds away per instantiation and neither
    /// path costs anything at runtime.
    const FUSES_STRUCTURAL_BYTES: bool = true;

    /// Close a JSON object (`}`).
    #[inline]
    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.write_byte(b'}')
    }

    /// Open a JSON array (`[`).
    #[inline]
    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.write_byte(b'[')
    }

    /// Close a JSON array (`]`).
    #[inline]
    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.write_byte(b']')
    }

    /// Write the comma between two elements of an object or array.
    #[inline]
    fn separator(&mut self) -> Result<(), Self::Error> {
        self.write_byte(b',')
    }

    /// Write an object key and its colon: `"key":`. The key is escaped
    /// by the sink, like [`Self::write_escaped_str`].
    #[inline]
    fn object_key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.write_escaped_str(key)?;
        self.write_byte(b':')
    }

    /// Open a JSON array on the hinted path — the structural twin of
    /// [`Self::write_byte_hinted`]. Only the array writer after a
    /// `reserve_hint` uses this; the brace-pair defaults mirror
    /// [`Self::begin_array`] / [`Self::end_array`].
    #[inline]
    fn begin_array_hinted(&mut self) -> Result<(), Self::Error> {
        self.write_byte_hinted(b'[')
    }

    /// Comma between elements on the hinted path.
    #[inline]
    fn separator_hinted(&mut self) -> Result<(), Self::Error> {
        self.write_byte_hinted(b',')
    }

    /// Close a JSON array on the hinted path.
    #[inline]
    fn end_array_hinted(&mut self) -> Result<(), Self::Error> {
        self.write_byte_hinted(b']')
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

    /// Write a finite `f64` for which the caller has reserved
    /// `f64::MAX_SERIALIZED_LEN` (32) bytes of sink capacity.
    ///
    /// Returns `Ok(true)` when the digits were written, `Ok(false)` when
    /// `f` is non-finite — in which case nothing is emitted and the
    /// caller turns the `false` into a typed error. Sinks that cannot
    /// use a reservation forward to [`Self::write_float_f64`].
    #[cfg(feature = "alloc")]
    #[inline]
    fn write_float_f64_hinted(&mut self, f: f64) -> Result<bool, Self::Error> {
        if f.is_finite() {
            self.write_float_f64(f)?;
            return Ok(true);
        }
        Ok(false)
    }
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

    #[inline]
    fn on_invalid_utf8(&mut self) -> Self::Error {
        Error::new(ErrorKind::InvalidUtf8, Position::START)
    }

    /// Single shared escape walk ([`crate::escape`]); this sink keeps
    /// the default quotes-via-`write_byte` shape.
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        write_escaped(self, s)
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

    /// Raw byte append when the reserved tail can hold it, `Vec::push`
    /// (grow) when it cannot. After a `reserve_hint` the hint-sized
    /// region writes without touching the capacity branch; a hint that
    /// under-reserves only costs the normal amortized growth, never
    /// memory safety.
    #[inline]
    fn write_byte_hinted(&mut self, b: u8) -> Result<(), Self::Error> {
        if self.out.len() < self.out.capacity() {
            // SAFETY: `len < capacity` checked above; write in place and
            // bump the length — `Vec::push` without the branch.
            unsafe {
                let len = self.out.len();
                core::ptr::write(self.out.as_mut_ptr().add(len), b);
                self.out.set_len(len + 1);
            }
        } else {
            self.out.push(b);
        }
        Ok(())
    }

    #[inline]
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }

    #[inline]
    fn on_invalid_utf8(&mut self) -> Self::Error {
        Error::new(ErrorKind::InvalidUtf8, Position::START)
    }

    #[inline]
    fn write_raw_bytes(&mut self, b: &[u8]) -> Result<(), Self::Error> {
        self.out.extend_from_slice(b);
        Ok(())
    }

    /// Single shared escape walk ([`crate::escape`]); byte-slice copies
    /// instead of `&str` through the custom `write_str_raw`.
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        write_escaped(self, s)
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

    /// Reserved-tail float write. Raw-pointer path when ≥ 32 bytes of
    /// tail remain (the formatter's worst case), buffer copy otherwise.
    /// Returns `false` for non-finite input without emitting anything,
    /// which the array writer converts to `NonFiniteFloat`.
    #[inline]
    fn write_float_f64_hinted(&mut self, f: f64) -> Result<bool, Self::Error> {
        if f.to_bits() & crate::float::EXP_MASK != crate::float::EXP_MASK
            && self.out.capacity() - self.out.len() >= crate::float::FORMAT_BUF_LEN
        {
            // SAFETY: tail checked above; the formatter writes at most
            // `FORMAT_BUF_LEN` (32) ASCII bytes there and returns the
            // count used for `set_len`.
            unsafe {
                let len = self.out.len();
                let dst = self.out.as_mut_ptr().add(len);
                let written = crate::float::format_finite_to_ptr(f, dst);
                self.out.set_len(len + written);
            }
            return Ok(true);
        }
        self.write_float_f64(f)?;
        Ok(f.is_finite())
    }
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
        (18, 999_999_999_999_999_999),   // lzcnt  4: 18 or 19
        (18, 576_460_752_303_423_487),   // lzcnt  5: always 18
        (18, 288_230_376_151_711_743),   // lzcnt  6: always 18
        (17, 99_999_999_999_999_999),    // lzcnt  7: 17 or 18
        (17, 72_057_594_037_927_935),    // lzcnt  8: always 17
        (17, 36_028_797_018_963_967),    // lzcnt  9: always 17
        (16, 9_999_999_999_999_999),     // lzcnt 10: 16 or 17
        (16, 9_007_199_254_740_991),     // lzcnt 11: always 16
        (16, 4_503_599_627_370_495),     // lzcnt 12: always 16
        (16, 2_251_799_813_685_247),     // lzcnt 13: always 16
        (15, 999_999_999_999_999),       // lzcnt 14: 15 or 16
        (15, 562_949_953_421_311),       // lzcnt 15: always 15
        (15, 281_474_976_710_655),       // lzcnt 16: always 15
        (14, 99_999_999_999_999),        // lzcnt 17: 14 or 15
        (14, 70_368_744_177_663),        // lzcnt 18: always 14
        (14, 35_184_372_088_831),        // lzcnt 19: always 14
        (13, 9_999_999_999_999),         // lzcnt 20: 13 or 14
        (13, 8_796_093_022_207),         // lzcnt 21: always 13
        (13, 4_398_046_511_103),         // lzcnt 22: always 13
        (13, 2_199_023_255_551),         // lzcnt 23: always 13
        (12, 999_999_999_999),           // lzcnt 24: 12 or 13
        (12, 549_755_813_887),           // lzcnt 25: always 12
        (12, 274_877_906_943),           // lzcnt 26: always 12
        (11, 99_999_999_999),            // lzcnt 27: 11 or 12
        (11, 68_719_476_735),            // lzcnt 28: always 11
        (11, 34_359_738_367),            // lzcnt 29: always 11
        (10, 9_999_999_999),             // lzcnt 30: 10 or 11
        (10, 8_589_934_591),             // lzcnt 31: always 10
        (10, 4_294_967_295),             // lzcnt 32: always 10
        (10, 2_147_483_647),             // lzcnt 33: always 10
        (9, 999_999_999),                // lzcnt 34: 9 or 10
        (9, 536_870_911),                // lzcnt 35: always 9
        (9, 268_435_455),                // lzcnt 36: always 9
        (8, 99_999_999),                 // lzcnt 37: 8 or 9
        (8, 67_108_863),                 // lzcnt 38: always 8
        (8, 33_554_431),                 // lzcnt 39: always 8
        (7, 9_999_999),                  // lzcnt 40: 7 or 8
        (7, 8_388_607),                  // lzcnt 41: always 7
        (7, 4_194_303),                  // lzcnt 42: always 7
        (7, 2_097_151),                  // lzcnt 43: always 7
        (6, 999_999),                    // lzcnt 44: 6 or 7
        (6, 524_287),                    // lzcnt 45: always 6
        (6, 262_143),                    // lzcnt 46: always 6
        (5, 99_999),                     // lzcnt 47: 5 or 6
        (5, 65_535),                     // lzcnt 48: always 5
        (5, 32_767),                     // lzcnt 49: always 5
        (4, 9_999),                      // lzcnt 50: 4 or 5
        (4, 8_191),                      // lzcnt 51: always 4
        (4, 4_095),                      // lzcnt 52: always 4
        (4, 2_047),                      // lzcnt 53: always 4
        (3, 999),                        // lzcnt 54: 3 or 4
        (3, 511),                        // lzcnt 55: always 3
        (3, 255),                        // lzcnt 56: always 3
        (2, 99),                         // lzcnt 57: 2 or 3
        (2, 63),                         // lzcnt 58: always 2
        (2, 31),                         // lzcnt 59: always 2
        (1, 9),                          // lzcnt 60: 1 or 2
        (1, 7),                          // lzcnt 61: always 1
        (1, 3),                          // lzcnt 62: always 1
        (1, 1),                          // lzcnt 63: always 1
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
//
// The `expect`s below are unreachable: every byte written is from
// `DIGIT_LUT` (ASCII '0'..='9') or the literal `b'-'`. They are
// monomorphization-time guards against a formatter-internal bug.
#[allow(clippy::cast_possible_truncation)]
fn format_u64(n: u64, buf: &mut [u8; 20]) -> &str {
    let digits = fast_digit_count(n);
    write_digits_backward(n, buf.as_mut_ptr(), digits);
    core::str::from_utf8(&buf[..digits]).expect("integer formatter emits ASCII")
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
    core::str::from_utf8(&buf[..total]).expect("integer formatter emits ASCII")
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
    core::str::from_utf8(&buf[pos..]).expect("integer formatter emits ASCII")
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
    core::str::from_utf8(&buf[..total]).expect("integer formatter emits ASCII")
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

    /// Used by the slice writer's reservation math. Primitives with a known
    /// maximum output length (floats, ints, bools) override this. Variable-length
    /// types (`str`, `Vec<T>`, structs) keep
    /// the default, since their per-element size depends on payload.
    /// A type that lies about its bound cannot break memory safety: the
    /// hinted write paths fall back to checked writes when the reserved
    /// tail is exhausted, so a wrong value only costs a reallocation.
    const MAX_SERIALIZED_LEN: usize = 0;

    /// Serialize `self` into `w`.
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error>;
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
// `expect` below is unreachable from user input: every `JsonWrite` path
// emits valid UTF-8 by construction (see `ByteSink`'s impl). The check
// guards against a serializer-internal bug, not a user-triggerable case.
#[cfg(feature = "alloc")]
#[allow(clippy::missing_panics_doc)]
pub fn to_string<T: ToJson + ?Sized>(value: &T) -> Result<String, Error> {
    let bytes = to_vec(value)?;
    Ok(String::from_utf8(bytes).expect("json-bourne emits only valid UTF-8"))
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
    fn on_invalid_utf8(&mut self) -> Self::Error {
        Error::new(ErrorKind::InvalidUtf8, Position::START)
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
    fn on_invalid_utf8(&mut self) -> Self::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            ErrorKind::InvalidUtf8.to_string(),
        )
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
            Err(_) => Err(adapter
                .err
                .unwrap_or_else(|| std::io::Error::other("fmt error in float formatter"))),
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

    /// Close the container opened by `begin_object` / `begin_array`.
    /// An immediately-closed empty container stays compact (`[]` /
    /// `{}`); otherwise the close lands on its own indented line.
    /// Paired with the begin hooks, so `depth` can only ever track
    /// actually-opened containers — it cannot underflow the way a
    /// byte-sniffing close could.
    fn end_container(&mut self, close: u8) {
        if let Some(open) = self.pending_open.take() {
            self.out.push(open as char);
            self.out.push(close as char);
            return;
        }
        self.depth -= 1;
        self.newline_and_indent();
        self.out.push(close as char);
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

    // Raw text reaches this sink as opaque content; writers must emit
    // structure through the begin/end/separator/object_key hooks.
    const FUSES_STRUCTURAL_BYTES: bool = false;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        // A nested open resolves the outer container as non-empty
        // before deferring itself — the pending slot holds one byte.
        self.flush_pending_open();
        self.pending_open = Some(b'{');
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.end_container(b'}');
        Ok(())
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.flush_pending_open();
        self.pending_open = Some(b'[');
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.end_container(b']');
        Ok(())
    }

    fn begin_array_hinted(&mut self) -> Result<(), Self::Error> {
        self.begin_array()
    }

    fn separator_hinted(&mut self) -> Result<(), Self::Error> {
        self.separator()
    }

    fn end_array_hinted(&mut self) -> Result<(), Self::Error> {
        self.end_array()
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        // Only reachable after a value has been written, which already
        // flushed the pending open.
        self.out.push(',');
        self.newline_and_indent();
        Ok(())
    }

    fn object_key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.flush_pending_open();
        self.write_byte(b'"')?;
        // Quote is already open, so only the body goes through the
        // shared walk — and through this sink, whose writes flush
        // `pending_open` for nested containers.
        write_escaped_body(self, key)?;
        self.write_byte(b'"')?;
        self.out.push_str(": ");
        Ok(())
    }

    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error> {
        self.flush_pending_open();
        self.out.push(b as char);
        Ok(())
    }

    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error> {
        if s.is_empty() {
            return Ok(());
        }
        self.flush_pending_open();
        self.out.push_str(s);
        Ok(())
    }

    #[inline]
    fn on_invalid_utf8(&mut self) -> Self::Error {
        Error::new(ErrorKind::InvalidUtf8, Position::START)
    }

    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        // Drive the shared walk through this sink so its own
        // `write_byte`/`write_str_raw` flush `pending_open` on the
        // first write — writing `self.out` directly would emit inside
        // the still-open container line.
        write_escaped(self, s)
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

// `isize` / `usize` widen to 64-bit on the platforms json-bourne supports.
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

/// Upper bound on a single `reserve_hint` from the slice writer. Bounds
/// the sink's peak overshoot when serializing very large arrays of
/// small elements (audit 4.5.2).
const RESERVE_WINDOW: usize = 256 * 1024;

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
    w.begin_array()?;
    // Peel the first element so the inner loop never re-evaluates a
    // `first` flag — every subsequent element unconditionally writes
    // `,` then itself. Saves one branch per element on hot Vec/slice
    // paths where the iterator is `ExactSizeIterator` and the compiler
    // can hoist the check out of the loop.
    let mut iter = iter.into_iter();
    if let Some(v) = iter.next() {
        v.write_json(w)?;
        for v in iter {
            w.separator()?;
            v.write_json(w)?;
        }
    }
    w.end_array()
}

/// Like `write_array`, but callable only after `reserve_hint` has been
/// invoked with at least `2 + len + len * MAX_SERIALIZED_LEN` bytes.
/// Delimiters go through [`JsonWrite::write_byte_hinted`], which uses a
/// raw tail write after a hint and falls back to the checked push — so
/// the reservation is a performance contract between the writer and the
/// sink, never a safety one.
#[allow(clippy::doc_markdown)]
fn write_array_hinted<T: ToJson, W: JsonWrite + ?Sized>(
    slice: &[T],
    w: &mut W,
) -> Result<(), W::Error> {
    w.begin_array_hinted()?;
    if let Some((first, rest)) = slice.split_first() {
        first.write_json(w)?;
        for v in rest {
            w.separator_hinted()?;
            v.write_json(w)?;
        }
    }
    w.end_array_hinted()
}

impl<T: ToJson> ToJson for [T] {
    const MIN_SERIALIZED_LEN: usize = 2; // "[]"
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        // Pre-reserve when the element type has a known upper bound. The
        // hint covers brackets (`[`, `]`), commas (1 per element), and
        // the per-element max payload, so the delimited hot loop stays
        // on the sink's raw-tail write path. Under-reserving is safe —
        // the sink falls back to checked writes — just slower.
        //
        // The hint is bounded to RESERVE_WINDOW so a huge array reserves
        // a bounded overshoot instead of `len * (MAX + 1)` up front: a
        // 10M-element `Vec<i64>` of small numbers asked for ~210 MB
        // against ~30 MB of output (audit 4.5.2). Each window ends on an
        // element boundary where a fresh hint re-arms the sink's raw
        // tail, so the fast path is preserved; only the per-window
        // boundary elements fall back to the checked path.
        if T::MAX_SERIALIZED_LEN != 0 {
            let full = self
                .len()
                .saturating_mul(T::MAX_SERIALIZED_LEN.saturating_add(1))
                .saturating_add(2);
            if full <= RESERVE_WINDOW {
                w.reserve_hint(full);
                return write_array_hinted(self, w);
            }
            // Elements per ~RESERVE_WINDOW of output; the final partial
            // window rides on the sink's normal growth.
            let per_window = (RESERVE_WINDOW / (T::MAX_SERIALIZED_LEN + 1)).max(1);
            let chunk = per_window.min(self.len());
            w.reserve_hint(chunk * (T::MAX_SERIALIZED_LEN + 1) + 2);
            let mut rest = self;
            w.begin_array_hinted()?;
            let mut first = true;
            while rest.len() > chunk {
                let (head, tail) = rest.split_at(chunk);
                for v in head {
                    if !first {
                        w.separator_hinted()?;
                    }
                    first = false;
                    v.write_json(w)?;
                }
                rest = tail;
                w.reserve_hint(chunk * (T::MAX_SERIALIZED_LEN + 1));
            }
            for v in rest {
                if !first {
                    w.separator_hinted()?;
                }
                first = false;
                v.write_json(w)?;
            }
            return w.end_array_hinted();
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
                w.begin_array()?;
                self.$first_idx.write_json(w)?;
                $(
                    w.separator()?;
                    self.$idx.write_json(w)?;
                )*
                w.end_array()
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
    use super::{JsonWrite, ToJson};
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
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            // The hinted path keeps `f` in xmm and writes straight into
            // the sink's reserved tail when 32 bytes are available; on
            // sinks without a tail it degrades to `write_float_f64`.
            if w.write_float_f64_hinted(*self)? {
                return Ok(());
            }
            // Non-finite: the hinted write emitted nothing. Hand the
            // sink the actual value — its documented contract is to
            // reject non-finite input through `Self::Error` — rather
            // than a fabricated `NaN` a tolerant sink would happily
            // serialize (audit 3.14).
            w.write_float_f64(*self)
        }
    }

    /// `f32` widens losslessly to `f64` for serialization. The decoded
    /// form on the parse side narrows via `as f32`, mirroring this.
    impl ToJson for f32 {
        const MIN_SERIALIZED_LEN: usize = 1;
        const MAX_SERIALIZED_LEN: usize = 32;
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            #[allow(clippy::cast_precision_loss)]
            let widened = f64::from(*self);
            if w.write_float_f64_hinted(widened)? {
                return Ok(());
            }
            // See the `f64` impl: the real value, not a placeholder.
            w.write_float_f64(widened)
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
        w.begin_object()?;
        let mut first = true;
        for (k, v) in iter {
            if !first {
                w.separator()?;
            }
            w.object_key(k.as_str())?;
            v.write_json(w)?;
            first = false;
        }
        w.end_object()
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
