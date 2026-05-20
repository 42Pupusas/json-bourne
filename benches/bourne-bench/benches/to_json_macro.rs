//! Head-to-head benches for the `to_json!` macro.
//!
//! Each benchmark group has up to three rows over the same input:
//!   - `macro` — the `to_json!`-emitted `ToJson` impl.
//!   - `hand`  — a manually written `ToJson` impl over an identically
//!     shaped struct/enum. A meaningful gap between this and `macro`
//!     surfaces overhead the declarative macro is leaving on the table
//!     (extra branches, suboptimal punctuation order, etc.).
//!   - `serde_json` — `serde_json::to_string` on a `serde::Serialize`
//!     mirror. Absolute-scale anchor against the de-facto baseline.
//!
//! Coverage targets every shape `to_json!` supports:
//!   - small struct (`SMALL_OBJECT` shape)                    — `struct_small`
//!   - realistic 8-field struct (`MetricEvent`)               — `struct_metric`
//!   - struct with `&str` borrow + escape-heavy values        — `struct_borrowed_escape`
//!   - struct with `skip_if_none` / `rename` (Some + None)    — `struct_decorated`
//!   - newtype tuple struct                                   — `tuple_newtype`
//!   - multi-field tuple struct                               — `tuple_multi`
//!   - externally-tagged enum (all four variant kinds)        — `enum_external`
//!   - internally-tagged enum (`tag = "type"`)                — `enum_internal`
//!   - adjacently-tagged enum (`tag, content`)                — `enum_adjacent`
//!   - untagged enum                                          — `enum_untagged`
//!
//! The struct/enum benches all run over `Vec<T>` of `N=1000` so per-
//! record dispatch dominates the wall-clock instead of fixed setup.

use bourne::{JsonWrite, ToJson, to_json, to_string};
use bourne_bench::SMALL_OBJECT;
use serde::Serialize;

fn main() {
    divan::main();
}

const N: usize = 1_000;

// Bench-fixture conversions. Every fixture builder iterates `0..N` with
// `N = 1_000`, well within u32 / i32 / i64 / f64-mantissa range; these
// keep call sites lint-clean without bare `as` casts.
fn as_u32(i: usize) -> u32 {
    u32::try_from(i).expect("loop index N=1_000 fits u32")
}
fn as_i32(i: usize) -> i32 {
    i32::try_from(i).expect("loop index N=1_000 fits i32")
}
fn as_i64(i: usize) -> i64 {
    i64::try_from(i).expect("loop index N=1_000 fits i64")
}
fn as_f64(i: usize) -> f64 {
    // u32 widens losslessly to f64; the i->u32 step is the bounded one.
    f64::from(as_u32(i))
}

// ===========================================================================
// 1. Small struct — same shape as `SMALL_OBJECT`.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    struct UserMacro<'input> {
        id: u64,
        name: &'input str,
        verified: bool,
        followers: u32,
        bio: Option<&'input str>,
        links: Vec<&'input str>,
    }
}

#[derive(Debug)]
struct UserHand<'input> {
    id: u64,
    name: &'input str,
    verified: bool,
    followers: u32,
    bio: Option<&'input str>,
    links: Vec<&'input str>,
}

impl ToJson for UserHand<'_> {
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_byte(b'{')?;
        w.write_str_raw("\"id\":")?;
        self.id.write_json(w)?;
        w.write_str_raw(",\"name\":")?;
        self.name.write_json(w)?;
        w.write_str_raw(",\"verified\":")?;
        self.verified.write_json(w)?;
        w.write_str_raw(",\"followers\":")?;
        self.followers.write_json(w)?;
        w.write_str_raw(",\"bio\":")?;
        self.bio.write_json(w)?;
        w.write_str_raw(",\"links\":")?;
        self.links.write_json(w)?;
        w.write_byte(b'}')
    }
}

#[derive(Debug, Serialize)]
struct UserSerde<'a> {
    id: u64,
    name: &'a str,
    verified: bool,
    followers: u32,
    bio: Option<&'a str>,
    links: Vec<&'a str>,
}

fn user_fixture<'a>() -> (UserMacro<'a>, UserHand<'a>, UserSerde<'a>) {
    // Same logical content as `SMALL_OBJECT`. Values are static so all
    // three structs share the same borrows; no allocation per build.
    let m = UserMacro {
        id: 1_234_567_890,
        name: "alice",
        verified: true,
        followers: 42,
        bio: None,
        links: vec!["a", "b", "c"],
    };
    let h = UserHand {
        id: 1_234_567_890,
        name: "alice",
        verified: true,
        followers: 42,
        bio: None,
        links: vec!["a", "b", "c"],
    };
    let s = UserSerde {
        id: 1_234_567_890,
        name: "alice",
        verified: true,
        followers: 42,
        bio: None,
        links: vec!["a", "b", "c"],
    };
    (m, h, s)
}

mod to_json_struct_small {
    use super::{user_fixture, SMALL_OBJECT, to_string};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let (m, _, _) = user_fixture();
        bencher
            .counter(divan::counter::BytesCount::new(SMALL_OBJECT.len()))
            .bench(|| {
                let out = to_string(divan::black_box(&m)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn hand(bencher: divan::Bencher) {
        let (_, h, _) = user_fixture();
        bencher
            .counter(divan::counter::BytesCount::new(SMALL_OBJECT.len()))
            .bench(|| {
                let out = to_string(divan::black_box(&h)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let (_, _, s) = user_fixture();
        bencher
            .counter(divan::counter::BytesCount::new(SMALL_OBJECT.len()))
            .bench(|| {
                let out = serde_json::to_string(divan::black_box(&s)).unwrap();
                divan::black_box(out);
            });
    }
}

// ===========================================================================
// 2. Realistic 8-field struct — `MetricEvent` over Vec of N=1000.
//    The headline serialize number: heterogeneous fields, mixed
//    int/string/f64, exercises the per-record write path.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    struct MetricEventMacro<'input> {
        ts: u64,
        host: &'input str,
        metric: &'input str,
        count: u64,
        bytes: u64,
        latency_ms: f64,
        cpu: f64,
        throughput_rps: f64,
    }
}

#[derive(Debug)]
struct MetricEventHand<'input> {
    ts: u64,
    host: &'input str,
    metric: &'input str,
    count: u64,
    bytes: u64,
    latency_ms: f64,
    cpu: f64,
    throughput_rps: f64,
}

impl ToJson for MetricEventHand<'_> {
    fn write_json<W: JsonWrite + ?Sized>(&self, w: &mut W) -> Result<(), W::Error> {
        w.write_byte(b'{')?;
        w.write_str_raw("\"ts\":")?;
        self.ts.write_json(w)?;
        w.write_str_raw(",\"host\":")?;
        self.host.write_json(w)?;
        w.write_str_raw(",\"metric\":")?;
        self.metric.write_json(w)?;
        w.write_str_raw(",\"count\":")?;
        self.count.write_json(w)?;
        w.write_str_raw(",\"bytes\":")?;
        self.bytes.write_json(w)?;
        w.write_str_raw(",\"latency_ms\":")?;
        self.latency_ms.write_json(w)?;
        w.write_str_raw(",\"cpu\":")?;
        self.cpu.write_json(w)?;
        w.write_str_raw(",\"throughput_rps\":")?;
        self.throughput_rps.write_json(w)?;
        w.write_byte(b'}')
    }
}

#[derive(Debug, Serialize)]
struct MetricEventSerde<'a> {
    ts: u64,
    host: &'a str,
    metric: &'a str,
    count: u64,
    bytes: u64,
    latency_ms: f64,
    cpu: f64,
    throughput_rps: f64,
}

// Distinct host buffers so &str lifetimes are valid for the bench
// duration.  Stored once at top-level so each iteration's Vec build
// is only the borrow, not the format.
const HOSTS: &[&str] = &[
    "node-0", "node-1", "node-2", "node-3", "node-4", "node-5", "node-6", "node-7",
];

fn metric_macro_vec() -> Vec<MetricEventMacro<'static>> {
    (0..N)
        .map(|i| MetricEventMacro {
            ts: 1_700_000_000_000 + i as u64,
            host: HOSTS[i % HOSTS.len()],
            metric: "req.latency",
            count: i as u64 % 10_000,
            bytes: 1024 * (i as u64 % 1_000_000),
            latency_ms: as_f64(i % 500) + 0.125,
            cpu: as_f64(i % 100) / 100.0,
            throughput_rps: as_f64(i) * 12.345,
        })
        .collect()
}

fn metric_hand_vec() -> Vec<MetricEventHand<'static>> {
    (0..N)
        .map(|i| MetricEventHand {
            ts: 1_700_000_000_000 + i as u64,
            host: HOSTS[i % HOSTS.len()],
            metric: "req.latency",
            count: i as u64 % 10_000,
            bytes: 1024 * (i as u64 % 1_000_000),
            latency_ms: as_f64(i % 500) + 0.125,
            cpu: as_f64(i % 100) / 100.0,
            throughput_rps: as_f64(i) * 12.345,
        })
        .collect()
}

fn metric_serde_vec() -> Vec<MetricEventSerde<'static>> {
    (0..N)
        .map(|i| MetricEventSerde {
            ts: 1_700_000_000_000 + i as u64,
            host: HOSTS[i % HOSTS.len()],
            metric: "req.latency",
            count: i as u64 % 10_000,
            bytes: 1024 * (i as u64 % 1_000_000),
            latency_ms: as_f64(i % 500) + 0.125,
            cpu: as_f64(i % 100) / 100.0,
            throughput_rps: as_f64(i) * 12.345,
        })
        .collect()
}

mod to_json_struct_metric_1000 {
    use super::{metric_macro_vec, to_string, metric_hand_vec, metric_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = metric_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = to_string(divan::black_box(&m)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn hand(bencher: divan::Bencher) {
        let h = metric_hand_vec();
        let payload_bytes = to_string(&h).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = to_string(divan::black_box(&h)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = metric_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = serde_json::to_string(divan::black_box(&s)).unwrap();
                divan::black_box(out);
            });
    }
}

// ===========================================================================
// 3. Borrowed-`&str` struct with escape-heavy field values.
//    Pins the macro on its headline use case (zero-copy parse -> write
//    back out) and exercises the `write_escaped_str` slow path.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    struct LogLineMacro<'input> {
        level: &'input str,
        message: &'input str,
        request_id: &'input str,
    }
}

#[derive(Debug, Serialize)]
struct LogLineSerde<'a> {
    level: &'a str,
    message: &'a str,
    request_id: &'a str,
}

fn log_macro_vec() -> Vec<LogLineMacro<'static>> {
    (0..N)
        .map(|i| LogLineMacro {
            level: if i % 7 == 0 { "ERROR" } else { "info" },
            // Every 3rd line carries quote+backslash+newline, forcing the
            // escape path on roughly one third of fields per record.
            message: if i % 3 == 0 {
                "user said \"hi\"\nthen left\\"
            } else {
                "request handled in 12ms"
            },
            request_id: "req-2c4f-9aa1",
        })
        .collect()
}

fn log_serde_vec() -> Vec<LogLineSerde<'static>> {
    (0..N)
        .map(|i| LogLineSerde {
            level: if i % 7 == 0 { "ERROR" } else { "info" },
            message: if i % 3 == 0 {
                "user said \"hi\"\nthen left\\"
            } else {
                "request handled in 12ms"
            },
            request_id: "req-2c4f-9aa1",
        })
        .collect()
}

mod to_json_struct_borrowed_escape_1000 {
    use super::{log_macro_vec, to_string, log_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = log_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = to_string(divan::black_box(&m)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = log_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = serde_json::to_string(divan::black_box(&s)).unwrap();
                divan::black_box(out);
            });
    }
}

// ===========================================================================
// 4. Decorated struct — `rename` + `skip` + `skip_if_none`.
//    Two flavors of the workload: half the records have `note=Some(..)`,
//    half are `None`, so the bench averages over both branches of the
//    skip_if_none emit.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    struct DecoratedMacro {
        #[bourne(rename = "user-id")]
        user_id: u32,
        #[bourne(skip)]
        cached: u32,
        #[bourne(skip_if_none)]
        note: Option<String>,
        value: u32,
    }
}

#[derive(Debug, Serialize)]
struct DecoratedSerde {
    #[serde(rename = "user-id")]
    user_id: u32,
    #[serde(skip)]
    #[allow(dead_code)]
    cached: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    value: u32,
}

fn decorated_macro_vec() -> Vec<DecoratedMacro> {
    (0..N)
        .map(|i| DecoratedMacro {
            user_id: as_u32(i),
            cached: 99,
            note: if i % 2 == 0 {
                Some(String::from("note text"))
            } else {
                None
            },
            value: as_u32(i) * 7,
        })
        .collect()
}

fn decorated_serde_vec() -> Vec<DecoratedSerde> {
    (0..N)
        .map(|i| DecoratedSerde {
            user_id: as_u32(i),
            cached: 99,
            note: if i % 2 == 0 {
                Some(String::from("note text"))
            } else {
                None
            },
            value: as_u32(i) * 7,
        })
        .collect()
}

mod to_json_struct_decorated_1000 {
    use super::{decorated_macro_vec, to_string, decorated_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = decorated_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = to_string(divan::black_box(&m)).unwrap();
                divan::black_box(out);
            });
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = decorated_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| {
                let out = serde_json::to_string(divan::black_box(&s)).unwrap();
                divan::black_box(out);
            });
    }
}

// ===========================================================================
// 5. Tuple structs — newtype + multi-field.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    struct UserId(u64);
}

#[derive(Debug, Serialize)]
#[serde(transparent)]
struct UserIdSerde(u64);

to_json! {
    #[derive(Debug)]
    struct Triple(i64, String, bool);
}

#[derive(Debug, Serialize)]
struct TripleSerde(i64, String, bool);

fn newtype_vec() -> Vec<UserId> {
    (0..N).map(|i| UserId(i as u64)).collect()
}
fn newtype_serde_vec() -> Vec<UserIdSerde> {
    (0..N).map(|i| UserIdSerde(i as u64)).collect()
}
fn triple_vec() -> Vec<Triple> {
    (0..N)
        .map(|i| Triple(as_i64(i), format!("item-{i}"), i % 2 == 0))
        .collect()
}
fn triple_serde_vec() -> Vec<TripleSerde> {
    (0..N)
        .map(|i| TripleSerde(as_i64(i), format!("item-{i}"), i % 2 == 0))
        .collect()
}

mod to_json_tuple_newtype_1000 {
    use super::{newtype_vec, to_string, newtype_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = newtype_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = newtype_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}

mod to_json_tuple_multi_1000 {
    use super::{triple_vec, to_string, triple_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = triple_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = triple_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}

// ===========================================================================
// 6. Externally-tagged enum — all four variant kinds.
// ===========================================================================

to_json! {
    #[derive(Debug)]
    enum ShapeMacro {
        Circle,
        Wrapper(u32),
        Pair(u32, String),
        Box { w: u32, h: u32 },
    }
}

#[derive(Debug, Serialize)]
enum ShapeSerde {
    Circle,
    Wrapper(u32),
    Pair(u32, String),
    Box { w: u32, h: u32 },
}

// Cycle through every variant kind so the per-iter average folds in
// each match arm. A bench that hits only `Circle` would tell us
// nothing about the heavier variants.
fn shape_macro_vec() -> Vec<ShapeMacro> {
    (0..N)
        .map(|i| match i % 4 {
            0 => ShapeMacro::Circle,
            1 => ShapeMacro::Wrapper(as_u32(i)),
            2 => ShapeMacro::Pair(as_u32(i), format!("p{i}")),
            _ => ShapeMacro::Box {
                w: as_u32(i),
                h: as_u32(i * 2),
            },
        })
        .collect()
}
fn shape_serde_vec() -> Vec<ShapeSerde> {
    (0..N)
        .map(|i| match i % 4 {
            0 => ShapeSerde::Circle,
            1 => ShapeSerde::Wrapper(as_u32(i)),
            2 => ShapeSerde::Pair(as_u32(i), format!("p{i}")),
            _ => ShapeSerde::Box {
                w: as_u32(i),
                h: as_u32(i * 2),
            },
        })
        .collect()
}

mod to_json_enum_external_1000 {
    use super::{shape_macro_vec, to_string, shape_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = shape_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = shape_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}

// ===========================================================================
// 7. Internally-tagged enum.
// ===========================================================================

to_json! {
    #[bourne(tag = "type")]
    #[derive(Debug)]
    enum EventMacro {
        Heartbeat,
        Click { x: u32, y: u32 },
        Move { x: u32, y: u32, dx: i32, dy: i32 },
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum EventSerde {
    Heartbeat,
    Click { x: u32, y: u32 },
    Move { x: u32, y: u32, dx: i32, dy: i32 },
}

fn event_macro_vec() -> Vec<EventMacro> {
    (0..N)
        .map(|i| match i % 3 {
            0 => EventMacro::Heartbeat,
            1 => EventMacro::Click {
                x: as_u32(i),
                y: as_u32(i) * 2,
            },
            _ => EventMacro::Move {
                x: as_u32(i),
                y: as_u32(i),
                dx: -as_i32(i),
                dy: as_i32(i) / 2,
            },
        })
        .collect()
}
fn event_serde_vec() -> Vec<EventSerde> {
    (0..N)
        .map(|i| match i % 3 {
            0 => EventSerde::Heartbeat,
            1 => EventSerde::Click {
                x: as_u32(i),
                y: as_u32(i) * 2,
            },
            _ => EventSerde::Move {
                x: as_u32(i),
                y: as_u32(i),
                dx: -as_i32(i),
                dy: as_i32(i) / 2,
            },
        })
        .collect()
}

mod to_json_enum_internal_1000 {
    use super::{event_macro_vec, to_string, event_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = event_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = event_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}

// ===========================================================================
// 8. Adjacently-tagged enum.
// ===========================================================================

to_json! {
    #[bourne(tag = "t", content = "c")]
    #[derive(Debug)]
    enum MsgMacro {
        Ping,
        Echo(String),
        Pair(u32, u32),
        Body { text: String },
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "t", content = "c")]
enum MsgSerde {
    Ping,
    Echo(String),
    Pair(u32, u32),
    Body { text: String },
}

fn msg_macro_vec() -> Vec<MsgMacro> {
    (0..N)
        .map(|i| match i % 4 {
            0 => MsgMacro::Ping,
            1 => MsgMacro::Echo(format!("hi-{i}")),
            2 => MsgMacro::Pair(as_u32(i), as_u32(i * 3)),
            _ => MsgMacro::Body {
                text: format!("body-{i}"),
            },
        })
        .collect()
}
fn msg_serde_vec() -> Vec<MsgSerde> {
    (0..N)
        .map(|i| match i % 4 {
            0 => MsgSerde::Ping,
            1 => MsgSerde::Echo(format!("hi-{i}")),
            2 => MsgSerde::Pair(as_u32(i), as_u32(i * 3)),
            _ => MsgSerde::Body {
                text: format!("body-{i}"),
            },
        })
        .collect()
}

mod to_json_enum_adjacent_1000 {
    use super::{msg_macro_vec, to_string, msg_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = msg_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = msg_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}

// ===========================================================================
// 9. Untagged enum.
// ===========================================================================

to_json! {
    #[bourne(untagged)]
    #[derive(Debug)]
    enum MixedMacro {
        Nothing,
        One(u32),
        Two(u32, u32),
        Body { name: String },
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum MixedSerde {
    Nothing,
    One(u32),
    Two(u32, u32),
    Body { name: String },
}

fn mixed_macro_vec() -> Vec<MixedMacro> {
    (0..N)
        .map(|i| match i % 4 {
            0 => MixedMacro::Nothing,
            1 => MixedMacro::One(as_u32(i)),
            2 => MixedMacro::Two(as_u32(i), as_u32(i * 7)),
            _ => MixedMacro::Body {
                name: format!("n-{i}"),
            },
        })
        .collect()
}
fn mixed_serde_vec() -> Vec<MixedSerde> {
    (0..N)
        .map(|i| match i % 4 {
            0 => MixedSerde::Nothing,
            1 => MixedSerde::One(as_u32(i)),
            2 => MixedSerde::Two(as_u32(i), as_u32(i * 7)),
            _ => MixedSerde::Body {
                name: format!("n-{i}"),
            },
        })
        .collect()
}

mod to_json_enum_untagged_1000 {
    use super::{mixed_macro_vec, to_string, mixed_serde_vec};

    #[divan::bench]
    fn r#macro(bencher: divan::Bencher) {
        let m = mixed_macro_vec();
        let payload_bytes = to_string(&m).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(to_string(divan::black_box(&m)).unwrap()));
    }

    #[divan::bench]
    fn serde_json(bencher: divan::Bencher) {
        let s = mixed_serde_vec();
        let payload_bytes = serde_json::to_string(&s).unwrap().len();
        bencher
            .counter(divan::counter::BytesCount::new(payload_bytes))
            .bench(|| divan::black_box(serde_json::to_string(divan::black_box(&s)).unwrap()));
    }
}
