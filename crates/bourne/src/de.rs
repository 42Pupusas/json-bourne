//! Type-driven deserialization.
//!
//! The [`FromJson`] trait is the heart of `bourne`. Each type knows how to
//! parse itself from JSON given direct access to the [`Lexer`].
//!
//! Why a lexer instead of an event stream? Typed parsing already enforces
//! the JSON grammar by virtue of which method gets called when — `Vec<T>`
//! knows it's parsing an array, a struct knows it's parsing an object. The
//! streaming `Event` API has to thread a state machine through every value
//! to enforce the same grammar; for typed consumers that dispatch is pure
//! overhead. By driving the lexer directly we skip the state machine.
//!
//! Inside `Vec<T>::from_lex` the loop is roughly:
//!
//! ```ignore
//! lex.array_start()?;          // consume `[`
//! loop {
//!     out.push(T::from_lex(lex)?);
//!     if lex.array_continue(b']')? { break; }
//! }
//! ```
//!
//! Each element costs exactly one `T::from_lex` plus one `array_continue`
//! — no per-element `next_event`, no `match self.state`.

use bourne_core::{Error, ErrorKind, Event, JsonNum, Lexer, Parser, ValueKind};

/// Parse a value of type `T` from a slice of JSON bytes.
pub fn parse<'input, T: FromJson<'input>>(input: &'input [u8]) -> Result<T, Error> {
    let mut p: Parser<'input> = Parser::new(input);
    let lex = p.lexer();
    let value = T::from_lex(lex)?;
    lex.finish()?;
    Ok(value)
}

/// Parse from a `&str`.
pub fn parse_str<'input, T: FromJson<'input>>(input: &'input str) -> Result<T, Error> {
    parse(input.as_bytes())
}

/// Types that know how to deserialize themselves from JSON via a [`Lexer`].
///
/// The `'input` lifetime is the lifetime of the input bytes. Implementors
/// that borrow from input (e.g. `&'input str`) tie their output to `'input`;
/// owned implementors (e.g. `String`) leave `'input` unused.
pub trait FromJson<'input>: Sized {
    /// Parse one value of `Self` from the lexer's current position.
    ///
    /// Whitespace before the value is consumed by the lexer's value-reading
    /// methods, so impls do not need to skip whitespace themselves.
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error>;

    /// Optional fast path for `Vec<Self>`. The default implementation drives
    /// `from_lex` once per element. Types where the per-element streaming
    /// detour is pure overhead (the integer types, `&str`) override this to
    /// lex-and-parse directly.
    #[cfg(feature = "alloc")]
    #[doc(hidden)]
    fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
        let mut out = alloc::vec::Vec::new();
        if lex.array_start()? {
            return Ok(out);
        }
        out.push(Self::from_lex(lex)?);
        while !lex.array_continue(b']')? {
            out.push(Self::from_lex(lex)?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

#[inline]
const fn type_error(lex: &Lexer<'_>, kind: ErrorKind) -> Error {
    Error::new(kind, lex.position())
}

impl<'input> FromJson<'input> for bool {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        match lex.read_value()? {
            Event::Bool(b) => Ok(b),
            _ => Err(type_error(lex, ErrorKind::ExpectedBool)),
        }
    }
}

impl<'input> FromJson<'input> for () {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        match lex.read_value()? {
            Event::Null => Ok(()),
            _ => Err(type_error(lex, ErrorKind::ExpectedNull)),
        }
    }
}

impl<'input> FromJson<'input> for &'input str {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        // The fast path: skip Event entirely.
        match lex.peek_value_kind()? {
            ValueKind::String => lex.parse_str_value(),
            _ => Err(type_error(lex, ErrorKind::ExpectedString)),
        }
    }

    /// Fused-pass fast path for `Vec<&str>`. Falls back to the streaming
    /// path for the first element only so an immediate `]` (empty array)
    /// is handled cleanly.
    #[cfg(feature = "alloc")]
    fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
        let mut out: alloc::vec::Vec<&'input str> = alloc::vec::Vec::new();
        if lex.array_start()? {
            return Ok(out);
        }
        out.push(lex.parse_str_value()?);
        while !lex.array_continue(b']')? {
            out.push(lex.parse_str_value()?);
        }
        Ok(out)
    }
}

macro_rules! impl_int {
    ($($t:ty => $accessor:ident),* $(,)?) => {
        $(
            impl<'input> FromJson<'input> for $t {
                fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
                    match lex.peek_value_kind()? {
                        ValueKind::Number => {
                            let n: JsonNum = lex.read_number()?;
                            let big = n.$accessor(lex.input())
                                .map_err(|kind| Error::new(kind, lex.position()))?;
                            <$t>::try_from(big).map_err(|_| {
                                Error::new(ErrorKind::NumberOutOfRange, lex.position())
                            })
                        }
                        _ => Err(type_error(lex, ErrorKind::ExpectedNumber)),
                    }
                }

                /// Fused-pass fast path: skip `JsonNum` entirely.
                #[cfg(feature = "alloc")]
                fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
                    let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
                    if lex.array_start()? {
                        return Ok(out);
                    }
                    let v = lex.parse_i64_value()?;
                    out.push(<$t>::try_from(v).map_err(|_| {
                        Error::new(ErrorKind::NumberOutOfRange, lex.position())
                    })?);
                    while !lex.array_continue(b']')? {
                        let v = lex.parse_i64_value()?;
                        out.push(<$t>::try_from(v).map_err(|_| {
                            Error::new(ErrorKind::NumberOutOfRange, lex.position())
                        })?);
                    }
                    Ok(out)
                }
            }
        )*
    };
}

impl_int!(i8 => as_i64, i16 => as_i64, i32 => as_i64, i64 => as_i64, isize => as_i64);
impl_int!(u8 => as_u64, u16 => as_u64, u32 => as_u64, u64 => as_u64, usize => as_u64);

impl<'input> FromJson<'input> for f64 {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        match lex.peek_value_kind()? {
            ValueKind::Number => {
                let n: JsonNum = lex.read_number()?;
                n.as_f64(lex.input())
                    .map_err(|kind| Error::new(kind, lex.position()))
            }
            _ => Err(type_error(lex, ErrorKind::ExpectedNumber)),
        }
    }
}

impl<'input> FromJson<'input> for f32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        f64::from_lex(lex).map(|v| v as Self)
    }
}

// ---------------------------------------------------------------------------
// Composite impls
// ---------------------------------------------------------------------------

impl<'input, T: FromJson<'input>> FromJson<'input> for Option<T> {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        match lex.peek_value_kind()? {
            ValueKind::Null => {
                let _ = lex.read_value()?;
                Ok(None)
            }
            _ => Ok(Some(T::from_lex(lex)?)),
        }
    }
}

impl<'input, T: FromJson<'input>, const N: usize> FromJson<'input> for [T; N] {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        let mut slots: [Option<T>; N] = core::array::from_fn(|_| None);

        let empty = lex.array_start()?;
        if empty {
            if N == 0 {
                return Ok(core::array::from_fn(|i| slots[i].take().expect("slot filled")));
            }
            return Err(type_error(lex, ErrorKind::TypeMismatch));
        }

        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = Some(T::from_lex(lex)?);
            let closed = lex.array_continue(b']')?;
            if closed {
                if i + 1 == N {
                    return Ok(core::array::from_fn(|i| slots[i].take().expect("slot filled")));
                }
                return Err(type_error(lex, ErrorKind::TypeMismatch));
            }
        }
        // Filled all N slots without seeing `]` after the last one: too long.
        Err(type_error(lex, ErrorKind::TypeMismatch))
    }
}

// Tuples — keep small and explicit.
//
// The macro generates a fixed-arity walk: for `(A, B, C)` we parse A, then
// require `,`; parse B, then require `,`; parse C, then require `]`.
// `array_continue` returns true when it consumed `]` and false when it
// consumed `,`, which is exactly the polarity we need for the last vs
// non-last branch.
macro_rules! impl_tuple {
    ($last:tt: $LAST:ident $(, $idx:tt: $T:ident)*) => {
        impl<'input, $LAST: FromJson<'input> $(, $T: FromJson<'input>)*>
            FromJson<'input> for ($LAST, $($T,)*)
        {
            #[allow(non_snake_case)]
            fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
                if lex.array_start()? {
                    return Err(type_error(lex, ErrorKind::TypeMismatch));
                }
                let $LAST = <$LAST>::from_lex(lex)?;
                $(
                    if lex.array_continue(b']')? {
                        return Err(type_error(lex, ErrorKind::TypeMismatch));
                    }
                    let $T = <$T>::from_lex(lex)?;
                )*
                if !lex.array_continue(b']')? {
                    return Err(type_error(lex, ErrorKind::TypeMismatch));
                }
                let _ = ($last $(, $idx)*); // silence unused-tt warnings
                Ok(($LAST, $($T,)*))
            }
        }
    };
}

impl_tuple!(0: A);
impl_tuple!(0: A, 1: B);
impl_tuple!(0: A, 1: B, 2: C);
impl_tuple!(0: A, 1: B, 2: C, 3: D);
impl_tuple!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);

// ---------------------------------------------------------------------------
// alloc-gated impls
// ---------------------------------------------------------------------------

#[cfg(feature = "alloc")]
mod alloc_impls {
    extern crate alloc;
    use super::{FromJson, type_error};
    use alloc::borrow::Cow;
    use alloc::string::String;
    use alloc::vec::Vec;
    use bourne_core::{Error, ErrorKind, JsonStr, Lexer, ValueKind};

    impl<'input> FromJson<'input> for String {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            match lex.peek_value_kind()? {
                ValueKind::String => {
                    // Drive the lexer directly with the no-validate variant.
                    // The decoder below performs the same validation walk
                    // as part of decoding, so paying validate_escapes here
                    // (the lexer's default) would be a redundant pass over
                    // every escaped string. perf showed this redundant
                    // pass at 42% of total time on Vec<String> with escapes.
                    let js = lex.read_string_no_validate()?;
                    decode_string(js, lex)
                }
                _ => Err(type_error(lex, ErrorKind::ExpectedString)),
            }
        }
    }

    /// Borrow-when-you-can, allocate-when-you-must. The right default for
    /// most string fields: ~95% of production JSON has no escapes, so this
    /// avoids the per-element allocation that `String` pays. When escapes
    /// are present the cost is identical to `String`.
    impl<'input> FromJson<'input> for Cow<'input, str> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            match lex.peek_value_kind()? {
                ValueKind::String => {
                    let js = lex.read_string_no_validate()?;
                    if let Some(borrowed) = js.as_str(lex.input()) {
                        return Ok(Cow::Borrowed(borrowed));
                    }
                    Ok(Cow::Owned(decode_owned(js, lex)?))
                }
                _ => Err(type_error(lex, ErrorKind::ExpectedString)),
            }
        }
    }

    /// Owned-`String` decode given a `JsonStr`. Branches on `has_escapes`:
    /// the no-escape path borrows the validated bytes and copies into a
    /// fresh `String`; the escape path goes through `decode_owned`.
    #[inline]
    fn decode_string(js: JsonStr, lex: &Lexer<'_>) -> Result<String, Error> {
        if let Some(borrowed) = js.as_str(lex.input()) {
            return Ok(String::from(borrowed));
        }
        decode_owned(js, lex)
    }

    /// Shared owned-decode path for `String` and `Cow::Owned`. Pulled out
    /// so the two impls cannot drift on capacity hint, error mapping, or
    /// the (subtle) raw-bytes-missing case.
    fn decode_owned(s: JsonStr, lex: &Lexer<'_>) -> Result<String, Error> {
        // Capacity hint is the raw byte length: the decoded form is never
        // longer than the encoded form (every escape sequence produces at
        // most as many UTF-8 bytes as it occupies on the wire).
        let raw = s
            .raw_bytes(lex.input())
            .ok_or_else(|| Error::new(ErrorKind::InvalidEscape, lex.position()))?;
        let mut out = String::with_capacity(raw.len());
        decode_escapes(raw, &mut out).map_err(|kind| Error::new(kind, lex.position()))?;
        Ok(out)
    }

    impl<'input, T: FromJson<'input>> FromJson<'input> for Vec<T> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            T::vec_from_lex(lex)
        }
    }

    /// Decode a JSON string body into `dst`, expanding escape sequences.
    ///
    /// `raw` is the bytes between (but not including) the surrounding `"`s,
    /// produced by [`Lexer::read_string_no_validate`]. This function is
    /// the *only* validator on that path: it covers every escape sequence
    /// (each escape is inspected to dispatch into the right branch), every
    /// hex digit (via `parse_hex4`), and every surrogate pairing — so the
    /// lexer can skip the redundant `validate_escapes` walk.
    ///
    /// Output is always valid UTF-8: the non-escape bytes are validated by
    /// the lexer's inline UTF-8 walk, and `\u`-derived bytes come from
    /// `encode_utf8` on a checked `char`.
    ///
    /// # Safety justification for the `unsafe` block
    ///
    /// The literal-byte path uses `core::str::from_utf8_unchecked`. The
    /// invariant: every byte in `raw` reached this function via the lexer,
    /// which validates UTF-8 inline against the RFC 3629 byte ranges as it
    /// scans (see `Parser::consume_utf8_multibyte` and `scan_ascii_string_run`
    /// in `bourne-core`). The bytes between escapes are therefore valid
    /// UTF-8 by construction — re-validating them in safe code is the
    /// `from_utf8` re-walk that perf showed at ~12% of total time.
    /// `bourne`'s `unsafe_code = "deny"` lint is overridden for this one
    /// function with `#[allow]`, mirroring the same localized exception
    /// `bourne-core` makes at the equivalent site.
    #[allow(unsafe_code)]
    fn decode_escapes(raw: &[u8], dst: &mut String) -> Result<(), ErrorKind> {
        let mut i = 0;
        while i < raw.len() {
            let b = raw[i];
            if b != b'\\' {
                // Literal byte run: find the next `\` (or end) and append the
                // whole stretch in one push. This is the hot path for strings
                // with sparse escapes (most production payloads).
                let start = i;
                while i < raw.len() && raw[i] != b'\\' {
                    i += 1;
                }
                // SAFETY: see the function-level comment. The lexer
                // validated these bytes as UTF-8 inline.
                let chunk = unsafe { core::str::from_utf8_unchecked(&raw[start..i]) };
                dst.push_str(chunk);
                continue;
            }
            // At a backslash. Need at least one more byte.
            i += 1;
            if i >= raw.len() {
                return Err(ErrorKind::InvalidEscape);
            }
            match raw[i] {
                b'"' => dst.push('"'),
                b'\\' => dst.push('\\'),
                b'/' => dst.push('/'),
                b'b' => dst.push('\u{0008}'),
                b'f' => dst.push('\u{000C}'),
                b'n' => dst.push('\n'),
                b'r' => dst.push('\r'),
                b't' => dst.push('\t'),
                b'u' => {
                    if i + 5 > raw.len() {
                        return Err(ErrorKind::InvalidUnicodeEscape);
                    }
                    let cp = parse_hex4(&raw[i + 1..i + 5])?;
                    i += 4; // advance past the four hex digits; the +1 below covers `u`
                    if (0xD800..=0xDBFF).contains(&cp) {
                        // High surrogate — must be followed by `\uXXXX` low.
                        // After the four hex digits, i points one before
                        // the next byte; bump past it then check for `\u`.
                        if i + 7 > raw.len() || raw[i + 1] != b'\\' || raw[i + 2] != b'u' {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        let low = parse_hex4(&raw[i + 3..i + 7])?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        // Combine surrogate pair into a codepoint.
                        let high_off = cp - 0xD800;
                        let low_off = low - 0xDC00;
                        let scalar = 0x1_0000 + (high_off << 10) + low_off;
                        let ch = char::from_u32(scalar)
                            .ok_or(ErrorKind::InvalidUnicodeEscape)?;
                        dst.push(ch);
                        i += 6; // skip `\uXXXX`
                    } else if (0xDC00..=0xDFFF).contains(&cp) {
                        return Err(ErrorKind::UnpairedSurrogate);
                    } else {
                        // BMP scalar — char::from_u32 always succeeds for
                        // values outside the surrogate range.
                        let ch = char::from_u32(cp)
                            .ok_or(ErrorKind::InvalidUnicodeEscape)?;
                        dst.push(ch);
                    }
                }
                _ => return Err(ErrorKind::InvalidEscape),
            }
            i += 1;
        }
        Ok(())
    }

    /// Same digit-walk as `bourne-core`'s `parse_hex4`. Duplicated here
    /// because it is a four-line helper and re-exposing it from `bourne-core`
    /// would widen the public API of a crate that's deliberately small.
    fn parse_hex4(bytes: &[u8]) -> Result<u32, ErrorKind> {
        let mut v: u32 = 0;
        for &b in bytes {
            let d = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(ErrorKind::InvalidUnicodeEscape),
            };
            v = (v << 4) | u32::from(d);
        }
        Ok(v)
    }

    // Ensure JsonStr stays imported even if a future refactor drops the
    // direct reference. The trait impl above uses it transitively.
    #[allow(dead_code)]
    type _UseJsonStr = JsonStr;
}
