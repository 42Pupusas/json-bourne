use crate::error::{Error, ErrorKind, Position};
use core::fmt;

/// A JSON string slice as it appears in the source.
///
/// Borrows from the input. Decoding escapes requires a caller-provided
/// buffer (see future `decode_into` API). For strings with no escapes,
/// the borrowed bytes are already valid UTF-8 and can be exposed as `&str`.
#[derive(Copy, Clone)]
pub struct JsonStr<'input> {
    raw: &'input [u8],
    has_escapes: bool,
}

impl<'input> JsonStr<'input> {
    pub(crate) const fn new(raw: &'input [u8], has_escapes: bool) -> Self {
        Self { raw, has_escapes }
    }

    /// The raw bytes between (but not including) the surrounding quotes.
    pub const fn as_raw_bytes(&self) -> &'input [u8] {
        self.raw
    }

    pub const fn has_escapes(&self) -> bool {
        self.has_escapes
    }

    /// Returns the string as `&str` when it contains no escapes.
    /// When escapes are present, the caller must decode into a buffer.
    pub fn as_str(&self) -> Option<&'input str> {
        if self.has_escapes {
            None
        } else {
            // Lexer validates this is UTF-8 before producing a JsonStr.
            core::str::from_utf8(self.raw).ok()
        }
    }
}

impl fmt::Debug for JsonStr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(s) => write!(f, "JsonStr({s:?})"),
            None => write!(f, "JsonStr(<escaped {} bytes>)", self.raw.len()),
        }
    }
}

impl PartialEq for JsonStr<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw && self.has_escapes == other.has_escapes
    }
}

impl Eq for JsonStr<'_> {}

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

    pub const fn as_raw_bytes(&self) -> &'input [u8] {
        self.raw
    }

    /// The raw number text. Always ASCII (lexer guarantees this).
    pub fn as_str(&self) -> &'input str {
        // Lexer only accepts the ASCII subset RFC 8259 allows for numbers.
        core::str::from_utf8(self.raw).unwrap_or("")
    }

    pub const fn position(&self) -> Position {
        self.position
    }

    /// True if the literal contains `.`, `e`, or `E` — i.e. is not an integer.
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
    // Reject anything with a fractional or exponent part — those are not u64.
    if raw.iter().any(|b| matches!(*b, b'.' | b'e' | b'E' | b'-')) {
        return None;
    }
    let mut acc: u64 = 0;
    for &b in raw {
        let d = (b as char).to_digit(10)?;
        acc = acc.checked_mul(10)?.checked_add(u64::from(d))?;
    }
    Some(acc)
}

fn parse_i64(raw: &[u8]) -> Option<i64> {
    if raw.iter().any(|b| matches!(*b, b'.' | b'e' | b'E')) {
        return None;
    }
    let (negative, digits) = match raw.split_first() {
        Some((&b'-', rest)) => (true, rest),
        _ => (false, raw),
    };
    if digits.is_empty() {
        return None;
    }
    let mut acc: i64 = 0;
    for &b in digits {
        let d = i64::from((b as char).to_digit(10)?);
        acc = acc.checked_mul(10)?;
        acc = if negative {
            acc.checked_sub(d)?
        } else {
            acc.checked_add(d)?
        };
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
