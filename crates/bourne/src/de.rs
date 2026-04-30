//! Type-driven deserialization.
//!
//! The [`FromJson`] trait is the heart of `bourne`. Each type knows how to
//! parse itself from JSON given the [`Event`] that opens its value (already
//! consumed by the caller) and the remaining event source.
//!
//! Why pre-consumed? Container impls (Vec, struct, array) need to discover
//! "is the next event still part of me?" without paying for one-event
//! lookahead. By making the caller responsible for pulling the next event
//! and the impl responsible for handling whatever it received, we eliminate
//! the buffered-peek layer entirely. The simplest sketch:
//!
//! ```ignore
//! // Inside Vec<T>::from_event, after StartArray:
//! loop {
//!     let next = source.next_event()?;
//!     match next {
//!         Some(Event::EndArray) => return Ok(out),
//!         Some(start)           => out.push(T::from_event(source, start)?),
//!         None                  => return Err(unexpected_eof),
//!     }
//! }
//! ```
//!
//! Each element costs exactly one `next_event` call.

use bourne_core::{Error, ErrorKind, Event, Parser, Position};

/// A pull source of [`Event`]s.
///
/// Just `next_event` + position — the previous one-event peek capability
/// is no longer needed because the trait passes the first event of each
/// value to the impl directly.
pub trait EventSource<'input> {
    fn next_event(&mut self) -> Result<Option<Event<'input>>, Error>;
    fn position(&self) -> Position;
}

impl<'input, const MAX_DEPTH: usize> EventSource<'input> for Parser<'input, MAX_DEPTH> {
    fn next_event(&mut self) -> Result<Option<Event<'input>>, Error> {
        Self::next_event(self)
    }

    fn position(&self) -> Position {
        Self::position(self)
    }
}

/// Parse a value of type `T` from a slice of JSON bytes.
///
/// Pulls the first event itself, dispatches to `T::from_event`, then
/// requires that no further data follows.
pub fn parse<'input, T: FromJson<'input>>(input: &'input [u8]) -> Result<T, Error> {
    let mut p: Parser<'input> = Parser::new(input);
    let first = p
        .next_event()?
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, p.position()))?;
    let value = T::from_event(&mut p, first)?;
    if p.next_event()?.is_some() {
        return Err(Error::new(ErrorKind::TrailingData, p.position()));
    }
    Ok(value)
}

/// Parse from a `&str`.
pub fn parse_str<'input, T: FromJson<'input>>(input: &'input str) -> Result<T, Error> {
    parse(input.as_bytes())
}

/// Types that know how to deserialize themselves from a stream of JSON events.
///
/// The `'input` lifetime is the lifetime of the input bytes. Implementors
/// that borrow from input (e.g. `&'input str`) tie their output to `'input`;
/// owned implementors (e.g. `String`) leave `'input` unused.
pub trait FromJson<'input>: Sized {
    /// Continue parsing this type given that `start` was just consumed
    /// from `source`.
    ///
    /// For scalar types, `start` *is* the value (e.g. `Event::Number(...)`)
    /// and the impl returns immediately. For containers, `start` is the
    /// `StartObject` / `StartArray` and the impl consumes the rest.
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn type_error<'input, S: EventSource<'input>>(source: &S, kind: ErrorKind) -> Error {
    Error::new(kind, source.position())
}

fn next_or_eof<'input, S: EventSource<'input>>(source: &mut S) -> Result<Event<'input>, Error> {
    source
        .next_event()?
        .ok_or_else(|| type_error(source, ErrorKind::UnexpectedEof))
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

impl<'input> FromJson<'input> for bool {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        match start {
            Event::Bool(b) => Ok(b),
            _ => Err(type_error(source, ErrorKind::ExpectedBool)),
        }
    }
}

impl<'input> FromJson<'input> for () {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        match start {
            Event::Null => Ok(()),
            _ => Err(type_error(source, ErrorKind::ExpectedNull)),
        }
    }
}

impl<'input> FromJson<'input> for &'input str {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        match start {
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
                fn from_event<S: EventSource<'input>>(
                    source: &mut S,
                    start: Event<'input>,
                ) -> Result<Self, Error> {
                    match start {
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
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        match start {
            Event::Number(n) => n.as_f64(),
            _ => Err(type_error(source, ErrorKind::ExpectedNumber)),
        }
    }
}

impl<'input> FromJson<'input> for f32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        f64::from_event(source, start).map(|v| v as Self)
    }
}

// ---------------------------------------------------------------------------
// Composite impls
// ---------------------------------------------------------------------------

impl<'input, T: FromJson<'input>> FromJson<'input> for Option<T> {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        match start {
            Event::Null => Ok(None),
            other => Ok(Some(T::from_event(source, other)?)),
        }
    }
}

impl<'input, T: FromJson<'input>, const N: usize> FromJson<'input> for [T; N] {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        if !matches!(start, Event::StartArray) {
            return Err(type_error(source, ErrorKind::ExpectedArray));
        }

        // Build into Option<T> slots so we can drop already-initialized
        // elements on error without MaybeUninit (forbidden by the
        // workspace-wide unsafe_code = "deny" + per-crate "forbid").
        let mut slots: [Option<T>; N] = core::array::from_fn(|_| None);

        for slot in &mut slots {
            let ev = next_or_eof(source)?;
            if matches!(ev, Event::EndArray) {
                return Err(type_error(source, ErrorKind::TypeMismatch));
            }
            *slot = Some(T::from_event(source, ev)?);
        }

        // Now require EndArray.
        match next_or_eof(source)? {
            Event::EndArray => {}
            _ => return Err(type_error(source, ErrorKind::TypeMismatch)),
        }

        Ok(core::array::from_fn(|i| slots[i].take().expect("slot filled")))
    }
}

// Tuples — keep small and explicit.
macro_rules! impl_tuple {
    ($($idx:tt: $T:ident),+) => {
        impl<'input, $($T: FromJson<'input>),+> FromJson<'input> for ($($T,)+) {
            fn from_event<S: EventSource<'input>>(
                source: &mut S,
                start: Event<'input>,
            ) -> Result<Self, Error> {
                if !matches!(start, Event::StartArray) {
                    return Err(type_error(source, ErrorKind::ExpectedArray));
                }
                let value = ( $(
                    {
                        let ev = next_or_eof(source)?;
                        <$T>::from_event(source, ev)?
                    },
                )+ );
                match next_or_eof(source)? {
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
    use super::{EventSource, FromJson, next_or_eof, type_error};
    use alloc::string::String;
    use alloc::vec::Vec;
    use bourne_core::{Error, ErrorKind, Event};

    impl<'input> FromJson<'input> for String {
        fn from_event<S: EventSource<'input>>(
            source: &mut S,
            start: Event<'input>,
        ) -> Result<Self, Error> {
            match start {
                Event::String(s) => s.as_str().map_or_else(
                    // Decoding escapes into a buffer is the next milestone.
                    || Err(type_error(source, ErrorKind::InvalidEscape)),
                    |borrowed| Ok(Self::from(borrowed)),
                ),
                _ => Err(type_error(source, ErrorKind::ExpectedString)),
            }
        }
    }

    impl<'input, T: FromJson<'input>> FromJson<'input> for Vec<T> {
        fn from_event<S: EventSource<'input>>(
            source: &mut S,
            start: Event<'input>,
        ) -> Result<Self, Error> {
            if !matches!(start, Event::StartArray) {
                return Err(type_error(source, ErrorKind::ExpectedArray));
            }
            let mut out = Self::new();
            loop {
                let ev = next_or_eof(source)?;
                match ev {
                    Event::EndArray => return Ok(out),
                    other => out.push(T::from_event(source, other)?),
                }
            }
        }
    }
}
