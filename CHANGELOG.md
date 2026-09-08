# Changelog

All notable changes to bourne will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Variant tags in derived enums were written through the raw byte-literal
  writer, so a `#[bourne(rename = "a\"b")]` produced invalid JSON (`"a"b"`),
  and the bare-string unit-variant parse path rejected every escaped tag.
  Tags now serialize through the escaping writer and parse through the
  borrow-or-decode key path used for object keys; escape-bearing renames
  round-trip in all three tagging modes. `json_bourne::KeyCow` is a new
  public alias for the decoded tag type the derive emits.
- The default `JsonWrite::write_escaped_str` pushed every byte through the
  byte-oriented `write_byte`, so sinks whose `write_byte` is char-oriented
  mangled non-ASCII strings: `to_fmt("é")` produced `"Ã©"`. The default now
  splits the string into literal runs and copies each through `write_str_raw`,
  keeping multi-byte UTF-8 intact; all sinks now agree byte-for-byte on any
  input (pinned by a cross-sink test over unicode/escape/control mixes).
- `#[bourne(tag = "…", rename_all = "…")]` (internally tagged) and the adjacent
  `tag`/`content` form now apply `rename_all` (and per-variant `rename`) when
  parsing, matching the serialize side. Previously these enums emitted their
  cased variant tags and then rejected their own output with `UnknownField`;
  round-trips work for all four tagging modes now.
- `Vec<u64>` / `Vec<usize>` and `u64`/`usize` fields in derived structs no
  longer reject values above `i64::MAX`. The fused array fast path went through
  the signed parser and failed on exactly the values `u64` exists to hold;
  a new `Lexer::parse_u64_value` (rejecting a leading `-`, including `-0`)
  backs the unsigned path, and the derive emits it too. All unsigned entry
  points now agree: `-0` and negatives are rejected, `u64::MAX` parses.
- `#[derive(FromJson)]` structs gain fused fast paths for `f64`, `f32`, and
  `bool` fields (previously only integers and `&str` bypassed the generic
  dispatch), matching the hand-written-impl shape.
- **(security)** The array serialization fast path treated
  `ToJson::MAX_SERIALIZED_LEN` — a safe associated const on a public trait — as
  a hard precondition for unchecked `ptr::write` appends, so a downstream impl
  that under-declared its bound could trigger a heap buffer overflow from
  entirely safe code. The bound is now a reservation hint only: hinted sink
  writes take a raw tail write when capacity remains and fall back to the
  checked path otherwise. The `write_byte_unchecked`,
  `write_float_f64_unchecked{,_finite}`, `write_float_f64_taint`,
  `write_json_in_reserved`, `NEEDS_VALIDATION`, `pre_validate_slice` and
  `take_nonfinite_taint` APIs are removed; `JsonWrite::write_byte_hinted` and
  `JsonWrite::write_float_f64_hinted` replace them and are safe.
- **(security)** `JsonStr::as_str` and `JsonNum::as_str` built `&str` values
  with `str::from_utf8_unchecked` on a caller-supplied buffer, so passing any
  other byte slice of sufficient length produced a `&str` over unvalidated
  bytes. Both are now fully safe (`from_utf8`); the unchecked borrow path
  survives as `pub(crate) JsonStr::as_str_in_input` for the crate's own
  escape-decoding impls, where the input invariant is established by the
  lexer. `JsonNum` decoding accessors (`as_i64`/`as_u64`/…) are unaffected —
  they never consult `as_str`.
- CI no longer references the removed `bourne-core` package; the workflow and
  `scripts/ci.sh` (new — reproduces the CI job list locally) now target
  `json-bourne` / `bourne-derive`, and the bare-metal `no_std` job builds
  `json-bourne` with and without `alloc`.
- Enabling both of `bourne-bench`'s allocator features (`alloc-profile` and
  `compare-mem`) is now a `compile_error!` instead of a link-time
  `#[global_allocator]` conflict.

## [0.2.2] - 2026-07-23

### Changed
- `bourne-derive` now depends on `syn 3` (was `syn 2`). No generated-code
  or attribute-surface changes; `syn 3`'s `ItemImpl`/API churn doesn't
  touch any construct `bourne-derive` uses. Downstream consumers that
  also depend on `syn` directly (e.g. build-time codegen tools) can now
  unify on a single `syn` major version instead of carrying both 2 and 3
  in their dependency tree.

## [0.2.1] - 2026-07-14

### Fixed
- `#[derive(ToJson)]` on a named struct emitted invalid JSON — a missing
  separator comma — whenever a plain (un-renamed) field was immediately
  followed by a `#[bourne(skip_if_none)]` `Option` field that was present.
  The fast path that folds a plain field's `,"key":` into a compile-time
  literal never set the internal `__first` flag to `false`, so the
  following optional field's runtime comma (gated on `!__first`) was
  wrongly suppressed, producing output like `{"a":1,"b":2"c":3}`. This
  broke every OpenAI-compatible API request in downstream consumers
  (`ChatRequest`'s `stream` → `stream_options`, assistant tool-call
  messages' `role` → `tool_calls`). Fixed by setting `__first = false` in
  the plain fast path; added a regression test.

## [0.2.0] - 2026-07-13

### Added
- Container attribute `#[bourne(rename_all = "…")]` for the `from_json!`,
  `to_json!`, and `json!` macros. Supports all eight serde casings
  (`lowercase`, `UPPERCASE`, `PascalCase`, `camelCase`, `snake_case`,
  `SCREAMING_SNAKE_CASE`, `kebab-case`, `SCREAMING-KEBAB-CASE`) and applies
  to struct field keys and externally-tagged enum variant tags on both the
  parse and serialize sides. An explicit per-field / per-variant
  `#[bourne(rename = "…")]` still wins. Case conversion runs at compile time
  via a dependency-free `const fn` (no proc-macro).
- `#[bourne(deny_unknown_fields = false)]` is now accepted by the combined
  `json!` macro (previously only `from_json!` honored it), so a type that
  needs both round-trip impls can also opt into lenient parsing.

### Internal
- New dependency-free compile-time case converter (`src/casing.rs`),
  exposed via inherent `const fn` methods on `Casing` (`from_name`,
  `convert`, `rename`). A `#[const_trait]` would be the natural home but
  const traits are still unstable on stable Rust; revisit when
  `const_trait_impl` lands.

## [0.1.0] - 2026-05-02

Initial public release.

### Added
- Streaming `Lexer` and `Parser` — `no_std`, zero alloc.
- Typed `FromJson` / `ToJson` traits with primitive, composite, alloc, and
  std impls (Vec, BTreeMap/HashMap, Option, tuples, fixed arrays, Box/Rc/Arc,
  Cow, char, Duration, SystemTime, IpAddr family, PathBuf).
- Declarative `from_json!` and `to_json!` macros covering structs, tuple
  structs, externally / internally / adjacently tagged enums, untagged
  enums; field attrs `rename`, `default`, `skip`, `skip_if_none`; container
  attr `deny_unknown_fields = false`.
- `to_string`, `to_vec`, `to_writer`, `to_string_pretty` entry points.
- `JsonWrite` sink trait with `StringSink`, `IoWriteSink`, `FmtWriteSink`,
  `PrettyStringSink`.
- Optional `indexmap` feature: `FromJson` / `ToJson` for `IndexMap` /
  `IndexSet` preserving insertion order.
- Bounded depth (default 128, const-generic).
- SSE2 fast paths on `x86_64` for ASCII string scan and backslash search.
- `cargo-fuzz` targets for streaming and typed parsing.
- JSONTestSuite conformance test (~318 cases).
- proptest property tests including full-range f64 round-trip.
- Counting-allocator regression test
  (`crates/bourne/tests/zero_alloc.rs`) pinning the zero-allocation
  guarantees.

[Unreleased]: https://github.com/illuminodes/bourne/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/illuminodes/bourne/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/illuminodes/bourne/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/illuminodes/bourne/releases/tag/v0.1.0
