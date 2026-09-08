# Changelog

All notable changes to bourne will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- The parse nesting stack packs one frame into a single bit (`u128`), so
  constructing a lexer clears 24 bytes instead of 136 and the struct is
  register-friendly. Nesting depth is now capped at 128 for *all* custom
  `MAX_DEPTH` parameterizations — `Stack::new` asserts this at compile
  time for non-default choices (the default is 128), a breaking change
  for anyone who raised it past 128 (audit 4.4.4).

### Tests
- Fused integer paths (`parse_i64_value` / `parse_u64_value` /
  `parse_i128_value`) now have direct lexer-level tests: 1–19 digit
  literals against `str::parse`, the 19/20-digit and i64/i128 range
  boundaries, stop-at-first-non-digit cursor position, and rejection of
  trailing fraction/exponent after full-length literals.

### Performance
- Pretty-printed parse benches added (`pretty_*` fixtures and a
  `pretty_stream_vs_dom` group): arrays and a small object, one newline +
  indentation per member. Baseline shows bourne's streaming parser ahead
  of serde_json's DOM on pretty input by the same margin as compact input
  (audit 4.4.1).
- String-escape scanning uses the lexer's SSE2 shape: a 16-byte compare
  locates the next quote/backslash/control byte instead of a per-byte walk,
  for both serialization (`ser::escape::find_escape`) and, unchanged,
  decoding. Escape-sparse strings — the common payload — copy as large
  literal runs (audit 4.5.1).
- The slice writer reserves in bounded 256 KiB windows and re-hints as it
  crosses each window, instead of reserving `len * (MAX + 1)` up front:
  a 10 M-element `Vec<i64>` reserved ~210 MB against ~30 MB of output;
  peak extra reservation is now one window (audit 4.5.2). Same-run
  medians on the float-array bench hold within noise of the previous
  design (bourne 283 µs vs serde 255 µs at n=10 000, vs 250/227 before),
  with the raw-tail write path preserved inside each window.
- The derive-vs-hand-written serialize gap closed (audit 4.1): generated
  writers emit fused fast paths for `f64`, `f32`, and `bool` fields and go
  through `Lexer::parse_u64_value` for unsigned fields, and the pretty-
  print rework let the derive write `bool` via a branchless `true`/`false`
  literal. `to_json_struct_metric` (the 18% deficit scenario) now runs
  even with the hand impl; `to_json_struct_small` median 79.6 ns vs
  hand 89.7 ns.

### Fixed
- `char` deserialization of escape-bearing single-char strings allocated a
  `String` for a decoded value at most 4 bytes; it now decodes onto a
  stack scratch through the same escape walk general strings use.
- Inputs over `MAX_INPUT_LEN` (~2 GiB) reached `parse` as a panic via
  `Lexer::new`'s const `assert!`; they now return
  `ErrorKind::InputTooLarge` from `parse`, and `Lexer::try_new` /
  `Parser::try_new` expose the same `Result` construction (the panicking
  `new` constructors remain for const callers) (audit 3.15).
- `JsonWrite::write_raw_bytes` `expect`ed valid UTF-8 on a `pub` method
  fed by user-written `ToJson` impls; non-UTF-8 input now returns `Err`
  (`ErrorKind::InvalidUtf8`) through a new `on_invalid_utf8` hook each
  sink maps into its own error type (audit 3.15). The `is_option` derive
  helper documents its known alias blindness (`type Maybe<T> =
  Option<T>` reads as required — same limitation as serde).
- Variant tags in derived enums were written through the raw byte-literal
  writer, so a `#[bourne(rename = "a\"b")]` produced invalid JSON (`"a"b"`),
  and the bare-string unit-variant parse path rejected every escaped tag.
  Tags now serialize through the escaping writer and parse through the
  borrow-or-decode key path used for object keys; escape-bearing renames
  round-trip in all three tagging modes. `json_bourne::KeyCow` is a new
  public alias for the decoded tag type the derive emits.
- Field attributes (`rename`, `skip`, `default`) inside enum struct variants
  were silently ignored: the generated code matched the verbatim Rust field
  name on parse and fused it into the output on serialize. Variant fields
  now resolve keys exactly like plain structs — explicit `rename` wins,
  then the enum's `rename_all`, then the field name — and honor `skip` and
  `default` in all four tagging modes.
- Lenient structs (`deny_unknown_fields = false`) failed on unknown values
  containing escape-bearing keys: the skip path decoded every key it walked
  and rejected the escape, discarding a value the caller never asked to
  read. Keys inside skipped objects are now consumed as raw byte spans —
  still shape-validated, never decoded — so `InvalidEscape` surfaces only
  for keys and strings the caller actually receives.
- The `f64`/`f32` non-finite error path delivered its failure by writing a
  fabricated `NaN` through the sink. In-crate sinks reject NaN, so the error
  surfaced — but a custom sink whose `write_float_f64` tolerates NaN would
  have silently serialized placeholder bytes. The writer now hands the sink
  the actual value, so its documented reject-non-finite contract is what
  produces the error (audit 3.14).
- Map keys typed `&str` reported escape-bearing keys as `InvalidEscape`,
  accusing the document of a malformed escape when the escape is valid and
  the real limitation is that a borrow-only key type cannot hold a decoded
  key. A dedicated `ErrorKind::BorrowedKeyNeedsDecode` names the actual
  problem and suggests the owning alternatives (audit 3.13).
### Changed
- Inputs over `MAX_INPUT_LEN` (~2 GiB) reach `parse` as
  `ErrorKind::InputTooLarge` instead of panicking; `Lexer::try_new` and
  `Parser::try_new` expose the same `Result`-returning construction, and
  the panicking `new` constructors remain for const callers (audit 3.15).
- `JsonWrite::write_raw_bytes` returns `Err` (`ErrorKind::InvalidUtf8`, via
  a new required `on_invalid_utf8` hook each sink maps into its own error
  type) for non-UTF-8 input instead of `expect`-panicking — the bytes can
  come from a user-written `ToJson` impl the compiler cannot verify (audit
  3.15).
- `char` deserialization decodes escapes onto a 4-byte stack scratch via
  the same escape walk as general strings instead of allocating a `String`
  for a value at most 4 bytes (audit 3.15); the `is_option` derive helper
  documents its known alias blindness (`type Maybe<T> = Option<T>` reads
  as required — same limitation as serde).
- `Parser::parse_i64_value` / `parse_str_value` did not update the grammar
  state, so trailing data after a scalar root document went undetected
  (`Parser::new(b"1 2").parse_i64_value()` followed by `next_event` yielded a
  second document instead of `TrailingData`), a missing comma inside a
  container could pass silently, and resuming event-driven parsing after a
  fast-path scalar mis-parsed. Both now sync state through the same shared
  helper as the sibling fast-path methods, which also deduplicates the
  state-after-value logic (audit 3.12).
- Internally tagged enums accepted a duplicated tag key: `{"type":"Dog",
  "type":123}` parsed (the unit arm skipped any further `tag` key and struct
  variants skipped it as a sibling). The tag now behaves like a named-struct
  field — a second occurrence reports `DuplicateKey` in both variant forms
  (audit 3.11).
- Empty tuple structs and empty tuple variants were rejected for the derive
  in every mode: their writer emitted `[]` but the tuple reader reports
  `TypeMismatch` on an empty array, so `to_json` output could never parse
  back. They now fail at compile time with an explanation, matching the
  existing unit-struct rejection (audit 3.10).
- The string-escape walk lives in one place (`ser::escape`) shared by every
  sink through `JsonWrite`. Previously `StringSink`, `ByteSink` and
  `PrettyStringSink` each carried their own copy of the same run-splitting
  loop, and `PrettyStringSink::object_key` escaped keys by constructing a
  transient `StringSink` around its own buffer. No behavior change; the
  cross-sink agreement test now also covers `IoWriteSink`, the one sink
  that uses the default walk.
- `to_string_pretty` serialized derive-generated types compactly: the
  derive fused structural bytes (`,"id":`) into raw literals that the
  pretty sink treats as opaque text, so a derived struct printed on one
  line while a `Vec` field inside it was indented. `JsonWrite` now has
  explicit structural methods (`begin_object`, `object_key`,
  `separator`, …) that pretty sinks override, and an associated const
  (`FUSES_STRUCTURAL_BYTES`) lets generated writers keep the fused
  compact path for byte-oriented sinks — compact output is byte-identical
  and the pretty path folds the choice at compile time. This also removes
  the pretty sink's bracket-reparsing in `write_byte` and its
  `depth -= 1` underflow hazard.
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
