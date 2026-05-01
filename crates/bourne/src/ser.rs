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
    type Error = core::convert::Infallible;

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
    let mut out = String::new();
    let mut sink = StringSink::new(&mut out);
    // `StringSink::Error` is `Infallible`; the only way `write_json`
    // returns `Err` is via a typed-level wrapper that has its own error
    // path — currently the float impls (PR 2). Until those land, this
    // call is structurally infallible, but the signature commits early
    // so users don't have to migrate when it does.
    match value.write_json(&mut sink) {
        Ok(()) => Ok(out),
        Err(infallible) => match infallible {},
    }
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
}

// Keep `Position` / `ErrorKind` imports live so PR 2's float path can lean
// on them without re-importing. The float impls return
// `Error::new(ErrorKind::NonFiniteFloat, Position::START)` since serializer
// errors don't have an input-byte position to point at.
#[allow(dead_code)]
const _: fn() -> Error = || Error::new(ErrorKind::NonFiniteFloat, Position::START);
