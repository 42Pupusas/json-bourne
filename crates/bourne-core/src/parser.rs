use crate::error::{Error, ErrorKind, Position};
use crate::event::{Event, JsonNum, JsonStr, MAX_INPUT_LEN};

/// Default maximum container nesting depth.
///
/// Guards against pathological inputs (e.g. millions of `[`s) that would
/// otherwise drive recursive consumers into stack overflow. Override at
/// compile time by parameterizing [`Parser`] with a different `MAX_DEPTH`.
pub const DEFAULT_MAX_DEPTH: usize = 128;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Frame {
    Array,
    Object,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    /// Document start — expecting a value, no events emitted yet.
    Start,
    /// Just emitted a value at the document root — only whitespace + EOF allowed.
    DocumentEnd,
    /// Inside an array, expecting either a value or `]`.
    ArrayValueOrEnd,
    /// Inside an array, just emitted a value, expecting `,` or `]`.
    ArrayCommaOrEnd,
    /// Inside an object, expecting either a key or `}`.
    ObjectKeyOrEnd,
    /// Inside an object, just emitted a key, expecting `:`. The arm fuses
    /// `:` consumption with the value parse, so we never observe a separate
    /// "expecting value" state — that's why there's no `ObjectValue` here.
    ObjectColon,
    /// Inside an object, just emitted a value, expecting `,` or `}`.
    ObjectCommaOrEnd,
}

/// Streaming JSON parser over a borrowed byte slice.
///
/// Pull-based: each call to [`Parser::next_event`] advances the input and
/// returns one [`Event`]. Returns `Ok(None)` once the document has been fully
/// consumed (and any trailing whitespace has been validated).
///
/// `MAX_DEPTH` is the maximum nesting depth of containers the parser will
/// accept. It is also the size of the inline nesting stack, so picking a
/// small value reduces the parser's stack footprint as well as bounding
/// untrusted input. The default is [`DEFAULT_MAX_DEPTH`].
#[derive(Debug)]
pub struct Parser<'input, const MAX_DEPTH: usize = DEFAULT_MAX_DEPTH> {
    input: &'input [u8],
    /// Byte offset of the next-to-read byte. Line and column are reconstructed
    /// lazily from `input[..offset]` whenever a `Position` is needed (errors,
    /// public `position()` calls). The hot path never touches them, which is
    /// what lets `bump()` compile to a single increment.
    offset: usize,
    state: State,
    stack: Stack<MAX_DEPTH>,
}

/// Fixed-capacity nesting stack. Avoids `alloc` for the parser itself.
///
/// Capacity is fixed at compile time by `Parser`'s `MAX_DEPTH` parameter,
/// so the inline array sized to it is also the depth bound.
#[derive(Debug)]
struct Stack<const MAX_DEPTH: usize> {
    frames: [Frame; MAX_DEPTH],
    len: usize,
}

impl<const MAX_DEPTH: usize> Stack<MAX_DEPTH> {
    const fn new() -> Self {
        Self {
            frames: [Frame::Array; MAX_DEPTH],
            len: 0,
        }
    }

    const fn push(&mut self, frame: Frame) -> Result<(), ()> {
        if self.len >= MAX_DEPTH {
            return Err(());
        }
        self.frames[self.len] = frame;
        self.len += 1;
        Ok(())
    }

    const fn pop(&mut self) -> Option<Frame> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            Some(self.frames[self.len])
        }
    }

    const fn top(&self) -> Option<Frame> {
        if self.len == 0 {
            None
        } else {
            Some(self.frames[self.len - 1])
        }
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<'input, const MAX_DEPTH: usize> Parser<'input, MAX_DEPTH> {
    /// Construct a parser over `input`.
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
            state: State::Start,
            stack: Stack::new(),
        }
    }

    #[must_use]
    pub const fn position(&self) -> Position {
        compute_position(self.input, self.offset)
    }

    /// The input slice the parser was constructed with. Consumers use this
    /// to materialize `&str`/`&[u8]` from `JsonStr` and `JsonNum`, which
    /// store offsets rather than fat pointers (so the `Event` enum fits in
    /// one xmm register).
    #[must_use]
    pub const fn input(&self) -> &'input [u8] {
        self.input
    }

    // The state machine is intrinsically large — splitting it would scatter
    // dispatch across one private fn per state and obscure the grammar walk.
    #[allow(clippy::too_many_lines)]
    pub fn next_event(&mut self) -> Result<Option<Event>, Error> {
        loop {
            self.skip_whitespace();
            return match self.state {
                State::Start => match self.peek() {
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                    Some(_) => {
                        let ev = self.parse_value()?;
                        // If the value opened a container, we stay inside it;
                        // otherwise the document is done.
                        if self.stack.is_empty() {
                            self.state = State::DocumentEnd;
                        }
                        Ok(Some(ev))
                    }
                },
                State::DocumentEnd => {
                    if self.peek().is_some() {
                        Err(self.err(ErrorKind::TrailingData))
                    } else {
                        Ok(None)
                    }
                }
                State::ArrayValueOrEnd => match self.peek() {
                    Some(b']') => {
                        self.bump();
                        Ok(Some(self.close_container(Frame::Array)?))
                    }
                    Some(_) => {
                        let ev = self.parse_value()?;
                        if matches!(
                            ev,
                            Event::String(_)
                                | Event::Number(_)
                                | Event::Bool(_)
                                | Event::Null
                                | Event::EndArray
                                | Event::EndObject
                        ) {
                            // Scalar (or just-closed container) → expect `,` or `]` next.
                            self.state = State::ArrayCommaOrEnd;
                        }
                        Ok(Some(ev))
                    }
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
                State::ArrayCommaOrEnd => match self.peek() {
                    Some(b',') => {
                        // After `,` inside an array we already know the next
                        // token must be a value (or an error for `,]`). Skip
                        // the loop-continue + state-table re-dispatch and go
                        // straight to parse_value here. For integer-array
                        // workloads this collapses 6+ jumps per element into
                        // a single fused path.
                        self.bump();
                        self.skip_whitespace();
                        if self.peek() == Some(b']') {
                            return Err(self.err(ErrorKind::UnexpectedByte(b']')));
                        }
                        let ev = self.parse_value()?;
                        if matches!(
                            ev,
                            Event::String(_)
                                | Event::Number(_)
                                | Event::Bool(_)
                                | Event::Null
                                | Event::EndArray
                                | Event::EndObject
                        ) {
                            self.state = State::ArrayCommaOrEnd;
                        }
                        Ok(Some(ev))
                    }
                    Some(b']') => {
                        self.bump();
                        Ok(Some(self.close_container(Frame::Array)?))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
                State::ObjectKeyOrEnd => match self.peek() {
                    Some(b'}') => {
                        self.bump();
                        Ok(Some(self.close_container(Frame::Object)?))
                    }
                    Some(b'"') => {
                        let s = self.parse_string()?;
                        self.state = State::ObjectColon;
                        Ok(Some(Event::Key(s)))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
                State::ObjectColon => match self.peek() {
                    Some(b':') => {
                        // Same fusion as ArrayCommaOrEnd's `,` arm: after `:`
                        // we know the next event is a value, so parse it here
                        // instead of falling back into the state-table loop.
                        self.bump();
                        self.skip_whitespace();
                        let ev = self.parse_value()?;
                        if matches!(
                            ev,
                            Event::String(_)
                                | Event::Number(_)
                                | Event::Bool(_)
                                | Event::Null
                                | Event::EndArray
                                | Event::EndObject
                        ) {
                            self.state = State::ObjectCommaOrEnd;
                        }
                        Ok(Some(ev))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
                State::ObjectCommaOrEnd => match self.peek() {
                    Some(b',') => {
                        self.bump();
                        // After `,` inside an object, only a key is valid (no trailing comma).
                        self.state = State::ObjectKeyOrEnd;
                        // But `,` followed by `}` is *not* allowed, so we need
                        // to actually require a key here, not "key or end".
                        // Re-enter the loop; ObjectKeyOrEnd will reject `}` for us
                        // by... actually it accepts `}`. Fix: distinguish the two.
                        // We use ObjectColon-style trick: peek ahead.
                        self.skip_whitespace();
                        if self.peek() == Some(b'}') {
                            return Err(self.err(ErrorKind::UnexpectedByte(b'}')));
                        }
                        continue;
                    }
                    Some(b'}') => {
                        self.bump();
                        Ok(Some(self.close_container(Frame::Object)?))
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
            };
        }
    }

    fn close_container(&mut self, expected: Frame) -> Result<Event, Error> {
        let popped = self.stack.pop().ok_or_else(|| self.err(ErrorKind::UnexpectedByte(b']')))?;
        if popped != expected {
            return Err(self.err(ErrorKind::TypeMismatch));
        }
        self.state = match self.stack.top() {
            None => State::DocumentEnd,
            Some(Frame::Array) => State::ArrayCommaOrEnd,
            Some(Frame::Object) => State::ObjectCommaOrEnd,
        };
        Ok(match expected {
            Frame::Array => Event::EndArray,
            Frame::Object => Event::EndObject,
        })
    }

    #[inline]
    fn parse_value(&mut self) -> Result<Event, Error> {
        let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
        match b {
            b'{' => {
                self.bump();
                self.push_frame(Frame::Object)?;
                self.state = State::ObjectKeyOrEnd;
                Ok(Event::StartObject)
            }
            b'[' => {
                self.bump();
                self.push_frame(Frame::Array)?;
                self.state = State::ArrayValueOrEnd;
                Ok(Event::StartArray)
            }
            b'"' => self.parse_string().map(Event::String),
            b't' => self.parse_keyword(b"true", Event::Bool(true)),
            b'f' => self.parse_keyword(b"false", Event::Bool(false)),
            b'n' => self.parse_keyword(b"null", Event::Null),
            b'-' | b'0'..=b'9' => self.parse_number().map(Event::Number),
            other => Err(self.err(ErrorKind::UnexpectedByte(other))),
        }
    }

    fn push_frame(&mut self, frame: Frame) -> Result<(), Error> {
        self.stack
            .push(frame)
            .map_err(|()| self.err(ErrorKind::DepthLimitExceeded))
    }

    fn parse_keyword(&mut self, kw: &[u8], event: Event) -> Result<Event, Error> {
        for &expected in kw {
            match self.peek() {
                Some(b) if b == expected => self.bump(),
                Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
        Ok(event)
    }

    fn parse_string(&mut self) -> Result<JsonStr, Error> {
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
                    if has_escapes {
                        // Escape syntax (and the surrogate pairing rules) is
                        // not part of the byte-walk above; validate it once
                        // here. Full decode is still the consumer's job.
                        let raw = &self.input[start..end];
                        validate_escapes(raw)
                            .map_err(|kind| self.err(kind))?;
                    }
                    // The JsonStr just stores offsets; consumers materialize
                    // a `&str`/`&[u8]` lazily via `as_str(input)`. The
                    // unsafe `from_utf8_unchecked` lives there now (with
                    // the same RFC 3629 inline-validation safety argument).
                    // `Parser::new` capped input at MAX_INPUT_LEN, so the
                    // offsets fit in 31 bits and the `as u32` is lossless.
                    #[allow(clippy::cast_possible_truncation)]
                    return Ok(JsonStr::new(start as u32, end as u32, has_escapes));
                }
                b'\\' => {
                    has_escapes = true;
                    self.bump();
                    // Consume one escape so the closing quote isn't mistaken.
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
                0x20..=0x7F => {
                    // Plain ASCII inside the string — scan the run all at once
                    // so the inner loop is just a memchr-style "find next
                    // interesting byte" rather than per-byte bump/peek.
                    self.scan_ascii_string_run();
                }
                // High-bit byte: start of a multi-byte UTF-8 sequence.
                // Consume the leading byte and the correct number of
                // continuation bytes, validating each as we go. Avoids the
                // separate `from_utf8` pass over the whole string at the end.
                _ => self.consume_utf8_multibyte()?,
            }
        }
    }

    /// Advance past a run of plain ASCII bytes (0x20..=0x7F minus `"` and `\`).
    ///
    /// Stops when the next byte is `"`, `\`, a control char (<0x20), or a
    /// high-bit byte. The caller's outer loop then dispatches on what we left.
    fn scan_ascii_string_run(&mut self) {
        // Stop conditions: `"` (0x22), `\` (0x5C), control char (<0x20),
        // or high-bit byte (>=0x80). Frame this as "subtract 0x20, then
        // any byte that lands outside 0x00..=0x5F (i.e. in 0x60..) is
        // either a high-bit byte or a control char (which wrapped through
        // 0). Combined with the two specific-byte checks, the per-byte
        // test compiles to two compares + one mask, no table lookup."
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

    /// Consume a UTF-8 multi-byte sequence starting at the current position.
    ///
    /// Called only when `peek()` has returned a byte >= 0x80. Validates
    /// length-encoding against the byte ranges allowed by RFC 3629
    /// (no overlongs, no surrogates, no codepoints above U+10FFFF) so
    /// the resulting slice is guaranteed valid UTF-8.
    fn consume_utf8_multibyte(&mut self) -> Result<(), Error> {
        // SAFETY of correctness: this mirrors the table in RFC 3629
        // section 4 — same bounds, same rejected ranges. Codepoints
        // outside U+10FFFF and the surrogate range U+D800..=U+DFFF are
        // rejected by the per-byte range checks below.
        let leading = self
            .peek()
            .ok_or_else(|| self.err(ErrorKind::InvalidUtf8))?;

        // Per RFC 3629: legal leading byte ranges and the ranges their
        // continuations may take.
        let (extra, second_lo, second_hi) = match leading {
            0xC2..=0xDF => (1, 0x80, 0xBF),
            0xE0 => (2, 0xA0, 0xBF),       // disallow overlong 3-byte
            0xE1..=0xEC | 0xEE..=0xEF => (2, 0x80, 0xBF),
            0xED => (2, 0x80, 0x9F),       // exclude surrogates
            0xF0 => (3, 0x90, 0xBF),       // disallow overlong 4-byte
            0xF1..=0xF3 => (3, 0x80, 0xBF),
            0xF4 => (3, 0x80, 0x8F),       // cap at U+10FFFF
            _ => return Err(self.err(ErrorKind::InvalidUtf8)),
        };
        self.bump();

        // Second byte has tighter bounds; remaining bytes are plain 0x80..=0xBF.
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

    #[inline]
    fn parse_number(&mut self) -> Result<JsonNum, Error> {
        let start = self.offset;

        if self.peek() == Some(b'-') {
            self.bump();
        }

        // integer part
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

        // optional fraction
        if self.peek() == Some(b'.') {
            self.bump();
            let frac_start = self.offset;
            self.scan_digit_run();
            if self.offset == frac_start {
                return Err(self.err(ErrorKind::InvalidNumber));
            }
        }

        // optional exponent
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

        // Same `as u32` lossless argument as parse_string — see Parser::new.
        #[allow(clippy::cast_possible_truncation)]
        let result = JsonNum::new(start as u32, self.offset as u32);
        Ok(result)
    }

    /// Advance over a run of ASCII digits without per-byte position tracking.
    ///
    /// This is the inner loop of integer-array parsing. By scanning a slice
    /// directly and writing `self.offset` once at the end, the per-byte
    /// store-pos.offset / store-pos.column traffic that dominated the old
    /// `peek/bump` version is eliminated.
    fn scan_digit_run(&mut self) {
        let bytes = self.input;
        let mut i = self.offset;
        let end = bytes.len();
        while i < end {
            let b = bytes[i];
            // ASCII-digit fast path: `b - b'0' < 10` only for digits.
            if b.wrapping_sub(b'0') >= 10 {
                break;
            }
            i += 1;
        }
        self.offset = i;
    }

    fn skip_whitespace(&mut self) {
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

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    /// Advance one byte. Caller must have verified there is one (e.g. via a
    /// prior `peek()` returning `Some`). The hot path only ever calls this
    /// after a successful peek, so the bounds check is redundant — keep it
    /// as a debug assertion to catch any future caller mistakes without
    /// paying for it in release.
    fn bump(&mut self) {
        debug_assert!(self.offset < self.input.len());
        self.offset += 1;
    }

    const fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, compute_position(self.input, self.offset))
    }
}

/// Build a `Position` for a given byte offset.
///
/// Trivial now — the position is just the offset. Line/column are
/// reconstructed lazily via `Position::resolve(input)` only when an error
/// is actually rendered or the consumer explicitly asks. Capping input
/// length at `Parser::new` means the `as u32` is lossless.
#[allow(clippy::cast_possible_truncation)]
const fn compute_position(_input: &[u8], offset: usize) -> Position {
    Position::new(offset as u32)
}

/// Validate that all `\` escapes inside `raw` are well-formed.
///
/// Walks the slice without decoding into an output buffer. Surrogate
/// pairing rules (high surrogate must be followed by low surrogate) are
/// enforced here so consumers don't need to redo the work.
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
                        // High surrogate — must be followed by \uDC00..=\uDFFF.
                        if i + 6 > raw.len() || raw[i] != b'\\' || raw[i + 1] != b'u' {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        let low = parse_hex4(&raw[i + 2..i + 6])?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(ErrorKind::UnpairedSurrogate);
                        }
                        i += 6;
                    } else if (0xDC00..=0xDFFF).contains(&cp) {
                        // Lone low surrogate.
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
