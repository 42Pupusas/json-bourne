use core::fmt;

/// Byte offset into the input where an event was emitted or an error fired.
///
/// `Position` used to carry `(offset, line, column)` (16 bytes), which
/// pushed `Result<Option<Event>, Error>` to 24 bytes — too big to return
/// in two GP registers, forcing every `next_event` call to spill through
/// stack memory. Now it's just an offset (4 bytes); line and column are
/// reconstructed on demand by `Position::resolve(input)`, which scans
/// `input[..offset]` for newlines. The scan only fires at error sites
/// or when the consumer explicitly asks for them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub offset: u32,
}

/// Line and column reconstructed from a `Position` against its input.
///
/// 1-based on both axes (matches the convention used by most editors).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

impl Position {
    pub const START: Self = Self { offset: 0 };

    #[must_use]
    pub const fn new(offset: u32) -> Self {
        Self { offset }
    }

    /// Compute 1-based line and column by scanning the input up to this
    /// position. O(offset) — only call when actually rendering an error.
    #[must_use]
    pub fn resolve(&self, input: &[u8]) -> LineColumn {
        let upto = core::cmp::min(self.offset as usize, input.len());
        let mut line: u32 = 1;
        let mut last_newline: usize = 0;
        let mut had_newline = false;
        let mut i = 0;
        while i < upto {
            if input[i] == b'\n' {
                line += 1;
                last_newline = i + 1;
                had_newline = true;
            }
            i += 1;
        }
        let col_start = if had_newline { last_newline } else { 0 };
        let column = u32::try_from(upto - col_start + 1).unwrap_or(u32::MAX);
        LineColumn { line, column }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte {}", self.offset)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    UnexpectedByte(u8),
    UnexpectedEof,
    InvalidEscape,
    /// The escape is valid; the caller's key type (`&str`) cannot hold a
    /// decoded key, which requires ownership. Use `String` or `Cow<str>`.
    BorrowedKeyNeedsDecode,
    InvalidUnicodeEscape,
    UnpairedSurrogate,
    InvalidUtf8,
    InvalidNumber,
    NumberOutOfRange,
    ControlCharInString,
    TrailingData,
    DepthLimitExceeded,
    ExpectedValue,
    ExpectedString,
    ExpectedNumber,
    ExpectedBool,
    ExpectedNull,
    ExpectedArray,
    ExpectedObject,
    TypeMismatch,
    DuplicateKey,
    MissingField,
    UnknownField,
    NonFiniteFloat,
    /// Input exceeds [`crate::MAX_INPUT_LEN`] (~2 GB) — a representational
    /// limit of the packed-offset `Event`, not a document problem.
    InputTooLarge,
}

impl ErrorKind {
    /// Lexer / parser–level errors.
    #[must_use]
    const fn lexical_msg(self) -> Option<&'static str> {
        let msg = match self {
            Self::UnexpectedByte(_) => "unexpected byte",
            Self::UnexpectedEof => "unexpected end of input",
            Self::InvalidEscape => "invalid string escape",
            Self::BorrowedKeyNeedsDecode => {
                "key contains an escape: borrow-only key type cannot represent it; \
                 use String or Cow<str>"
            }
            Self::InvalidUnicodeEscape => "invalid \\u escape",
            Self::UnpairedSurrogate => "unpaired UTF-16 surrogate in \\u escape",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::InvalidNumber => "invalid number literal",
            Self::NumberOutOfRange => "number does not fit target type",
            Self::ControlCharInString => "control character in string literal",
            Self::TrailingData => "trailing data after JSON value",
            Self::DepthLimitExceeded => "nesting depth limit exceeded",
            _ => return None,
        };
        Some(msg)
    }

    /// Static human-readable message for every variant except
    /// `UnexpectedByte`, which is formatted dynamically by `Display`.
    #[must_use]
    const fn static_msg(self) -> &'static str {
        if let Some(s) = self.lexical_msg() {
            return s;
        }
        match self {
            Self::ExpectedValue => "expected JSON value",
            Self::ExpectedString => "expected string",
            Self::ExpectedNumber => "expected number",
            Self::ExpectedBool => "expected boolean",
            Self::ExpectedNull => "expected null",
            Self::ExpectedArray => "expected array",
            Self::ExpectedObject => "expected object",
            Self::TypeMismatch => "type mismatch",
            Self::DuplicateKey => "duplicate object key",
            Self::MissingField => "missing required field",
            Self::UnknownField => "unknown field",
            Self::NonFiniteFloat => "non-finite float not representable in JSON",
            Self::InputTooLarge => "input exceeds the maximum supported JSON document size",
            _ => "error",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::UnexpectedByte(b) = self {
            return write!(f, "unexpected byte 0x{b:02x}");
        }
        f.write_str(self.static_msg())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub position: Position,
}

impl Error {
    #[must_use]
    pub const fn new(kind: ErrorKind, position: Position) -> Self {
        Self { kind, position }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.position)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
