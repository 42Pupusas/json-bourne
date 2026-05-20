# Profiling bourne

The `profile` binary is a long-running, single-workload harness for
`perf` / flamegraph — wall-clock samples on real hardware.

## perf + flamegraph

### One-time setup

Lower `kernel.perf_event_paranoid` so `perf` can capture user-space
stacks without root:

```sh
sudo sysctl -w kernel.perf_event_paranoid=1
```

Install the flamegraph tool (pure Rust, no system deps):

```sh
cargo install inferno
```

### Build

The `profiling` Cargo profile inherits from `release` but keeps
line-table debug info so `perf` resolves function names. Frame pointers
have to come from `RUSTFLAGS` since stable Cargo can't set them per-profile:

```sh
RUSTFLAGS="-C force-frame-pointers=yes" \
  cargo build --profile profiling \
              --features profile \
              --bin profile
```

The binary lives at `target/profiling/profile`.

### Workloads

```sh
target/profiling/profile <workload>
```

Available workloads:

| name                   | what it stresses                          |
|------------------------|-------------------------------------------|
| `stream_small`         | streaming loop, small object              |
| `stream_ints_10k`      | streaming + number lexer                  |
| `stream_strings_10k`   | streaming + string lexer                  |
| `typed_struct`         | FromJson dispatch + key matching          |
| `vec_i64_10k`          | typed integer parsing (worst-case slot)   |
| `vec_borrowed_str_10k` | typed borrowed-string fast path           |
| `vec_string_10k`       | typed owned-string allocation path        |

Each loops the parse tens of thousands to millions of times, sized
to take ~5 seconds.

### Capture and view

Wall-clock sampling with full call graph:

```sh
perf record -g --call-graph=dwarf \
    target/profiling/profile vec_i64_10k
perf report
```

Flamegraph (visual):

```sh
perf script | inferno-collapse-perf | inferno-flamegraph > flame.svg
xdg-open flame.svg
```

If `perf record` exits with "permission denied" even after the sysctl,
check that `/proc/sys/kernel/perf_event_paranoid` actually returned `1`
or lower (some distros restore it on reboot — make it persistent in
`/etc/sysctl.conf`).
