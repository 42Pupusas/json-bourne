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

    /// Append a single ASCII byte. Used for structural punctuation
    /// (`{`, `}`, `[`, `]`, `,`, `:`, `"`).
    fn write_byte(&mut self, b: u8) -> Result<(), Self::Error>;

    /// Append a `&str` verbatim. Caller is responsible for any escaping
    /// — this is the structural / pre-escaped path. For user string
    /// payloads, call [`Self::write_escaped_str`] instead.
    fn write_str_raw(&mut self, s: &str) -> Result<(), Self::Error>;

    /// Append a JSON-quoted, escaped string (including the surrounding
    /// `"` characters). The default impl escapes one byte at a time
    /// through `write_byte`; sinks with bulk-write capability (like
    /// [`StringSink`]) override with a literal-run fast path.
    fn write_escaped_str(&mut self, s: &str) -> Result<(), Self::Error> {
        self.write_byte(b'"')?;
        for &b in s.as_bytes() {
            write_escape_byte(self, b)?;
        }
        self.write_byte(b'"')
    }

    /// Write a signed 64-bit integer as a JSON number.
    fn write_int_i64(&mut self, n: i64) -> Result<(), Self::Error> {
        let mut buf = [0u8; 20];
        let s = format_i64(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write an unsigned 64-bit integer as a JSON number.
    fn write_int_u64(&mut self, n: u64) -> Result<(), Self::Error> {
        let mut buf = [0u8; 20];
        let s = format_u64(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write a signed 128-bit integer as a JSON number.
    fn write_int_i128(&mut self, n: i128) -> Result<(), Self::Error> {
        let mut buf = [0u8; 40];
        let s = format_i128(n, &mut buf);
        self.write_str_raw(s)
    }

    /// Write an unsigned 128-bit integer as a JSON number.
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
// Integer formatters (jeaiii-style two-digit LUT).
// ---------------------------------------------------------------------------
//
// `format!("{n}")` routes through `core::fmt::Formatter`, which on profile
// is the dominant cost when a payload is integer-heavy. Hand-rolled
// two-digit-at-a-time formatting using a 200-byte LUT runs ~4× faster on
// a `Vec<i64>` workload and produces byte-for-byte identical output.
//
// The LUT is the digit pairs `"00".."99"`; we index it with the bottom
// two decimal digits of the running value and write them out backward
// into a stack buffer. Then we reverse-slice the buffer.

const DIGIT_LUT: &[u8; 200] = b"\
0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

/// Format a `u64` into `buf` (big-endian text). Returns the slice of
/// `buf` that holds the digits.
//
// Truncating casts are bounded by the loop guards: `n % 100 < 100` fits
// `usize` on every supported target (≥16-bit), and the `n < 10` arm fits
// `u8`. Allowing crate-locally is more honest than per-line allows.
#[allow(clippy::cast_possible_truncation)]
fn format_u64(mut n: u64, buf: &mut [u8; 20]) -> &str {
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
    // SAFETY: every byte written is from the digit LUT (ASCII '0'..'9'),
    // which is valid UTF-8.
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[pos..])
    }
}

fn format_i64(n: i64, buf: &mut [u8; 20]) -> &str {
    if n >= 0 {
        // n is non-negative so `as u64` is the unsigned-equivalent value.
        #[allow(clippy::cast_sign_loss)]
        return format_u64(n as u64, buf);
    }
    // Negate via unsigned magnitude so `i64::MIN` round-trips. Widening
    // through i128 keeps `-i64::MIN` representable; `unsigned_abs()` then
    // collapses to a u128 which fits u64 because the magnitude of any
    // i64 value is ≤ 2^63.
    #[allow(clippy::cast_possible_truncation)]
    let mag = i128::from(n).unsigned_abs() as u64;
    let mut tmp = [0u8; 20];
    let s = format_u64(mag, &mut tmp);
    let len = s.len();
    let pos = buf.len() - len - 1;
    buf[pos] = b'-';
    buf[pos + 1..pos + 1 + len].copy_from_slice(s.as_bytes());
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[pos..])
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
    // Same `i128::MIN` round-trip trick as the i64 path.
    let mag = n.unsigned_abs();
    let mut tmp = [0u8; 40];
    let s = format_u128(mag, &mut tmp);
    let len = s.len();
    let pos = buf.len() - len - 1;
    buf[pos] = b'-';
    buf[pos + 1..pos + 1 + len].copy_from_slice(s.as_bytes());
    #[allow(unsafe_code)]
    unsafe {
        core::str::from_utf8_unchecked(&buf[pos..])
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
    /// Serialize `self` into `w`.
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error>;
}

/// Serialize `value` into a fresh `String`.
///
/// Returns [`Error`] (rather than `Infallible`) so non-finite floats and
/// other typed-level failures have a place to surface. The underlying
/// [`StringSink`] is itself infallible.
#[cfg(feature = "alloc")]
pub fn to_string<T: ToJson + ?Sized>(value: &T) -> Result<String, Error> {
    // 128-byte initial capacity matches `serde_json::to_string`. Most
    // realistic JSON shapes (small structs, log lines, metric records)
    // fit in 128–512 bytes, so a one-shot allocation here saves the
    // 5-7 grow-and-copy reallocations a fresh `String::new()` would
    // pay for the same payload. For larger output the cost is one
    // unnecessary 128-byte alloc up front, amortized to nothing once
    // the first realloc kicks in.
    let mut out = String::with_capacity(128);
    let mut sink = StringSink::new(&mut out);
    value.write_json(&mut sink)?;
    Ok(out)
}

/// Serialize `value` into a fresh `Vec<u8>`.
#[cfg(feature = "alloc")]
pub fn to_vec<T: ToJson + ?Sized>(value: &T) -> Result<alloc::vec::Vec<u8>, Error> {
    to_string(value).map(String::into_bytes)
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

impl ToJson for bool {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_str_raw(if *self { "true" } else { "false" })
    }
}

impl ToJson for () {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_str_raw("null")
    }
}

impl ToJson for str {
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
#[allow(clippy::cast_possible_wrap, clippy::cast_lossless)]
impl ToJson for isize {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_i64(*self as i64)
    }
}

#[allow(clippy::cast_lossless)]
impl ToJson for usize {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_u64(*self as u64)
    }
}

impl ToJson for i128 {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_i128(*self)
    }
}

impl ToJson for u128 {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_int_u128(*self)
    }
}

// Composite: Option<T> writes `null` for None or the inner value for Some.
// Mirrors the FromJson direction.
impl<T: ToJson> ToJson for Option<T> {
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

impl<T: ToJson> ToJson for [T] {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        write_array(self.iter(), w)
    }
}

impl<T: ToJson, const N: usize> ToJson for [T; N] {
    #[inline]
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        write_array(self.iter(), w)
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
    use super::{JsonWrite, ToJson};
    use alloc::borrow::Cow;
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::string::String;
    use alloc::sync::Arc;

    impl ToJson for String {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(self.as_str())
        }
    }

    impl ToJson for Cow<'_, str> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_escaped_str(self.as_ref())
        }
    }

    impl ToJson for f64 {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_float_f64(*self)
        }
    }

    /// `f32` widens losslessly to `f64` for serialization. The decoded
    /// form on the parse side narrows via `as f32`, mirroring this.
    impl ToJson for f32 {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            w.write_float_f64(f64::from(*self))
        }
    }

    /// JSON has no `char` primitive — encode as a one-character string,
    /// matching the `FromJson` direction.
    impl ToJson for char {
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            let mut buf = [0u8; 4];
            let s: &str = self.encode_utf8(&mut buf);
            w.write_escaped_str(s)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Box<T> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Rc<T> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson + ?Sized> ToJson for Arc<T> {
        #[inline]
        fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
            (**self).write_json(w)
        }
    }

    impl<T: ToJson> ToJson for alloc::vec::Vec<T> {
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
}

#[cfg(feature = "alloc")]
pub use alloc_impls::MapKeyOut;

// Keep `Position` / `ErrorKind` imports live so PR 2's float path can lean
// on them without re-importing. The float impls return
// `Error::new(ErrorKind::NonFiniteFloat, Position::START)` since serializer
// errors don't have an input-byte position to point at.
#[allow(dead_code)]
const _: fn() -> Error = || Error::new(ErrorKind::NonFiniteFloat, Position::START);
