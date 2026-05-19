//! Stateless JSON lexer.
//!
//! `Lexer` is the byte-walking layer. It knows how to recognize and consume
//! one JSON token at a time — strings, numbers, keywords, structural bytes
//! — but it does not enforce the grammar that says "an array element is
//! followed by `,` or `]`." That bookkeeping lives in `Parser`, which wraps
//! a `Lexer` plus a `State` enum.
//!
//! The split exists because typed consumers (`Vec<T>::from_lex`,
//! `Struct::from_lex`) already enforce the grammar by virtue of which
//! method they call when. They walk the input recursively and don't need
//! the `State` dispatch — calling it per element is pure overhead. By
//! exposing the lexer directly, those consumers skip the state machine
//! entirely; only streaming consumers using `Parser::next_event` pay for
//! it.
//!
//! Container nesting is still tracked here (for depth limiting and matched
//! `]`/`}` validation), but it's a plain push/pop on `Stack`, not a state
//! machine.

use crate::error::{Error, ErrorKind, Position};
use crate::event::{Event, JsonNum, JsonStr, MAX_INPUT_LEN};

/// `i64::MIN`'s magnitude as a `u64`: `i64::MAX as u64 + 1`. This is the
/// largest `u64` value whose negation still fits in `i64`. Used by
/// `parse_i64_value` to validate sign-aware bounds — `-9223372036854775808`
/// is legal even though `+9223372036854775808` is not.
const I64_MIN_MAGNITUDE: u64 = (i64::MAX as u64) + 1;

/// `i128::MIN`'s magnitude as a `u128`. Same trick as `I64_MIN_MAGNITUDE`,
/// scaled up. `parse_i128_value` accumulates into `u128` so the negative
/// edge case (`-170141183460469231731687303715884105728`) is representable
/// during the lex pass before the sign-aware bounds check.
const I128_MIN_MAGNITUDE: u128 = (i128::MAX as u128) + 1;

/// `u128::MAX` is `2^128 - 1` ≈ 3.4 × 10^38, so 38 decimal digits always
/// fit a `u128` without overflow check. The 39-digit boundary is exactly
/// `10^38`–`u128::MAX`; from digit 39 onward we have to use checked
/// arithmetic. Mirrors the 19-digit fast path in `parse_i64_value`.
const U128_FAST_DIGITS: u32 = 38;

/// Default maximum container nesting depth.
///
/// Guards against pathological inputs (e.g. millions of `[`s) that would
/// otherwise drive recursive consumers into stack overflow. Override at
/// compile time by parameterizing [`Lexer`] (and [`Parser`](crate::Parser))
/// with a different `MAX_DEPTH`.
pub const DEFAULT_MAX_DEPTH: usize = 128;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum Frame {
    Array,
    Object,
}

/// Fixed-capacity nesting stack. Avoids `alloc` for the parser itself.
///
/// Capacity is fixed at compile time by `Lexer`'s `MAX_DEPTH` parameter,
/// so the inline array sized to it is also the depth bound.
#[derive(Debug)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct Stack<const MAX_DEPTH: usize> {
    frames: [Frame; MAX_DEPTH],
    len: usize,
}

impl<const MAX_DEPTH: usize> Stack<MAX_DEPTH> {
    pub(crate) const fn new() -> Self {
        Self {
            frames: [Frame::Array; MAX_DEPTH],
            len: 0,
        }
    }

    pub(crate) const fn push(&mut self, frame: Frame) -> Result<(), ()> {
        if self.len >= MAX_DEPTH {
            return Err(());
        }
        self.frames[self.len] = frame;
        self.len += 1;
        Ok(())
    }

    pub(crate) const fn pop(&mut self) -> Option<Frame> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            Some(self.frames[self.len])
        }
    }

    pub(crate) const fn top(&self) -> Option<Frame> {
        if self.len == 0 {
            None
        } else {
            Some(self.frames[self.len - 1])
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn truncate(&mut self, new_len: usize) {
        if new_len <= self.len {
            self.len = new_len;
        }
    }
}

/// Stateless JSON lexer over a borrowed byte slice.
///
/// Each `read_*` method consumes one syntactic token from the input. The
/// caller is responsible for invoking the right method at the right time
/// — there is no state machine that says "after reading a key, you must
/// call `expect_byte(b':')` next." Typed consumers (`FromJson` impls) drive
/// the lexer directly; streaming consumers go through `Parser` instead.
#[derive(Debug)]
pub struct Lexer<'input, const MAX_DEPTH: usize = DEFAULT_MAX_DEPTH> {
    input: &'input [u8],
    /// Byte offset of the next-to-read byte. Line and column are reconstructed
    /// lazily from `input[..offset]` whenever a `Position` is needed (errors,
    /// public `position()` calls). The hot path never touches them, which is
    /// what lets `bump()` compile to a single increment.
    offset: usize,
    pub(crate) stack: Stack<MAX_DEPTH>,
}

/// Saved lexer position + nesting depth, restorable via [`Lexer::restore`].
///
/// Returned by [`Lexer::checkpoint`]. The two fields together capture
/// everything a typed consumer might mutate while exploring an input
/// speculatively — the byte cursor and the container-frame stack depth.
/// Restoring rewinds both, putting the lexer back into the exact state it
/// was in when the checkpoint was taken.
///
/// Used by enum dispatch for representations that must try a variant and
/// retry another on failure (`#[bourne(untagged)]`) or that must locate a
/// tag field before parsing the rest of the object
/// (`#[bourne(tag = "...")]`).
#[derive(Copy, Clone, Debug)]
pub struct Checkpoint {
    offset: usize,
    stack_len: usize,
}

/// Result of `Lexer::peek_value_kind`. Tells a caller what kind of value
/// will be produced by the next `read_*` call without committing to it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Object,
    Array,
    String,
    Number,
    True,
    False,
    Null,
}

impl<'input, const MAX_DEPTH: usize> Lexer<'input, MAX_DEPTH> {
    /// Construct a lexer over `input`.
    ///
    /// # Panics
    ///
    /// Panics if `input.len() > MAX_INPUT_LEN` (~2 GB). The packed offset
    /// representation in `JsonStr`/`JsonNum` reserves the top bit of a `u32`
    /// for `has_escapes`, so positions are limited to 31 bits. Real-world
    /// JSON documents are far smaller than this; consumers needing larger
    /// streams should chunk and parse incrementally.
    #[must_use]
    pub const fn new(input: &'input [u8]) -> Self {
        assert!(input.len() <= MAX_INPUT_LEN, "input exceeds MAX_INPUT_LEN");
        Self {
            input,
            offset: 0,
            stack: Stack::new(),
        }
    }

    #[must_use]
    pub const fn position(&self) -> Position {
        compute_position(self.input, self.offset)
    }

    /// The input slice the lexer was constructed with. Consumers use this
    /// to materialize `&str`/`&[u8]` from `JsonStr` and `JsonNum`, which
    /// store offsets rather than fat pointers.
    #[must_use]
    pub const fn input(&self) -> &'input [u8] {
        self.input
    }

    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Snapshot the current cursor and nesting depth. Pair with
    /// [`restore`](Self::restore) to roll the lexer back after a
    /// speculative parse — typically used by `#[bourne(untagged)]` enum
    /// dispatch to try variants in order.
    ///
    /// The returned [`Checkpoint`] is opaque: do not construct one yourself
    /// or mix checkpoints across different `Lexer` instances.
    #[must_use]
    pub const fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            offset: self.offset,
            stack_len: self.stack.len(),
        }
    }

    /// Roll the lexer back to a previously taken [`Checkpoint`].
    ///
    /// Restores both the byte cursor and the container-frame stack depth.
    /// Intended for the speculative-retry pattern: take a checkpoint,
    /// attempt a parse, on `Err` call `restore` and try a different shape.
    ///
    /// The checkpoint must have been produced by `self.checkpoint()`. If
    /// the checkpoint is from a different lexer or from after a `restore`
    /// to a deeper depth, behavior is logically incoherent (the `truncate`
    /// no-ops if asked to grow), though never memory-unsafe.
    pub const fn restore(&mut self, cp: Checkpoint) {
        self.offset = cp.offset;
        self.stack.truncate(cp.stack_len);
    }

    /// Skip whitespace then peek at the next byte to determine the kind of
    /// value that begins there. Does not consume the byte. Returns
    /// `UnexpectedEof` if there is no next byte.
    pub fn peek_value_kind(&mut self) -> Result<ValueKind, Error> {
        self.skip_whitespace();
        let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
        match b {
            b'{' => Ok(ValueKind::Object),
            b'[' => Ok(ValueKind::Array),
            b'"' => Ok(ValueKind::String),
            b'-' | b'0'..=b'9' => Ok(ValueKind::Number),
            b't' => Ok(ValueKind::True),
            b'f' => Ok(ValueKind::False),
            b'n' => Ok(ValueKind::Null),
            other => Err(self.err(ErrorKind::UnexpectedByte(other))),
        }
    }

    /// Skip whitespace then read one full JSON value, returning the matching
    /// `Event`. For containers, opens the container (consuming `[` or `{`)
    /// and pushes a frame onto the nesting stack — the caller is responsible
    /// for matching `]`/`}` later via `read_array_continue` /
    /// `read_object_continue` (or by going through `Parser`).
    pub fn read_value(&mut self) -> Result<Event, Error> {
        self.skip_whitespace();
        let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
        match b {
            b'{' => {
                self.bump();
                self.push_frame(Frame::Object)?;
                Ok(Event::StartObject)
            }
            b'[' => {
                self.bump();
                self.push_frame(Frame::Array)?;
                Ok(Event::StartArray)
            }
            b'"' => self.read_string().map(Event::String),
            b't' => self.read_keyword(b"true", Event::Bool(true)),
            b'f' => self.read_keyword(b"false", Event::Bool(false)),
            b'n' => self.read_keyword(b"null", Event::Null),
            b'-' | b'0'..=b'9' => self.read_number().map(Event::Number),
            other => Err(self.err(ErrorKind::UnexpectedByte(other))),
        }
    }

    /// Pop a container frame; verifies the popped frame matches `expected`.
    /// Caller has already consumed the closing `]` or `}`.
    pub(crate) fn pop_frame(&mut self, expected: Frame) -> Result<(), Error> {
        let popped = self
            .stack
            .pop()
            .ok_or_else(|| self.err(ErrorKind::UnexpectedByte(b']')))?;
        if popped != expected {
            return Err(self.err(ErrorKind::TypeMismatch));
        }
        Ok(())
    }

    pub(crate) fn push_frame(&mut self, frame: Frame) -> Result<(), Error> {
        self.stack
            .push(frame)
            .map_err(|()| self.err(ErrorKind::DepthLimitExceeded))
    }

    pub(crate) fn read_keyword(&mut self, kw: &[u8], event: Event) -> Result<Event, Error> {
        for &expected in kw {
            match self.peek() {
                Some(b) if b == expected => self.bump(),
                Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
        Ok(event)
    }

    /// Read a JSON string token. Cursor must be at the opening `"`. Returns
    /// a `JsonStr` covering the bytes between the quotes (exclusive).
    ///
    /// When the body contains escape sequences, the deferred `validate_escapes`
    /// pass runs before returning — every escape is checked for syntactic
    /// validity (escape kind, hex digits, surrogate pairing). The contract
    /// for stream/Event consumers: a returned `JsonStr` with
    /// `has_escapes() == true` means the escapes are well-formed.
    pub fn read_string(&mut self) -> Result<JsonStr, Error> {
        self.read_string_inner(true)
    }

    /// Like [`read_string`](Self::read_string), but skip the deferred
    /// `validate_escapes` pass. The caller commits to performing
    /// equivalent validation as part of decoding (the typed `String`
    /// / `Cow<str>` impls do exactly this).
    ///
    /// This exists because `validate_escapes` and an eager decoder do
    /// overlapping work: the deferred validation walks the body checking
    /// every escape; the decoder walks the body to actually emit the
    /// decoded form, and naturally has to inspect every escape anyway.
    /// On profile, the redundant `validate_escapes` walk was 42% of total
    /// time on the `mixed_length_strings_with_escapes` corpus when going
    /// to `Vec<String>`. Skipping it here gives the typed path a faster
    /// route without weakening the stream-consumer contract.
    pub fn read_string_no_validate(&mut self) -> Result<JsonStr, Error> {
        self.read_string_inner(false)
    }

    fn read_string_inner(&mut self, validate: bool) -> Result<JsonStr, Error> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.bump(); // opening quote
        let start = self.offset;
        let mut has_escapes = false;
        loop {
            let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
            match b {
                b'"' => {
                    let end = self.offset;
                    self.bump(); // closing quote
                    if validate && has_escapes {
                        let raw = &self.input[start..end];
                        validate_escapes(raw).map_err(|kind| self.err(kind))?;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    return Ok(JsonStr::new(start as u32, end as u32, has_escapes));
                }
                b'\\' => {
                    has_escapes = true;
                    self.bump();
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.bump();
                        }
                        Some(b'u') => {
                            self.bump();
                            for _ in 0..4 {
                                match self.peek() {
                                    Some(b) if b.is_ascii_hexdigit() => self.bump(),
                                    Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                                    None => return Err(self.err(ErrorKind::UnexpectedEof)),
                                }
                            }
                        }
                        Some(_) => return Err(self.err(ErrorKind::InvalidEscape)),
                        None => return Err(self.err(ErrorKind::UnexpectedEof)),
                    }
                }
                0..=0x1F => return Err(self.err(ErrorKind::ControlCharInString)),
                0x20..=0x7F => self.scan_ascii_string_run(),
                _ => self.consume_utf8_multibyte()?,
            }
        }
    }

    /// Read a JSON number token. Cursor must be at `-` or a digit. Returns a
    /// `JsonNum` covering the literal.
    #[inline]
    pub fn read_number(&mut self) -> Result<JsonNum, Error> {
        let start = self.offset;

        if self.peek() == Some(b'-') {
            self.bump();
        }

        match self.peek() {
            Some(b'0') => {
                self.bump();
            }
            Some(b'1'..=b'9') => {
                self.bump();
                self.scan_digit_run();
            }
            Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => return Err(self.err(ErrorKind::UnexpectedEof)),
        }

        if self.peek() == Some(b'.') {
            self.bump();
            let frac_start = self.offset;
            self.scan_digit_run();
            if self.offset == frac_start {
                return Err(self.err(ErrorKind::InvalidNumber));
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let exp_start = self.offset;
            self.scan_digit_run();
            if self.offset == exp_start {
                return Err(self.err(ErrorKind::InvalidNumber));
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let result = JsonNum::new(start as u32, self.offset as u32);
        Ok(result)
    }

    /// Parse a JSON integer directly into `i64`, fusing lex and conversion.
    ///
    /// Caller must position the lexer at the first byte of the value
    /// (after any whitespace). Returns the parsed `i64` and leaves the
    /// cursor at the byte after the number. Rejects fractional and
    /// exponent forms — those are not integers.
    pub fn parse_i64_value(&mut self) -> Result<i64, Error> {
        let start = self.offset;
        let bytes = self.input;
        let end = bytes.len();
        let mut i = start;

        let negative = matches!(bytes.get(i), Some(&b'-'));
        if negative {
            i += 1;
        }

        let digits_start = i;
        match bytes.get(i).copied() {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                // Accumulate as u64 so positive `i64::MIN.unsigned_abs()`
                // (= 9223372036854775808) fits during the lex pass. The
                // sign-aware bounds check happens at the end.
                let mut acc: u64 = 0;
                let mut count: u32 = 0;
                while i < end {
                    let d = bytes[i].wrapping_sub(b'0');
                    if d >= 10 {
                        break;
                    }
                    if count < 19 {
                        // Up to 19 digits fit in u64 without overflow; the
                        // 20-digit boundary is u64::MAX.
                        acc = acc * 10 + u64::from(d);
                    } else {
                        acc = acc
                            .checked_mul(10)
                            .and_then(|v| v.checked_add(u64::from(d)))
                            .ok_or_else(|| {
                                self.offset = i;
                                self.err(ErrorKind::NumberOutOfRange)
                            })?;
                    }
                    i += 1;
                    count += 1;
                }
                if i == digits_start {
                    self.offset = i;
                    return Err(self.err(ErrorKind::InvalidNumber));
                }
                self.offset = i;
                if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
                    return Err(self.err(ErrorKind::ExpectedNumber));
                }
                // Sign-aware bounds. i64::MIN's magnitude is exactly
                // i64::MAX as u64 + 1 = 9223372036854775808; any larger
                // negative or positive doesn't fit.
                if negative {
                    if acc <= I64_MIN_MAGNITUDE {
                        // `0i64.wrapping_sub_unsigned(acc)` produces i64::MIN
                        // when `acc == I64_MIN_MAGNITUDE`, and the correct
                        // negative i64 for any smaller magnitude.
                        return Ok(0i64.wrapping_sub_unsigned(acc));
                    }
                    return Err(self.err(ErrorKind::NumberOutOfRange));
                }
                // `try_from` here is the same conditional as comparing
                // against `i64::MAX as u64`, but in a form clippy
                // recognizes — and avoids the cast_possible_wrap
                // exception we previously took to silence it.
                if let Ok(n) = i64::try_from(acc) {
                    return Ok(n);
                }
                return Err(self.err(ErrorKind::NumberOutOfRange));
            }
            Some(b) => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedByte(b)));
            }
            None => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedEof));
            }
        }
        self.offset = i;
        if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
            return Err(self.err(ErrorKind::ExpectedNumber));
        }
        Ok(0)
    }

    /// Parse a JSON integer directly into `i128`, fusing lex and conversion.
    ///
    /// Same shape as [`parse_i64_value`] but scaled to 128-bit. On profile,
    /// routing `Vec<i128>` through `JsonNum::as_i128` (which calls
    /// `str::parse::<i128>`) was 60% of the workload; the generic
    /// `str::parse` path uses checked arithmetic on every digit. The
    /// fast path here skips overflow checks for the first 38 digits
    /// (which always fit a `u128`) and only pays them on the 39-digit
    /// boundary case.
    ///
    /// Caller must position the lexer at the first byte of the value.
    /// Rejects fractional and exponent forms.
    ///
    /// [`parse_i64_value`]: Self::parse_i64_value
    pub fn parse_i128_value(&mut self) -> Result<i128, Error> {
        let start = self.offset;
        let bytes = self.input;
        let end = bytes.len();
        let mut i = start;

        let negative = matches!(bytes.get(i), Some(&b'-'));
        if negative {
            i += 1;
        }

        let digits_start = i;
        match bytes.get(i).copied() {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                // Accumulate as u128 so the negative edge case
                // (i128::MIN's magnitude = i128::MAX + 1) fits during
                // the lex pass; sign-aware bounds check at the end.
                let mut acc: u128 = 0;
                let mut count: u32 = 0;
                while i < end {
                    let d = bytes[i].wrapping_sub(b'0');
                    if d >= 10 {
                        break;
                    }
                    if count < U128_FAST_DIGITS {
                        acc = acc * 10 + u128::from(d);
                    } else {
                        acc = acc
                            .checked_mul(10)
                            .and_then(|v| v.checked_add(u128::from(d)))
                            .ok_or_else(|| {
                                self.offset = i;
                                self.err(ErrorKind::NumberOutOfRange)
                            })?;
                    }
                    i += 1;
                    count += 1;
                }
                if i == digits_start {
                    self.offset = i;
                    return Err(self.err(ErrorKind::InvalidNumber));
                }
                self.offset = i;
                if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
                    return Err(self.err(ErrorKind::ExpectedNumber));
                }
                if negative {
                    if acc <= I128_MIN_MAGNITUDE {
                        // Same wrapping_sub_unsigned trick as the i64
                        // path — produces i128::MIN at the boundary,
                        // correct negative i128 below it.
                        return Ok(0i128.wrapping_sub_unsigned(acc));
                    }
                    return Err(self.err(ErrorKind::NumberOutOfRange));
                }
                if let Ok(n) = i128::try_from(acc) {
                    return Ok(n);
                }
                return Err(self.err(ErrorKind::NumberOutOfRange));
            }
            Some(b) => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedByte(b)));
            }
            None => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedEof));
            }
        }
        self.offset = i;
        if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
            return Err(self.err(ErrorKind::ExpectedNumber));
        }
        Ok(0)
    }

    /// Parse a JSON unsigned integer directly into `u128`, fusing lex
    /// and conversion. Rejects negative literals, fractional, and
    /// exponent forms.
    ///
    /// Caller must position the lexer at the first byte of the value.
    pub fn parse_u128_value(&mut self) -> Result<u128, Error> {
        let start = self.offset;
        let bytes = self.input;
        let end = bytes.len();
        let mut i = start;

        // Unsigned: a leading `-` is rejected outright.
        if matches!(bytes.get(i), Some(&b'-')) {
            self.offset = i;
            return Err(self.err(ErrorKind::NumberOutOfRange));
        }

        let digits_start = i;
        match bytes.get(i).copied() {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                let mut acc: u128 = 0;
                let mut count: u32 = 0;
                while i < end {
                    let d = bytes[i].wrapping_sub(b'0');
                    if d >= 10 {
                        break;
                    }
                    if count < U128_FAST_DIGITS {
                        acc = acc * 10 + u128::from(d);
                    } else {
                        acc = acc
                            .checked_mul(10)
                            .and_then(|v| v.checked_add(u128::from(d)))
                            .ok_or_else(|| {
                                self.offset = i;
                                self.err(ErrorKind::NumberOutOfRange)
                            })?;
                    }
                    i += 1;
                    count += 1;
                }
                if i == digits_start {
                    self.offset = i;
                    return Err(self.err(ErrorKind::InvalidNumber));
                }
                self.offset = i;
                if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
                    return Err(self.err(ErrorKind::ExpectedNumber));
                }
                return Ok(acc);
            }
            Some(b) => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedByte(b)));
            }
            None => {
                self.offset = i;
                return Err(self.err(ErrorKind::UnexpectedEof));
            }
        }
        self.offset = i;
        if matches!(bytes.get(i), Some(&b'.' | &b'e' | &b'E')) {
            return Err(self.err(ErrorKind::ExpectedNumber));
        }
        Ok(0)
    }

    /// Parse a JSON number directly into `f64`, fusing lex and decode.
    ///
    /// Caller must position the lexer at the first byte of the value
    /// (after any whitespace). Skips the `JsonNum` middle layer that
    /// `read_number()` + `JsonNum::as_f64` would build — saves one
    /// `Option`-wrapped slice and the per-element struct construction.
    /// On profile, `Vec<f64>` was ~50% slower than `Vec<i64>` largely
    /// because of this missing fast path.
    ///
    /// Rejects non-finite results (out-of-range literals like `1e400`
    /// decode to `±inf` from `str::parse::<f64>`, which JSON disallows).
    pub fn parse_f64_value(&mut self) -> Result<f64, Error> {
        let start = self.offset;
        // Reuse the byte-walk of `read_number` (it already handles the
        // `-?` integer + optional `.frac` + optional `e[+-]?digits`
        // grammar correctly). Then slice the run we just walked and
        // hand it to libcore's `str::parse::<f64>` — the same routine
        // `JsonNum::as_f64` calls, just without the JsonNum struct.
        let _span = self.read_number()?;
        let end = self.offset;
        // SAFETY: `read_number` only advances over the JSON number
        // grammar's ASCII subset (`-`, digits, `.`, `e`, `E`, `+`).
        // Always valid UTF-8.
        #[allow(unsafe_code)]
        let s = unsafe { core::str::from_utf8_unchecked(&self.input[start..end]) };
        let v: f64 = s.parse().map_err(|_| self.err(ErrorKind::InvalidNumber))?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(self.err(ErrorKind::NumberOutOfRange))
        }
    }

    /// Read a JSON string and return it as a borrowed `&'input str`. Errors
    /// if the string contains escape sequences — those require a caller-owned
    /// decode buffer, which `bourne` does not allocate.
    ///
    /// Caller must position the lexer at the opening `"`. On return the
    /// cursor is past the closing `"`. The returned slice points into the
    /// original input — zero copy.
    pub fn parse_str_value(&mut self) -> Result<&'input str, Error> {
        match self.peek() {
            Some(b'"') => self.bump(),
            Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => return Err(self.err(ErrorKind::UnexpectedEof)),
        }
        let start = self.offset;
        loop {
            let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
            match b {
                b'"' => {
                    let end = self.offset;
                    self.bump();
                    let raw = &self.input[start..end];
                    // SAFETY: every byte was validated against the RFC 3629
                    // ranges by the byte walk above (ASCII fast arm or
                    // `consume_utf8_multibyte`).
                    return Ok(unsafe { core::str::from_utf8_unchecked(raw) });
                }
                b'\\' => return Err(self.err(ErrorKind::InvalidEscape)),
                0..=0x1F => return Err(self.err(ErrorKind::ControlCharInString)),
                0x20..=0x7F => self.scan_ascii_string_run(),
                _ => self.consume_utf8_multibyte()?,
            }
        }
    }

    /// Skip whitespace then expect `,` or the array-end byte. Returns
    /// `true` if at end (caller should stop), `false` to continue with
    /// another element.
    ///
    /// On `]` this also pops the matching frame from the nesting stack so
    /// a subsequent operation resumes correctly in the enclosing context.
    #[inline]
    pub fn array_continue(&mut self, end_byte: u8) -> Result<bool, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b) if b == end_byte => {
                self.bump();
                let frame = if end_byte == b']' { Frame::Array } else { Frame::Object };
                self.pop_frame(frame)?;
                Ok(true)
            }
            Some(b',') => {
                self.bump();
                self.skip_whitespace();
                Ok(false)
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// After a `StartObject`, return the next key as a borrowed `&'input str`,
    /// or `None` if the object closes immediately. The cursor is left
    /// positioned at the byte after the key's `:`, ready for the caller
    /// to parse the field's value.
    #[inline]
    pub fn object_first_key(&mut self) -> Result<Option<&'input str>, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'}') => {
                self.bump();
                self.pop_frame(Frame::Object)?;
                Ok(None)
            }
            Some(b'"') => {
                let key = self.parse_str_value()?;
                self.skip_whitespace();
                match self.peek() {
                    Some(b':') => self.bump(),
                    Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => return Err(self.err(ErrorKind::UnexpectedEof)),
                }
                self.skip_whitespace();
                Ok(Some(key))
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Like [`object_first_key`], but returns the key as a raw [`JsonStr`]
    /// span — borrowed if escape-free, recording `has_escapes()` if not.
    /// Callers can decode escapes (typically into an `alloc::borrow::Cow`)
    /// when they may be present.
    ///
    /// [`object_first_key`]: Self::object_first_key
    #[inline]
    pub fn object_first_key_lex(&mut self) -> Result<Option<JsonStr>, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'}') => {
                self.bump();
                self.pop_frame(Frame::Object)?;
                Ok(None)
            }
            Some(b'"') => {
                let key = self.read_string_no_validate()?;
                self.skip_whitespace();
                match self.peek() {
                    Some(b':') => self.bump(),
                    Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => return Err(self.err(ErrorKind::UnexpectedEof)),
                }
                self.skip_whitespace();
                Ok(Some(key))
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// After a field's value, advance to the next key or close the
    /// object. Returns `Some(key)` for the next field or `None` if the
    /// object closed (`}` consumed and stack popped).
    #[inline]
    pub fn object_next_key(&mut self) -> Result<Option<&'input str>, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'}') => {
                self.bump();
                self.pop_frame(Frame::Object)?;
                Ok(None)
            }
            Some(b',') => {
                self.bump();
                self.skip_whitespace();
                match self.peek() {
                    Some(b'"') => {
                        let key = self.parse_str_value()?;
                        self.skip_whitespace();
                        match self.peek() {
                            Some(b':') => self.bump(),
                            Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                            None => return Err(self.err(ErrorKind::UnexpectedEof)),
                        }
                        self.skip_whitespace();
                        Ok(Some(key))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                }
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Like [`object_next_key`], but returns the key as a raw [`JsonStr`]
    /// span. See [`object_first_key_lex`] for the escape-decoding pattern
    /// callers should follow.
    ///
    /// [`object_next_key`]: Self::object_next_key
    /// [`object_first_key_lex`]: Self::object_first_key_lex
    #[inline]
    pub fn object_next_key_lex(&mut self) -> Result<Option<JsonStr>, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'}') => {
                self.bump();
                self.pop_frame(Frame::Object)?;
                Ok(None)
            }
            Some(b',') => {
                self.bump();
                self.skip_whitespace();
                match self.peek() {
                    Some(b'"') => {
                        let key = self.read_string_no_validate()?;
                        self.skip_whitespace();
                        match self.peek() {
                            Some(b':') => self.bump(),
                            Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                            None => return Err(self.err(ErrorKind::UnexpectedEof)),
                        }
                        self.skip_whitespace();
                        Ok(Some(key))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                }
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Expect the byte that opens an array (`[`), advance past it, push a
    /// nesting frame, and skip whitespace to the first element (or `]`).
    /// Returns `true` if the array is empty (the closing `]` has just been
    /// consumed and the frame has been popped).
    #[inline]
    pub fn array_start(&mut self) -> Result<bool, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'[') => {
                self.bump();
                self.push_frame(Frame::Array)?;
                self.skip_whitespace();
                if self.peek() == Some(b']') {
                    self.bump();
                    self.pop_frame(Frame::Array)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Expect the byte that opens an object (`{`), advance past it, push a
    /// nesting frame, and skip whitespace. Does not consume any keys.
    #[inline]
    pub fn object_start(&mut self) -> Result<(), Error> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => {
                self.bump();
                self.push_frame(Frame::Object)?;
                self.skip_whitespace();
                Ok(())
            }
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Require that no further (non-whitespace) data follows. Used by
    /// the typed entry point to reject `"1 2"` and similar.
    pub fn finish(&mut self) -> Result<(), Error> {
        self.skip_whitespace();
        if self.peek().is_some() {
            Err(self.err(ErrorKind::TrailingData))
        } else {
            Ok(())
        }
    }

    /// Consume one complete JSON value and discard it.
    ///
    /// Walks whatever value sits at the cursor — primitive, string,
    /// number, array, or object — and advances past it. For composite
    /// values, every nested element is also skipped. The structural
    /// frame stack stays balanced: this method pushes and pops the
    /// same frames `read_value` would.
    ///
    /// Validation is the same as `read_value`: malformed input
    /// (control char in string, lone surrogate, malformed number,
    /// etc.) still raises an `Error`. The "skip" here means "throw
    /// away the value", not "throw away the parse". A lenient
    /// `deny_unknown_fields = false` consumer wants the cursor
    /// advanced, but it still wants the surrounding object to
    /// parse correctly afterward — which requires lexing the skipped
    /// value to find its end.
    ///
    /// Used by `#[derive(FromJson)]` when a struct opts into
    /// `#[bourne(deny_unknown_fields = false)]` to consume the value
    /// associated with an unrecognized key.
    pub fn skip_value(&mut self) -> Result<(), Error> {
        let event = self.read_value()?;
        match event {
            Event::StartArray => self.skip_array_body(),
            Event::StartObject => self.skip_object_body(),
            // Primitives consumed inline by `read_value`; nothing to
            // do but return.
            Event::String(_) | Event::Number(_) | Event::Bool(_) | Event::Null => Ok(()),
            // `read_value` only returns Start*/scalar events. The
            // End*/Key variants are produced by the streaming
            // Parser, not the bare lexer, so they cannot appear
            // here. Match exhaustively anyway so a future Event
            // variant is a compile error rather than a silent skip.
            Event::EndArray | Event::EndObject | Event::Key(_) => {
                Err(self.err(ErrorKind::UnexpectedByte(b']')))
            }
        }
    }

    /// Drive `array_continue` until the matching `]` closes the frame.
    /// `read_value` already consumed the opening `[` and pushed the
    /// frame; this finishes the job. Used only from `skip_value`.
    fn skip_array_body(&mut self) -> Result<(), Error> {
        // Empty array: `]` follows immediately, with the frame already
        // pushed by read_value. We need to consume `]` and pop. The
        // shared logic lives in array_continue, so peek and dispatch.
        self.skip_whitespace();
        if matches!(self.peek(), Some(b']')) {
            self.bump();
            return self.pop_frame(Frame::Array);
        }
        // Non-empty: at least one element, then either `,` (continue)
        // or `]` (done).
        self.skip_value()?;
        while !self.array_continue(b']')? {
            self.skip_value()?;
        }
        Ok(())
    }

    /// Drive `object_first_key` / `object_next_key` until the matching
    /// `}` closes the frame. The keys themselves are consumed (we
    /// don't need them); only the values need explicit skipping.
    fn skip_object_body(&mut self) -> Result<(), Error> {
        let mut key = self.object_first_key()?;
        while key.is_some() {
            self.skip_value()?;
            key = self.object_next_key()?;
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // byte-level helpers
    // -------------------------------------------------------------------

    #[inline]
    fn scan_ascii_string_run(&mut self) {
        // x86_64 ABI guarantees SSE2 — no runtime detection needed.
        // The `bourne_no_simd` cfg disables the SIMD path; used by miri
        // (which doesn't model SSE2 intrinsics) and by curious users.
        #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
        // SAFETY: SSE2 is part of the x86_64 ABI baseline. Every x86_64
        // CPU has it; rustc's default target features include `+sse2`.
        // The intrinsics are `unsafe` by signature, not because we're
        // doing anything memory-unsafe — `_mm_loadu_si128` accepts
        // unaligned pointers and we walk only valid input bytes.
        unsafe {
            self.scan_ascii_string_run_sse2();
        }
        #[cfg(not(all(target_arch = "x86_64", not(bourne_no_simd))))]
        self.scan_ascii_string_run_scalar();
    }

    /// Scalar fallback for the ASCII string scan. Used on non-x86_64 targets
    /// and as the inner loop's tail when fewer than 16 bytes remain.
    #[inline]
    fn scan_ascii_string_run_scalar(&mut self) {
        let bytes = self.input;
        let mut i = self.offset;
        let end = bytes.len();
        while i < end {
            let b = bytes[i];
            if b == b'"' || b == b'\\' || !(0x20..0x80).contains(&b) {
                break;
            }
            i += 1;
        }
        self.offset = i;
    }

    /// SSE2-accelerated ASCII string scan. Walks 16 bytes at a time looking
    /// for the first "stop byte" (`"`, `\`, control char <0x20, or high-bit
    /// byte ≥0x80) and advances `self.offset` to it.
    ///
    /// Algorithm: load 16 bytes, build a 16-bit bitmask where bit `k` is set
    /// iff `bytes[i+k]` is a stop byte. If any bit is set, advance by the
    /// trailing-zero count to land on the first stop byte. Otherwise advance
    /// by 16 and continue.
    ///
    /// The "control or high-bit" test is fused into a single `cmplt_epi8`:
    /// reading bytes as `i8`, both `<0x20` (e.g. `0x05` = 5) and `≥0x80`
    /// (e.g. `0xC3` = -61) compare-less-than the constant 0x20. So one
    /// signed-compare instruction covers both stop categories.
    ///
    /// # Safety
    ///
    /// Only the SSE2 target feature is required. On `x86_64` it's part of the
    /// ABI baseline; the cfg gate at the call site enforces this.
    #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    unsafe fn scan_ascii_string_run_sse2(&mut self) {
        use core::arch::x86_64::{
            _mm_cmpeq_epi8, _mm_cmplt_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128,
            _mm_set1_epi8,
        };

        let bytes = self.input;
        let end = bytes.len();
        let mut i = self.offset;

        // Splat constants. The `as i8` casts wrap by design — SSE2 byte
        // compares are always signed at the silicon level; we want the bit
        // patterns for `"`, `\`, and 0x20 regardless of sign interpretation.
        let quote = _mm_set1_epi8(b'"' as i8);
        let backslash = _mm_set1_epi8(b'\\' as i8);
        // `cmplt_epi8(b, 0x20)` flags both `b<0x20` (controls) AND `b>=0x80`
        // (high-bit bytes interpreted as negative i8). One compare, two stops.
        let lt_threshold = _mm_set1_epi8(0x20_i8);

        while i + 16 <= end {
            // SAFETY: `i + 16 <= end` checked above; the pointer + 16 bytes
            // lie inside `bytes`. `_mm_loadu_si128` accepts unaligned addresses.
            let chunk = unsafe { _mm_loadu_si128(bytes.as_ptr().add(i).cast()) };
            let m_quote = _mm_cmpeq_epi8(chunk, quote);
            let m_back = _mm_cmpeq_epi8(chunk, backslash);
            let m_ctrl_or_hi = _mm_cmplt_epi8(chunk, lt_threshold);
            let mask = _mm_or_si128(_mm_or_si128(m_quote, m_back), m_ctrl_or_hi);
            // movemask returns i32 in [0, 0xFFFF]; cast to u32 is lossless.
            let bits = _mm_movemask_epi8(mask) as u32;
            if bits != 0 {
                i += bits.trailing_zeros() as usize;
                self.offset = i;
                return;
            }
            i += 16;
        }

        // Tail: scalar walk for the final <16 bytes.
        self.offset = i;
        self.scan_ascii_string_run_scalar();
    }

    #[inline]
    fn consume_utf8_multibyte(&mut self) -> Result<(), Error> {
        let leading = self
            .peek()
            .ok_or_else(|| self.err(ErrorKind::InvalidUtf8))?;

        let (extra, second_lo, second_hi) = match leading {
            0xC2..=0xDF => (1, 0x80, 0xBF),
            0xE0 => (2, 0xA0, 0xBF),
            0xE1..=0xEC | 0xEE..=0xEF => (2, 0x80, 0xBF),
            0xED => (2, 0x80, 0x9F),
            0xF0 => (3, 0x90, 0xBF),
            0xF1..=0xF3 => (3, 0x80, 0xBF),
            0xF4 => (3, 0x80, 0x8F),
            _ => return Err(self.err(ErrorKind::InvalidUtf8)),
        };
        self.bump();

        match self.peek() {
            Some(b) if b >= second_lo && b <= second_hi => self.bump(),
            _ => return Err(self.err(ErrorKind::InvalidUtf8)),
        }
        for _ in 1..extra {
            match self.peek() {
                Some(0x80..=0xBF) => self.bump(),
                _ => return Err(self.err(ErrorKind::InvalidUtf8)),
            }
        }
        Ok(())
    }

    fn scan_digit_run(&mut self) {
        let bytes = self.input;
        let mut i = self.offset;
        let end = bytes.len();
        while i < end {
            let b = bytes[i];
            if b.wrapping_sub(b'0') >= 10 {
                break;
            }
            i += 1;
        }
        self.offset = i;
    }

    pub(crate) fn skip_whitespace(&mut self) {
        let bytes = self.input;
        let mut i = self.offset;
        let end = bytes.len();
        while i < end {
            let b = bytes[i];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                i += 1;
            } else {
                break;
            }
        }
        self.offset = i;
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    pub(crate) fn bump(&mut self) {
        debug_assert!(self.offset < self.input.len());
        self.offset += 1;
    }

    pub(crate) const fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, compute_position(self.input, self.offset))
    }
}

#[allow(clippy::cast_possible_truncation)]
const fn compute_position(_input: &[u8], offset: usize) -> Position {
    Position::new(offset as u32)
}

fn validate_escapes(raw: &[u8]) -> Result<(), ErrorKind> {
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b == b'\\' {
            i += 1;
            if i >= raw.len() {
                return Err(ErrorKind::InvalidEscape);
            }
            match raw[i] {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 1,
                b'u' => {
                    if i + 5 > raw.len() {
                        return Err(ErrorKind::InvalidUnicodeEscape);
                    }
                    let cp = parse_hex4(&raw[i + 1..i + 5])?;
                    i += 5;
                    if (0xD800..=0xDBFF).contains(&cp) {
                        if i + 6 > raw.len() || raw[i] != b'\\' || raw[i + 1] != b'u' {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        let low = parse_hex4(&raw[i + 2..i + 6])?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        i += 6;
                    } else if (0xDC00..=0xDFFF).contains(&cp) {
                        return Err(ErrorKind::UnpairedSurrogate);
                    }
                }
                _ => return Err(ErrorKind::InvalidEscape),
            }
        } else if b < 0x20 {
            return Err(ErrorKind::ControlCharInString);
        } else {
            i += 1;
        }
    }
    Ok(())
}

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
