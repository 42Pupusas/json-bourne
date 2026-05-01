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
    use alloc::string::String;
    use alloc::vec::Vec;
    use bourne_core::{Error, ErrorKind, Event, Lexer};

    impl<'input> FromJson<'input> for String {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            match lex.read_value()? {
                Event::String(s) => s.as_str(lex.input()).map_or_else(
                    // Decoding escapes into a buffer is the next milestone.
                    || Err(type_error(lex, ErrorKind::InvalidEscape)),
                    |borrowed| Ok(Self::from(borrowed)),
                ),
                _ => Err(type_error(lex, ErrorKind::ExpectedString)),
            }
        }
    }

    impl<'input, T: FromJson<'input>> FromJson<'input> for Vec<T> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            T::vec_from_lex(lex)
        }
    }

}
