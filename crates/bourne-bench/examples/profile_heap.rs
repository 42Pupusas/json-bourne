//! DHAT-instrumented workload for heap profiling.
//!
//! Run:
//!   cargo run --release --features dhat-heap --example profile_heap
//!
//! Produces `dhat-heap.json` next to the binary. View at:
//!   https://nnethercote.github.io/dh_view/dh_view.html
//!
//! What we want to *see* in the output:
//!   - the streaming-only loop is at zero bytes / zero allocations
//!   - the typed `Vec<i64>` workload allocates exactly 1 Vec backing buffer
//!     per parse (and only grows as needed)
//!   - the typed `Vec<&str>` workload allocates only the outer Vec, no per-
//!     string copies (zero-copy borrowed strings)
//!   - the typed `Vec<String>` workload allocates one String per element
//!
//! If any of those expectations is wrong, we have a regression in the
//! zero-copy story that the alloctest crate didn't cover.

use bourne::parse;
use bourne_bench::{int_array, string_array};
use bourne_core::Parser;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn drain_streaming(input: &[u8]) {
    let mut p: Parser<'_> = Parser::new(input);
    while let Some(_) = p.next_event().expect("valid input") {}
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    // Workload sizes are picked to make per-allocation costs distinguishable
    // in the viewer without making the run take seconds.
    let ints_src = int_array(10_000);
    let strs_src = string_array(10_000);

    // 1. Streaming-only — should show zero allocations from this section.
    for _ in 0..100 {
        drain_streaming(ints_src.as_bytes());
        drain_streaming(strs_src.as_bytes());
    }

    // 2. Typed Vec<i64> — Vec backing buffer only.
    for _ in 0..100 {
        let v: Vec<i64> = parse(ints_src.as_bytes()).unwrap();
        std::hint::black_box(v);
    }

    // 3. Typed Vec<&str> — outer Vec only, strings borrow input.
    for _ in 0..100 {
        let v: Vec<&str> = parse(strs_src.as_bytes()).unwrap();
        std::hint::black_box(v);
    }

    // 4. Typed Vec<String> — one allocation per element.
    for _ in 0..100 {
        let v: Vec<String> = parse(strs_src.as_bytes()).unwrap();
        std::hint::black_box(v);
    }

    eprintln!("done; see dhat-heap.json");
}
