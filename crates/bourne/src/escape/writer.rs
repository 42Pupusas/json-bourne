//! JSON string escaping — the single implementation shared by every sink.
//!
//! One escape walk, one byte classifier (`needs_escape`), and the shared
//! LUTs. Sinks drive it through `JsonWrite`, so the walk behaves as each
//! sink needs: `StringSink`/`ByteSink`/`PrettyStringSink` emit multi-byte
//! UTF-8 through `write_str_raw`; char-oriented sinks like `FmtWriteSink`
//! stay byte-safe because the default `JsonWrite::write_escaped_str`
//! routes literal runs through `write_str_raw` and only escape bytes
//! through `write_byte` (audit 3.9).

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

/// Offset of the first byte in `bytes` that [`needs_escape`], if any.
///
/// On `x86_64` a 16-byte SSE2 compare walks the run — one vector load
/// and two compares (`"`, `\`) plus an unsigned `< 0x20` test folded as
/// `_mm_cmpeq_epi8(_mm_and_si128(v, 0xE0), 0)` trick-free `min` compare
/// — per 16 bytes, replacing the per-byte scalar walk that dominated
/// profiles of escape-sparse strings (audit 4.5.1; same shape as
/// `de.rs`'s `find_backslash`).
#[inline]
pub fn find_escape(bytes: &[u8]) -> Option<usize> {
    #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
    {
        find_escape_sse2(bytes)
    }
    #[cfg(not(all(target_arch = "x86_64", not(bourne_no_simd))))]
    {
        bytes.iter().position(|&b| needs_escape(b))
    }
}

#[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
#[allow(unsafe_code, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
// the dispatch wrapper inlines it away; coverage can't attribute the body.
#[allow(unknown_lints, crappy)]
#[inline]
fn find_escape_sse2(bytes: &[u8]) -> Option<usize> {
    use core::arch::x86_64::{
        _mm_cmpeq_epi8, _mm_loadu_si128, _mm_min_epu8, _mm_movemask_epi8, _mm_or_si128,
        _mm_set1_epi8,
    };

    let n = bytes.len();
    let mut i = 0;
    // SAFETY: SSE2 is part of the x86_64 ABI baseline; `_mm_loadu_si128`
    // accepts unaligned addresses, and `i + 16 <= n` keeps the load in
    // bounds.
    unsafe {
        let quote = _mm_set1_epi8(b'"' as i8);
        let backslash = _mm_set1_epi8(b'\\' as i8);
        // Control bytes are < 0x20: unsigned min with 0x1F equals the
        // byte itself only when it is ≤ 0x1F, so comparing the min back
        // against the chunk tests the whole class.
        let ctrl = _mm_set1_epi8(0x1F);
        while i + 16 <= n {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
            let is_quote = _mm_cmpeq_epi8(chunk, quote);
            let is_backslash = _mm_cmpeq_epi8(chunk, backslash);
            let is_ctrl = _mm_cmpeq_epi8(_mm_min_epu8(chunk, ctrl), chunk);
            let mask =
                _mm_movemask_epi8(_mm_or_si128(_mm_or_si128(is_quote, is_backslash), is_ctrl))
                    as u32;
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 16;
        }
    }
    // Tail: scalar walk for the final <16 bytes.
    bytes[i..]
        .iter()
        .position(|&b| needs_escape(b))
        .map(|off| i + off)
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
/// that have already emitted the opening quote. Runs of safe bytes are
/// located with [`find_escape`] (SSE2 on `x86_64`) rather than a per-byte
/// scan, then copied in one `write_str_raw` each.
pub fn write_escaped_body<W: JsonWrite + ?Sized>(w: &mut W, s: &str) -> Result<(), W::Error> {
    let bytes = s.as_bytes();
    let mut start = 0;
    while let Some(off) = find_escape(&bytes[start..]) {
        let i = start + off;
        // Escape bytes are all ASCII, so `start` and `i` sit on char
        // boundaries and the stretch between them is valid UTF-8 —
        // multi-byte sequences stay intact inside one `write_str_raw`.
        if start < i {
            w.write_str_raw(&s[start..i])?;
        }
        write_escape_byte(w, bytes[i])?;
        start = i + 1;
    }
    if start < bytes.len() {
        w.write_str_raw(&s[start..])?;
    }
    Ok(())
}
