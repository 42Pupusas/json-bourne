//! Type-driven deserialization.
//!
//! The [`FromJson`] trait is the heart of `bourne`. Each type that knows how
//! to parse itself from JSON consumes a contiguous sequence of [`Event`]s
//! starting with the event for its own value. Containers consume their own
//! `Start*` and `End*` events; scalars consume exactly one event.
//!
//! The top-level [`parse`] function wires a [`Parser`] to an implementor.

use bourne_core::{Error, ErrorKind, Event, Parser, Position};

/// A pull source of [`Event`]s with one-event lookahead.
///
/// Implementations buffer at most one event so that [`FromJson`] impls
/// (notably `Option<T>`) can branch on the next event without consuming it.
pub trait EventSource<'input> {
    /// Consume and return the next event, or `Ok(None)` at end of document.
    fn next_event(&mut self) -> Result<Option<Event<'input>>, Error>;

    /// Look at the next event without consuming it.
    fn peek_event(&mut self) -> Result<Option<&Event<'input>>, Error>;

    /// Current byte position in the input. Used for error reporting.
    fn position(&self) -> Position;
}

/// One-event-lookahead adapter over a raw [`Parser`].
#[derive(Debug)]
pub struct PeekableParser<'input> {
    inner: Parser<'input>,
    peeked: Option<Event<'input>>,
}

impl<'input> PeekableParser<'input> {
    #[must_use]
    pub const fn new(input: &'input [u8]) -> Self {
        Self { inner: Parser::new(input), peeked: None }
    }
}

impl<'input> EventSource<'input> for PeekableParser<'input> {
    fn next_event(&mut self) -> Result<Option<Event<'input>>, Error> {
        if let Some(ev) = self.peeked.take() {
            return Ok(Some(ev));
        }
        self.inner.next_event()
    }

    fn peek_event(&mut self) -> Result<Option<&Event<'input>>, Error> {
        if self.peeked.is_none() {
            self.peeked = self.inner.next_event()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn position(&self) -> Position {
        self.inner.position()
    }
}

/// Parse a value of type `T` from a slice of JSON bytes.
///
/// Consumes the entire document; if any non-whitespace data follows the
/// parsed value, returns [`ErrorKind::TrailingData`].
pub fn parse<'input, T: FromJson<'input>>(input: &'input [u8]) -> Result<T, Error> {
    let mut p = PeekableParser::new(input);
    let value = T::from_json(&mut p)?;
    if p.next_event()?.is_some() {
        return Err(Error::new(ErrorKind::TrailingData, p.position()));
    }
    Ok(value)
}

/// Parse a value from a `&str`. Convenience wrapper over [`parse`].
pub fn parse_str<'input, T: FromJson<'input>>(input: &'input str) -> Result<T, Error> {
    parse(input.as_bytes())
}

/// Types that know how to deserialize themselves from a stream of JSON events.
///
/// The `'input` lifetime is the lifetime of the input bytes. Implementors that
/// borrow from input (e.g. `&'input str`) tie their output to `'input`; owned
/// implementors (e.g. `String`) leave `'input` unused.
pub trait FromJson<'input>: Sized {
    /// Parse exactly one value, consuming events from the source.
    ///
    /// On entry, the next event from `source` is the start of this type's
    /// value. On success, all events belonging to that value have been
    /// consumed (no more, no less).
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expect_event<'input, S: EventSource<'input>>(source: &mut S) -> Result<Event<'input>, Error> {
    source
        .next_event()?
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, source.position()))
}

fn type_error<'input, S: EventSource<'input>>(source: &S, kind: ErrorKind) -> Error {
    Error::new(kind, source.position())
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

impl<'input> FromJson<'input> for bool {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match expect_event(source)? {
            Event::Bool(b) => Ok(b),
            _ => Err(type_error(source, ErrorKind::ExpectedBool)),
        }
    }
}

impl<'input> FromJson<'input> for () {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match expect_event(source)? {
            Event::Null => Ok(()),
            _ => Err(type_error(source, ErrorKind::ExpectedNull)),
        }
    }
}

impl<'input> FromJson<'input> for &'input str {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match expect_event(source)? {
            Event::String(s) => s
                .as_str()
                .ok_or_else(|| type_error(source, ErrorKind::InvalidEscape)),
            _ => Err(type_error(source, ErrorKind::ExpectedString)),
        }
    }
}

macro_rules! impl_int {
    ($($t:ty => $accessor:ident),* $(,)?) => {
        $(
            impl<'input> FromJson<'input> for $t {
                fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
                    match expect_event(source)? {
                        Event::Number(n) => {
                            let big = n.$accessor()?;
                            <$t>::try_from(big).map_err(|_| {
                                Error::new(ErrorKind::NumberOutOfRange, n.position())
                            })
                        }
                        _ => Err(type_error(source, ErrorKind::ExpectedNumber)),
                    }
                }
            }
        )*
    };
}

impl_int!(i8 => as_i64, i16 => as_i64, i32 => as_i64, i64 => as_i64, isize => as_i64);
impl_int!(u8 => as_u64, u16 => as_u64, u32 => as_u64, u64 => as_u64, usize => as_u64);

impl<'input> FromJson<'input> for f64 {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match expect_event(source)? {
            Event::Number(n) => n.as_f64(),
            _ => Err(type_error(source, ErrorKind::ExpectedNumber)),
        }
    }
}

impl<'input> FromJson<'input> for f32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        f64::from_json(source).map(|v| v as Self)
    }
}

// ---------------------------------------------------------------------------
// Composite impls
// ---------------------------------------------------------------------------

impl<'input, T: FromJson<'input>> FromJson<'input> for Option<T> {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match source.peek_event()? {
            Some(Event::Null) => {
                let _ = source.next_event()?;
                Ok(None)
            }
            Some(_) => Ok(Some(T::from_json(source)?)),
            None => Err(type_error(source, ErrorKind::UnexpectedEof)),
        }
    }
}

impl<'input, T: FromJson<'input>, const N: usize> FromJson<'input> for [T; N] {
    fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
        match expect_event(source)? {
            Event::StartArray => {}
            _ => return Err(type_error(source, ErrorKind::ExpectedArray)),
        }

        // Build into a fixed-size buffer of `Option<T>` so we can drop already-
        // initialized elements on error without `MaybeUninit` (forbidden by
        // unsafe_code = "forbid"). N=0 is handled by the empty loop.
        let mut slots: [Option<T>; N] = core::array::from_fn(|_| None);

        for slot in &mut slots {
            // Before consuming a value, check whether the array is shorter than N.
            if matches!(source.peek_event()?, Some(Event::EndArray)) {
                return Err(type_error(source, ErrorKind::TypeMismatch));
            }
            *slot = Some(T::from_json(source)?);
        }

        // Now require EndArray — anything else means the array was longer than N.
        match expect_event(source)? {
            Event::EndArray => {}
            _ => return Err(type_error(source, ErrorKind::TypeMismatch)),
        }

        // Pull the values out. Every slot was set above.
        Ok(core::array::from_fn(|i| slots[i].take().expect("slot filled")))
    }
}

// Tuples — keep small and explicit.
macro_rules! impl_tuple {
    ($($idx:tt: $T:ident),+) => {
        impl<'input, $($T: FromJson<'input>),+> FromJson<'input> for ($($T,)+) {
            fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
                match expect_event(source)? {
                    Event::StartArray => {}
                    _ => return Err(type_error(source, ErrorKind::ExpectedArray)),
                }
                let value = ( $( <$T>::from_json(source)?, )+ );
                match expect_event(source)? {
                    Event::EndArray => Ok(value),
                    _ => Err(type_error(source, ErrorKind::TypeMismatch)),
                }
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
    use super::{EventSource, FromJson, expect_event, type_error};
    use alloc::string::String;
    use alloc::vec::Vec;
    use bourne_core::{Error, ErrorKind, Event};

    impl<'input> FromJson<'input> for String {
        fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
            match expect_event(source)? {
                Event::String(s) => s.as_str().map_or_else(
                    // Decoding escapes into a buffer is the next milestone.
                    // Until then, escaped strings can't deserialize as `String`.
                    || Err(type_error(source, ErrorKind::InvalidEscape)),
                    |borrowed| Ok(Self::from(borrowed)),
                ),
                _ => Err(type_error(source, ErrorKind::ExpectedString)),
            }
        }
    }

    impl<'input, T: FromJson<'input>> FromJson<'input> for Vec<T> {
        fn from_json<S: EventSource<'input>>(source: &mut S) -> Result<Self, Error> {
            match expect_event(source)? {
                Event::StartArray => {}
                _ => return Err(type_error(source, ErrorKind::ExpectedArray)),
            }
            let mut out = Self::new();
            loop {
                if matches!(source.peek_event()?, Some(Event::EndArray)) {
                    let _ = source.next_event()?;
                    return Ok(out);
                }
                out.push(T::from_json(source)?);
            }
        }
    }
}
