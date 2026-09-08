use crate::error::{Error, ErrorKind};
use crate::event::{Event, JsonStr};
use crate::lexer::{DEFAULT_MAX_DEPTH, Frame, Lexer};

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
    /// Inside an object, just emitted a key, expecting `:`. The streaming
    /// fast-path fuses `:` consumption with the value parse so this state
    /// rarely surfaces in `next_event`.
    ObjectColon,
    /// Inside an object, after `:`, expecting a value. Used by the
    /// fast-path object API as a handoff point when the caller falls back
    /// to `next_event` for a single value.
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
/// `Parser` is the streaming API. It owns a [`Lexer`] (the byte walker) plus
/// a small grammar state machine that enforces JSON's "value, then comma,
/// then value" structure when emitting events serially. Typed consumers
/// (`json_bourne::FromJson`) drive the lexer directly and bypass this state
/// machine — for them, the type's recursive structure already enforces the
/// grammar, and the dispatch through `match self.state` is pure overhead.
///
/// `MAX_DEPTH` is the maximum nesting depth of containers the parser will
/// accept. It is also the size of the inline nesting stack, so picking a
/// small value reduces the parser's stack footprint as well as bounding
/// untrusted input. The default is [`DEFAULT_MAX_DEPTH`].
#[derive(Debug)]
pub struct Parser<'input, const MAX_DEPTH: usize = DEFAULT_MAX_DEPTH> {
    lex: Lexer<'input, MAX_DEPTH>,
    state: State,
}

impl<'input, const MAX_DEPTH: usize> Parser<'input, MAX_DEPTH> {
    /// Construct a parser over `input`.
    ///
    /// # Panics
    ///
    /// Panics if `input.len()` exceeds `MAX_INPUT_LEN`. See
    /// [`Lexer::new`].
    #[must_use]
    pub const fn new(input: &'input [u8]) -> Self {
        Self {
            lex: Lexer::new(input),
            state: State::Start,
        }
    }

    /// Borrow the underlying lexer mutably. Typed consumers use this to
    /// drive parsing directly without going through the `next_event`
    /// state machine.
    pub const fn lexer(&mut self) -> &mut Lexer<'input, MAX_DEPTH> {
        &mut self.lex
    }

    #[must_use]
    pub const fn position(&self) -> crate::error::Position {
        self.lex.position()
    }

    /// The input slice the parser was constructed with.
    #[must_use]
    pub const fn input(&self) -> &'input [u8] {
        self.lex.input()
    }

    pub fn next_event(&mut self) -> Result<Option<Event>, Error> {
        self.lex.skip_whitespace();
        match self.state {
            State::Start => self.ev_start(),
            State::DocumentEnd => self.ev_document_end(),
            State::ArrayValueOrEnd => self.ev_array_value_or_end(),
            State::ArrayCommaOrEnd => self.ev_array_comma_or_end(),
            State::ObjectKeyOrEnd => self.ev_object_key_or_end(),
            State::ObjectColon => self.ev_object_colon(),
            State::ObjectValue => self.ev_object_value(),
            State::ObjectCommaOrEnd => self.ev_object_comma_or_end(),
        }
    }

    fn ev_start(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
            Some(_) => {
                let ev = self.lex.read_value()?;
                self.state = state_after_value(&ev, State::DocumentEnd);
                Ok(Some(ev))
            }
        }
    }

    fn ev_document_end(&self) -> Result<Option<Event>, Error> {
        if self.lex.peek().is_some() {
            Err(self.lex.err(ErrorKind::TrailingData))
        } else {
            Ok(None)
        }
    }

    fn ev_array_value_or_end(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            Some(b']') => {
                self.lex.bump();
                Ok(Some(self.close_container(Frame::Array)?))
            }
            Some(_) => {
                let ev = self.lex.read_value()?;
                self.state = state_after_value(&ev, State::ArrayCommaOrEnd);
                Ok(Some(ev))
            }
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
        }
    }

    fn ev_array_comma_or_end(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            Some(b',') => {
                self.lex.bump();
                self.lex.skip_whitespace();
                if self.lex.peek() == Some(b']') {
                    return Err(self.lex.err(ErrorKind::UnexpectedByte(b']')));
                }
                let ev = self.lex.read_value()?;
                self.state = state_after_value(&ev, State::ArrayCommaOrEnd);
                Ok(Some(ev))
            }
            Some(b']') => {
                self.lex.bump();
                Ok(Some(self.close_container(Frame::Array)?))
            }
            Some(b) => Err(self.lex.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
        }
    }

    fn ev_object_key_or_end(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            Some(b'}') => {
                self.lex.bump();
                Ok(Some(self.close_container(Frame::Object)?))
            }
            Some(b'"') => {
                let s = self.lex.read_string()?;
                self.state = State::ObjectColon;
                Ok(Some(Event::Key(s)))
            }
            Some(b) => Err(self.lex.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
        }
    }

    fn ev_object_colon(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            Some(b':') => {
                self.lex.bump();
                self.lex.skip_whitespace();
                let ev = self.lex.read_value()?;
                self.state = state_after_value(&ev, State::ObjectCommaOrEnd);
                Ok(Some(ev))
            }
            Some(b) => Err(self.lex.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
        }
    }

    fn ev_object_value(&mut self) -> Result<Option<Event>, Error> {
        let ev = self.lex.read_value()?;
        self.state = state_after_value(&ev, State::ObjectCommaOrEnd);
        Ok(Some(ev))
    }

    fn ev_object_comma_or_end(&mut self) -> Result<Option<Event>, Error> {
        match self.lex.peek() {
            Some(b',') => {
                self.lex.bump();
                self.lex.skip_whitespace();
                if self.lex.peek() == Some(b'}') {
                    return Err(self.lex.err(ErrorKind::UnexpectedByte(b'}')));
                }
                self.ev_object_key_or_end()
            }
            Some(b'}') => {
                self.lex.bump();
                self.close_container(Frame::Object).map(Some)
            }
            Some(b) => Err(self.lex.err(ErrorKind::UnexpectedByte(b))),
            None => Err(self.lex.err(ErrorKind::UnexpectedEof)),
        }
    }

    fn close_container(&mut self, expected: Frame) -> Result<Event, Error> {
        self.lex.pop_frame(expected)?;
        self.state = self.state_value_consumed();
        Ok(match expected {
            Frame::Array => Event::EndArray,
            Frame::Object => Event::EndObject,
        })
    }

    // -----------------------------------------------------------------
    // Fast-path methods for typed consumers.
    //
    // These mirror the methods on `Lexer` but additionally synchronize
    // `self.state` so a subsequent `next_event` call resumes correctly.
    // Typed consumers that drive the lexer directly via `Parser::lexer()`
    // skip these and avoid the state writes entirely.
    // -----------------------------------------------------------------

    /// State once a value — scalar or fully-closed container — is
    /// consumed at the current nesting level: what may follow depends
    /// only on the enclosing frame.
    const fn state_value_consumed(&self) -> State {
        match self.lex.stack.top() {
            None => State::DocumentEnd,
            Some(Frame::Array) => State::ArrayCommaOrEnd,
            Some(Frame::Object) => State::ObjectCommaOrEnd,
        }
    }

    pub fn parse_i64_value(&mut self) -> Result<i64, Error> {
        let v = self.lex.parse_i64_value()?;
        self.state = self.state_value_consumed();
        Ok(v)
    }

    pub fn parse_str_value(&mut self) -> Result<&'input str, Error> {
        let s = self.lex.parse_str_value()?;
        self.state = self.state_value_consumed();
        Ok(s)
    }

    /// On `[` push a frame and, for empty arrays, pop and synchronize state.
    pub fn array_start(&mut self) -> Result<bool, Error> {
        let empty = self.lex.array_start()?;
        if empty {
            self.state = self.state_value_consumed();
        } else {
            self.state = State::ArrayValueOrEnd;
        }
        Ok(empty)
    }

    pub fn array_continue(&mut self, end_byte: u8) -> Result<bool, Error> {
        let closed = self.lex.array_continue(end_byte)?;
        if closed {
            self.state = self.state_value_consumed();
        }
        Ok(closed)
    }

    pub fn object_first_key(&mut self) -> Result<Option<&'input str>, Error> {
        let key = self.lex.object_first_key()?;
        self.state = match key {
            None => self.state_value_consumed(),
            Some(_) => State::ObjectValue,
        };
        Ok(key)
    }

    pub fn object_next_key(&mut self) -> Result<Option<&'input str>, Error> {
        let key = self.lex.object_next_key()?;
        self.state = match key {
            None => self.state_value_consumed(),
            Some(_) => State::ObjectValue,
        };
        Ok(key)
    }

    /// Like [`object_first_key`], but returns the key as a [`JsonStr`]
    /// span so the caller can decode escapes when present. The fast
    /// `&str`-returning variant rejects any backslash; this one carries
    /// escape-bearing keys through.
    ///
    /// [`object_first_key`]: Self::object_first_key
    pub fn object_first_key_lex(&mut self) -> Result<Option<JsonStr>, Error> {
        let key = self.lex.object_first_key_lex()?;
        self.state = match key {
            None => self.state_value_consumed(),
            Some(_) => State::ObjectValue,
        };
        Ok(key)
    }

    /// Like [`object_next_key`], but returns the key as a [`JsonStr`]
    /// span. See [`object_first_key_lex`].
    ///
    /// [`object_next_key`]: Self::object_next_key
    /// [`object_first_key_lex`]: Self::object_first_key_lex
    pub fn object_next_key_lex(&mut self) -> Result<Option<JsonStr>, Error> {
        let key = self.lex.object_next_key_lex()?;
        self.state = match key {
            None => self.state_value_consumed(),
            Some(_) => State::ObjectValue,
        };
        Ok(key)
    }
}

#[inline]
const fn state_after_value(ev: &Event, scalar_next: State) -> State {
    match ev {
        Event::StartObject => State::ObjectKeyOrEnd,
        Event::StartArray => State::ArrayValueOrEnd,
        _ => scalar_next,
    }
}
