//! Memory comparison: bourne vs serde_json.
//!
//! Each workload runs once through bourne and once through serde_json with
//! a process-wide counting allocator installed. We report:
//!   - allocs: number of trips to the allocator
//!   - total : cumulative bytes requested across the parse
//!   - peak  : maximum bytes live at any point during the parse
//!
//! Throughput is criterion's job (`benches/compare.rs`); this is the
//! complementary memory story. A library can win one and lose the other.
//!
//! Run:
//!   cargo run --release --features compare-mem --bin compare_mem

use bourne::{EventSource, FromJson, parse};
use bourne_alloctest::measure;
use bourne_bench::{SMALL_OBJECT, int_array, string_array};
use bourne_core::{Error, ErrorKind, Event, Parser};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Struct shapes — same fields on both sides so the comparison is fair.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct UserBourne<'input> {
    id: u64,
    name: &'input str,
    verified: bool,
    followers: u32,
    bio: Option<&'input str>,
    links: Vec<&'input str>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UserSerde<'a> {
    id: u64,
    #[serde(borrow)]
    name: &'a str,
    verified: bool,
    followers: u32,
    #[serde(borrow)]
    bio: Option<&'a str>,
    #[serde(borrow)]
    links: Vec<&'a str>,
}

impl<'input> FromJson<'input> for UserBourne<'input> {
    fn from_event<S: EventSource<'input>>(
        source: &mut S,
        start: Event<'input>,
    ) -> Result<Self, Error> {
        if !matches!(start, Event::StartObject) {
            return Err(Error::new(ErrorKind::ExpectedObject, source.position()));
        }
        let mut id: Option<u64> = None;
        let mut name: Option<&'input str> = None;
        let mut verified: Option<bool> = None;
        let mut followers: Option<u32> = None;
        let mut bio: Option<&'input str> = None;
        let mut links: Option<Vec<&'input str>> = None;
        loop {
            let ev = source
                .next_event()?
                .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, source.position()))?;
            let key = match ev {
                Event::EndObject => break,
                Event::Key(k) => k,
                _ => return Err(Error::new(ErrorKind::TypeMismatch, source.position())),
            };
            let key_str = key
                .as_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidEscape, source.position()))?;
            let val_ev = source
                .next_event()?
                .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, source.position()))?;
            match key_str {
                "id" => id = Some(u64::from_event(source, val_ev)?),
                "name" => name = Some(<&str>::from_event(source, val_ev)?),
                "verified" => verified = Some(bool::from_event(source, val_ev)?),
                "followers" => followers = Some(u32::from_event(source, val_ev)?),
                "bio" => bio = Option::<&str>::from_event(source, val_ev)?,
                "links" => links = Some(Vec::<&str>::from_event(source, val_ev)?),
                _ => return Err(Error::new(ErrorKind::UnknownField, source.position())),
            }
        }
        Ok(Self {
            id: id.ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
            name: name.ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
            verified: verified
                .ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
            followers: followers
                .ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
            bio,
            links: links.ok_or_else(|| Error::new(ErrorKind::MissingField, source.position()))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct Report {
    allocs: usize,
    bytes: usize,
}

fn run_bourne_drain(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let mut p: Parser<'_> = Parser::new(input);
        let mut count = 0usize;
        while let Some(_) = p.next_event().expect("valid input") {
            count += 1;
        }
        count
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_value(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: serde_json::Value = serde_json::from_slice(input).expect("valid input");
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_struct(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let u: UserBourne<'_> = parse(input).unwrap();
        u
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_struct(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let u: UserSerde<'_> = serde_json::from_slice(input).unwrap();
        u
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_i64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<i64> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_i64(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<i64> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_str<'input>(input: &'input [u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<&'input str> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_str<'input>(input: &'input [u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<&'input str> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_bourne_vec_string(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<String> = parse(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn run_serde_vec_string(input: &[u8]) -> Report {
    let (_, snap) = measure(|| {
        let v: Vec<String> = serde_json::from_slice(input).unwrap();
        v
    });
    Report { allocs: snap.allocs, bytes: snap.bytes }
}

fn print_row(workload: &str, b: Report, s: Report) {
    let ratio_allocs = if s.allocs == 0 {
        "—".to_string()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let r = (b.allocs as f64 / s.allocs as f64) * 100.0;
        format!("{r:>5.0}%")
    };
    let ratio_bytes = if s.bytes == 0 {
        "—".to_string()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let r = (b.bytes as f64 / s.bytes as f64) * 100.0;
        format!("{r:>5.0}%")
    };
    println!(
        "  {:<28}  {:>6}  {:>9}  |  {:>6}  {:>9}  |  {:>6}  {:>6}",
        workload,
        format!("{}", b.allocs),
        format!("{} B", b.bytes),
        format!("{}", s.allocs),
        format!("{} B", s.bytes),
        ratio_allocs,
        ratio_bytes,
    );
}

fn header() {
    println!();
    println!(
        "  {:<28}  {:>18}  |  {:>18}  |  {:>15}",
        "workload", "bourne", "serde_json", "ratio (b/s)",
    );
    println!(
        "  {:<28}  {:>6}  {:>9}  |  {:>6}  {:>9}  |  {:>6}  {:>6}",
        "", "allocs", "bytes", "allocs", "bytes", "allocs", "bytes",
    );
    println!("  {}", "-".repeat(85));
}

fn main() {
    // Touch the allocator a couple of times before measuring; the very first
    // `format!` / `String::new` call in a process can pull in lazy-init
    // pages that would otherwise be charged to the first measured workload.
    let _warm = format!("{}", std::process::id());

    println!("Memory: bourne vs serde_json (single parse, no warmup)");
    header();

    // 1. stream-vs-DOM. bourne is streaming (zero alloc); serde builds Value.
    let small = SMALL_OBJECT.as_bytes();
    print_row(
        "stream small_object",
        run_bourne_drain(small),
        run_serde_value(small),
    );
    let big_ints = int_array(10_000);
    print_row(
        "stream ints/10000",
        run_bourne_drain(big_ints.as_bytes()),
        run_serde_value(big_ints.as_bytes()),
    );
    let big_strs = string_array(10_000);
    print_row(
        "stream strings/10000",
        run_bourne_drain(big_strs.as_bytes()),
        run_serde_value(big_strs.as_bytes()),
    );

    // 2. Typed struct.
    print_row(
        "typed_struct small",
        run_bourne_struct(small),
        run_serde_struct(small),
    );

    // 3. Vec<i64>.
    let s = int_array(100);
    print_row(
        "vec_i64/100",
        run_bourne_vec_i64(s.as_bytes()),
        run_serde_vec_i64(s.as_bytes()),
    );
    let s = int_array(10_000);
    print_row(
        "vec_i64/10000",
        run_bourne_vec_i64(s.as_bytes()),
        run_serde_vec_i64(s.as_bytes()),
    );

    // 4. Vec<&str> — borrowed both sides.
    let s = string_array(100);
    print_row(
        "vec_borrowed_str/100",
        run_bourne_vec_str(s.as_bytes()),
        run_serde_vec_str(s.as_bytes()),
    );
    let s = string_array(10_000);
    print_row(
        "vec_borrowed_str/10000",
        run_bourne_vec_str(s.as_bytes()),
        run_serde_vec_str(s.as_bytes()),
    );

    // 5. Vec<String> — owned both sides.
    let s = string_array(100);
    print_row(
        "vec_string/100",
        run_bourne_vec_string(s.as_bytes()),
        run_serde_vec_string(s.as_bytes()),
    );
    let s = string_array(10_000);
    print_row(
        "vec_string/10000",
        run_bourne_vec_string(s.as_bytes()),
        run_serde_vec_string(s.as_bytes()),
    );

    println!();
    println!("ratio < 100% means bourne uses less.");
    println!();
}
