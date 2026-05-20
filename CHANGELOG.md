# Changelog

All notable changes to bourne will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-05-02

Initial public release.

### Added
- Streaming `Lexer` and `Parser` (`bourne-core`) — `no_std`, zero alloc.
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

[Unreleased]: https://github.com/illuminodes/bourne/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/illuminodes/bourne/releases/tag/v0.1.0
