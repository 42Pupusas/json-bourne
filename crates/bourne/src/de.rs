//! Type-driven deserialization.
//!
//! The [`FromJson`] trait is the heart of `json-bourne`. Each type knows how to
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

use crate::{Error, ErrorKind, Event, JsonNum, Lexer, ValueKind};

/// Parse a value of type `T` from a slice of JSON bytes.
///
/// Inputs above [`crate::MAX_INPUT_LEN`] (~2 GB) return
/// [`ErrorKind::InputTooLarge`] rather than panicking.
pub fn parse<'input, T: FromJson<'input>>(input: &'input [u8]) -> Result<T, Error> {
    let mut lex = Lexer::try_new(input)?;
    let value = T::from_lex(&mut lex)?;
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

macro_rules! impl_uint {
    ($($t:ty),* $(,)?) => {
        $(
            impl<'input> FromJson<'input> for $t {
                fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
                    match lex.peek_value_kind()? {
                        ValueKind::Number => {
                            let n: JsonNum = lex.read_number()?;
                            let big = n.as_u64(lex.input())
                                .map_err(|kind| Error::new(kind, lex.position()))?;
                            <$t>::try_from(big).map_err(|_| {
                                Error::new(ErrorKind::NumberOutOfRange, lex.position())
                            })
                        }
                        _ => Err(type_error(lex, ErrorKind::ExpectedNumber)),
                    }
                }

                /// Fused-pass fast path via `parse_u64_value`. The signed
                /// one rejects everything above `i64::MAX`; this keeps the
                /// whole `u64` range parseable inside `Vec<$t>`.
                #[cfg(feature = "alloc")]
                fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
                    let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
                    if lex.array_start()? {
                        return Ok(out);
                    }
                    let v = lex.parse_u64_value()?;
                    out.push(<$t>::try_from(v).map_err(|_| {
                        Error::new(ErrorKind::NumberOutOfRange, lex.position())
                    })?);
                    while !lex.array_continue(b']')? {
                        let v = lex.parse_u64_value()?;
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

impl_uint!(u8, u16, u32, u64, usize);

// 128-bit ints — fused lex + decode via `parse_i128_value` /
// `parse_u128_value`. The earlier path went through `JsonNum::as_*128`
// which delegated to `str::parse::<i128>`; perf showed that taking 60%
// of the `Vec<i128>` workload because `str::parse` uses checked
// arithmetic on every digit. The bespoke parser skips overflow checks
// for the first 38 digits (which always fit a `u128`).
macro_rules! impl_int_wide {
    ($($t:ty => $parse_fn:ident),* $(,)?) => {
        $(
            impl<'input> FromJson<'input> for $t {
                fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
                    match lex.peek_value_kind()? {
                        ValueKind::Number => lex.$parse_fn(),
                        _ => Err(type_error(lex, ErrorKind::ExpectedNumber)),
                    }
                }

                /// Fused-pass fast path for `Vec<Self>`: skip the
                /// per-element peek-and-dispatch.
                #[cfg(feature = "alloc")]
                fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
                    let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
                    if lex.array_start()? {
                        return Ok(out);
                    }
                    out.push(lex.$parse_fn()?);
                    while !lex.array_continue(b']')? {
                        out.push(lex.$parse_fn()?);
                    }
                    Ok(out)
                }
            }
        )*
    };
}

impl_int_wide!(i128 => parse_i128_value, u128 => parse_u128_value);

impl<'input> FromJson<'input> for f64 {
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        match lex.peek_value_kind()? {
            ValueKind::Number => lex.parse_f64_value(),
            _ => Err(type_error(lex, ErrorKind::ExpectedNumber)),
        }
    }

    /// Fused-pass fast path for `Vec<f64>`: skip `JsonNum` and the
    /// type-peek per element. The `array_continue` polarity is the
    /// same shape the integer impls use.
    #[cfg(feature = "alloc")]
    fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
        let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
        if lex.array_start()? {
            return Ok(out);
        }
        out.push(lex.parse_f64_value()?);
        while !lex.array_continue(b']')? {
            out.push(lex.parse_f64_value()?);
        }
        Ok(out)
    }
}

impl<'input> FromJson<'input> for f32 {
    /// Reject literals whose magnitude can't fit `f32` instead of
    /// silently coercing to `±inf`. Subnormals and finite-but-imprecise
    /// values still narrow as a normal cast (the JSON spec doesn't
    /// promise lossless f32 round-trip; libstd's `f64 as f32` rounds
    /// to the nearest representable value).
    #[allow(clippy::cast_possible_truncation)]
    fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
        let v = f64::from_lex(lex)?;
        let narrowed = v as Self;
        if narrowed.is_finite() {
            Ok(narrowed)
        } else {
            Err(Error::new(ErrorKind::NumberOutOfRange, lex.position()))
        }
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
                return Ok(core::array::from_fn(|i| {
                    slots[i].take().expect("slot filled")
                }));
            }
            return Err(type_error(lex, ErrorKind::TypeMismatch));
        }

        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = Some(T::from_lex(lex)?);
            let closed = lex.array_continue(b']')?;
            if closed {
                if i + 1 == N {
                    return Ok(core::array::from_fn(|i| {
                        slots[i].take().expect("slot filled")
                    }));
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
pub use alloc_impls::{KeyCow, MapKey, key_to_cow};

#[cfg(feature = "alloc")]
mod alloc_impls {
    extern crate alloc;
    use super::{FromJson, type_error};
    use crate::{Error, ErrorKind, JsonStr, Lexer, ValueKind};
    use alloc::borrow::Cow;
    use alloc::string::String;
    use alloc::vec::Vec;

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
                    if let Some(borrowed) = js.as_str_in_input(lex.input()) {
                        return Ok(Cow::Borrowed(borrowed));
                    }
                    Ok(Cow::Owned(decode_owned(js, lex)?))
                }
                _ => Err(type_error(lex, ErrorKind::ExpectedString)),
            }
        }
    }

    /// JSON has no `char` type — pick "string of exactly one Unicode
    /// scalar value" as the convention. Empty strings, multi-character
    /// strings, and strings whose decoded form is not exactly one
    /// scalar all reject. Escapes are honored (e.g. `"\n"`, `"é"`).
    impl<'input> FromJson<'input> for char {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            match lex.peek_value_kind()? {
                ValueKind::String => {
                    let js = lex.read_string_no_validate()?;
                    // Borrow path: no escapes — read the validated UTF-8
                    // directly and require exactly one scalar.
                    if let Some(borrowed) = js.as_str_in_input(lex.input()) {
                        return single_char(borrowed)
                            .ok_or_else(|| type_error(lex, ErrorKind::TypeMismatch));
                    }
                    // Escape path: the decoded form of a single-char
                    // string is at most 4 UTF-8 bytes, so decode onto
                    // the stack with the same escape walk the general
                    // string path uses — no allocation. Overflow (a
                    // multi-scalar string) simply truncates, and the
                    // length check below rejects it.
                    let raw = js
                        .raw_bytes(lex.input())
                        .ok_or_else(|| Error::new(ErrorKind::InvalidEscape, lex.position()))?;
                    let mut scratch = EscapeScratch::<4>::new();
                    decode_escapes_into(raw, &mut scratch)
                        .map_err(|kind| Error::new(kind, lex.position()))?;
                    let s = scratch
                        .as_str()
                        .ok_or_else(|| type_error(lex, ErrorKind::TypeMismatch))?;
                    single_char(s).ok_or_else(|| type_error(lex, ErrorKind::TypeMismatch))
                }
                _ => Err(type_error(lex, ErrorKind::ExpectedString)),
            }
        }
    }

    // -----------------------------------------------------------------
    // Map and set collections.
    //
    // JSON object keys are always strings; the lexer's `_lex`-suffixed
    // key methods surface them as `JsonStr` so callers can decode
    // escapes when present. `key_to_cow` below borrows when escape-free
    // and decodes otherwise — yielding `Cow<'input, str>`. The MapKey
    // adapter then converts that into the user's chosen K. Supported
    // keys: String, Cow<'input, str>, and (escape-free only)
    // &'input str.
    //
    // `BTreeMap`/`BTreeSet` are alloc-gated (live in `alloc`).
    // `HashMap`/`HashSet` are std-gated (live in `std`).
    // -----------------------------------------------------------------

    /// Materialize an object key from a [`JsonStr`] span. Borrows when the
    /// key is escape-free; decodes into an owned `String` otherwise.
    pub fn key_to_cow<'input>(js: JsonStr, lex: &Lexer<'input>) -> Result<Cow<'input, str>, Error> {
        if let Some(borrowed) = js.as_str_in_input(lex.input()) {
            return Ok(Cow::Borrowed(borrowed));
        }
        Ok(Cow::Owned(decode_owned(js, lex)?))
    }

    /// Internal adapter from a decoded JSON key to the user's key type.
    ///
    /// Sealed by the limited set of impls we provide; users who need a
    /// custom key type should hand-write the `FromJson` impl for the map
    /// or wrap the key in a newtype.
    ///
    /// Returns `Result` because not every adapter is total — `&'input str`
    /// keys can only borrow, so escape-bearing keys reject with
    /// [`ErrorKind::BorrowedKeyNeedsDecode`] (the escape itself is valid;
    /// the key type is the limitation).
    pub trait MapKey<'input>: Sized {
        fn from_key(key: Cow<'input, str>, lex: &Lexer<'input>) -> Result<Self, Error>;
    }

    /// The decoded object-key type derive-generated code holds between
    /// the key read and the match.
    ///
    /// Borrows when the key is escape-free, owns a fresh `String` otherwise.
    /// Exported as `json_bourne::KeyCow` so generated code never names
    /// `::alloc` directly (which breaks std-only consumers).
    pub type KeyCow<'input> = Cow<'input, str>;

    impl<'input> MapKey<'input> for String {
        #[inline]
        fn from_key(key: Cow<'input, str>, _lex: &Lexer<'input>) -> Result<Self, Error> {
            Ok(key.into_owned())
        }
    }

    impl<'input> MapKey<'input> for Cow<'input, str> {
        #[inline]
        fn from_key(key: Self, _lex: &Lexer<'input>) -> Result<Self, Error> {
            Ok(key)
        }
    }

    impl<'input> MapKey<'input> for &'input str {
        /// Borrowing `&'input str` keys cannot represent escape-bearing
        /// keys, since the decoded form lives in a fresh allocation
        /// outside the input. Reject those at runtime; callers that
        /// expect escapes should use `Cow<'input, str>` or `String`.
        #[inline]
        fn from_key(key: Cow<'input, str>, lex: &Lexer<'input>) -> Result<Self, Error> {
            match key {
                Cow::Borrowed(s) => Ok(s),
                Cow::Owned(_) => Err(type_error(lex, ErrorKind::BorrowedKeyNeedsDecode)),
            }
        }
    }

    trait DupMap<K, V> {
        fn new_empty() -> Self;
        fn try_insert(&mut self, k: K, v: V) -> bool;
    }

    impl<K: Ord, V> DupMap<K, V> for alloc::collections::BTreeMap<K, V> {
        fn new_empty() -> Self {
            Self::new()
        }
        fn try_insert(&mut self, k: K, v: V) -> bool {
            self.insert(k, v).is_none()
        }
    }

    #[cfg(feature = "std")]
    impl<K, V, S> DupMap<K, V> for std::collections::HashMap<K, V, S>
    where
        K: ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn new_empty() -> Self {
            Self::with_hasher(S::default())
        }
        fn try_insert(&mut self, k: K, v: V) -> bool {
            self.insert(k, v).is_none()
        }
    }

    #[cfg(feature = "indexmap")]
    impl<K, V, S> DupMap<K, V> for indexmap::IndexMap<K, V, S>
    where
        K: ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn new_empty() -> Self {
            Self::with_hasher(S::default())
        }
        fn try_insert(&mut self, k: K, v: V) -> bool {
            self.insert(k, v).is_none()
        }
    }

    fn parse_map<'input, K, V, M>(lex: &mut Lexer<'input>) -> Result<M, Error>
    where
        K: MapKey<'input>,
        V: FromJson<'input>,
        M: DupMap<K, V>,
    {
        if !matches!(lex.peek_value_kind()?, ValueKind::Object) {
            return Err(type_error(lex, ErrorKind::TypeMismatch));
        }
        lex.object_start()?;
        let mut out = M::new_empty();
        let mut maybe_key = lex.object_first_key_lex()?;
        while let Some(js) = maybe_key {
            let key_cow = key_to_cow(js, lex)?;
            let key = K::from_key(key_cow, lex)?;
            let v = V::from_lex(lex)?;
            if !out.try_insert(key, v) {
                return Err(Error::new(ErrorKind::DuplicateKey, lex.position()));
            }
            maybe_key = lex.object_next_key_lex()?;
        }
        Ok(out)
    }

    impl<'input, K, V> FromJson<'input> for alloc::collections::BTreeMap<K, V>
    where
        K: MapKey<'input> + Ord,
        V: FromJson<'input>,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_map(lex)
        }
    }

    #[cfg(feature = "std")]
    impl<'input, K, V, S> FromJson<'input> for std::collections::HashMap<K, V, S>
    where
        K: MapKey<'input> + ::core::hash::Hash + Eq,
        V: FromJson<'input>,
        S: ::core::hash::BuildHasher + Default,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_map(lex)
        }
    }

    trait SetInsert<T> {
        fn new_empty() -> Self;
        fn push(&mut self, v: T);
    }

    impl<T: Ord> SetInsert<T> for alloc::collections::BTreeSet<T> {
        fn new_empty() -> Self {
            Self::new()
        }
        fn push(&mut self, v: T) {
            self.insert(v);
        }
    }

    #[cfg(feature = "std")]
    impl<T, S> SetInsert<T> for std::collections::HashSet<T, S>
    where
        T: ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn new_empty() -> Self {
            Self::with_hasher(S::default())
        }
        fn push(&mut self, v: T) {
            self.insert(v);
        }
    }

    #[cfg(feature = "indexmap")]
    impl<T, S> SetInsert<T> for indexmap::IndexSet<T, S>
    where
        T: ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn new_empty() -> Self {
            Self::with_hasher(S::default())
        }
        fn push(&mut self, v: T) {
            self.insert(v);
        }
    }

    fn parse_set<'input, T, C>(lex: &mut Lexer<'input>) -> Result<C, Error>
    where
        T: FromJson<'input>,
        C: SetInsert<T>,
    {
        let mut out = C::new_empty();
        if lex.array_start()? {
            return Ok(out);
        }
        out.push(T::from_lex(lex)?);
        while !lex.array_continue(b']')? {
            out.push(T::from_lex(lex)?);
        }
        Ok(out)
    }

    impl<'input, T> FromJson<'input> for alloc::collections::BTreeSet<T>
    where
        T: FromJson<'input> + Ord,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_set(lex)
        }
    }

    #[cfg(feature = "std")]
    impl<'input, T, S> FromJson<'input> for std::collections::HashSet<T, S>
    where
        T: FromJson<'input> + ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_set(lex)
        }
    }

    // -----------------------------------------------------------------
    // std::time and std::net adapters.
    //
    // JSON has no native types for any of these, so we pick a single
    // convention per type. These are deliberately simple — projects
    // with bespoke encodings (RFC 3339 timestamps, custom Duration
    // shapes) should hand-write a wrapper newtype.
    // -----------------------------------------------------------------

    /// Parse a `Duration` from JSON `Number` (seconds, possibly fractional).
    /// Negative inputs and non-finite values are rejected. Subsecond
    /// precision is preserved to nanosecond resolution.
    ///
    /// Implementation note: pre-validates the input to a finite,
    /// non-negative value within `Duration`'s u64-seconds range, then
    /// delegates to `Duration::from_secs_f64`. The earlier hand-rolled
    /// `trunc`/`mul`/`round`/`clamp` sequence was ~3.5× slower than
    /// `from_secs_f64` on profile (the libstd routine inlines a
    /// branch-light decomposition of the f64 mantissa).
    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::time::Duration {
        #[allow(clippy::cast_precision_loss)]
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            // Skip the `f64::from_lex` detour: that does a
            // `peek_value_kind` first, which `parse_f64_value`'s
            // own type check makes redundant for the Number arm.
            let secs_f = match lex.peek_value_kind()? {
                ValueKind::Number => lex.parse_f64_value()?,
                _ => return Err(type_error(lex, ErrorKind::ExpectedNumber)),
            };
            // `Duration::from_secs_f64` panics on these inputs;
            // convert to typed errors before calling.
            if !secs_f.is_finite() || secs_f < 0.0 || secs_f >= (u64::MAX as f64) {
                return Err(type_error(lex, ErrorKind::NumberOutOfRange));
            }
            Ok(Self::from_secs_f64(secs_f))
        }

        /// Fused-pass fast path for `Vec<Duration>`. Mirrors the
        /// `Vec<f64>` override: `parse_f64_value` directly per
        /// element, no type-peek detour past the first.
        #[cfg(feature = "alloc")]
        #[allow(clippy::cast_precision_loss)]
        fn vec_from_lex(lex: &mut Lexer<'input>) -> Result<alloc::vec::Vec<Self>, Error> {
            #[inline]
            fn convert(lex: &Lexer<'_>, secs_f: f64) -> Result<std::time::Duration, Error> {
                if !secs_f.is_finite() || secs_f < 0.0 || secs_f >= (u64::MAX as f64) {
                    return Err(type_error(lex, ErrorKind::NumberOutOfRange));
                }
                Ok(std::time::Duration::from_secs_f64(secs_f))
            }
            let mut out: alloc::vec::Vec<Self> = alloc::vec::Vec::new();
            if lex.array_start()? {
                return Ok(out);
            }
            let secs_f = lex.parse_f64_value()?;
            out.push(convert(lex, secs_f)?);
            while !lex.array_continue(b']')? {
                let secs_f = lex.parse_f64_value()?;
                out.push(convert(lex, secs_f)?);
            }
            Ok(out)
        }
    }

    /// Parse a `SystemTime` from JSON `Number` (seconds since
    /// `UNIX_EPOCH`, possibly fractional and possibly negative).
    /// Mirrors the `Duration` adapter but allows negative values for
    /// pre-epoch timestamps, which the `SystemTime` arithmetic model
    /// supports.
    ///
    /// Subsecond precision is preserved to nanosecond resolution via
    /// `Duration::from_secs_f64`.
    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::time::SystemTime {
        #[allow(clippy::cast_precision_loss)]
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            let secs_f = match lex.peek_value_kind()? {
                ValueKind::Number => lex.parse_f64_value()?,
                _ => return Err(type_error(lex, ErrorKind::ExpectedNumber)),
            };
            // Reject non-finite inputs and clamp the magnitude to a
            // range Duration::from_secs_f64 won't panic on.
            if !secs_f.is_finite() || secs_f.abs() >= (u64::MAX as f64) {
                return Err(type_error(lex, ErrorKind::NumberOutOfRange));
            }
            let abs = std::time::Duration::from_secs_f64(secs_f.abs());
            let epoch = std::time::UNIX_EPOCH;
            let st = if secs_f < 0.0 {
                epoch.checked_sub(abs)
            } else {
                epoch.checked_add(abs)
            };
            st.ok_or_else(|| type_error(lex, ErrorKind::NumberOutOfRange))
        }
    }

    /// Generic adapter for any type whose canonical text form is parseable
    /// via `FromStr`. Used below for `IpAddr`, `Ipv4Addr`, `Ipv6Addr`,
    /// `SocketAddr`, and `PathBuf`. Failures map to `TypeMismatch`.
    #[cfg(feature = "std")]
    fn parse_from_str<'input, T>(lex: &mut Lexer<'input>) -> Result<T, Error>
    where
        T: ::core::str::FromStr,
    {
        let cow: Cow<'input, str> = Cow::<'input, str>::from_lex(lex)?;
        cow.parse::<T>()
            .map_err(|_| type_error(lex, ErrorKind::TypeMismatch))
    }

    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::net::IpAddr {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_from_str(lex)
        }
    }

    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::net::Ipv4Addr {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_from_str(lex)
        }
    }

    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::net::Ipv6Addr {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_from_str(lex)
        }
    }

    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::net::SocketAddr {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_from_str(lex)
        }
    }

    /// `PathBuf::from` is infallible on every byte sequence that round-
    /// trips through UTF-8, so this never fails after string decode.
    /// Use `parse_from_str` anyway for symmetry with the other adapters.
    #[cfg(feature = "std")]
    impl<'input> FromJson<'input> for std::path::PathBuf {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            let s: String = String::from_lex(lex)?;
            Ok(Self::from(s))
        }
    }

    /// Transparent wrapper: parses an inner `T` and boxes it. The JSON
    /// shape is identical to `T`'s — there is no JSON construct that
    /// "owns" a value the way a `Box` does, and serde's convention is
    /// the same.
    impl<'input, T: FromJson<'input>> FromJson<'input> for alloc::boxed::Box<T> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            T::from_lex(lex).map(Self::new)
        }
    }

    /// Transparent wrapper, same as `Box<T>`. `Rc` is single-threaded;
    /// users that need cross-thread sharing should use `Arc<T>` instead.
    impl<'input, T: FromJson<'input>> FromJson<'input> for alloc::rc::Rc<T> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            T::from_lex(lex).map(Self::new)
        }
    }

    /// Transparent wrapper. `Arc` requires `target_has_atomic = "ptr"`
    /// transitively via `alloc::sync`; we don't gate it explicitly because
    /// `json-bourne`'s supported targets all have it.
    impl<'input, T: FromJson<'input>> FromJson<'input> for alloc::sync::Arc<T> {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            T::from_lex(lex).map(Self::new)
        }
    }

    /// Returns `Some(c)` iff `s` consists of exactly one Unicode scalar.
    #[inline]
    fn single_char(s: &str) -> Option<char> {
        let mut it = s.chars();
        let c = it.next()?;
        if it.next().is_some() {
            return None;
        }
        Some(c)
    }

    /// Owned-`String` decode given a `JsonStr`. Branches on `has_escapes`:
    /// the no-escape path borrows the validated bytes and copies into a
    /// fresh `String`; the escape path goes through `decode_owned`.
    #[inline]
    fn decode_string(js: JsonStr, lex: &Lexer<'_>) -> Result<String, Error> {
        if let Some(borrowed) = js.as_str_in_input(lex.input()) {
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

    /// SIMD-accelerated single-byte search for `\\` inside a slice.
    /// Returns the offset of the first backslash, or `None` if none.
    ///
    /// Why this exists: the literal-byte run inside `decode_escapes` is
    /// the inner loop on strings with sparse escapes (~1 escape per 50
    /// bytes is typical for production payloads). On profile, the scalar
    /// `while i < n && bytes[i] != b'\\'` walk was 64% of `decode_owned`'s
    /// time. SSE2's `_mm_cmpeq_epi8` + `_mm_movemask_epi8` walks 16 bytes
    /// per iteration with the same correctness; on `x86_64` the gain is
    /// ~10x for long literal runs.
    ///
    /// Same `unsafe_code` justification as the parent function: SSE2
    /// is part of the `x86_64` ABI baseline, the `target_feature` arm
    /// is statically enabled on `x86_64`, and the unsafe is mechanical
    /// (intrinsics carry unsafe by signature, not by memory-safety).
    #[allow(unsafe_code)]
    #[inline]
    fn find_backslash(bytes: &[u8]) -> Option<usize> {
        // Two distinct cfg-gated function bodies — splitting them into
        // separate items per arch avoids a `return` inside one cfg
        // branch (which clippy flags as `needless_return`) while
        // keeping each arm a single expression. The `bourne_no_simd`
        // cfg disables the SIMD path (used by miri).
        #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
        {
            find_backslash_sse2(bytes)
        }
        #[cfg(not(all(target_arch = "x86_64", not(bourne_no_simd))))]
        {
            bytes.iter().position(|&b| b == b'\\')
        }
    }

    #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
    #[allow(unsafe_code, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    #[inline]
    fn find_backslash_sse2(bytes: &[u8]) -> Option<usize> {
        use core::arch::x86_64::{
            _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
        };

        let n = bytes.len();
        let mut i = 0;
        // SAFETY: SSE2 is part of the x86_64 ABI baseline; rustc's default
        // target features include `+sse2`, so the intrinsics are statically
        // available. `_mm_loadu_si128` is documented as accepting unaligned
        // addresses, and the bounds check `i + 16 <= n` ensures the load
        // stays inside `bytes`.
        unsafe {
            let backslash = _mm_set1_epi8(b'\\' as i8);
            while i + 16 <= n {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
                let m = _mm_cmpeq_epi8(chunk, backslash);
                let bits = _mm_movemask_epi8(m) as u32;
                if bits != 0 {
                    return Some(i + bits.trailing_zeros() as usize);
                }
                i += 16;
            }
        }
        // Tail: scalar walk for the final <16 bytes.
        bytes[i..]
            .iter()
            .position(|&b| b == b'\\')
            .map(|off| i + off)
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
    /// in the lexer). The bytes between escapes are therefore valid
    /// UTF-8 by construction — re-validating them in safe code is the
    /// `from_utf8` re-walk that perf showed at ~12% of total time (the
    /// audit on 2026-05-19 measured the safe variant at 1.68× slower,
    /// pushing json-bourne below `serde_json` on the escape-heavy workload).
    /// `json-bourne`'s `unsafe_code = "deny"` lint is overridden for this one
    /// function with `#[allow]`, mirroring the same localized exception
    /// the lexer makes at the equivalent site.
    ///
    /// The outer loop dispatches between literal-byte runs and escape
    /// sequences. Escape decoding is delegated to `decode_simple_escape`
    /// (the 8 single-byte arms) and `decode_unicode_escape_char` (the
    /// `\u` branch including surrogate-pair logic) so this fn stays
    /// inside the project's cyclomatic-complexity budget.
    #[inline]
    fn decode_escapes(raw: &[u8], dst: &mut String) -> Result<(), ErrorKind> {
        decode_escapes_into(raw, dst)
    }

    /// The escape walk, generic over the destination so the `char`
    /// path can decode onto a stack buffer without allocating.
    #[allow(unsafe_code)]
    fn decode_escapes_into<S: EscapeSink>(raw: &[u8], dst: &mut S) -> Result<(), ErrorKind> {
        let mut i = 0;
        while i < raw.len() {
            if raw[i] != b'\\' {
                // Literal byte run: find the next `\` (or end) and append the
                // whole stretch in one push. This is the hot path for strings
                // with sparse escapes (most production payloads).
                //
                // Use SIMD scan when available — the scalar walk that used
                // to live here was 64% of total decode time on profile.
                let start = i;
                i = find_backslash(&raw[i..]).map_or(raw.len(), |off| i + off);
                // SAFETY: see the function-level comment. The lexer
                // validated these bytes as UTF-8 inline.
                let chunk = unsafe { core::str::from_utf8_unchecked(&raw[start..i]) };
                dst.push_str(chunk);
                continue;
            }
            // At a backslash: need at least one more byte.
            if i + 1 >= raw.len() {
                return Err(ErrorKind::InvalidEscape);
            }
            if raw[i + 1] == b'u' {
                let (ch, last) = decode_unicode_escape_char(raw, i + 1)?;
                dst.push(ch);
                i = last + 1;
            } else {
                dst.push(decode_simple_escape(raw[i + 1])?);
                i += 2;
            }
        }
        Ok(())
    }

    /// Decode a single non-`u` JSON escape byte to its char. The 8
    /// match arms (`"`, `\\`, `/`, `b`, `f`, `n`, `r`, `t`) plus the
    /// catch-all error arm live here so `decode_escapes` doesn't carry
    /// their cyclomatic complexity.
    #[inline]
    const fn decode_simple_escape(b: u8) -> Result<char, ErrorKind> {
        Ok(match b {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000C}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            _ => return Err(ErrorKind::InvalidEscape),
        })
    }

    /// Decode `\uXXXX` (and optionally a surrogate-pair `\uYYYY`) into
    /// a single `char`. `i` points at the `u` of the first escape.
    /// Returns the char and the index of the last hex digit consumed.
    ///
    /// Pure — no output-channel dependency — so the `char::FromJson`
    /// path reuses it to decode onto the stack.
    fn decode_unicode_escape_char(raw: &[u8], i: usize) -> Result<(char, usize), ErrorKind> {
        if i + 5 > raw.len() {
            return Err(ErrorKind::InvalidUnicodeEscape);
        }
        let cp = parse_hex4(&raw[i + 1..i + 5])?;
        // After the four hex digits, the consumed-up-to index is i + 4.
        let new_i = i + 4;
        if (0xDC00..=0xDFFF).contains(&cp) {
            return Err(ErrorKind::UnpairedSurrogate);
        }
        if (0xD800..=0xDBFF).contains(&cp) {
            if new_i + 7 > raw.len() || raw[new_i + 1] != b'\\' || raw[new_i + 2] != b'u' {
                return Err(ErrorKind::UnpairedSurrogate);
            }
            let low = parse_hex4(&raw[new_i + 3..new_i + 7])?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(ErrorKind::UnpairedSurrogate);
            }
            let scalar = 0x1_0000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
            let ch = char::from_u32(scalar).ok_or(ErrorKind::InvalidUnicodeEscape)?;
            return Ok((ch, new_i + 6));
        }
        // BMP scalar — char::from_u32 always succeeds for values
        // outside the surrogate range.
        let ch = char::from_u32(cp).ok_or(ErrorKind::InvalidUnicodeEscape)?;
        Ok((ch, new_i))
    }

    /// Destination for decoded escape output — a `String` on the general
    /// path, a fixed stack buffer on the `char` path.
    trait EscapeSink {
        fn push(&mut self, c: char);
        fn push_str(&mut self, s: &str);
    }

    impl EscapeSink for String {
        #[inline]
        fn push(&mut self, c: char) {
            Self::push(self, c);
        }
        #[inline]
        fn push_str(&mut self, s: &str) {
            Self::push_str(self, s);
        }
    }

    /// Fixed-capacity scratch for decoding at most `N` UTF-8 bytes.
    /// A decoded single scalar is ≤ 4 bytes, so the `char` path uses
    /// `EscapeScratch<4>` instead of allocating a `String`.
    struct EscapeScratch<const N: usize> {
        buf: [u8; N],
        len: usize,
    }

    impl<const N: usize> EscapeScratch<N> {
        const fn new() -> Self {
            Self {
                buf: [0; N],
                len: 0,
            }
        }
        fn as_str(&self) -> Option<&str> {
            core::str::from_utf8(&self.buf[..self.len]).ok()
        }
    }

    impl<const N: usize> EscapeSink for EscapeScratch<N> {
        #[inline]
        fn push(&mut self, c: char) {
            let mut enc = [0u8; 4];
            self.push_str(c.encode_utf8(&mut enc));
        }
        #[inline]
        fn push_str(&mut self, s: &str) {
            if let Some(rest) = self.buf.get_mut(self.len..self.len + s.len()) {
                rest.copy_from_slice(s.as_bytes());
                self.len += s.len();
            }
        }
    }

    /// Same digit-walk as the lexer's `parse_hex4`. Duplicated here
    /// rather than re-exporting because it is a four-line helper and
    /// keeping it private avoids widening the public API.
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

    // -----------------------------------------------------------------
    // IndexMap / IndexSet (optional `indexmap` feature).
    //
    // Mirror image of the BTreeMap / HashMap impls. The novel property
    // here is *insertion-order preservation*: the parsed map iterates
    // in the order keys appeared in the JSON input, which downstream
    // consumers depend on for stable output (canonical JSON, log
    // round-trips, golden-file tests).
    // -----------------------------------------------------------------

    #[cfg(feature = "indexmap")]
    impl<'input, K, V, S> FromJson<'input> for indexmap::IndexMap<K, V, S>
    where
        K: MapKey<'input> + ::core::hash::Hash + Eq,
        V: FromJson<'input>,
        S: ::core::hash::BuildHasher + Default,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_map(lex)
        }
    }

    #[cfg(feature = "indexmap")]
    impl<'input, T, S> FromJson<'input> for indexmap::IndexSet<T, S>
    where
        T: FromJson<'input> + ::core::hash::Hash + Eq,
        S: ::core::hash::BuildHasher + Default,
    {
        fn from_lex(lex: &mut Lexer<'input>) -> Result<Self, Error> {
            parse_set(lex)
        }
    }
}
