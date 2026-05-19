# bourne

Type-driven JSON for Rust. `no_std`-first, zero dependencies, zero allocations
for borrowed parses.

```rust
use bourne::{from_json, parse_str, to_string};

from_json! {
    #[derive(Debug, PartialEq)]
    struct User<'input> {
        id: u64,
        name: &'input str,
        active: bool,
    }
}

let u: User<'_> = parse_str(r#"{"id":1,"name":"alice","active":true}"#).unwrap();
assert_eq!(u.name, "alice");
```

## Why

`bourne` skips the generic `Value` middle layer. Each type knows how to parse
itself directly from the lexer — the typed structure already enforces JSON's
grammar, so the per-event state machine is pure overhead for typed consumers.
The result is faster typed parsing and zero allocations on the borrow path.

- **No proc-macros.** `from_json!` and `to_json!` are declarative
  `macro_rules!`, so the dependency graph is empty.
- **`no_std` everywhere.** `bourne-core` is `no_std` always; `bourne` is
  `no_std + alloc` with optional `std` for the `HashMap` / `std::net` /
  `std::path` / `std::io` adapters.
- **Borrowed strings by default.** `&'input str` and `Cow<'input, str>` parse
  zero-copy when the input contains no escapes.
- **Bounded by construction.** Container nesting is depth-limited (default 128,
  const-generic). The streaming parser is a state machine with no recursion.

## Crates

| Crate          | Purpose                                                    |
|----------------|------------------------------------------------------------|
| `bourne-core`  | Streaming `Lexer` and `Parser`. `no_std`, zero alloc.      |
| `bourne`       | `FromJson` / `ToJson` traits, primitive impls, macros.     |

The `bourne-bench` and `bourne-alloctest` crates are workspace-internal and
are not published.

## Features (`bourne` crate)

| Feature     | Default | Pulls in                                            |
|-------------|---------|-----------------------------------------------------|
| `std`       | yes     | `alloc`, `bourne-core/std`, `HashMap`, `std::net`, `std::path`, `io::Write` adapter |
| `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, `BTreeMap`/`Set`, escape decoding, `to_string` |
| `indexmap`  | no      | `FromJson`/`ToJson` for `indexmap::IndexMap` (insertion order) |

`bourne` builds in `no_std + alloc` with `default-features = false, features = ["alloc"]`.
For pure `no_std` (`bourne-core` only) build with `default-features = false`.

## Status

This is a v0.1 release. The public API is reserved-the-right-to-break until
v1.0. The streaming layer is fuzzed; conformance against
[JSONTestSuite][jts] is asserted in CI; zero-allocation guarantees are pinned
by a counting-allocator test.

[jts]: https://github.com/nst/JSONTestSuite

## License

Dual-licensed under MIT and Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE`.
