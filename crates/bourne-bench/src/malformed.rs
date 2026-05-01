//! Malformed and adversarial JSON inputs.
//!
//! Each entry is a `(name, bytes, expected_kind)` triple. The bench
//! harness asserts that every input is rejected (no panics, no UB) and
//! measures rejection latency. A parser that takes 100× longer to reject
//! malformed input than to accept valid input is a `DoS` vector — slow
//! rejection is itself a vulnerability.
//!
//! The expected `ErrorKind` is part of the fixture so this doubles as a
//! regression suite: if a future change causes a different kind of error
//! at a different byte offset, the bench panics with a clear message.
//! Drop the assertion entirely if you want pure throughput.

use bourne_core::ErrorKind;

/// One malformed input plus the expected error kind. Position is checked
/// loosely (any error at all is acceptable for some inputs — see flags).
pub struct Bad {
    pub name: &'static str,
    pub bytes: &'static [u8],
    /// Expected error kind. Set to `None` for inputs where multiple kinds
    /// are equally valid (e.g. truncations) — the bench just asserts Err.
    pub kind: Option<ErrorKind>,
}

impl core::fmt::Debug for Bad {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bad").field("name", &self.name).finish_non_exhaustive()
    }
}

/// The full corpus. Adding cases is cheap; the bench iterates the slice.
pub const CORPUS: &[Bad] = &[
    // -- truncation: cursor falls off the end mid-token --
    Bad {
        name: "truncated_string",
        bytes: br#""abc"#,
        kind: Some(ErrorKind::UnexpectedEof),
    },
    Bad {
        name: "truncated_array_open",
        bytes: b"[",
        kind: Some(ErrorKind::UnexpectedEof),
    },
    Bad {
        name: "truncated_object_after_key",
        bytes: br#"{"k""#,
        kind: None,
    },
    Bad {
        name: "truncated_object_after_colon",
        bytes: br#"{"k":"#,
        kind: Some(ErrorKind::UnexpectedEof),
    },
    Bad {
        name: "truncated_keyword",
        bytes: b"tru",
        kind: Some(ErrorKind::UnexpectedEof),
    },
    Bad {
        name: "truncated_unicode_escape",
        bytes: br#""\u00"#,
        kind: None,
    },
    // -- escape errors --
    Bad {
        name: "invalid_escape",
        bytes: br#""\q""#,
        kind: Some(ErrorKind::InvalidEscape),
    },
    Bad {
        name: "lone_high_surrogate",
        bytes: br#""\uD800""#,
        kind: Some(ErrorKind::UnpairedSurrogate),
    },
    Bad {
        name: "lone_low_surrogate",
        bytes: br#""\uDC00""#,
        kind: Some(ErrorKind::UnpairedSurrogate),
    },
    Bad {
        name: "high_surrogate_then_non_surrogate",
        bytes: br#""\uD800A""#,
        kind: Some(ErrorKind::UnpairedSurrogate),
    },
    Bad {
        // Lexer rejects the first non-hex byte with UnexpectedByte(b'Z')
        // during the inline `\u` walk. `InvalidUnicodeEscape` only fires
        // from the deferred `validate_escapes` pass, which runs only on
        // strings the inline pass accepted as well-formed escapes. Either
        // rejection is correct; pin to the actual one.
        name: "non_hex_in_unicode_escape",
        bytes: br#""\u00ZZ""#,
        kind: Some(ErrorKind::UnexpectedByte(b'Z')),
    },
    // -- string body errors --
    Bad {
        name: "control_char_in_string",
        bytes: b"\"a\nb\"",
        kind: Some(ErrorKind::ControlCharInString),
    },
    // -- UTF-8 violations (lexer must reject inline) --
    Bad {
        // 0xC0 is an overlong encoding leading byte — RFC 3629 forbids.
        name: "utf8_overlong_2byte",
        bytes: b"\"\xC0\x80\"",
        kind: Some(ErrorKind::InvalidUtf8),
    },
    Bad {
        // 0xED 0xA0 0x80 is the UTF-8 encoding of U+D800 (a surrogate),
        // forbidden by RFC 3629.
        name: "utf8_surrogate_3byte",
        bytes: b"\"\xED\xA0\x80\"",
        kind: Some(ErrorKind::InvalidUtf8),
    },
    Bad {
        // F4 is legal as a 4-byte leader, but the second byte must be
        // 0x80..=0x8F to keep the codepoint <= U+10FFFF. 0x90 here exceeds.
        name: "utf8_above_max_codepoint",
        bytes: b"\"\xF4\x90\x80\x80\"",
        kind: Some(ErrorKind::InvalidUtf8),
    },
    Bad {
        // Lone continuation byte without a leader.
        name: "utf8_lone_continuation",
        bytes: b"\"\x80\"",
        kind: Some(ErrorKind::InvalidUtf8),
    },
    // -- number errors --
    Bad {
        name: "leading_zero",
        bytes: b"01",
        kind: Some(ErrorKind::TrailingData),
    },
    Bad {
        name: "bare_minus",
        bytes: b"-",
        kind: Some(ErrorKind::UnexpectedEof),
    },
    Bad {
        name: "minus_no_digits",
        bytes: b"-x",
        kind: Some(ErrorKind::UnexpectedByte(b'x')),
    },
    Bad {
        name: "fraction_no_digits",
        bytes: b"1.",
        kind: Some(ErrorKind::InvalidNumber),
    },
    Bad {
        name: "exponent_no_digits",
        bytes: b"1e",
        kind: Some(ErrorKind::InvalidNumber),
    },
    // -- structural errors --
    Bad {
        name: "trailing_data",
        bytes: b"null x",
        kind: Some(ErrorKind::TrailingData),
    },
    Bad {
        name: "trailing_comma_array",
        bytes: b"[1,]",
        kind: Some(ErrorKind::UnexpectedByte(b']')),
    },
    Bad {
        name: "trailing_comma_object",
        bytes: br#"{"a":1,}"#,
        kind: Some(ErrorKind::UnexpectedByte(b'}')),
    },
    Bad {
        name: "missing_colon",
        bytes: br#"{"a"1}"#,
        kind: Some(ErrorKind::UnexpectedByte(b'1')),
    },
    Bad {
        name: "object_with_non_string_key",
        bytes: br"{1:2}",
        kind: Some(ErrorKind::UnexpectedByte(b'1')),
    },
    Bad {
        name: "mismatched_close_paren",
        bytes: b"[1,2}",
        kind: None,
    },
    // -- depth bomb just past the limit --
    // 129 levels of `[` then `1` then 129 `]`: parser's DEFAULT_MAX_DEPTH
    // is 128, so this should reject with DepthLimitExceeded.
    Bad {
        name: "depth_bomb_129",
        bytes: DEPTH_BOMB_129,
        kind: Some(ErrorKind::DepthLimitExceeded),
    },
];

const DEPTH_BOMB_129: &[u8] = const {
    // 129 opens, '1', 129 closes.
    let mut buf = [0u8; 129 + 1 + 129];
    let mut i = 0;
    while i < 129 {
        buf[i] = b'[';
        i += 1;
    }
    buf[129] = b'1';
    let mut j = 0;
    while j < 129 {
        buf[130 + j] = b']';
        j += 1;
    }
    // SAFETY: leak as 'static via const promotion of the byte buffer.
    // Have to go via a static binding.
    &{ buf } // yields &'static [u8; N]
};
