# Changelog

All notable changes to bourne will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **derive: pretty serialization of renamed and conditional fields emitted
  invalid JSON** — missing separators between object members. The
  runtime-separator codegen gated the comma on the sink's
  `FUSES_STRUCTURAL_BYTES` flag, which pretty sinks set false, so the
  comma was dropped along with the fusion: `to_string_pretty` returned
  `Ok` with `{\n  "a": 1"second": 2\n}` for a struct using `rename` or
  `skip_if_none`. Separator emission is a structural event on every
  sink; only the key+colon representation may fuse. Affected renamed
  fields, `skip_if_none` fields, members following a skipped one, enum
  struct variants under internal/adjacent tagging, and `rename_all`
  combinations (release audit R1).
- derive: contradictory enum configuration is now rejected at compile
  time instead of producing output the reader rejects: `tag` and
  `content` naming the same member, `content` without `tag`, and
  `untagged` combined with `tag`/`content` (release audit R3).
- tests/build: the self dev-dependency no longer re-enables default
  features, so `cargo test --no-default-features` now runs what it
  claims: alloc-dependent integration tests are declared via
  `required-features`, and `derive` implies `alloc` (generated readers
  walk keys through the alloc-gated `KeyCow` API) (release audit R4).
- lexer: the SSE2 string scanner's register-only intrinsic calls are
  now inside explicit `unsafe` blocks, silencing the nine E0133
  warnings under the 1.85 MSRV (edition-2024 `unsafe fn` bodies do not
  grant implicit unsafe context) while remaining warning-clean on
  current toolchains via `unused_unsafe` (release audit R7).

### Changed

- CI: the host test matrix no longer builds bare-metal targets it never
  installs. Cross-target builds moved entirely into the `no_std` job
  (which installs its matrix target) and the shared script accepts the
  target as an argument. Windows runs the script under an explicit bash
  shell (release audit R2).
- docs: the Duration/SystemTime adapters document binary64 rounding
  honestly instead of promising nanosecond preservation at all
  magnitudes; the serializer module header no longer describes the
  float formatter as a future ryu port; the 2026-09 audit's float
  branch-miss attribution percentages carry a correction note
  (release audits R5/R8).

### Changed

- **Breaking:** `Lexer::new` / `Lexer::try_new` and `Parser::new` /
  `Parser::try_new` now exist only at the default depth and work with no
  type annotation. Previously they lived in the generic impl block, which
  defeated the struct's `= DEFAULT_MAX_DEPTH` const default: the bare
  `Parser::new(b"{}")` shape failed to compile with E0284, and every call
  site had to carry a `Parser<'_>`-style annotation (audit 2026-09 F1).
  Custom depths now go through the new `Lexer::with_depth` /
  `Parser::with_depth`, which keep the depth choice visible at the call
  site (the depth is also the inline stack's size) and panic like before
  on depth > 128. Code that built custom-depth lexers with `new` — e.g.
  `let l: Lexer<'_, 8> = Lexer::new(..)` or `Lexer::<8>::new(..)` — must
  switch to `with_depth`.

### Removed

- **Breaking:** the optional `indexmap` feature and its `FromJson`/`ToJson`
  impls for `IndexMap`/`IndexSet` (present since 0.1.0). If you relied on
  insertion-order maps, wrap your own `IndexMap` in a newtype and implement
  the traits on the newtype, or feed one from the streaming `Lexer`.

### Changed

- The crate-level `#![allow(unsafe_code)]` is gone; the workspace
  `unsafe_code = "deny"` lint is now live for `json-bourne`. The seven
  existing `unsafe` sites each carry their own `#[allow(unsafe_code)]`
  next to the `SAFETY:` comment that justifies them, so a *new* `unsafe`
  fails the build until it is justified in place (audit 3.2).
- `parse_f64_value` gained a fused fast path for plain decimal literals
  (no exponent) whose mantissa fits 2^53: the mantissa and `10^-k` are
  both exact, so one IEEE divide is correctly rounded and bit-identical
  to `str::parse` — no `JsonNum`, no `String`, no libcore Eisel-Lemire
  machinery. 1.9× on a plain-decimal `Vec<f64>` array; exponent-heavy
  payloads unchanged. Guarded by differential tests: every 3-digit
  fraction shape, boundary mantissas at 2^53, and 20 000 seeded random
  literals checked bit-for-bit against `str::parse` (audit 4.4.3).
- The parse nesting stack packs one frame into a single bit (`u128`), so
  constructing a lexer clears 24 bytes instead of 136 and the struct is
  register-friendly. Nesting depth is now capped at 128 for *all* custom
  `MAX_DEPTH` parameterizations — `Stack::new` asserts this at compile
  time for non-default choices (the default is 128), a breaking change
  for anyone who raised it past 128 (audit 4.4.4).
- ser: `write_display` (the `std::net` `IpAddr`/`Ipv4Addr`/`Ipv6Addr`/
  `SocketAddr` writers) formats into a 64-byte stack buffer
  (`DisplayScratch`) instead of allocating a `String` per value
  (audit 4.5.3).
- derive: adjacently-tagged enums no longer skip and re-read the payload
  when the tag key precedes `content` (the order the serializer emits):
  the payload is parsed exactly once from a checkpoint, and a tail walk
  preserves the old trailing-key rejection (audit 4.5.4).
- **Breaking (no_std):** `ToJson` for `f64`/`f32` and the `JsonWrite`
  float methods are no longer `alloc`-gated, so no-std builds now
  serialize floats (audit 5.4). `FmtWriteSink` is available without
  `alloc`. `StringSink`, `ByteSink`, `to_string`, `to_vec`, `to_fmt`,
  `to_string_pretty`, and `MapKeyOut` are unchanged and remain
  `alloc`-gated.

### Tests
- Escape handling is one module (`src/escape/`): `Hex4` reads `\uXXXX`
  digits for the decoder, the validator and the lexer, replacing the
  `parse_hex4` that was copied into both `de.rs` and `lexer.rs`;
  `EscapeDecoder` and `EscapeValidator` own the two walks. `de.rs` lost
  ~200 lines (audit 3.15/6).
- `bourne-derive/src/lib.rs` shrank from 1 902 to 101 lines, split into
  `attrs`, `field_plan`, `naming`, `acquire`, `generics`, `shape`, and
  the `from_json` / `to_json` codegen modules; every free function is now
  a method on the type that owns it. Generated output is unchanged for
  every case that already worked (audit 6).
- `lib.rs` shrank from 3 751 to 1 461 lines: its ~2 300 lines of inline
  `#[cfg(test)]` modules moved to nine `src/tests/*.rs` modules (those
  reaching crate internals) and three `tests/` integration files
  (`ser_roundtrip`, `to_json_derive`, `derive_roundtrip` — public API
  only, so running them out-of-crate also pins that the surface is
  exported and the derives resolve for external consumers). No test was
  added, removed, or changed: 281 before and after (audit 6).
- Fused integer paths (`parse_i64_value` / `parse_u64_value` /
  `parse_i128_value`) now have direct lexer-level tests: 1–19 digit
  literals against `str::parse`, the 19/20-digit and i64/i128 range
  boundaries, stop-at-first-non-digit cursor position, and rejection of
  trailing fraction/exponent after full-length literals.

### Fixed
- `char` accepted some multi-character strings on the escape path.
  `parse_str::<char>(r#""\u0041BCDE""#)` returned `Ok('A')` instead of
  an error: the 4-byte stack buffer backing the no-allocation escape
  decode discarded any run too large to fit, leaving exactly one scalar
  for the "exactly one scalar" check to accept. Inputs whose tail only
  partly fit (`"\u0041BC"`) were rejected, so the existing tests missed
  it. The decode sink now reports overflow as `TypeMismatch`. Strings
  with no escapes were never affected (audit 3.15/6).
- derive: `#[bourne(skip_if_none)]` inside an enum struct variant emitted
  the field anyway (`{"Rec":{"a":1,"note":null,"b":2}}`), and
  `#[bourne(deny_unknown_fields = false)]` on an enum was ignored inside
  its variants, so an unknown key still returned `UnknownField`. Both
  worked correctly on plain named structs. These were the last two
  instances of audit 3.6: variant fields and struct fields had separate
  codegen paths, and the earlier 3.6 fix had only ported the attributes
  it enumerated. Both shapes now build one `FieldPlan` consumed by a
  shared object reader/writer, so the two paths cannot disagree again
  (audit 3.6/6).
- docs: the README and rustdoc feature tables still said `f64`/`f32`
  round-trips need `alloc` after the float serializer was un-gated;
  `to_fmt` is documented as the allocation-free float path and ships in
  `no_std` builds (audit 5.2/5.4).
- derive: `FromJson` field types written as `&'a str` with a lifetime name
  other than `'input` silently took the generic `FromJson` dispatch instead
  of the direct string read. `acquire_expr` now matches the type
  structurally, so any borrowed-`str` field takes the fast path (audit
  4.3.3/4.3.4).
- `to_string` returned `Err` for parser-visible failures but *panicked* on
  non-UTF-8 bytes injected by a hand-written `ToJson` impl through the
  unvalidated `ByteSink::write_raw_bytes` path. It now returns
  `Err(ErrorKind::InvalidUtf8)`. Previously it panicked with an `expect`
  whose message claimed the case was impossible (audit 4.4.5).

- derive: `FromJson` key dispatch on structs and enum variants now compiles
  to literal-pattern matches (length-switch + memcmp) instead of sequential
  string-guard evaluation; `rename_all` keys are hoisted into `const` items
  computed at compile time. Derived deserialize closes from 1.145× to 1.107×
  of the hand-written impl on the `metric_events` fixture (audit 4.3.1).

### Performance
- derive: key acquisition is now borrowed-first. Escape-free keys — the
  overwhelming majority — are borrowed with a single string walk; an
  escape-bearing key is decoded by one retry through the existing
  `key_to_cow` path. Combined with the literal-pattern dispatch, the derived
  struct impl now parses `metric_events_1000` slightly *faster* than the
  hand-written bench impl (156.5 µs vs 164.1 µs median), closing the audit's
  ~18 % derive gap entirely (audit 4.3.2).
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
