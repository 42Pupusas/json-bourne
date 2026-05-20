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

/// Array of N "GitHub event"-shaped objects.
///
/// Each is ~400-600 bytes:
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

/// Array of N coordinate pairs: `[[lat, lng], ...]`.
///
/// All f64s with a
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

/// Array of N JWT-shaped objects: numeric-id-heavy.
///
/// Each entry has a
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
        let iat: u64 = exp - 3_600_000;
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
///      blobs, embedded JSON-in-string). The ceiling here is deliberately
///      up to 64 KB — production JSON regularly has fields this size and
///      a 4 KB ceiling under-represents that reality.
const MIXED_LEN_BUCKETS: &[(u32, usize, usize)] =
    &[(10, 4, 15), (6, 20, 80), (3, 100, 400), (1, 1024, 65_536)];

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
///
/// # Panics
///
/// Cannot panic in practice. The internal `.expect("at least one
/// bucket")` and `.expect("weights sum to 20")` guard invariants that
/// hold for the static `MIXED_LEN_BUCKETS` table — they exist to fail
/// loudly if the table is ever edited inconsistently, not because real
/// inputs reach them.
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

/// Same length distribution as [`mixed_length_string_array`], but each
/// string carries roughly one escape per 50 bytes of body.
///
/// Escapes cycle
/// through the cheap two-byte forms (`\n`, `\t`, `\"`, `\\`) and the
/// six-byte unicode form (`é`) so the parser sees both fast-path
/// escape decoding and the slow `\u`-validation path.
///
/// Why this matters: `serde_json`'s borrowed `&str` deserialization only
/// works when the source has no escapes. The existing
/// `mixed_length_string_array` corpus has zero escapes, so the bourne-vs-
/// `serde_json` head-to-head only covers the no-escape fast path. This
/// variant forces both libraries off that path: bourne owns the bytes
/// after `validate_escapes`, `serde_json` falls back to building a `Cow`
/// per element. That's where most production string fields actually live.
///
/// Bucket lengths refer to the JSON-text body length (between the quotes),
/// not the decoded string length. An escape sequence counts as its
/// on-the-wire byte width.
///
/// # Panics
///
/// Cannot panic in practice. The internal `.expect("at least one
/// bucket")` and `.expect("weights sum to 20")` guard invariants that
/// hold for the static `MIXED_LEN_BUCKETS` table \u2014 they exist to fail
/// loudly if the table is ever edited inconsistently, not because real
/// inputs reach them.
#[must_use]
pub fn mixed_length_string_array_with_escapes(n: usize) -> String {
    // Five escape sequences cycled in order: four 2-byte forms plus one
    // 6-byte `\u00XX` form. Inserted every ~50 bytes of output, so density
    // is ~5-6%. The `\u` form forces the lexer through the deferred
    // `validate_escapes` slow path that the simple `\n`-etc. escapes skip.
    const ESCAPES: [&str; 5] = [r"\n", r"\t", r#"\""#, r"\\", r"\u00e9"];
    const STRIDE: usize = 50;

    debug_assert_eq!(
        MIXED_LEN_BUCKETS.iter().map(|(w, _, _)| w).sum::<u32>(),
        20,
        "MIXED_LEN_BUCKETS weights must sum to 20",
    );

    let max_len = MIXED_LEN_BUCKETS
        .iter()
        .map(|(_, _, m)| *m)
        .max()
        .expect("at least one bucket");
    let reps = max_len.div_ceil(STRING_BODY_SOURCE.len()).max(1);
    let body: String = STRING_BODY_SOURCE.repeat(reps);

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
        let span = hi - lo + 1;
        let target_len = lo + i.wrapping_mul(2_654_435_761) % span;

        // Emit body bytes interleaved with escape sequences. Track JSON-text
        // length so we land on `target_len` regardless of escape widths.
        s.push('"');
        let start = s.len();
        let mut body_pos = 0;
        let mut esc_pos = i; // rotate per element so we don't repeat the same pattern
        let mut written = 0usize;
        while written < target_len {
            let remaining = target_len - written;
            // Decide whether the next chunk is literal-body or an escape.
            // First chunk is always literal (so the string doesn't open
            // with an escape), and we cycle to escape every STRIDE bytes.
            let want_escape = written > 0 && written % STRIDE == 0;
            if want_escape {
                let esc = ESCAPES[esc_pos % ESCAPES.len()];
                esc_pos += 1;
                if esc.len() <= remaining {
                    s.push_str(esc);
                    written += esc.len();
                    continue;
                }
                // Not enough room for this escape; fall through to literal fill.
            }
            // Literal stretch: as many body bytes as fit before the next
            // STRIDE boundary, capped by remaining.
            let next_boundary = ((written / STRIDE) + 1) * STRIDE;
            let chunk = remaining.min(next_boundary.saturating_sub(written).max(1));
            // Body wraps; mod into range.
            let body_start = body_pos % body.len();
            // Take up to `chunk` bytes, but stop at end-of-body to keep the
            // slice valid; if we hit the end, the next iteration wraps.
            let avail = body.len() - body_start;
            let take = chunk.min(avail);
            s.push_str(&body[body_start..body_start + take]);
            body_pos += take;
            written += take;
        }
        debug_assert_eq!(s.len() - start, target_len);
        s.push('"');
    }
    s.push(']');
    s
}

/// Array of N strings with non-ASCII bytes.
///
/// Cycles through Latin-1
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

/// Config-file-shaped document: a single object with branching nested
/// objects 3-5 levels deep.
///
/// Mimics what `package.json`, `tsconfig.json`,
/// Kubernetes manifests, or service-discovery payloads look like — not
/// "array of N similar records" but one structured object with a tree of
/// heterogeneous children. `n_services` controls the fan-out at the
/// deepest level so the overall size scales linearly.
///
/// Why this matters: every other realistic fixture is a flat array of
/// records. None of them exercise the parser's frame stack across a
/// branching tree of *objects*, which is the dominant shape for config,
/// GraphQL responses, and infra metadata.
#[must_use]
pub fn nested_config_doc(n_services: usize) -> String {
    let mut s = String::with_capacity(n_services * 280 + 512);
    s.push_str(
        r#"{"apiVersion":"v1","kind":"ServiceMesh","metadata":{"name":"prod-mesh","namespace":"infra","labels":{"env":"prod","tier":"backend","region":"us-east-1"},"annotations":{"deploy/owner":"platform","deploy/sha":"abcdef0123456789","deploy/timestamp":"2026-04-30T12:00:00Z"}},"spec":{"defaults":{"timeouts":{"connect_ms":1000,"read_ms":5000,"write_ms":5000,"idle_ms":60000},"retries":{"max_attempts":3,"backoff":{"initial_ms":100,"max_ms":2000,"multiplier":2.0,"jitter":0.1}},"circuit_breaker":{"enabled":true,"thresholds":{"error_rate":0.05,"min_requests":20,"window_ms":10000}}},"services":["#,
    );
    for i in 0..n_services {
        if i > 0 {
            s.push(',');
        }
        let proto = if i % 3 == 0 { "grpc" } else { "http" };
        let port = 8000 + (i % 1000);
        let _ = write!(
            &mut s,
            r#"{{"name":"svc-{i:04}","port":{port},"protocol":"{proto}","upstream":{{"discovery":"dns","host":"svc-{i:04}.internal","health":{{"path":"/healthz","interval_ms":5000,"timeout_ms":1000,"unhealthy_threshold":3}}}},"routes":[{{"match":{{"prefix":"/api/v1/"}},"policy":{{"timeout_ms":3000,"retry":{{"on":"5xx","attempts":2}}}}}},{{"match":{{"prefix":"/admin/"}},"policy":{{"timeout_ms":30000,"auth":{{"required":true,"providers":["jwt","mtls"]}}}}}}],"tags":{{"team":"team-{}","tier":"{}","critical":{}}}}}"#,
            i % 16,
            if i % 4 == 0 { "edge" } else { "core" },
            i % 5 == 0,
        );
    }
    s.push_str(r"]}}");
    s
}

/// Single object with `n_keys` short string-keyed fields, all numeric
/// values.
///
/// Mimics protobuf-decoded records, feature-flag bundles, and
/// metric snapshots — shapes where one object carries hundreds of fields
/// rather than many small objects each carrying a few. Exercises the
/// per-key dispatch path far more than the GitHub/log corpora do.
///
/// Keys are 8 bytes (`field_NNN`), values are 6-digit decimals, so the
/// document is ~25 bytes per key. 500 keys is ~12 KB — typical for the
/// shape but big enough that keyed dispatch dominates parse time.
#[must_use]
pub fn wide_key_object(n_keys: usize) -> String {
    let mut s = String::with_capacity(n_keys * 28 + 2);
    s.push('{');
    for i in 0..n_keys {
        if i > 0 {
            s.push(',');
        }
        // Six-digit values keep the lexer in the fast 1-9 digit path so
        // the bench measures key dispatch, not number parsing.
        let _ = write!(&mut s, r#""field_{i:03}":{}"#, 100_000 + i);
    }
    s.push('}');
    s
}

/// One huge GeoJSON-FeatureCollection-shaped document.
///
/// A single top-level
/// object with one large `features` array; each feature is a small nested
/// object containing a `geometry` (coords array) and a `properties` bag.
/// Total size is ~`n_features * 220` bytes — at `n_features = 25_000` that
/// is ~5 MB, which approximates a single tile of OpenStreetMap data, a
/// large GraphQL response, or a paginated API response in one shot.
///
/// Why this matters: every other realistic corpus benches "array of N
/// records, each independent." A multi-MB single document is a different
/// load on the parser — sustained throughput across one continuous stream
/// rather than dispatch-amortized-over-records. If there is a per-call
/// fixed cost in `Parser::new` that hides in small fixtures, this surfaces
/// it; if there is a per-byte cost that scales linearly, this is where it
/// shows.
#[must_use]
pub fn giant_geojson_doc(n_features: usize) -> String {
    let mut s = String::with_capacity(n_features * 240 + 256);
    s.push_str(r#"{"type":"FeatureCollection","generator":"bourne-bench","timestamp":"2026-04-30T12:00:00Z","features":["#);
    for i in 0..n_features {
        if i > 0 {
            s.push(',');
        }
        let lat = (i % 180) as f64 / 2.0 - 89.999;
        let lng = (i % 360) as f64 - 179.999;
        let elev = (i % 4000) as f64 + 0.5;
        let kind = match i % 4 {
            0 => "node",
            1 => "way",
            2 => "relation",
            _ => "area",
        };
        let _ = write!(
            &mut s,
            r#"{{"type":"Feature","id":{i},"geometry":{{"type":"Point","coordinates":[{lng:.6},{lat:.6},{elev:.2}]}},"properties":{{"osm_id":{},"kind":"{kind}","name":"feature-{i:06}","amenity":"none","tags":{{"created_by":"bourne-bench","version":1,"changeset":{}}}}}}}"#,
            10_000_000 + i,
            500_000 + i,
        );
    }
    s.push_str(r"]}");
    s
}

/// Array of N records, each containing both ints and floats with realistic
/// magnitudes.
///
/// Mimics metric-event payloads, financial ticks, or sensor
/// readings — every other realistic corpus is "all ints" (`jwt_ids`) or
/// "all floats" (`geo_array`), but real records mix them per row.
///
/// The float decoder pays `f64::from_str` for every float; the int decoder
/// uses the fused `parse_i64_value` fast path. Mixing them per record means
/// the dispatch cost can't be amortized away by sticking on one branch.
#[must_use]
pub fn metric_event_array(n: usize) -> String {
    let mut s = String::with_capacity(n * 180);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let ts: u64 = 1_700_000_000_000 + i as u64;
        let count: u64 = i as u64 % 10_000;
        let bytes: u64 = 1024 * (i as u64 % 1_000_000);
        // Three different float magnitudes so f64::from_str sees variety.
        let latency_ms = (i % 500) as f64 + 0.125;
        let cpu = (i % 100) as f64 / 100.0;
        let throughput = (i as f64) * 12.345;
        let _ = write!(
            &mut s,
            r#"{{"ts":{ts},"host":"node-{}","metric":"req.latency","count":{count},"bytes":{bytes},"latency_ms":{latency_ms:.3},"cpu":{cpu:.4},"throughput_rps":{throughput:.2}}}"#,
            i % 64,
        );
    }
    s.push(']');
    s
}

/// Same record shape and same values as [`metric_event_array`], but with
/// every object's keys emitted in **reverse declaration order**.
///
/// JSON
/// objects are unordered by spec, so a correct typed parser must accept
/// either ordering. This bench measures whether the field-dispatch match
/// in a hand-written `FromJson` impl performs the same regardless of key
/// order — branch predictor warmth, switch-table layout, and any
/// optimization that assumes "first key in object is first arm of match"
/// would surface here as a slowdown.
#[must_use]
pub fn metric_event_array_reversed_keys(n: usize) -> String {
    let mut s = String::with_capacity(n * 180);
    s.push('[');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        let ts: u64 = 1_700_000_000_000 + i as u64;
        let count: u64 = i as u64 % 10_000;
        let bytes: u64 = 1024 * (i as u64 % 1_000_000);
        let latency_ms = (i % 500) as f64 + 0.125;
        let cpu = (i % 100) as f64 / 100.0;
        let throughput = (i as f64) * 12.345;
        // Reverse order vs metric_event_array: throughput_rps -> ts.
        let _ = write!(
            &mut s,
            r#"{{"throughput_rps":{throughput:.2},"cpu":{cpu:.4},"latency_ms":{latency_ms:.3},"bytes":{bytes},"count":{count},"metric":"req.latency","host":"node-{}","ts":{ts}}}"#,
            i % 64,
        );
    }
    s.push(']');
    s
}
