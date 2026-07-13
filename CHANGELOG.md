# Changelog

All notable changes to bourne will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/illuminodes/bourne/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/illuminodes/bourne/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/illuminodes/bourne/releases/tag/v0.1.0
