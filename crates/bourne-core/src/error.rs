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
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedByte(b) => write!(f, "unexpected byte 0x{b:02x}"),
            Self::UnexpectedEof => f.write_str("unexpected end of input"),
            Self::InvalidEscape => f.write_str("invalid string escape"),
            Self::InvalidUnicodeEscape => f.write_str("invalid \\u escape"),
            Self::UnpairedSurrogate => f.write_str("unpaired UTF-16 surrogate in \\u escape"),
            Self::InvalidUtf8 => f.write_str("invalid UTF-8"),
            Self::InvalidNumber => f.write_str("invalid number literal"),
            Self::NumberOutOfRange => f.write_str("number does not fit target type"),
            Self::ControlCharInString => f.write_str("control character in string literal"),
            Self::TrailingData => f.write_str("trailing data after JSON value"),
            Self::DepthLimitExceeded => f.write_str("nesting depth limit exceeded"),
            Self::ExpectedValue => f.write_str("expected JSON value"),
            Self::ExpectedString => f.write_str("expected string"),
            Self::ExpectedNumber => f.write_str("expected number"),
            Self::ExpectedBool => f.write_str("expected boolean"),
            Self::ExpectedNull => f.write_str("expected null"),
            Self::ExpectedArray => f.write_str("expected array"),
            Self::ExpectedObject => f.write_str("expected object"),
            Self::TypeMismatch => f.write_str("type mismatch"),
            Self::DuplicateKey => f.write_str("duplicate object key"),
            Self::MissingField => f.write_str("missing required field"),
            Self::UnknownField => f.write_str("unknown field"),
        }
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
