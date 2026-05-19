//! Float-formatting head-to-head: bourne (`write!`-based) vs the ryu crate
//! vs `serde_json`'s full serializer.
//!
//! Purpose: decide whether porting ryu inline into bourne is worth the
//! ~600-line maintenance cost. The bench gives us three numbers per
//! workload:
//!
//! - `bourne_write` — what users get today (libstd `Display` via `write!`,
//!   which does use ryu internally but pays the `Formatter` indirection).
//! - `ryu_direct` — the ceiling for an inline port (calling the `ryu`
//!   crate's `Buffer::format` directly into a `String`).
//! - `serde_json` — `serde_json::to_string` of the same `Vec<f64>`, for
//!   an absolute-scale anchor.
//!
//! Workloads are deliberately float-dominated `Vec<f64>` payloads: ints
//! and structural punctuation are negligible at sizes >=1000 elements,
//! so the bench isolates float cost.

use bourne::to_string;

fn main() {
    divan::main();
}

/// Reproducible pseudo-random `Vec<f64>` covering a realistic range
/// of magnitudes. Seeded so successive bench runs are comparable.
//
// Casts here are part of the *fixture content*, not measured code. The
// `as f64` of a u64 is the documented LCG-to-float conversion (top 53
// bits → mantissa) and is the same trick `rand` uses; allowing locally.
#[allow(clippy::cast_precision_loss)]
fn make_floats(n: usize) -> Vec<f64> {
    // Cheap LCG — not for crypto, just for stable per-position values
    // across runs. Avoids pulling in `rand` for a fixture.
    let mut state: u64 = 0xCAFE_BABE_DEAD_BEEF;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Mix a few magnitudes so the bench isn't all near-1.0:
        // ~10% small (1e-6), ~10% large (1e6), ~80% in [-100, 100].
        let bucket = i % 10;
        let mantissa = (state >> 11) as f64 / (1u64 << 53) as f64; // [0, 1)
        let signed = mantissa.mul_add(2.0, -1.0); // [-1, 1)
        let v = match bucket {
            0 => signed * 1e-6,
            1 => signed * 1e6,
            _ => signed * 100.0,
        };
        out.push(v);
    }
    out
}

mod bourne_write {
    use super::*;

    // -------- bourne (production write!-based path) --------
    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = make_floats(n);
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let s = to_string(divan::black_box(&floats)).unwrap();
                divan::black_box(s);
            });
    }
}

mod ryu_direct {
    use super::*;

    // -------- ryu crate direct (target for inline port) --------
    // Build the same JSON shape (`[f0,f1,...]`) by hand so the
    // comparison isolates float formatting from punctuation.
    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = make_floats(n);
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let mut out = String::with_capacity(n * 24);
                let mut buf = ryu::Buffer::new();
                out.push('[');
                let mut first = true;
                for f in divan::black_box(&floats) {
                    if !first {
                        out.push(',');
                    }
                    out.push_str(buf.format(*f));
                    first = false;
                }
                out.push(']');
                divan::black_box(out);
            });
    }
}

mod serde_json {
    use super::*;

    // -------- serde_json (absolute anchor) --------
    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = make_floats(n);
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let s = ::serde_json::to_string(divan::black_box(&floats)).unwrap();
                divan::black_box(s);
            });
    }
}

// ---------------------------------------------------------------------------
// Diagnostic: same float repeated.
//
// All elements are identical, so the teju math hits the *same* MULTIPLIERS
// entry every time and the digit-write LUT hits stay on the same cache
// lines. If bourne's cliff between n=1000 and n=10000 is data-dependent
// (random input → scattered LUT access pattern → L1d misses), the cliff
// should disappear here. If the cliff persists, the bottleneck is in the
// output-write loop, not the data-dependent inner work.
// ---------------------------------------------------------------------------

mod bourne_write_same {
    use super::*;

    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = vec![12345.6789_f64; n];
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let s = to_string(divan::black_box(&floats)).unwrap();
                divan::black_box(s);
            });
    }
}

mod serde_json_same {
    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = vec![12345.6789_f64; n];
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let s = ::serde_json::to_string(divan::black_box(&floats)).unwrap();
                divan::black_box(s);
            });
    }
}

// 4 distinct floats cycling — bounded branch-prediction state but still
// multi-magnitude. Tests whether the cliff comes from *unbounded variance*
// vs any variance at all.
mod bourne_write_four {
    use super::*;
    fn make(n: usize) -> Vec<f64> {
        let vals = [1.234567890123456_f64, 9876.54321e-3, 0.000123456789, 1.5e15];
        (0..n).map(|i| vals[i % 4]).collect()
    }
    #[divan::bench(args = [100, 1_000, 10_000])]
    fn bench(bencher: divan::Bencher, n: usize) {
        let floats = make(n);
        bencher
            .counter(divan::counter::ItemsCount::new(n))
            .bench(|| {
                let s = to_string(divan::black_box(&floats)).unwrap();
                divan::black_box(s);
            });
    }
}
