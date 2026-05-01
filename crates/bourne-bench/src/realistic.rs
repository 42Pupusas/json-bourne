//! Realistic JSON corpora.
//!
//! Each generator produces a document shaped like real production input
//! (logs, geo, GitHub events, JWT-id-heavy lists, varied-length strings).
//! The point isn't perfect fidelity — it's coverage of shapes the existing
//! uniform fixtures miss:
//!   - heterogeneous values per record
//!   - mixed string lengths (a 12-byte uniform corpus tells us nothing
//!     about scaling)
//!   - non-ASCII / unicode-escape bytes
//!   - heavy escape density
//!   - both wide and narrow integers in the same document

use core::fmt::Write as _;

extern crate alloc;
use alloc::string::String;

/// Array of N "GitHub event"-shaped objects. Each is ~400-600 bytes:
/// ID, action verb, ISO-8601 timestamp, two long URLs (~80 chars each),
/// a short user object, an optional payload bag with 3-5 fields. Mimics
/// the heterogeneity of `api.github.com/events`. Some records carry an
/// extra field, some don't, so the parser sees the optional-field path.
#[must_use]
pub fn github_event_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 480);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // Mix of widths in `id`: cycle through 5-digit, 10-digit, 19-digit.
        let id: u64 = match i % 3 {
            0 => 12345 + i as u64,
            1 => 4_000_000_000 + i as u64,
            _ => 9_223_372_036_000_000_000 + (i as u64 % 1000),
        };
        let action = match i % 4 {
            0 => "opened",
            1 => "closed",
            2 => "synchronize",
            _ => "review_requested",
        };
        let payload_extra = if i % 3 == 0 {
            r#","sender":{"id":1,"login":"alice","type":"User"}"#
        } else {
            ""
        };
        let _ = write!(
            &mut s,
            r#"{{"id":{id},"type":"PullRequestEvent","actor":{{"id":42,"login":"alice"}},"repo":{{"id":99,"name":"alice/example"}},"created_at":"2026-04-30T12:00:00Z","payload":{{"action":"{action}","number":{i},"pull_request":{{"id":{id},"url":"https://api.github.com/repos/alice/example/pulls/{i}","html_url":"https://github.com/alice/example/pull/{i}","title":"Fix bug in module {i}"}}{payload_extra}}}}}"#,
        );
    }
    s.push(']');
    s
}

/// Array of N "log line"-shaped objects: timestamp, level, message,
/// fields bag. Messages are 50-200 bytes (medium strings — past the SIMD
/// chunk boundary). `fields` is a small object with 3-5 entries.
#[must_use]
pub fn log_line_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 220);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let level = ["debug", "info", "warn", "error"][i % 4];
        // Vary message length: cycle short / medium / long.
        let msg = match i % 3 {
            0 => alloc::format!("processed request id={i} status=ok"),
            1 => alloc::format!(
                "user alice@example.com performed action update on resource id={i} duration_ms=42 trace_id=abcdef0123456789"
            ),
            _ => alloc::format!(
                "WARNING: retry budget exhausted for downstream service x after {i} attempts; backoff active for 30s, see runbook /runbooks/x-circuit-breaker for diagnosis steps and recovery procedures"
            ),
        };
        let _ = write!(
            &mut s,
            r#"{{"ts":"2026-04-30T12:00:00.123456Z","level":"{level}","msg":"{msg}","fields":{{"req_id":"{i}","trace_id":"abcdef0123456789","span_id":"deadbeef01234567","host":"node-{}"}}}}"#,
            i % 32,
        );
    }
    s.push(']');
    s
}

/// Array of N coordinate pairs: `[[lat, lng], ...]`. All f64s with a
/// fraction. The current `floats_decode` bench uses 5 fixed forms; this
/// one varies the magnitude over realistic ranges (-90..90, -180..180).
#[must_use]
pub fn geo_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 32);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // Pseudo-distribute to stress the float decoder across magnitudes.
        let lat = (i % 180) as f64 / 2.0 - 89.999;
        let lng = (i % 360) as f64 - 179.999;
        let _ = write!(&mut s, "[{lat:.6},{lng:.6}]");
    }
    s.push(']');
    s
}

/// Array of N JWT-shaped objects: numeric-id-heavy. Each entry has a
/// 19-digit `sub` (subject) and a 13-digit `exp` (ms timestamp), plus
/// short string fields. **This is the corpus where SIMD digit scanning
/// should win** — we did not have it before and that absence drove a
/// possibly-wrong revert.
#[must_use]
pub fn jwt_id_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 90);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let sub: u64 = 9_000_000_000_000_000_000 + (i as u64 % 100_000);
        let exp: u64 = 1_700_000_000_000 + (i as u64 % 86400 * 1000);
        let iat: u64 = exp - 3600_000;
        let _ = write!(
            &mut s,
            r#"{{"sub":{sub},"iss":"https://auth.example.com","aud":"api","exp":{exp},"iat":{iat},"jti":"abcdef0123456789"}}"#,
        );
    }
    s.push(']');
    s
}

/// Length-distribution buckets for [`mixed_length_string_array`]. Each
/// row is `(weight_out_of_20, min_len, max_len)` for one bucket. Lengths
/// are bytes of string body, not counting the surrounding quotes.
///
/// Buckets reflect what we see in real API responses:
///   - 50% short identifiers (IDs, status codes, enum-like values)
///   - 30% medium values (paths, names, short URLs)
///   - 15% description-length text (sentences, error messages)
///   -  5% long-form payloads (logs containing serialized state, base64
///     blobs, embedded JSON-in-string). The ceiling here is deliberately
///     up to 64 KB — production JSON regularly has fields this size and
///     a 4 KB ceiling under-represents that reality.
const MIXED_LEN_BUCKETS: &[(u32, usize, usize)] = &[
    (10, 4, 15),
    (6, 20, 80),
    (3, 100, 400),
    (1, 1024, 65_536),
];

/// Body source for `mixed_length_string_array`. Repeated until it covers
/// the largest declared bucket, so bucket bounds and source size stay
/// independent — change one and the other adapts.
const STRING_BODY_SOURCE: &str =
    "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-_/.+,;:!? ";

/// Array of N strings with a realistic length distribution. Bucket
/// weights and bounds live in [`MIXED_LEN_BUCKETS`].
///
/// The original `string_array` is all 12 bytes, which under-tests both
/// the SIMD chunk-scan (everything fits one chunk) and the long-string
/// path (nothing exceeds it). This corpus exposes both, and pushes the
/// long-form bucket up to 64 KB so the parser actually sees the kind
/// of payload-bearing string fields we encounter in production.
///
/// Output is deterministic: lengths within each bucket are driven by a
/// fixed multiply-mix of the index, so two runs with the same `n`
/// produce identical bytes.
#[must_use]
pub fn mixed_length_string_array(n: usize) -> String {
    debug_assert_eq!(
        MIXED_LEN_BUCKETS.iter().map(|(w, _, _)| w).sum::<u32>(),
        20,
        "MIXED_LEN_BUCKETS weights must sum to 20",
    );

    // Build a body buffer at least as long as the largest bucket. This
    // makes the slicing below trivially in-bounds regardless of which
    // bucket runs hottest.
    let max_len = MIXED_LEN_BUCKETS
        .iter()
        .map(|(_, _, m)| *m)
        .max()
        .expect("at least one bucket");
    let reps = max_len.div_ceil(STRING_BODY_SOURCE.len()).max(1);
    let body: String = STRING_BODY_SOURCE.repeat(reps);
    debug_assert!(body.len() >= max_len);

    // Average bytes/element for capacity hint. Rough — overshoot is fine.
    let avg: usize = MIXED_LEN_BUCKETS
        .iter()
        .map(|(w, lo, hi)| (*w as usize) * (lo + hi) / 2)
        .sum::<usize>()
        / 20
        + 3;

    let mut s = String::with_capacity(n * avg + 2);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // Walk cumulative weights to pick a bucket from `i % 20`.
        let mut slot = (i as u32) % 20;
        let (lo, hi) = MIXED_LEN_BUCKETS
            .iter()
            .find_map(|(w, lo, hi)| {
                if slot < *w {
                    Some((*lo, *hi))
                } else {
                    slot -= *w;
                    None
                }
            })
            .expect("weights sum to 20, so some bucket always matches");
        // Deterministic length within [lo, hi]. Knuth multiplicative hash.
        let span = hi - lo + 1;
        let len = lo + i.wrapping_mul(2_654_435_761) % span;
        s.push('"');
        s.push_str(&body[..len]);
        s.push('"');
    }
    s.push(']');
    s
}

/// Array of N strings with non-ASCII bytes. Cycles through Latin-1
/// (2-byte UTF-8), Cyrillic (2-byte), CJK (3-byte), and emoji (4-byte)
/// so the multi-byte UTF-8 path gets coverage at every length. Currently
/// **zero benches exercise `consume_utf8_multibyte`.**
#[must_use]
pub fn unicode_string_array(n: usize) -> String {
    // Pre-built bodies: each is ~50 bytes of mixed-width UTF-8 plus an ASCII tail.
    // Strings are unescaped — the lexer's UTF-8 validation runs on raw bytes.
    let bodies = [
        "café résumé naïve façade — Latin-1 chars throughout the line",
        "привет мир здравствуй земля — Cyrillic two-byte run with ascii tail",
        "你好世界今天天气很好 — three-byte CJK with mixed ASCII suffix",
        "🦀💯🚀 emoji line with four-byte sequences and ASCII text 🌍",
    ];
    let mut s = String::with_capacity(n * 80);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(&mut s, r#""{}""#, bodies[i % bodies.len()]);
    }
    s.push(']');
    s
}

/// Array of N strings with heavy escape density: ~10 escapes per string,
/// mix of `\n`, `\t`, `\"`, `\\`, and `\u00XX`. Real-world JSON-in-JSON
/// (logs containing serialized payloads) looks like this.
#[must_use]
pub fn escape_heavy_string_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 80);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        // 10 escapes: \n \t \" \\ A \r \/ \b \f é
        let _ = write!(
            &mut s,
            r#""line {i}\n\ttab\"quote\\backAslash\rcr\/slash\bbackspace\ffféaccent""#,
        );
    }
    s.push(']');
    s
}
