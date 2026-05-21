# json-bourne

Type-driven JSON for Rust. `no_std`-first, zero dependencies, zero allocations
for borrowed parses.

```rust
use json_bourne::{from_json, parse_str, to_string};

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

`json-bourne` skips the generic `Value` middle layer. Each type knows how to parse
itself directly from the lexer — the typed structure already enforces JSON's
grammar, so the per-event state machine is pure overhead for typed consumers.
The result is faster typed parsing and zero allocations on the borrow path.

- **No proc-macros.** `from_json!` and `to_json!` are declarative
  `macro_rules!`, so the dependency graph is empty.
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
| `alloc`     | yes     | `String`, `Vec`, `Box`/`Rc`/`Arc`, `BTreeMap`/`Set`, escape decoding, `to_string` |
| `indexmap`  | no      | `FromJson`/`ToJson` for `indexmap::IndexMap` (insertion order) |

`json-bourne` builds in `no_std + alloc` with `default-features = false, features = ["alloc"]`.
For pure `no_std` (streaming `Lexer` / `Parser` only) build with `default-features = false`.

## Status

This is a v0.1 release. The public API is reserved-the-right-to-break until
v1.0. The streaming layer is fuzzed; conformance against
[JSONTestSuite][jts] is asserted in CI; zero-allocation guarantees are pinned
by a counting-allocator test.

[jts]: https://github.com/nst/JSONTestSuite

## License

Licensed under MIT. See `LICENSE-MIT`.
