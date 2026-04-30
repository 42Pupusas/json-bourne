# Profiling bourne

Two complementary harnesses:

- **`profile` binary** — long-running, single-workload, for `perf` /
  flamegraph. Wall-clock samples on real hardware.
- **`iai` benchmark** — deterministic instruction count via
  `valgrind --tool=callgrind`. Same number every run, so even a 1%
  regression is a real signal.

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

## iai-callgrind

### One-time setup

Requires `valgrind`:

```sh
# Debian/Ubuntu
sudo apt install valgrind
# Fedora
sudo dnf install valgrind
# Arch
sudo pacman -S valgrind
```

### Run

```sh
cargo bench -p bourne-bench --bench iai
```

The first run prints absolute instruction counts. Subsequent runs print
deltas against the saved baseline, so you see exactly how many
instructions a change cost or saved per workload.

To establish a new baseline (e.g. after a known improvement):

```sh
cargo bench -p bourne-bench --bench iai -- --save-baseline
```

iai-callgrind is roughly 50–100× slower than the actual benchmark
(valgrind interprets every instruction) so don't expect interactive
turnaround — it's a "before-and-after a real change" tool, not an
exploratory one.
