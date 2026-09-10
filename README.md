# json-bourne

Type-driven JSON for Rust. `no_std`-first, zero dependencies, zero allocations
for borrowed parses.

```rust
use json_bourne::{FromJson, parse_str};

#[derive(Debug, PartialEq, FromJson)]
struct User<'input> {
    id: u64,
    name: &'input str,
    active: bool,
}

let u: User<'_> = parse_str(r#"{"id":1,"name":"alice","active":true}"#).unwrap();
assert_eq!(u.name, "alice");
```

```rust
use json_bourne::{ToJson, to_string};

#[derive(ToJson)]
struct Point { x: i32, y: i32 }

let s = to_string(&Point { x: 3, y: -7 }).unwrap();
assert_eq!(s, r#"{"x":3,"y":-7}"#);
```

## Why

`json-bourne` skips the generic `Value` middle layer. Each type knows how to parse
itself directly from the lexer — the typed structure already enforces JSON's
grammar, so the per-event state machine is pure overhead for typed consumers.
The result is faster typed parsing and zero allocations on the borrow path.

- **Derive-driven.** `#[derive(FromJson, ToJson)]` (the `derive` feature) generates
  the typed impls. The generated code itself is `no_std`; only the compile-time
  derive pulls in the proc-macro stack.
- **`no_std` everywhere.** The streaming `Lexer` / `Parser` layer is `no_std`
  always; with the default `std` feature off the crate is `no_std + alloc`,
  and turning `alloc` off too gives a pure `no_std` build (`HashMap` /
  `std::net` / `std::path` / `std::io` adapters live behind `std`).
- **Borrowed strings by default.** `&'input str` and `Cow<'input, str>` parse
  zero-copy when the input contains no escapes.
- **Bounded by construction.** Container nesting is depth-limited (default 128,
  const-generic). The streaming parser is a state machine with no recursion.

The `bourne-bench` crate is workspace-internal and is not published.

## Features

| Feature     | Default | Pulls in                                            |
|-------------|---------|-----------------------------------------------------|
| `std`       | yes     | `alloc`, `HashMap`, `std::net`, `std::path`, `io::Write` adapter |
| `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, `BTreeMap`/`Set`, escape decoding, `to_string`/`to_vec` |
| `derive`    | no      | `#[derive(FromJson, ToJson)]` via the companion `bourne-derive` crate |

`json-bourne` has **zero dependencies** in its default build. With the `derive`
feature on, `bourne-derive` (and its `syn`/`quote` build stack) joins the graph,
but only at compile time — generated code stays dependency-free. The old
optional `indexmap` integration (insertion-order maps) was removed; if you
need that, wrap your own `IndexMap` in a newtype and implement `FromJson`/`ToJson` for the newtype, or use the
streaming `Lexer` to feed one directly.

`json-bourne` builds in `no_std + alloc` with `default-features = false, features = ["alloc"]`.
For pure `no_std` (streaming `Lexer` / `Parser`, stack-only typed parsing, and float
serialization through `to_fmt` / `FmtWriteSink`) build with `default-features = false`.

## Status

The public API is reserved-the-right-to-break until v1.0. The streaming layer is
fuzzed; conformance against [JSONTestSuite][jts] is asserted in CI; zero-allocation
guarantees are pinned by a counting-allocator test. `scripts/ci.sh` reproduces the
CI job list locally.

[jts]: https://github.com/nst/JSONTestSuite

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this crate by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
