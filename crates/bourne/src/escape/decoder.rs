//! Decoding an escaped JSON string body back into text.
//!
//! `EscapeDecoder` owns the walk; `EscapeSink` is the destination, so
//! the same walk serves the allocating `String` path and the `char`
//! path's fixed stack buffer.

use crate::ErrorKind;

/// Destination for decoded escape output — a `String` on the general
/// path, a fixed stack buffer on the `char` path.
///
/// `push_str` returns `Err(ErrorKind::TypeMismatch)` when the sink
/// cannot accept the whole run, so a bounded sink reports overflow
/// instead of silently dropping output.
pub trait EscapeSink {
    fn push(&mut self, c: char) -> Result<(), ErrorKind>;
    fn push_str(&mut self, s: &str) -> Result<(), ErrorKind>;
}

#[cfg(feature = "alloc")]
impl EscapeSink for alloc::string::String {
    #[inline]
    fn push(&mut self, c: char) -> Result<(), ErrorKind> {
        Self::push(self, c);
        Ok(())
    }

    #[inline]
    fn push_str(&mut self, s: &str) -> Result<(), ErrorKind> {
        Self::push_str(self, s);
        Ok(())
    }
}

/// Fixed-capacity scratch for decoding at most `N` UTF-8 bytes. A
/// decoded single scalar is ≤ 4 bytes, so the `char` path uses
/// `EscapeScratch<4>` instead of allocating a `String`.
pub struct EscapeScratch<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> EscapeScratch<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.buf[..self.len]).ok()
    }
}

impl<const N: usize> EscapeSink for EscapeScratch<N> {
    #[inline]
    fn push(&mut self, c: char) -> Result<(), ErrorKind> {
        let mut enc = [0u8; 4];
        self.push_str(c.encode_utf8(&mut enc))
    }

    #[inline]
    fn push_str(&mut self, s: &str) -> Result<(), ErrorKind> {
        // Overflow must be reported, not absorbed: a dropped run makes a
        // too-long string look like a short one, and the `char` reader's
        // length check then accepts it (e.g. "\u0041BCDE" decoding to
        // 'A'). The error kind matches what that caller reports for a
        // string that is not exactly one scalar.
        let end = self
            .len
            .checked_add(s.len())
            .ok_or(ErrorKind::TypeMismatch)?;
        let rest = self
            .buf
            .get_mut(self.len..end)
            .ok_or(ErrorKind::TypeMismatch)?;
        rest.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// The escape walk: literal runs copied wholesale, escape sequences
/// expanded one at a time.
pub struct EscapeDecoder;

impl EscapeDecoder {
    /// Decode a JSON string body into `dst`, expanding escape sequences.
    ///
    /// `raw` is the bytes between (but not including) the surrounding
    /// `"`s, produced by `Lexer::read_string_no_validate`. This is the
    /// *only* validator on that path: it covers every escape sequence,
    /// every hex digit (via `Hex4`), and every surrogate pairing — so
    /// the lexer can skip the redundant `EscapeValidator` walk.
    ///
    /// Output is always valid UTF-8: non-escape bytes are validated by
    /// the lexer's inline UTF-8 walk, and `\u`-derived bytes come from
    /// `encode_utf8` on a checked `char`.
    ///
    /// # Safety justification for the `unsafe` block
    ///
    /// The literal-byte path uses `core::str::from_utf8_unchecked`. The
    /// invariant: every byte in `raw` reached this function via the
    /// lexer, which validates UTF-8 inline against the RFC 3629 byte
    /// ranges as it scans (`Parser::consume_utf8_multibyte` and
    /// `scan_ascii_string_run`). The bytes between escapes are therefore
    /// valid UTF-8 by construction — re-validating them in safe code is
    /// the `from_utf8` re-walk that perf showed at ~12% of total time
    /// (the audit on 2026-05-19 measured the safe variant at 1.68×
    /// slower, pushing json-bourne below `serde_json` on the
    /// escape-heavy workload). The crate's `unsafe_code = "deny"` is
    /// overridden here with `#[allow]`, mirroring the localized
    /// exception the lexer makes at the equivalent site.
    #[allow(unsafe_code)]
    pub fn decode<S: EscapeSink>(raw: &[u8], dst: &mut S) -> Result<(), ErrorKind> {
        let mut i = 0;
        while i < raw.len() {
            if raw[i] != b'\\' {
                // Literal byte run: find the next `\` (or end) and append
                // the whole stretch in one push. This is the hot path for
                // strings with sparse escapes (most production payloads),
                // and uses a SIMD scan where available — the scalar walk
                // that used to live here was 64% of total decode time.
                let start = i;
                i = Self::find_backslash(&raw[i..]).map_or(raw.len(), |off| i + off);
                // SAFETY: see the method-level comment. The lexer
                // validated these bytes as UTF-8 inline.
                let chunk = unsafe { core::str::from_utf8_unchecked(&raw[start..i]) };
                dst.push_str(chunk)?;
                continue;
            }
            if i + 1 >= raw.len() {
                return Err(ErrorKind::InvalidEscape);
            }
            if raw[i + 1] == b'u' {
                let (ch, last) = Self::unicode_escape(raw, i + 1)?;
                dst.push(ch)?;
                i = last + 1;
            } else {
                dst.push(Self::simple_escape(raw[i + 1])?)?;
                i += 2;
            }
        }
        Ok(())
    }

    /// Decode a single non-`u` JSON escape byte to its char.
    #[inline]
    const fn simple_escape(b: u8) -> Result<char, ErrorKind> {
        Ok(match b {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000C}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            _ => return Err(ErrorKind::InvalidEscape),
        })
    }

    /// Decode `\uXXXX` (and optionally a surrogate-pair `\uYYYY`) into a
    /// single `char`. `i` points at the `u` of the first escape. Returns
    /// the char and the index of the last hex digit consumed.
    fn unicode_escape(raw: &[u8], i: usize) -> Result<(char, usize), ErrorKind> {
        use super::Hex4;

        if i + 5 > raw.len() {
            return Err(ErrorKind::InvalidUnicodeEscape);
        }
        let cp = Hex4::read(&raw[i + 1..i + 5])?;
        let new_i = i + 4;
        if Hex4::is_low_surrogate(cp) {
            return Err(ErrorKind::UnpairedSurrogate);
        }
        if Hex4::is_high_surrogate(cp) {
            if new_i + 7 > raw.len() || raw[new_i + 1] != b'\\' || raw[new_i + 2] != b'u' {
                return Err(ErrorKind::UnpairedSurrogate);
            }
            let low = Hex4::read(&raw[new_i + 3..new_i + 7])?;
            if !Hex4::is_low_surrogate(low) {
                return Err(ErrorKind::UnpairedSurrogate);
            }
            let ch =
                char::from_u32(Hex4::combine(cp, low)).ok_or(ErrorKind::InvalidUnicodeEscape)?;
            return Ok((ch, new_i + 6));
        }
        let ch = char::from_u32(cp).ok_or(ErrorKind::InvalidUnicodeEscape)?;
        Ok((ch, new_i))
    }

    /// SIMD-accelerated single-byte search for `\` inside a slice.
    ///
    /// Why this exists: the literal-byte run inside `decode` is the inner
    /// loop on strings with sparse escapes (~1 escape per 50 bytes is
    /// typical for production payloads). On profile, the scalar
    /// `while i < n && bytes[i] != b'\\'` walk was 64% of the owned
    /// decode's time. SSE2's `_mm_cmpeq_epi8` + `_mm_movemask_epi8`
    /// walks 16 bytes per iteration with the same correctness; on
    /// `x86_64` the gain is ~10× for long literal runs.
    #[inline]
    fn find_backslash(bytes: &[u8]) -> Option<usize> {
        // Two distinct cfg-gated bodies — splitting them per arch avoids
        // a `return` inside one cfg branch (clippy's `needless_return`)
        // while keeping each arm a single expression. `bourne_no_simd`
        // disables the SIMD path (used by miri).
        #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
        {
            Self::find_backslash_sse2(bytes)
        }
        #[cfg(not(all(target_arch = "x86_64", not(bourne_no_simd))))]
        {
            bytes.iter().position(|&b| b == b'\\')
        }
    }

    /// Same `unsafe_code` justification as `decode`: SSE2 is part of the
    /// `x86_64` ABI baseline, the `target_feature` arm is statically
    /// enabled there, and the unsafe is mechanical (intrinsics carry
    /// `unsafe` by signature, not by memory-safety).
    #[cfg(all(target_arch = "x86_64", not(bourne_no_simd)))]
    #[allow(unsafe_code, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    #[inline]
    fn find_backslash_sse2(bytes: &[u8]) -> Option<usize> {
        use core::arch::x86_64::{
            _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
        };

        let n = bytes.len();
        let mut i = 0;
        // SAFETY: SSE2 is part of the x86_64 ABI baseline; rustc's default
        // target features include `+sse2`, so the intrinsics are statically
        // available. `_mm_loadu_si128` is documented as accepting unaligned
        // addresses, and the bounds check `i + 16 <= n` ensures the load
        // stays inside `bytes`.
        unsafe {
            let backslash = _mm_set1_epi8(b'\\' as i8);
            while i + 16 <= n {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
                let m = _mm_cmpeq_epi8(chunk, backslash);
                let bits = _mm_movemask_epi8(m) as u32;
                if bits != 0 {
                    return Some(i + bits.trailing_zeros() as usize);
                }
                i += 16;
            }
        }
        // Tail: scalar walk for the final <16 bytes.
        bytes[i..]
            .iter()
            .position(|&b| b == b'\\')
            .map(|off| i + off)
    }
}
