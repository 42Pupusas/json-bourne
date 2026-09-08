//! JSON string escaping — the single implementation shared by every sink.
//!
//! One escape walk (`EscapeWriter`), one byte classifier (`needs_escape`),
//! and the shared LUTs. Sinks drive it through `JsonWrite`, so the walk
//! behaves as each sink needs: `StringSink`/`ByteSink`/`PrettyStringSink`
//! emit multi-byte UTF-8 through `write_str_raw`; char-oriented sinks like
//! `FmtWriteSink` stay byte-safe because the default
//! `JsonWrite::write_escaped_str` routes literal runs through
//! `write_str_raw` and only escape bytes through `write_byte` (audit 3.9).
//!
//! `de.rs` reuses `needs_escape` to split its escape decoding runs, the
//! inverse of what happens here.

use crate::JsonWrite;

const HEX_LOWER: [u8; 16] = *b"0123456789abcdef";

/// Escape sequences for bytes 0x00–0x1F plus `"` and `\`. `0` means
/// the byte passes through verbatim; otherwise the value is the ASCII
/// char after `\` (e.g. `b'n'` for `\n`).
const ESCAPE_TABLE: [u8; 256] = {
    let mut t = [0u8; 256];
    t[b'"' as usize] = b'"';
    t[b'\\' as usize] = b'\\';
    t[b'\n' as usize] = b'n';
    t[b'\r' as usize] = b'r';
    t[b'\t' as usize] = b't';
    t[0x08] = b'b';
    t[0x0C] = b'f';
    t
};

/// Write one byte of a string body, applying JSON escape rules.
fn write_escape_byte<W: JsonWrite + ?Sized>(w: &mut W, b: u8) -> Result<(), W::Error> {
    let esc = ESCAPE_TABLE[b as usize];
    if esc != 0 {
        w.write_byte(b'\\')?;
        return w.write_byte(esc);
    }
    if b < 0x20 {
        w.write_byte(b'\\')?;
        w.write_byte(b'u')?;
        w.write_byte(b'0')?;
        w.write_byte(b'0')?;
        w.write_byte(HEX_LOWER[(b >> 4) as usize])?;
        return w.write_byte(HEX_LOWER[(b & 0x0F) as usize]);
    }
    w.write_byte(b)
}

/// Returns true for bytes that need an escape sequence inside a JSON string
/// body (quote, backslash, or any control byte `< 0x20`). Everything else —
/// including high-bit UTF-8 continuation bytes — is safe to write verbatim.
///
/// All matching bytes are ASCII, so run boundaries derived from this
/// predicate are always `char` boundaries of the surrounding `&str`.
#[inline]
pub const fn needs_escape(b: u8) -> bool {
    b == b'"' || b == b'\\' || b < 0x20
}

/// Write `s` as a complete JSON string — quotes included — escaping
/// quote, backslash and control bytes, copying every other stretch as a
/// literal run through `write_str_raw`.
pub fn write_escaped<W: JsonWrite + ?Sized>(w: &mut W, s: &str) -> Result<(), W::Error> {
    w.write_byte(b'"')?;
    write_escaped_body(w, s)?;
    w.write_byte(b'"')
}

/// Write the *body* of a JSON string without the surrounding quotes.
///
/// Same walk as [`write_escaped`], minus the outer writes, for callers
/// that have already emitted the opening quote.
pub fn write_escaped_body<W: JsonWrite + ?Sized>(w: &mut W, s: &str) -> Result<(), W::Error> {
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if needs_escape(b) {
            if start < i {
                // Escape bytes are all ASCII, so `start` and `i` sit on
                // char boundaries and the stretch between them is valid
                // UTF-8 — multi-byte sequences stay intact inside one
                // `write_str_raw` copy.
                w.write_str_raw(&s[start..i])?;
            }
            write_escape_byte(w, b)?;
            start = i + 1;
        }
    }
    if start < bytes.len() {
        w.write_str_raw(&s[start..])?;
    }
    Ok(())
}
