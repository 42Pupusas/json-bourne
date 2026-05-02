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
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

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

fn bench_floats(c: &mut Criterion) {
    let mut group = c.benchmark_group("floats");

    for &n in &[100usize, 1_000, 10_000] {
        let floats = make_floats(n);
        // Throughput counted in floats per iteration so the report
        // shows elem/sec — easier to reason about than bytes/sec for
        // this workload.
        group.throughput(Throughput::Elements(n as u64));

        // -------- bourne (production write!-based path) --------
        group.bench_function(format!("bourne_write/{n}"), |b| {
            b.iter(|| {
                let s = to_string(black_box(&floats)).unwrap();
                black_box(s);
            });
        });

        // -------- ryu crate direct (target for inline port) --------
        // Build the same JSON shape (`[f0,f1,...]`) by hand so the
        // comparison isolates float formatting from punctuation.
        group.bench_function(format!("ryu_direct/{n}"), |b| {
            b.iter(|| {
                let mut out = String::with_capacity(n * 24);
                let mut buf = ryu::Buffer::new();
                out.push('[');
                let mut first = true;
                for f in black_box(&floats) {
                    if !first {
                        out.push(',');
                    }
                    out.push_str(buf.format(*f));
                    first = false;
                }
                out.push(']');
                black_box(out);
            });
        });

        // -------- serde_json (absolute anchor) --------
        group.bench_function(format!("serde_json/{n}"), |b| {
            b.iter(|| {
                let s = serde_json::to_string(black_box(&floats)).unwrap();
                black_box(s);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_floats);
criterion_main!(benches);
