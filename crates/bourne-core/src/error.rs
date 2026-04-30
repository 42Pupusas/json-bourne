use core::fmt;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub offset: usize,
    pub line: u32,
    pub column: u32,
}

impl Position {
    pub const START: Self = Self { offset: 0, line: 1, column: 1 };

    pub(crate) const fn advance(&mut self, byte: u8) {
        self.offset += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {} column {} (byte {})", self.line, self.column, self.offset)
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
