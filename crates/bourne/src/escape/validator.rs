//! Checking an escaped JSON string body without decoding it.

use super::Hex4;
use crate::ErrorKind;

/// Validates escape sequences in a string body.
///
/// This is the lexer's deferred pass, backing the `read_string`
/// contract: a returned `JsonStr` with `has_escapes() == true` has
/// well-formed escapes. The typed readers skip it because
/// `EscapeDecoder` performs the same checks while producing output —
/// see `Lexer::read_string_no_validate`.
pub struct EscapeValidator;

impl EscapeValidator {
    pub fn check(raw: &[u8]) -> Result<(), ErrorKind> {
        let mut i = 0;
        while i < raw.len() {
            let b = raw[i];
            if b == b'\\' {
                i = Self::check_escape(raw, i + 1)?;
            } else if b < 0x20 {
                return Err(ErrorKind::ControlCharInString);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Validate the escape whose introducing `\` sat at `i - 1`.
    /// Returns the index just past the sequence.
    fn check_escape(raw: &[u8], i: usize) -> Result<usize, ErrorKind> {
        let Some(&kind) = raw.get(i) else {
            return Err(ErrorKind::InvalidEscape);
        };
        match kind {
            b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => Ok(i + 1),
            b'u' => Self::check_unicode(raw, i),
            _ => Err(ErrorKind::InvalidEscape),
        }
    }

    /// Validate `\uXXXX` at `i` (pointing at the `u`), plus its low
    /// surrogate when the first code point is a high surrogate.
    fn check_unicode(raw: &[u8], i: usize) -> Result<usize, ErrorKind> {
        if i + 5 > raw.len() {
            return Err(ErrorKind::InvalidUnicodeEscape);
        }
        let cp = Hex4::read(&raw[i + 1..i + 5])?;
        let next = i + 5;
        if Hex4::is_low_surrogate(cp) {
            return Err(ErrorKind::UnpairedSurrogate);
        }
        if !Hex4::is_high_surrogate(cp) {
            return Ok(next);
        }
        if next + 6 > raw.len() || raw[next] != b'\\' || raw[next + 1] != b'u' {
            return Err(ErrorKind::UnpairedSurrogate);
        }
        if !Hex4::is_low_surrogate(Hex4::read(&raw[next + 2..next + 6])?) {
            return Err(ErrorKind::UnpairedSurrogate);
        }
        Ok(next + 6)
    }
}
