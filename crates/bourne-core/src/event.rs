use crate::error::{Error, ErrorKind, Position};
use core::fmt;

/// A JSON string slice as it appears in the source.
///
/// Two-state representation:
///   * `Borrowed(&str)` — no escapes, lexer already validated UTF-8 inline,
///     the slice is ready to use with no per-access work.
///   * `Escaped(&[u8])` — at least one `\` was seen; the bytes are the raw
///     between-quote span, escape syntax has been validated, but full
///     decoding requires a caller-provided buffer.
///
/// This split exists so `as_str()` never has to re-validate UTF-8 — the
/// borrowed case has already done the work, and the escaped case can't
/// answer without a decode buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JsonStr<'input> {
    Borrowed(&'input str),
    Escaped(&'input [u8]),
}

impl<'input> JsonStr<'input> {
    pub(crate) const fn borrowed(s: &'input str) -> Self {
        Self::Borrowed(s)
    }

    pub(crate) const fn escaped(raw: &'input [u8]) -> Self {
        Self::Escaped(raw)
    }

    /// The raw bytes between (but not including) the surrounding quotes.
    /// Identical for both representations — `Borrowed`'s underlying
    /// `str::as_bytes()` is the same span the lexer captured.
    #[must_use]
    pub const fn as_raw_bytes(&self) -> &'input [u8] {
        match self {
            Self::Borrowed(s) => s.as_bytes(),
            Self::Escaped(raw) => raw,
        }
    }

    #[must_use]
    pub const fn has_escapes(&self) -> bool {
        matches!(self, Self::Escaped(_))
    }

    /// Returns the string as `&str` when it contains no escapes.
    /// When escapes are present, the caller must decode into a buffer.
    #[must_use]
    pub const fn as_str(&self) -> Option<&'input str> {
        match self {
            Self::Borrowed(s) => Some(s),
            Self::Escaped(_) => None,
        }
    }
}

/// A JSON number, kept as the original byte slice.
///
/// Numbers are not eagerly decoded — JSON numbers are arbitrary-precision
/// decimal and there is no Rust primitive that losslessly fits all of them.
/// Decoding is the consumer's choice via `as_i64`, `as_u64`, `as_f64`.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct JsonNum<'input> {
    raw: &'input [u8],
    position: Position,
}

impl<'input> JsonNum<'input> {
    pub(crate) const fn new(raw: &'input [u8], position: Position) -> Self {
        Self { raw, position }
    }

    #[must_use]
    pub const fn as_raw_bytes(&self) -> &'input [u8] {
        self.raw
    }

    /// The raw number text. Always ASCII (lexer guarantees this).
    #[must_use]
    pub fn as_str(&self) -> &'input str {
        // Lexer only accepts the ASCII subset RFC 8259 allows for numbers.
        core::str::from_utf8(self.raw).unwrap_or("")
    }

    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// True if the literal contains `.`, `e`, or `E` — i.e. is not an integer.
    #[must_use]
    pub fn is_float(&self) -> bool {
        self.raw.iter().any(|b| matches!(*b, b'.' | b'e' | b'E'))
    }

    pub fn as_i64(&self) -> Result<i64, Error> {
        parse_i64(self.raw).ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange, self.position))
    }

    pub fn as_u64(&self) -> Result<u64, Error> {
        parse_u64(self.raw).ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange, self.position))
    }

    pub fn as_f64(&self) -> Result<f64, Error> {
        // v1: route through core's str::parse. Replace with our own
        // dtoa-grade decoder later. Correctness now, performance later.
        self.as_str()
            .parse::<f64>()
            .map_err(|_| Error::new(ErrorKind::InvalidNumber, self.position))
    }
}

impl fmt::Debug for JsonNum<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsonNum({})", self.as_str())
    }
}

fn parse_u64(raw: &[u8]) -> Option<u64> {
    if raw.is_empty() {
        return None;
    }
    let mut acc: u64 = 0;
    for &b in raw {
        // ASCII-digit fast path: `b.wrapping_sub(b'0')` lands in 0..=9 only
        // for `b'0'..=b'9'`. Any other byte (including `.`, `e`, `E`, `-`)
        // gives a value >= 10 and aborts. One pass instead of two.
        let d = b.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        acc = acc.checked_mul(10)?.checked_add(u64::from(d))?;
    }
    Some(acc)
}

fn parse_i64(raw: &[u8]) -> Option<i64> {
    let (negative, digits) = match raw.split_first() {
        Some((&b'-', rest)) => (true, rest),
        _ => (false, raw),
    };
    if digits.is_empty() {
        return None;
    }
    // Two specialized loops keeps each iteration branch-free aside from the
    // overflow checks. Rustc usually hoists the `negative` test on its own,
    // but spelling it out leaves nothing to chance and reads as the intent.
    let mut acc: i64 = 0;
    if negative {
        for &b in digits {
            let d = b.wrapping_sub(b'0');
            if d >= 10 {
                return None;
            }
            acc = acc.checked_mul(10)?.checked_sub(i64::from(d))?;
        }
    } else {
        for &b in digits {
            let d = b.wrapping_sub(b'0');
            if d >= 10 {
                return None;
            }
            acc = acc.checked_mul(10)?.checked_add(i64::from(d))?;
        }
    }
    Some(acc)
}

/// A single event from the streaming parser.
///
/// The parser emits these in document order. Containers are delimited by
/// matched `Start*`/`End*` pairs. Inside an object, every value event is
/// preceded by a `Key` event for that field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<'input> {
    StartObject,
    EndObject,
    StartArray,
    EndArray,
    Key(JsonStr<'input>),
    String(JsonStr<'input>),
    Number(JsonNum<'input>),
    Bool(bool),
    Null,
}
