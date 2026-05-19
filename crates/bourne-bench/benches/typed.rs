//! Typed deserialization throughput. Compare against `stream` benches for the
//! same input to see the cost of `FromJson` on top of the streaming parser.

use bourne::parse;
use bourne_bench::{int_array, string_array};

fn main() {
    divan::main();
}

#[divan::bench(args = [10, 1_000, 100_000])]
fn vec_i64(bencher: divan::Bencher, n: usize) {
    let s = int_array(n);
    let bytes = s.as_bytes();
    bencher
        .counter(divan::counter::BytesCount::new(bytes.len()))
        .bench(|| {
            let v: Vec<i64> = parse(divan::black_box(bytes)).unwrap();
            divan::black_box(v);
        });
}

#[divan::bench(args = [10, 1_000, 100_000])]
fn vec_borrowed_str(bencher: divan::Bencher, n: usize) {
    let s = string_array(n);
    let bytes = s.as_bytes();
    // Borrowed: should not allocate per element (only the outer Vec).
    bencher
        .counter(divan::counter::BytesCount::new(bytes.len()))
        .bench(|| {
            let v: Vec<&str> = parse(divan::black_box(bytes)).unwrap();
            divan::black_box(v);
        });
}

#[divan::bench(args = [10, 1_000, 100_000])]
fn vec_string(bencher: divan::Bencher, n: usize) {
    let s = string_array(n);
    let bytes = s.as_bytes();
    // Owned: forces a String allocation per element.
    bencher
        .counter(divan::counter::BytesCount::new(bytes.len()))
        .bench(|| {
            let v: Vec<String> = parse(divan::black_box(bytes)).unwrap();
            divan::black_box(v);
        });
}
