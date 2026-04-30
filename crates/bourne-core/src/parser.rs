use crate::error::{Error, ErrorKind, Position};
use crate::event::{Event, JsonNum, JsonStr};

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
    /// Inside an object, just emitted a key, expecting `:`.
    ObjectColon,
    /// Inside an object, expecting a value (after `:`).
    ObjectValue,
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
    pos: Position,
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

    const fn push(&mut self, frame: Frame, pos: Position) -> Result<(), Error> {
        if self.len >= MAX_DEPTH {
            return Err(Error::new(ErrorKind::DepthLimitExceeded, pos));
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
    pub const fn new(input: &'input [u8]) -> Self {
        Self {
            input,
            pos: Position::START,
            state: State::Start,
            stack: Stack::new(),
        }
    }

    pub const fn position(&self) -> Position {
        self.pos
    }

    pub fn next_event(&mut self) -> Result<Option<Event<'input>>, Error> {
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
                        self.bump();
                        self.state = State::ArrayValueOrEnd;
                        // Reject trailing comma: `,]` is invalid.
                        self.skip_whitespace();
                        if self.peek() == Some(b']') {
                            return Err(self.err(ErrorKind::UnexpectedByte(b']')));
                        }
                        continue;
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
                        self.bump();
                        self.state = State::ObjectValue;
                        continue;
                    }
                    Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
                    None => Err(self.err(ErrorKind::UnexpectedEof)),
                },
                State::ObjectValue => {
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

    fn close_container(&mut self, expected: Frame) -> Result<Event<'input>, Error> {
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

    fn parse_value(&mut self) -> Result<Event<'input>, Error> {
        let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
        match b {
            b'{' => {
                self.bump();
                self.stack.push(Frame::Object, self.pos)?;
                self.state = State::ObjectKeyOrEnd;
                Ok(Event::StartObject)
            }
            b'[' => {
                self.bump();
                self.stack.push(Frame::Array, self.pos)?;
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

    fn parse_keyword(&mut self, kw: &[u8], event: Event<'input>) -> Result<Event<'input>, Error> {
        for &expected in kw {
            match self.peek() {
                Some(b) if b == expected => self.bump(),
                Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
        Ok(event)
    }

    fn parse_string(&mut self) -> Result<JsonStr<'input>, Error> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.bump(); // opening quote
        let start = self.pos.offset;
        let mut has_escapes = false;
        loop {
            let b = self.peek().ok_or_else(|| self.err(ErrorKind::UnexpectedEof))?;
            match b {
                b'"' => {
                    let raw = &self.input[start..self.pos.offset];
                    self.bump(); // closing quote
                    if !has_escapes {
                        // Borrowed path: bytes must be valid UTF-8.
                        if core::str::from_utf8(raw).is_err() {
                            return Err(self.err(ErrorKind::InvalidUtf8));
                        }
                    } else {
                        // Validate escape syntax now; full decode is the
                        // consumer's job (into a caller-provided buffer,
                        // which the streaming layer doesn't own).
                        validate_escapes(raw)
                            .map_err(|kind| Error::new(kind, self.pos))?;
                    }
                    return Ok(JsonStr::new(raw, has_escapes));
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
                _ => self.bump(),
            }
        }
    }

    fn parse_number(&mut self) -> Result<JsonNum<'input>, Error> {
        let start = self.pos.offset;
        let start_pos = self.pos;

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
                while let Some(b'0'..=b'9') = self.peek() {
                    self.bump();
                }
            }
            Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
            None => return Err(self.err(ErrorKind::UnexpectedEof)),
        }

        // optional fraction
        if self.peek() == Some(b'.') {
            self.bump();
            let frac_start = self.pos.offset;
            while let Some(b'0'..=b'9') = self.peek() {
                self.bump();
            }
            if self.pos.offset == frac_start {
                return Err(self.err(ErrorKind::InvalidNumber));
            }
        }

        // optional exponent
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let exp_start = self.pos.offset;
            while let Some(b'0'..=b'9') = self.peek() {
                self.bump();
            }
            if self.pos.offset == exp_start {
                return Err(self.err(ErrorKind::InvalidNumber));
            }
        }

        let raw = &self.input[start..self.pos.offset];
        Ok(JsonNum::new(raw, start_pos))
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.bump(),
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos.offset).copied()
    }

    fn bump(&mut self) {
        if let Some(b) = self.peek() {
            self.pos.advance(b);
        }
    }

    const fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.pos)
    }
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
