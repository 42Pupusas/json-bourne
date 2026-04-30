# bourne-fuzz

Fuzz targets for the bourne JSON parser.

This crate is excluded from the main workspace because cargo-fuzz requires
nightly and its own build profile.

## One-time setup

```sh
cargo install cargo-fuzz
rustup toolchain install nightly
```

## Running

```sh
# From the repo root:
cargo +nightly fuzz run stream     # streaming parser only
cargo +nightly fuzz run typed      # typed FromJson layer
```

`-- -max_total_time=60` will cap a run at 60 seconds. Discovered crash inputs
land in `fuzz/artifacts/<target>/`; corpora live in `fuzz/corpus/<target>/`.

## Targets

- **stream** — calls `Parser::next_event` until exhausted. Asserts the
  streaming layer never panics, never UBs, never loops forever.
- **typed** — calls `parse::<T>` for a handful of representative types.
  Catches FromJson impl panics that the streaming target alone wouldn't.
