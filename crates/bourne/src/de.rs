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
/// `Event` no longer carries a lifetime — string and number variants store
/// `(start, end)` offsets into the parser's input. Consumers materialize
/// the actual `&str`/`&[u8]` via `JsonStr::as_str(input)` etc., so the
/// trait exposes `input()` to provide that slice.
///
/// The `array_*` and `parse_*_value` methods are typed-fast-path escape
/// hatches: they let `FromJson` impls bypass the per-element streaming
/// detour for shapes where it duplicates work (e.g. integer arrays,
/// where the lexer walks every digit twice). They have default impls
/// `Parser` overrides; non-Parser sources can fall back on the events.
pub trait EventSource<'input> {
    fn next_event(&mut self) -> Result<Option<Event>, Error>;
    fn position(&self) -> Position;
    fn input(&self) -> &'input [u8];

    /// Open an array. Returns `true` if it was empty (already past `]`).
    fn array_start(&mut self) -> Result<bool, Error>;

    /// After an element, consume `,` (and skip whitespace to the next
    /// element) or the closing `end_byte` (`]` for arrays). Returns
    /// `true` if the closing byte was just consumed.
    fn array_continue(&mut self, end_byte: u8) -> Result<bool, Error>;

    /// Parse one signed-integer value, fusing the lex pass with the
    /// digit-to-i64 conversion. Cursor must be at a digit (or `-`).
    fn parse_i64_value(&mut self) -> Result<i64, Error>;

    /// Parse one borrowed-string value, returning a `&'input str` directly
    /// from the input bytes. Errors on escaped strings (those need a
    /// caller-owned decode buffer; bourne does not allocate). Cursor must
    /// be at the opening `"`.
    fn parse_str_value(&mut self) -> Result<&'input str, Error>;

    /// After a `StartObject` event, read the next key. Returns `None` if
    /// the object closed immediately (`{}`). Cursor is left at the start
    /// of the field's value.
    fn object_first_key(&mut self) -> Result<Option<&'input str>, Error>;

    /// After a field value, advance to the next key or close the object.
    /// Returns `None` if the object closed.
    fn object_next_key(&mut self) -> Result<Option<&'input str>, Error>;
}

impl<'input, const MAX_DEPTH: usize> EventSource<'input> for Parser<'input, MAX_DEPTH> {
    fn next_event(&mut self) -> Result<Option<Event>, Error> {
        Self::next_event(self)
    }

    fn position(&self) -> Position {
        Self::position(self)
    }

    fn input(&self) -> &'input [u8] {
        Self::input(self)
    }

    fn array_start(&mut self) -> Result<bool, Error> {
        Self::array_start(self)
    }

    fn array_continue(&mut self, end_byte: u8) -> Result<bool, Error> {
        Self::array_continue(self, end_byte)
    }

    fn parse_i64_value(&mut self) -> Result<i64, Error> {
        Self::parse_i64_value(self)
    }

    fn parse_str_value(&mut self) -> Result<&'input str, Error> {
        Self::parse_str_value(self)
    }

    fn object_first_key(&mut self) -> Result<Option<&'input str>, Error> {
        Self::object_first_key(self)
    }

    fn object_next_key(&mut self) -> Result<Option<&'input str>, Error> {
        Self::object_next_key(self)
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
        start: Event,
    ) -> Result<Self, Error>;

    /// Optional fast path for `Vec<Self>`. The default implementation
    /// drives the streaming parser through one event per element. Types
    /// where the streaming detour is pure overhead (the integer types,
    /// where the lexer's digit walk and the value-conversion digit walk
    /// repeat each other) override this to lex-and-parse directly,
    /// halving per-element work.
    ///
    /// `start` is the `StartArray` event; the implementation must consume
    /// up to and including the matching `EndArray`.
    #[cfg(feature = "alloc")]
    #[doc(hidden)]
    fn vec_from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event,
    ) -> Result<alloc::vec::Vec<Self>, Error> {
        if !matches!(start, Event::StartArray) {
            return Err(type_error(source, ErrorKind::ExpectedArray));
        }
        let mut out = alloc::vec::Vec::new();
        loop {
            let ev = next_or_eof(source)?;
            match ev {
                Event::EndArray => return Ok(out),
                other => out.push(Self::from_event(source, other)?),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn type_error<'input, S: EventSource<'input>>(source: &S, kind: ErrorKind) -> Error {
    Error::new(kind, source.position())
}

fn next_or_eof<'input, S: EventSource<'input>>(source: &mut S) -> Result<Event, Error> {
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
        start: Event,
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
        start: Event,
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
        start: Event,
    ) -> Result<Self, Error> {
        match start {
            Event::String(s) => s
                .as_str(source.input())
                .ok_or_else(|| type_error(source, ErrorKind::InvalidEscape)),
            _ => Err(type_error(source, ErrorKind::ExpectedString)),
        }
    }

    /// Fused-pass fast path for `Vec<&str>`. Same shape as the integer
    /// overrides: skip the per-element `next_event` / `Event::String` /
    /// `from_event` chain and hand the parser straight from the input
    /// bytes to a borrowed `&str`. Falls back to the streaming path if
    /// any element contains escape sequences (those need a decode buffer
    /// the parser doesn't own).
    #[cfg(feature = "alloc")]
    fn vec_from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event,
    ) -> Result<alloc::vec::Vec<Self>, Error> {
        if !matches!(start, Event::StartArray) {
            return Err(type_error(source, ErrorKind::ExpectedArray));
        }
        let mut out: alloc::vec::Vec<&'input str> = alloc::vec::Vec::new();
        // First element via the streaming path so we can detect immediate
        // `]` (empty array) without re-implementing whitespace handling
        // for that one case.
        let first_ev = next_or_eof(source)?;
        match first_ev {
            Event::EndArray => return Ok(out),
            ev => {
                let v = <&'input str>::from_event(source, ev)?;
                out.push(v);
            }
        }
        loop {
            if source.array_continue(b']')? {
                return Ok(out);
            }
            let s = source.parse_str_value()?;
            out.push(s);
        }
    }
}

macro_rules! impl_int {
    ($($t:ty => $accessor:ident),* $(,)?) => {
        $(
            impl<'input> FromJson<'input> for $t {
                fn from_event<S: EventSource<'input>>(
                    source: &mut S,
                    start: Event,
                ) -> Result<Self, Error> {
                    match start {
                        Event::Number(n) => {
                            let big = n.$accessor(source.input())
                                .map_err(|kind| Error::new(kind, source.position()))?;
                            <$t>::try_from(big).map_err(|_| {
                                Error::new(ErrorKind::NumberOutOfRange, source.position())
                            })
                        }
                        _ => Err(type_error(source, ErrorKind::ExpectedNumber)),
                    }
                }

                /// Fused-pass fast path for `Vec<$t>`. Skips the per-element
                /// `next_event` round trip — the streaming layer would lex
                /// each integer's digits into a `JsonNum`, then the typed
                /// layer would walk the same digits a second time to build
                /// the value. `parse_i64_value` does both in one pass.
                ///
                /// `start` is `Event::StartArray`, so `[` has already been
                /// consumed; we just need to handle the immediate-`]`
                /// (empty array) case before the first element.
                #[cfg(feature = "alloc")]
                fn vec_from_event<S: EventSource<'input>>(
                    source: &mut S,
                    start: Event,
                ) -> Result<alloc::vec::Vec<Self>, Error> {
                    if !matches!(start, Event::StartArray) {
                        return Err(type_error(source, ErrorKind::ExpectedArray));
                    }
                    let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
                    // Detect empty array: peek the first event and stop on
                    // `EndArray` before entering the fast loop. The first
                    // element costs one `next_event` either way; subsequent
                    // elements use the fused path.
                    let first_ev = next_or_eof(source)?;
                    match first_ev {
                        Event::EndArray => return Ok(out),
                        ev => {
                            let v = <Self>::from_event(source, ev)?;
                            out.push(v);
                        }
                    }
                    loop {
                        if source.array_continue(b']')? {
                            return Ok(out);
                        }
                        let v = source.parse_i64_value()?;
                        let narrow = <$t>::try_from(v).map_err(|_| {
                            Error::new(ErrorKind::NumberOutOfRange, source.position())
                        })?;
                        out.push(narrow);
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
        start: Event,
    ) -> Result<Self, Error> {
        match start {
            Event::Number(n) => n
                .as_f64(source.input())
                .map_err(|kind| Error::new(kind, source.position())),
            _ => Err(type_error(source, ErrorKind::ExpectedNumber)),
        }
    }
}

impl<'input> FromJson<'input> for f32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event,
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
        start: Event,
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
        start: Event,
    ) -> Result<Self, Error> {
        if !matches!(start, Event::StartArray) {
            return Err(type_error(source, ErrorKind::ExpectedArray));
        }

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
                start: Event,
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
    use super::{EventSource, FromJson, type_error};
    use alloc::string::String;
    use alloc::vec::Vec;
    use bourne_core::{Error, ErrorKind, Event};

    impl<'input> FromJson<'input> for String {
        fn from_event<S: EventSource<'input>>(
            source: &mut S,
            start: Event,
        ) -> Result<Self, Error> {
            match start {
                Event::String(s) => s.as_str(source.input()).map_or_else(
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
            start: Event,
        ) -> Result<Self, Error> {
            // Dispatch through the trait method so types that override
            // `vec_from_event` (e.g. integer types) can supply a fused
            // lex-and-parse fast path. For T without an override the
            // default drives the streaming parser one event at a time.
            T::vec_from_event(source, start)
        }
    }
}
