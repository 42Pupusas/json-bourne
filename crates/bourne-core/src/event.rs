use crate::error::ErrorKind;
use core::fmt;

/// Top bit of `start_packed` carries `has_escapes`. JSON inputs are limited
/// to `i32::MAX` bytes (~2 GB) so the lower 31 bits are enough for a real
/// document. Values above 2 GB are rejected at parse time.
const ESCAPED_BIT: u32 = 1 << 31;
const OFFSET_MASK: u32 = !ESCAPED_BIT;

/// Maximum byte size of a JSON document this parser will accept. Bounded by
/// our packed-offset representation in `Event`.
pub const MAX_INPUT_LEN: usize = OFFSET_MASK as usize;

/// A JSON string span — a `(start, end)` pair into the original input.
///
/// Materializing into a real `&str` requires the original input slice.
/// Stored as two `u32`s instead of a fat `&[u8]` so the parent `Event`
/// fits in 16 bytes (one xmm register), eliminating the per-event memory
/// shuffles that dominated typed parsing in profiling.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct JsonStr {
    /// Low 31 bits: byte offset where the string content starts (after the
    /// opening `"`). Top bit: `has_escapes` flag.
    start_packed: u32,
    /// Byte offset one-past the end of the content (the closing `"`).
    end: u32,
}

impl JsonStr {
    pub(crate) const fn new(start: u32, end: u32, has_escapes: bool) -> Self {
        let start_packed = if has_escapes { start | ESCAPED_BIT } else { start };
        Self { start_packed, end }
    }

    /// `true` iff the lexer saw a `\` in the string. When false, the raw
    /// bytes are already valid UTF-8 (the lexer validated them inline) and
    /// can be turned into `&str` for free.
    #[must_use]
    pub const fn has_escapes(&self) -> bool {
        self.start_packed & ESCAPED_BIT != 0
    }

    #[must_use]
    pub const fn start(&self) -> u32 {
        self.start_packed & OFFSET_MASK
    }

    #[must_use]
    pub const fn end(&self) -> u32 {
        self.end
    }

    /// Raw bytes between (but not including) the quotes, taken from `input`.
    ///
    /// Caller must pass the same input the parser was constructed with;
    /// otherwise the indices are meaningless. Returns `None` if the input
    /// is shorter than the recorded range (only happens if the caller
    /// passes a different/truncated buffer).
    #[must_use]
    pub fn raw_bytes<'input>(&self, input: &'input [u8]) -> Option<&'input [u8]> {
        let start = self.start() as usize;
        let end = self.end as usize;
        input.get(start..end)
    }

    /// The string as `&str` when it contains no escapes. Skips re-validation:
    /// the parser already ensured the bytes are valid UTF-8.
    ///
    /// Returns `None` when the string had escapes (caller must decode into
    /// a buffer) or when `input` doesn't cover the recorded range.
    #[must_use]
    pub fn as_str<'input>(&self, input: &'input [u8]) -> Option<&'input str> {
        if self.has_escapes() {
            return None;
        }
        let raw = self.raw_bytes(input)?;
        // SAFETY: the lexer validates every byte against the RFC 3629 byte
        // ranges as it scans (see `Parser::consume_utf8_multibyte` and the
        // ASCII fast arm). The slice is therefore valid UTF-8 and
        // `from_utf8_unchecked` is sound.
        Some(unsafe { core::str::from_utf8_unchecked(raw) })
    }
}

impl fmt::Debug for JsonStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Manual Debug because `start_packed` carries the `has_escapes` bit
        // packed into its high bit; the raw u32 would be misleading. Show
        // the unpacked logical fields instead.
        f.debug_struct("JsonStr")
            .field("start", &self.start())
            .field("end", &self.end)
            .field("has_escapes", &self.has_escapes())
            .finish_non_exhaustive()
    }
}

/// A JSON number span — `(start, end)` into the original input.
///
/// Numbers are not eagerly decoded — JSON numbers are arbitrary-precision
/// decimal and there is no Rust primitive that losslessly fits all of them.
/// Decoding is the consumer's choice via `as_i64`, `as_u64`, `as_f64`.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct JsonNum {
    start: u32,
    end: u32,
}

impl JsonNum {
    pub(crate) const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn start(&self) -> u32 {
        self.start
    }

    #[must_use]
    pub const fn end(&self) -> u32 {
        self.end
    }

    /// Raw bytes of the number literal, taken from the original input.
    #[must_use]
    pub fn raw_bytes<'input>(&self, input: &'input [u8]) -> Option<&'input [u8]> {
        input.get(self.start as usize..self.end as usize)
    }

    /// The raw number text. Always ASCII (lexer guarantees this).
    #[must_use]
    pub fn as_str<'input>(&self, input: &'input [u8]) -> &'input str {
        // SAFETY: lexer accepts only the ASCII subset RFC 8259 allows for
        // numbers. Always valid UTF-8.
        self.raw_bytes(input)
            .map_or("", |bytes| unsafe { core::str::from_utf8_unchecked(bytes) })
    }

    /// True if the literal contains `.`, `e`, or `E` — i.e. is not an integer.
    #[must_use]
    pub fn is_float(&self, input: &[u8]) -> bool {
        self.raw_bytes(input)
            .is_some_and(|bytes| bytes.iter().any(|b| matches!(*b, b'.' | b'e' | b'E')))
    }

    #[inline]
    pub fn as_i64(&self, input: &[u8]) -> Result<i64, ErrorKind> {
        let bytes = self.raw_bytes(input).ok_or(ErrorKind::InvalidNumber)?;
        parse_i64(bytes).ok_or(ErrorKind::NumberOutOfRange)
    }

    #[inline]
    pub fn as_u64(&self, input: &[u8]) -> Result<u64, ErrorKind> {
        let bytes = self.raw_bytes(input).ok_or(ErrorKind::InvalidNumber)?;
        parse_u64(bytes).ok_or(ErrorKind::NumberOutOfRange)
    }

    pub fn as_f64(&self, input: &[u8]) -> Result<f64, ErrorKind> {
        // v1: route through core's str::parse. Replace with our own
        // dtoa-grade decoder later. Correctness now, performance later.
        self.as_str(input)
            .parse::<f64>()
            .map_err(|_| ErrorKind::InvalidNumber)
    }
}

impl fmt::Debug for JsonNum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonNum")
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}

/// `u64::MAX` has 20 decimal digits. Up to 19 digits, the loop body
/// `acc * 10 + d` cannot overflow a `u64`, so we skip `checked_*` entirely
/// — the only `imul` plus `jo` pair per digit dominated profiles for
/// integer-array workloads. The 20-digit case retakes the slow path.
const U64_FAST_DIGITS: usize = 19;

/// `i64::MAX` has 19 decimal digits. The fast path handles up to 18 digits
/// (also covers all 18-or-fewer-digit negatives, since `i64::MIN` has 19
/// digits including the sign and we strip the sign first).
const I64_FAST_DIGITS: usize = 18;

#[inline]
fn parse_u64(raw: &[u8]) -> Option<u64> {
    if raw.is_empty() {
        return None;
    }
    if raw.len() <= U64_FAST_DIGITS {
        // Wide enough that overflow is impossible — skip checked arithmetic.
        // Each digit becomes `mul + add + cmp + jb` instead of
        // `mul + jo + cmp + jb`, removing the `jo` dependency on `mul`'s
        // overflow flag and shortening the critical path.
        let mut acc: u64 = 0;
        for &b in raw {
            let d = b.wrapping_sub(b'0');
            if d >= 10 {
                return None;
            }
            acc = acc * 10 + u64::from(d);
        }
        return Some(acc);
    }
    // 20+ digits: must check overflow on every step. u64 fits at most 20.
    let mut acc: u64 = 0;
    for &b in raw {
        let d = b.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        acc = acc.checked_mul(10)?.checked_add(u64::from(d))?;
    }
    Some(acc)
}

#[inline]
fn parse_i64(raw: &[u8]) -> Option<i64> {
    let (negative, digits) = match raw.split_first() {
        Some((&b'-', rest)) => (true, rest),
        _ => (false, raw),
    };
    if digits.is_empty() {
        return None;
    }
    if digits.len() <= I64_FAST_DIGITS {
        // 18 or fewer digits — both signs fit in i64 without overflow.
        let mut acc: i64 = 0;
        for &b in digits {
            let d = b.wrapping_sub(b'0');
            if d >= 10 {
                return None;
            }
            acc = acc * 10 + i64::from(d);
        }
        return Some(if negative { -acc } else { acc });
    }
    // 19+ digits: i64::MIN has 19 digits ("-9223372036854775808"), so we
    // must check at every step. Two loops to specialize add vs sub.
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
///
/// `Event` is 16 bytes — `(JsonStr|JsonNum: 8 bytes) + (tag: 1 byte) + 7
/// bytes padding`. This fits in one xmm register, so `next_event` returns
/// it without spilling to the stack — measurable in profiles as ~12% of
/// typed-parse runtime saved compared to the previous 32-byte representation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Event {
    StartObject,
    EndObject,
    StartArray,
    EndArray,
    Key(JsonStr),
    String(JsonStr),
    Number(JsonNum),
    Bool(bool),
    Null,
}
