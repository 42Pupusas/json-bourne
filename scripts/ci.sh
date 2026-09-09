#!/usr/bin/env bash
# Reproduce .github/workflows/ci.yml locally: `scripts/ci.sh [job]`
# (test | msrv | clippy | fmt | crap | miri | fuzz | all). No args runs
# everything except miri and fuzz, which need a nightly toolchain installed,
# and crap, which needs cargo-llvm-cov + cargo-crap and the llvm-tools-preview
# component (`cargo install cargo-llvm-cov cargo-crap --locked`).
set -euo pipefail

cd "$(dirname "$0")/.."

test_suite() {
  # `--all-features` is deliberately absent from the workspace run:
  # bourne-bench's `alloc-profile` (lib) and `compare-mem` (bin) features
  # each install a #[global_allocator] and cannot share one build.
  cargo test --workspace
  cargo test -p json-bourne --all-features
  cargo test -p json-bourne --no-default-features
  cargo test -p json-bourne --no-default-features --features alloc
  cargo test -p json-bourne --no-default-features --features alloc,derive
}

run_test() {
  test_suite
  for target in thumbv7em-none-eabihf aarch64-unknown-none; do
    cargo build -p json-bourne --no-default-features --target "$target"
    cargo build -p json-bourne --no-default-features --features alloc --target "$target"
  done
}

run_msrv() {
  cargo +1.85 build --workspace
}

run_clippy() {
  cargo clippy -p json-bourne -p bourne-derive --all-targets --all-features -- -D warnings
  cargo clippy -p json-bourne --no-default-features -- -D warnings
}

run_fmt() {
  cargo fmt --all -- --check
}

run_crap() {
  # Threshold / exclusions / allows live in .cargo-crap.toml (audit F4:
  # the config only counts once something runs it).
  cargo llvm-cov --workspace --lcov --output-path target/lcov.info
  cargo crap --workspace --lcov target/lcov.info --fail-above
}

run_miri() {
  MIRIFLAGS="-Zmiri-disable-isolation" RUSTFLAGS="--cfg bourne_no_simd" \
    cargo +nightly miri test -p json-bourne --lib
}

run_fuzz() {
  local targets=(stream typed derived)
  local t
  for t in "${targets[@]}"; do
    (cd fuzz && cargo +nightly fuzz run "$t" -- -max_total_time=60 -runs=10000)
  done
}

case "${1:-all}" in
  test)   run_test ;;
  msrv)   run_msrv ;;
  clippy) run_clippy ;;
  fmt)    run_fmt ;;
  crap)   run_crap ;;
  miri)   run_miri ;;
  fuzz)   run_fuzz ;;
  all)    run_test; run_msrv; run_clippy; run_fmt; run_crap ;;
  *) echo "usage: scripts/ci.sh [test|msrv|clippy|fmt|crap|miri|fuzz|all]" >&2; exit 2 ;;
esac
