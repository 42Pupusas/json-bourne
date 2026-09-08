//! A fixed-capacity stack buffer that satisfies [`fmt::Write`].
//!
//! [`ser::write_display`] formats `Display` values (IP and socket
//! addresses) before handing them to the string escape path. Every
//! adapter went through a heap `String` for this; the canonical forms
//! these types produce fit in a few dozen bytes, so the buffer lives
//! on the stack and the allocation disappears.

use core::fmt;

/// Longest form among the current callers is an IPv6 socket address:
/// `[ffff:ffff:ffff:ffff:ffff:ffff:255.255.255.255]:65535` = 56 bytes.
/// Rounded up so a future `Display` adapter for a similar type needs
/// no change here.
const DISPLAY_SCRATCH_CAP: usize = 64;

/// Fixed-capacity `fmt::Write` target. `write_str` drops the suffix
/// when the capacity is exhausted; callers format canonical
/// fixed-width types whose `Display` forms are known to fit.
// `pub` is capped to crate visibility by the `pub(crate)` module.
pub struct DisplayScratch {
    buf: [u8; DISPLAY_SCRATCH_CAP],
    len: usize,
}

impl DisplayScratch {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            buf: [0; DISPLAY_SCRATCH_CAP],
            len: 0,
        }
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        // `write_str` only stores `&str` bytes, so the filled prefix is
        // valid UTF-8.
        core::str::from_utf8(&self.buf[..self.len]).expect("filled prefix is valid UTF-8")
    }
}

impl fmt::Write for DisplayScratch {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let remaining = DISPLAY_SCRATCH_CAP - self.len;
        let take = s.len().min(remaining);
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayScratch;
    use core::fmt::Write as _;

    #[test]
    fn write_and_read_back() {
        let mut scratch = DisplayScratch::new();
        write!(scratch, "192.168.{}.{}", 0, 1).unwrap();
        assert_eq!(scratch.as_str(), "192.168.0.1");
    }

    #[test]
    fn multiple_writes_accumulate() {
        let mut scratch = DisplayScratch::new();
        write!(scratch, "[").unwrap();
        write!(scratch, "127.0.0.1").unwrap();
        write!(scratch, "]:8080").unwrap();
        assert_eq!(scratch.as_str(), "[127.0.0.1]:8080");
    }

    #[test]
    fn overflow_truncates_without_panicking() {
        let mut scratch = DisplayScratch::new();
        let long = "x".repeat(super::DISPLAY_SCRATCH_CAP + 16);
        write!(scratch, "{long}").unwrap();
        write!(scratch, "tail").unwrap();
        assert_eq!(scratch.as_str().len(), super::DISPLAY_SCRATCH_CAP);
        assert!(scratch.as_str().bytes().all(|b| b == b'x'));
    }

    #[test]
    fn exactly_full_is_accepted() {
        let mut scratch = DisplayScratch::new();
        let exact = "y".repeat(super::DISPLAY_SCRATCH_CAP);
        write!(scratch, "{exact}").unwrap();
        assert_eq!(scratch.as_str().len(), super::DISPLAY_SCRATCH_CAP);
    }
}
