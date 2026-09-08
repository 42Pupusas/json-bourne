//! The `\uXXXX` hex-digit reader.

use crate::ErrorKind;

/// Reads the four hex digits of a `\uXXXX` escape into a code point.
///
/// One implementation for the decoder, the validator and the lexer;
/// the digit walk used to be copied into `lexer.rs` and `de.rs`
/// separately (audit 3.15).
pub struct Hex4;

impl Hex4 {
    /// Decode exactly four hex digits. `bytes` must be 4 long; callers
    /// bounds-check the slice before calling.
    pub const fn read(bytes: &[u8]) -> Result<u32, ErrorKind> {
        let mut v: u32 = 0;
        let mut i = 0;
        while i < bytes.len() {
            let d = match bytes[i] {
                b @ b'0'..=b'9' => b - b'0',
                b @ b'a'..=b'f' => b - b'a' + 10,
                b @ b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(ErrorKind::InvalidUnicodeEscape),
            };
            v = (v << 4) | d as u32;
            i += 1;
        }
        Ok(v)
    }

    pub const fn is_high_surrogate(cp: u32) -> bool {
        0xD800 <= cp && cp <= 0xDBFF
    }

    pub const fn is_low_surrogate(cp: u32) -> bool {
        0xDC00 <= cp && cp <= 0xDFFF
    }

    /// Combine a validated surrogate pair into its scalar value.
    ///
    /// Only decoding needs the scalar; the validator stops at
    /// "the pair is well-formed", so this is `alloc`-gated with its
    /// caller rather than left as dead code in `no_std` builds.
    #[cfg(feature = "alloc")]
    pub const fn combine(high: u32, low: u32) -> u32 {
        0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00)
    }
}
