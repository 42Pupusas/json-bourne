# json-bourne 0.2.2 — correctness and performance audit

Commit audited: `de73ec9` (Bump bourne-derive to syn 3; release 0.2.2).
Toolchain: cargo 1.97.1, x86_64 Linux. No source changes were made; this file
is the only artifact.

Method: full read of `crates/bourne/src/*` and `crates/bourne-derive/src/lib.rs`,
the test suite, benches and CI config; the workspace test/clippy suite in every
feature combination; a throwaway probe crate (outside the repo, since deleted)
that exercised the public API against the behaviours claimed in the docs; two
bench runs (`compare`, `floats`) at reduced sample counts for orientation only.

## 1. Summary

The core lexer/parser is in good shape: the JSONTestSuite corpus passes, the
property tests pass, `no_std`/`alloc`/`std`/`all-features` all build and pass
clippy with `-D warnings`, and the streaming and typed hot paths are measurably
faster than `serde_json` on every parse workload benchmarked here.

The problems concentrate in three places:

1. **Two soundness holes reachable from safe code** (§3.1, §3.2). One is in the
   serializer's "reserved" fast path, which trusts a *safe* associated const on
   a public trait before doing unchecked pointer writes. The other is
   `JsonStr::as_str` / `JsonNum::as_str`, which build a `&str` with
   `from_utf8_unchecked` from a caller-supplied buffer.
2. **The derive macro diverges from the hand-written impls and from itself**
   (§3.3–§3.9): unsigned integers above `i64::MAX` are rejected in `Vec<u64>` and
   derived fields but accepted at top level; internally/adjacently tagged enums
   ignore `rename_all` on parse so they don't round-trip; variant tags are
   written unescaped; field attributes inside enum struct variants are silently
   ignored; empty tuple structs serialize to something they refuse to parse.
3. **CI is not running.** The workflow references packages (`bourne-core`,
   `bourne`) that no longer exist, and `cargo test --workspace --all-features`
   fails to build locally because two bench features each install a global
   allocator (§5.1). Every guarantee the README makes about CI is currently
   unverified.

Performance is competitive. The one workload where json-bourne loses to
serde_json is float-array serialization at n=10 000 (§4.2); the derive is ~18 %
slower than the hand-written impl on the realistic struct (§4.3), and the
reasons are identifiable in the generated code.

Severity scale used below: **S0** memory-unsafety reachable from safe code;
**S1** wrong output / wrong accept-reject with no error; **S2** inconsistent or
surprising behaviour that is at least reported as an error; **S3** docs, hygiene,
performance.

## 2. Verification performed

| Check | Result |
|---|---|
| `cargo test -p json-bourne --all-features` | 251 unit + 8 integration binaries pass, 2 doctests ignored |
| `cargo test -p json-bourne` (default = std) | pass |
| `cargo test -p json-bourne --no-default-features --features alloc` | pass (247 unit; std-only tests compiled out) |
| `cargo check -p json-bourne --no-default-features` | pass |
| `cargo check -p json-bourne --no-default-features --features alloc,derive` | pass (re-run post-indexmap-removal) |
| `cargo clippy -p json-bourne --all-features --all-targets -- -D warnings` | clean |
| `cargo clippy -p json-bourne --no-default-features -- -D warnings` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` (default features) | clean |
| `cargo test --workspace --all-features` | **build failure** — `#[global_allocator]` conflict between `bourne-bench` lib (`alloc-profile`) and `compare_mem` bin (`compare-mem`) |
| `cargo clippy -p bourne-core -p bourne …` (the CI command) | **fails** — packages do not exist |
| JSONTestSuite conformance (`tests/conformance.rs`) | pass |
| `cargo graph --report` | reports 0 types; the tool produced no structural signal for this workspace |

Probe results (behaviour observed at the public API, all on 0.2.2):

```
parse_str::<u64>("18446744073709551615")                      -> Ok
parse_str::<Vec<u64>>("[18446744073709551615]")               -> Err(NumberOutOfRange)
parse_str::<Unsigned{value:u64}>({"value":18446744073709551615}) -> Err(NumberOutOfRange)
parse_str::<u64>("-0")                                        -> Err
parse_str::<Vec<u64>>("[-0]")                                 -> Ok([0])
Lenient (deny_unknown_fields=false) {"unknown":{"a\u0062":1}} -> Err(InvalidEscape)
#[bourne(tag="kind", rename_all="snake_case")] enum: to_string then parse_str -> Err(UnknownField)
{"kind":"SomeItem","kind":123} into that enum                 -> Ok
enum { #[bourne(rename = "a\"b")] Item }: to_string           -> "a"b"   (invalid JSON)
struct Empty(): to_string -> "[]"; parse_str::<Empty>("[]")   -> Err(TypeMismatch)
to_fmt("é", &mut String)                                      -> "\"Ã©\""
to_string_pretty(&Two{a:1,b:2})                               -> {"a":1,"b":2}   (not pretty)
to_string_pretty(&Outer{inners:vec![Inner{x:1},Inner{x:2}]})  -> {"inners":[\n  {"x":1},\n  {"x":2}\n]}
Parser::new(b"1 2"): parse_i64_value()=1; next_event() -> Some(Number) ; next_event() -> None
```

## 3. Correctness findings

### 3.1 S0 — `ByteSink` unchecked writes trust a safe associated const

`crates/bourne/src/ser.rs`

- `ToJson::MAX_SERIALIZED_LEN` (line ~850) is a plain associated const on a
  public, safe trait.
- `<[T] as ToJson>::write_json` (line ~1517) reserves
  `len * (MAX_SERIALIZED_LEN + 1) + 2` bytes and then calls
  `write_array_reserved`, which emits `[`, `,`, `]` through
  `JsonWrite::write_byte_unchecked`.
- `ByteSink::write_byte_unchecked` (line 427) does `ptr::write(ptr.add(len))`
  + `set_len` with no capacity check.
- Between two unchecked comma writes, the element is written by the *default*
  `write_json_in_reserved`, which forwards to the safe `write_json`.

A safe downstream type can declare `MAX_SERIALIZED_LEN = 1` and write 100 000
bytes in `write_json`. `Vec::extend_from_slice` then grows to exactly
`len + additional` when `additional >= cap` (amortized growth is
`max(2*cap, required)`), leaving `len == capacity`. The next
`write_byte_unchecked(b',')` writes one byte past the allocation. This is a heap
overflow reachable without the caller writing `unsafe`.

Recommended fix: move the reserved path behind a crate-sealed
`unsafe trait BoundedSerialize` (or a sealed marker trait implemented only for
the in-crate primitives) so the unchecked writer can only be entered for types
the crate itself vouches for. Alternatively keep the branch but make
`write_byte_unchecked` a `debug_assert!(len < cap)` plus documented `unsafe`
requirement on the *trait* — the sealed approach is preferable because it keeps
the invariant local. Add a regression test with a lying impl and run it under
miri.

### 3.2 S0 — `JsonStr::as_str` / `JsonNum::as_str` build `&str` from a caller buffer

`crates/bourne/src/event.rs:76` and `:136`.

Both are safe `pub fn`s that take an arbitrary `&[u8]` and call
`from_utf8_unchecked` on `input[start..end]`. The doc says "caller must pass
the same input the parser was constructed with", but nothing enforces it. Any
other buffer of sufficient length yields a `&str` over unvalidated bytes.

Recommended fix (pick one):

- Make the public versions checked (`core::str::from_utf8(raw).ok()`); keep an
  internal `pub(crate) unsafe fn as_str_unchecked` for the lexer's own use where
  the invariant is established. The checked path is one SIMD pass over a short
  span; the typed hot paths (`parse_str_value`, `String::from_lex`) do not go
  through these methods and stay unaffected.
- Or store the input's base pointer + length in `Lexer` only and expose
  `Lexer::str_of(&self, JsonStr) -> &'input str` so the span can never be
  paired with a foreign buffer. This removes the `input` parameter from the
  public span API entirely and is the cleaner long-term shape.

Also: `#![allow(unsafe_code)]` at crate level (`lib.rs` ~line 100) neutralises the
workspace `unsafe_code = "deny"` lint for the whole crate, so the per-item
`#[allow(unsafe_code)]` annotations are decorative. Remove the crate-level allow
so each new `unsafe` needs an explicit opt-in.
— *Landed 2026-09.* The crate-level allow is gone. Removing it surfaced
exactly seven sites that were relying on it — `JsonStr::as_str_in_input`,
`Lexer::parse_str_value`, `scan_ascii_string_run` and its `_sse2` callee,
and `ByteSink`'s `write_byte_hinted` / `write_float_f64_hinted` — each of
which already carried a `SAFETY:` comment but no attribute. Each now has
its own `#[allow(unsafe_code)]`, so the workspace `deny` is live for the
crate and any *new* `unsafe` fails the build until it is justified in
place. Verified with clippy `-D warnings` under both `--all-features` and
`--no-default-features` (the cfg gates select different unsafe sites).

### 3.3 S1 — Unsigned integers above `i64::MAX` are rejected on the fast paths

- `de.rs:146–160`: the `impl_int!` `vec_from_lex` override uses
  `Lexer::parse_i64_value` for **all** ten types including `u64`/`usize`, then
  `try_from`. `Vec<u64>` therefore rejects `[18446744073709551615]`.
- `bourne-derive/src/lib.rs:222–256` (`acquire_expr`): `u64` and `usize` fields
  are also routed through `parse_i64_value`. Any derived struct with a `u64`
  field rejects values in `(i64::MAX, u64::MAX]`.
- Top-level `parse_str::<u64>` goes through `JsonNum::as_u64` and accepts them.

Second inconsistency in the same code: `parse_i64_value` accepts `-0` and yields
`0`, so `Vec<u64>` accepts `[-0]`, while `as_u64` rejects `-0`.

Recommended fix: add `Lexer::parse_u64_value` (mirror of `parse_i64_value`,
rejecting a leading `-` the way `parse_u128_value` does), and use it for the
unsigned half of `impl_int!` and in `acquire_expr`. Add tests for `u64::MAX`,
`-0`, and `-1` through `Vec<u64>`, derived fields, and top level.

### 3.4 S1 — Internally and adjacently tagged enums ignore `rename_all` when parsing

`bourne-derive/src/lib.rs:733` and `:857` call
`key_expr(vname, &rename, &None)`; the `ToJson` side (`to_json_enum`) passes
`&container.rename_all`. A `#[bourne(tag = "kind", rename_all = "snake_case")]`
enum serializes `some_item` and then rejects its own output with
`UnknownField`. Externally tagged enums are correct.

Fix: pass `&container.rename_all` in both parse paths; add a round-trip test
per tagging mode × `rename_all`.

### 3.5 S1 — Variant tag values are written unescaped

Seven sites in `bourne-derive/src/lib.rs` (`1282, 1332, 1355, 1376, 1386, 1403,
1421`) emit the variant tag with `write_str_raw(#tagkey)` between literal
quotes. A `#[bourne(rename = "a\"b")]` produces `"a"b"`. Field keys and the
`tag`/`content` names go through `write_escaped_str` and are fine.

Fix: either `write_escaped_str(#tagkey)` (simplest; costs one scan per enum
write), or validate at derive time that a rename contains no byte needing
escaping and emit a compile error otherwise (keeps the fused-literal fast path).
The second matches the crate's design better.

### 3.6 S1 — Field attributes inside enum struct variants are silently ignored

`struct_variant_read` (line 597) and `internal_struct_variant_read` (line 794)
use `fname.to_string()` as the key and never call `parse_field_attrs`; the
`ToJson` side (`write_named_fields`) does the same. `rename`, `skip`, `default`,
`skip_if_none` and container `rename_all` have no effect on variant fields, and
because the `bourne` attribute is registered by the derive, rustc does not warn
either. Unknown attribute names inside variants are also not rejected.

Fix: route variant fields through the same `FieldAttrs` model as named structs
(this is also the SRP cue to extract a `FieldPlan` type shared by both paths —
see §6).

— *Landed 2026-09, in two stages, and the second stage is the lesson.* The
first fix added a `VariantField` type carrying parsed attrs and a resolved
key, which made `rename`/`rename_all`/`skip`/`default` work. But it left the
variant path as a **separate implementation** beside the named-struct one, so
only the attributes someone thought to port were fixed. Two were not, and
were still broken afterwards:

  - `skip_if_none` in a struct variant emitted the field anyway:
    `{"Rec":{"a":1,"note":null,"b":2}}`. `write_variant_fields` checked
    `attrs.skip` but never `attrs.skip_if_none`.
  - `deny_unknown_fields = false` on an enum was ignored inside variants:
    `{"Rec":{"a":1,"zzz":9}}` returned `UnknownField`. `struct_variant_read`
    hardcoded a rejecting fallback arm.

The second stage did what this entry originally asked: both shapes now build
`FieldPlan` and are consumed by one `ObjectWriter`/`ObjectReader` pair, with
the only real difference — `self.field` vs a match binding — captured in an
`Access` enum. Neither remaining bug was patched directly; both stopped
reproducing because there is now one implementation instead of two agreeing
by hand. That is the difference between fixing the instances and fixing the
class, and it is why the §6 note insisted on the shared type rather than a
second attribute-parsing call.

### 3.7 S1 — `deny_unknown_fields = false` fails on escaped keys inside skipped values

`Lexer::skip_object_body` (`lexer.rs` ~1040) uses `object_first_key` /
`object_next_key`, which call `parse_str_value` and return `InvalidEscape` on
any `\`. A lenient struct therefore fails on
`{"unknown":{"a\u0062":1}}` even though the value should be discarded.

Fix: use `object_first_key_lex` / `object_next_key_lex` in the skip path (the
key is never materialised, so no decode is needed). Note that those `_lex`
variants skip the deferred surrogate-pair validation (`read_string_no_validate`),
so a skipped key with an unpaired surrogate would be accepted; decide whether
skip should validate and document it.

### 3.8 S1 — `to_string_pretty` does not pretty-print derived structs

`PrettyStringSink` recognises structure only through `write_byte`. The derive
emits fused literals (`b"{\"a\":"`, `b",\"b\":"`, `b"}"`) through
`write_raw_bytes` → `write_str_raw`, which the pretty sink treats as opaque text.
Observed: `{"a":1,"b":2}` for a derived struct, and mixed output when a derived
struct contains a `Vec` (the array is indented, the object is not). Enum
variants have the same issue.

Fix: give `JsonWrite` explicit structural methods (`begin_object`,
`end_object`, `object_key(&str)`, `begin_array`, `end_array`, `separator`)
with default impls that fuse to the current literals for byte sinks, and have
the derive and the in-crate impls call those. `PrettyStringSink` then overrides
them. This also removes the `debug_assert!(self.pending_open.is_none())` and the
`self.depth -= 1` underflow hazard in `write_byte`, since the sink no longer has
to infer structure from bytes.

### 3.9 S1 — `FmtWriteSink` corrupts non-ASCII strings

`FmtWriteSink::write_byte` (`ser.rs` ~1015) does `write_char(b as char)`. The
default `JsonWrite::write_escaped_str` (line 103) pushes every byte through
`write_byte`, so each UTF-8 continuation byte becomes its own Latin-1 code
point: `to_fmt("é")` yields `"Ã©"`. `StringSink`, `ByteSink` and
`PrettyStringSink` override `write_escaped_str` and are correct; `IoWriteSink`
writes raw bytes and is correct.

Fix: change the default `write_escaped_str` to emit literal runs via
`write_str_raw` and only escape bytes via `write_byte` (the same run-splitting
the sinks already do — extract it into one shared `EscapeWriter` and have every
sink use it). Add a test that every sink agrees on a non-ASCII input.

### 3.10 S1 — Empty tuple struct round-trip asymmetry

`to_json_tuple(0)` emits `[]`; `from_json_tuple` with `n == 0` returns
`TypeMismatch` on `[]` (`array_start` reports empty, which the multi-field path
treats as "too short"). Either accept `[]` for `n == 0` or reject the derive at
compile time. Recommend the latter, consistent with unit structs already being
rejected.

### 3.11 S2 — Internally tagged enums accept a duplicated tag key

`from_json_enum_internal` finds the first `tag` key, then re-parses; the unit
arm drains remaining keys and `skip_value`s any further `tag` key regardless of
its value. `{"kind":"SomeItem","kind":123}` parses. Struct variants behave the
same (`internal_struct_variant_read` has a `__bourne_k == #tag => skip_value()`
arm). Named structs report `DuplicateKey` for duplicate fields, so this is an
inconsistency rather than a spec violation. Fix: track "tag seen" and return
`DuplicateKey`.

### 3.12 S2 — `Parser` fast-path methods do not update the grammar state

`Parser::parse_i64_value` / `parse_str_value` (`parser.rs:231–237`) forward to
the lexer without touching `self.state`; the sibling `array_start`,
`object_first_key`, etc. do. After `Parser::new(b"1 2").parse_i64_value()`,
`next_event` returns `Some(Number(2))` and then `None` instead of
`TrailingData`. Either sync state (`DocumentEnd` at root, `*CommaOrEnd` inside
a container) or document these as lexer-level and move them off `Parser`.

### 3.13 S2 — `MapKey for &str` reports escaped keys as `InvalidEscape`

`de.rs:476`. The escape is valid; the limitation is the key type. A distinct
kind (`TypeMismatch`, or a new `BorrowedKeyNeedsDecode`) would not mislead users
into hunting for a malformed document.

### 3.14 S2 — Slice serialization error path is delivered by writing `NaN`

`<[T] as ToJson>::write_json` surfaces both the pre-validation failure and the
non-finite taint by calling `w.write_float_f64(f64::NAN)` and returning whatever
that produces. This works for the in-crate sinks (they all reject NaN) but a
custom sink whose `write_float_f64` tolerates NaN will silently receive the
junk bytes the taint path already emitted plus a literal `NaN`. Also, with a
user-owned `ByteSink`, the junk `1.0` substitutes remain in the caller's `Vec`
after the error. Recommend a dedicated `JsonWrite::non_finite_error()`
associated method that returns `Self::Error` (default: build it via
`write_float_f64(NaN)` for backward compatibility), and truncate the sink back
to the pre-array length on the taint path.

### 3.15 S3 — Smaller items

- `Lexer::new` / `parse()` panic (const `assert!`) on inputs over 2 GiB rather
  than returning `Err`. Documented, but `parse` is the main entry point; a
  `Result` is friendlier.
- `JsonWrite::write_raw_bytes` default `expect`s UTF-8 — a `pub` method that
  panics on user input from a custom `ToJson`. Make it `debug_assert` +
  document, or return `Err`.
- `char::from_lex` allocates a `String` on the escape path for a value that is
  at most 4 bytes; a stack `[u8; 8]` decode would avoid it.
- `is_option` in the derive is textual: a `type Maybe<T> = Option<T>` field
  becomes required. Same limitation as serde; worth a doc line.
- `parse_hex4` is duplicated in `lexer.rs` and `de.rs`.
- `JsonNum::as_str` returns `""` for a mismatched buffer, which then parses as
  `InvalidNumber` — masks the programming error. Tie to §3.2.
- Fuzz target `typed` does not cover `String`, maps, derived enums,
  `skip_value`, or `Checkpoint` restore; those are where §3.3–§3.11 live.
- No property test asserts `to_string(&x) == x.to_string()` for integers; the
  hand-rolled `fast_digit_count` table and `format_u64_direct` are covered only
  by fixed cases.

## 4. Performance findings

Numbers below are from a single quick run (`--sample-count 30 --max-time 0.1`)
on this machine; treat them as ranking, not as measurements.

### 4.1 Parse — json-bourne is ahead everywhere

| Workload (median) | json-bourne | serde_json | ratio |
|---|---|---|---|
| stream ints 10k (events vs `Value`) | 175 µs | 722 µs | 4.1× |
| stream strings 10k | 233 µs | 538 µs | 2.3× |
| typed metric_events 1000, hand-written | 205 µs | 305 µs | 1.5× |
| typed metric_events 1000, **derived** | 241 µs | 305 µs | 1.27× |
| `Vec<&str>` 10k | 67.8 µs | 219 µs | 3.2× |
| `Vec<i64>` 10k | 74.2 µs | 109 µs | 1.5× |
| `Vec<String>` 10k | 380 µs | 420 µs | 1.1× |

### 4.2 Serialize — floats lose at scale

| `Vec<f64>` → string | json-bourne | serde_json | ryu direct |
|---|---|---|---|
| n=100 | 1.93 µs (52 M/s) | 2.30 µs (43 M/s) | 2.60 µs |
| n=1000 | 19.2 µs (52 M/s) | 22.0 µs (45 M/s) | 26.0 µs |
| n=10000 | 231 µs (43 M/s) | 218 µs (46 M/s) | 313 µs |

Throughput per item drops ~17 % from n=1000 to n=10000 for json-bourne and is
flat for serde_json. Two hypotheses worth testing with `perf stat -e
L1-dcache-load-misses`:

1. `QUAD_LUT` (`float.rs`, 40 000 bytes in `.rodata`) competes with the output
   buffer for L1 once the working set exceeds L1. serde_json's formatter uses a
   200-byte pair table. Try a build with the 2-digit LUT only and compare at
   n=10000.
   — *Tested 2026-09; hypothesis refuted, no change landed.* The experiment
   was run exactly as specified: `write_digits_at_ptr` rebuilt to use only
   the 200-byte `DIGIT_LUT` (bottom 8 digits as four pair-copies, tail loop
   two digits per iteration), full test suite green, benched against the
   unmodified baseline on the same machine and run:

   | n=10 000, `bourne_write` median | throughput |
   |---|---|
   | `QUAD_LUT` (baseline) | 268 µs — 37.3 Mitem/s |
   | pair-LUT only | 301 µs — 33.3 Mitem/s |

   Dropping the 40 KB table made the target case **12 % slower**, and the
   n=1000→n=10000 cliff *survived* the change (37.6 → 33.3 Mitem/s, still
   ≈−12 %). A table-vs-buffer L1 conflict cannot explain a cliff that
   persists once the table is gone. `QUAD_LUT` stays.

   The bench's own diagnostics localize the real cause, and they were
   already in the file: the cliff appears **only** under unbounded magnitude
   variance. Scaling n=1000→n=10000, `bourne_write` loses ≈15 %
   (43.8→37.3 Mitem/s) while `bourne_write_same` (one repeated value) and
   `bourne_write_four` (4 cycling values) are flat, and `serde_json` is flat.
   `perf stat` on the `profile` binary's 10k float workloads, normalized per
   iteration (bourne 7 000 iters, serde 23 000):

   | per iteration | bourne | serde_json |
   |---|---|---|
   | cycles | 1 045 581 | 949 575 |
   | instructions | 3 386 625 | 2 287 721 |
   | IPC | 3.2 | 2.4 |
   | **branch-misses** | **7 050** | **843** |

   bourne retires 48 % more instructions at a *higher* IPC and mispredicts
   **8.4× more branches**. The cost is branch misprediction in the
   data-dependent shape selection — `write_f64_to_ptr`'s three-way
   fixed-point split plus the `point`/`digits_count` comparisons, and the
   variable trip counts of the digit loop — not cache pressure. serde_json's
   formatter is branch-lean by comparison. This reframes §4.2: the lever is
   reducing or straightening data-dependent branches in the shape selection
   (the sign branch was already removed this way, per the comment at
   `write_f64_to_ptr`), not shrinking tables. Left unimplemented pending a
   design that does not regress the flat cases.
2. The reservation `len * 33 + 2` is ~1.65× the real output (~20 bytes/float),
   so the `Vec` is realloc'd once to 330 KB; not a per-element cost, unlikely to
   be the cause, but it does inflate peak memory (§4.5).

### 4.3 Derive is ~18 % slower than the hand-written impl

Same shape, same input (`metric_events_1000`): 241 µs derived vs 205 µs
hand-written. Causes visible in the generated code (`bourne-derive/src/lib.rs`):

1. **Key dispatch uses guard arms.** `from_json_named` emits
   `__bourne_k if __bourne_k == "id" => …` for every field. Guards are evaluated
   sequentially as `str == str` calls; a `match key { "id" => …, "host" => … }`
   with literal patterns lets rustc switch on length first and memcmp once.
   The hand-written bench impl uses the literal-pattern form. For `rename_all`
   keys, emit `const` items and use them as patterns (allowed for `&str`).
   — *Landed 2026-07, with a smaller payoff than estimated.* All five derive
   dispatch sites (named structs, struct variants, and the external/internal/
   adjacent enum tag matches) now take literal patterns; `rename_all` keys are
   hoisted into uniquely named `const` items (the cased string is computed by
   `const fn` at compile time) used as patterns. Gap to the hand-written impl
   on `metric_events_1000`: 1.145× → 1.107× in declaration order; reversed-key
   order unchanged within noise. The remaining ~10% is dominated by item 2.
2. **Every key goes through `object_first_key_lex` + `key_to_cow`** (read span,
   check `has_escapes`, build `Cow`) instead of `object_first_key` →
   `parse_str_value`. Escaped keys are rare; try the borrowed path first and
   fall back to the `_lex` path only on `InvalidEscape`.
   — *Landed 2026-07.* New `object_first_key_str` / `object_next_key_str` read
   keys with the borrowing string walk and rewind on `InvalidEscape`;
   generated code retries the rejected key once with `object_key_cow`, which
   decodes exactly like the old path. The derived struct impl overtook the
   hand-written bench impl: 156.5 µs vs 164.1 µs on `metric_events_1000`
   (0.95×) — the audit's §4.3 headline ("derive is ~18 % slower") is now a
   4 % win. A `key_to_cow` missing `#[inline]` turned out to be half the
   original key-path cost; the borrowed-first codegen removed the rest.
3. **`acquire_expr` has no arms for `f64`, `f32`, `bool`, `Option<&str>`**, so
   those fields pay `peek_value_kind` + trait dispatch; add direct
   `parse_f64_value` / keyword arms. Three of the eight fields in the realistic
   struct are `f64`.
   — *Landed 2026-07, with two corrections to the audit.* The scalar arms the
   audit asked for already exist (`f64`, `f32`, `bool`, and `u64` direct — the
   last closed by §3.3's `parse_u64_value`); the audit text predated them.
   `Option<&str>` stays on the generic path by design: `Option<T>` must see
   `Null` to produce `None`, and the generic impl already peeks exactly once.
   Item 4 is folded in here: `acquire_expr` now matches `Type::Reference`
   structurally, so `&'a str` with any lifetime name takes the direct
   `parse_str_value` path; `&mut str`, `&String`, and `&[u8]` correctly do
   not (unit-tested).
4. `&'a str` (any lifetime name other than `'input`) misses the textual
   `"&'inputstr"` match and takes the generic path. Match on the type
   structure (`Type::Reference` to `str`) instead of the stringified type.
   — *Landed 2026-07 with item 3* (structural `Type::Reference` matching in
   `acquire_expr`, unit-tested for any lifetime and for near-miss types).
5. `u64` → `parse_i64_value` + `try_from` — fixed by §3.3's `parse_u64_value`.

### 4.4 Parse hot-path opportunities (ordered by expected payoff)

1. **Whitespace skipping is byte-at-a-time** (`lexer.rs:skip_whitespace`). For
   compact JSON this is a non-issue; for pretty-printed inputs (common for
   config files) it dominates. A 16-byte SSE2 compare against `{' ', '\t',
   '\n', '\r'}` mirrors the existing string scan.
   — *Measured 2026-07: no detectable win.* An SSE2 skipper was implemented
   and A/B'd against the scalar loop on new pretty-printed benches (arrays
   and an object, one newline + indent per member), with serde_json as the
   drift control; all deltas fell inside run-to-run noise. Realistic pretty
   JSON interleaves whitespace with tokens in 1–7 byte runs, and the scalar
   loop over such runs is already cheaper than the SIMD call + splat setup.
   Bench fixtures (`pretty_*` in `bourne-bench`) kept for future re-checks.
2. **Digit accumulation is byte-at-a-time** (`parse_i64_digits`,
   `scan_digit_run`). `Vec<i64>` is at 660 MB/s; SWAR 8-digit chunks (one
   `u64` load, subtract `0x30…`, range check, multiply-add) typically give
   1.5–2× on integer arrays.
   — *Measured 2026-07: 2.4× regression, reverted.* An 8-digit SWAR chunk
   reader (nibble filter + range check + pairwise fold, exhaustively tested
   against `str::parse`) was wired into all three fused integer paths and
   interleaved-A/B'd against the scalar loop: 75.9 µs → 185.2 µs median on
   `Vec<i64>` n=10 000 with serde_json flat as the control. The audit's
   estimate assumes long literals; realistic integer arrays hold 1–5 digit
   numbers, so every element pays a full failed SWAR probe (~10 ops) before
   falling into the scalar loop that was already cheaper — and a parser
   cannot know a literal's length before parsing it. SWAR would only pay on
   ≥8-digit-dominated payloads (ids, timestamps); the scalar path keeps the
   common case fast.
3. **`parse_f64_value` walks the literal twice**: once in `read_number` for
   grammar, once in `str::parse::<f64>`. A fused fast path for "≤19 digits, no
   exponent, ≤ 2^53" that assembles mantissa/exponent while scanning and falls
   back to `str::parse` otherwise would cover most telemetry floats.
   — *Landed 2026-09.* Plain decimal literals (no exponent) whose mantissa
   fits 2^53 assemble mantissa and `10^-k` during the grammar scan; both are
   exactly representable, so a single IEEE divide is correctly rounded and
   bit-identical to `str::parse` — no `JsonNum`, no `String`, no Eisel-Lemire
   path. 1.9× on a plain-decimal `Vec<f64>`; exponent-heavy payloads fall
   back unchanged. Differential tests cover every 3-digit fraction shape,
   the 2^53 boundary mantissas, and 20 000 seeded random literals checked
   bit-for-bit.
4. **`Stack<128>` is a 128-byte array** initialised on every `Lexer::new`. A
   frame is one bit; a `u128` bitset (depth ≤ 128) makes `Lexer` ~40 bytes and
   removes the memset — visible on the 190 ns small-object case.
   — *Landed 2026-09.* `Stack` is a `u128` bitset (`lexer.rs`); constructing
   a lexer clears 24 bytes instead of 136. Depth is now capped at 128 for
   *all* `MAX_DEPTH` parameterizations, with a compile-time assert in
   `Stack::new` for non-default choices — a breaking change for anyone who
   raised it past 128.
5. `to_string` validates the whole output with `String::from_utf8`. The
   invariant is already argued in the doc comment; an `unsafe`
   `from_utf8_unchecked` behind a debug-mode check saves a full pass on large
   outputs. Only do this after §3.1 is fixed, since it widens the trust placed
   in sink invariants.
   — *Rejected 2026-07: the invariant does not hold.* `ByteSink` (the sink
   `to_string` uses via `to_vec`) overrides `write_raw_bytes` with an
   unvalidated `extend_from_slice`, and hand-written `ToJson` impls are
   documented as unverifiable inputs. A lone continuation byte fed through
   such an impl reached the `expect` and panicked. Worse, `from_utf8_unchecked`
   would have turned that panic into instantiating a `String` with invalid
   contents. `to_string` is now the UTF-8 gate: it returns
   `Err(ErrorKind::InvalidUtf8)` instead of panicking, with the unsafe
   micro-opt dropped and a test pinning the behavior.

### 4.5 Serialize opportunities

1. **`write_escaped_str` is byte-at-a-time** in `StringSink` and `ByteSink`
   (`needs_escape` per byte). Reuse `find_backslash`'s SSE2 shape with three
   compares (`"`, `\`, `< 0x20`) — the same mask the lexer already computes.
   — *Landed 2026-09.* `escape::find_escape` locates the next byte needing
   an escape with a 16-byte SSE2 compare (same shape as `de.rs`'s
   `find_backslash`), with a scalar `position` fallback for non-x86_64 and
   `bourne_no_simd`. Escape-sparse strings — the common payload — now copy
   as large literal runs instead of per-byte walks.
2. **Over-reservation**: `[T]::write_json` reserves `len*(MAX+1)+2`; for
   `Vec<i64>` of small numbers that is 21 bytes per element against ~2 actual.
   A 10 M-element array reserves 210 MB for ~30 MB of output. Cap the hint
   (e.g. `min(hint, len * 8 + 2)` then let growth take over) or reserve in
   chunks.
   — *Landed 2026-09.* The slice writer reserves in bounded 256 KiB windows
   (`RESERVE_WINDOW`, `ser.rs`) and re-hints as it crosses each one, so peak
   extra reservation is one window rather than `len * (MAX + 1)`. The
   raw-tail write path is preserved inside each window; float-array medians
   held within noise of the previous design.
3. `write_display` (IP/socket addrs) allocates a `String` per value; a
   `[u8; 64]` `fmt::Write` adapter removes the allocation.
   — *Landed 2026-07.* `DisplayScratch` (`crates/bourne/src/display_scratch.rs`)
   is a 64-byte stack buffer with a `fmt::Write` impl (suffix dropped on
   overflow — callers format canonical fixed-width types that fit); the
   `write_display` adapters for the four `std::net` types now format into it.
   std-only, like its consumers, so the no-std build is unchanged.
4. Internally/adjacently tagged enums parse the payload twice (`skip_value`
   then real parse). When the tag key comes first (the common case for
   serialized output), the adjacent path can parse `content` directly without
   the checkpoint round-trip.
   — *Landed 2026-09.* When the tag is already known when `content` arrives,
   the generated walk snapshots the payload start and exits; the arm reads
   the payload forward from that checkpoint exactly once, then a tail walk
   rejects leftover keys (a leftover tag is `DuplicateKey`, anything else
   `UnknownField`) and consumes the closing brace. Content-first — the only
   order where the old code had to skip — still skips and restores. All
   original accept/reject behavior is preserved and pinned by new tests
   (duplicate content on either side of the tag, duplicate and unknown keys
   trailing an early-exit payload). Internally-tagged enums were out of the
   audit's scope and keep their checkpoint round-trip: their payload
   shares the tag's object, so keys after the tag are structurally required
   before the variant can be chosen.

## 5. Process and repository findings

### 5.1 S1 — CI is not exercising the code

`.github/workflows/ci.yml` references `-p bourne-core` and `-p bourne`; the
packages are `json-bourne`, `bourne-derive`, `bourne-bench`. The `clippy`,
`miri`, and `no_std` jobs fail at the first cargo invocation. The `test` job's
first step, `cargo test --workspace --all-features`, fails to compile because
`bourne-bench`'s `alloc-profile` (lib) and `compare-mem` (bin) features both
install a `#[global_allocator]`. The Cargo.toml comment says they are mutually
exclusive but nothing enforces it.

Fix: correct the package names; add a `compile_error!` when both bench features
are on, or move `compare_mem` to its own crate; run the exact CI command list
locally before merging (there is no `just`/`Makefile` target that reproduces
CI — add one).
— *Landed 2026-07.* `ci.yml` names `json-bourne`/`bourne-derive`; the test
job's matrix runs `scripts/ci.sh`, which carries the exact command list
(all-features deliberately excluded from the workspace run) and covers the
bare `--no-default-features`, `alloc`, and `alloc,derive` combos (the combo originally included `indexmap`, since
removed);
`bourne-bench/src/feature_guard.rs` has the `compile_error!`. The bare
no-std build the script exercises was itself broken until 5.4 (the lexer
reached alloc-gated `crate::float::POW10`); CI would have caught it had
this job existed when the gate was introduced — which is the audit's point.

### 5.2 S3 — README is stale

It documents `from_json!` / `to_json!` declarative macros, states "No
proc-macros" and "v0.1", none of which is true for 0.2.2 (derive crate, syn 3).
The crate-level rustdoc in `lib.rs` is current; the README should be generated
from or reconciled with it.
— *Landed 2026-07, reconciled 2026-09.* The rewrite (derive feature, feature
table, `scripts/ci.sh` pointer) predates the §5.4 float un-gating, whose
stale claim — "`f64`/`f32` round-trips need `alloc`" — this pass removed
from both the README and the rustdoc feature table (the rustdoc table was
also missing the `derive` row). `to_fmt` / `FmtWriteSink` are the
no-alloc float path and now ship ungated in both docs.

### 5.3 S3 — Repository hygiene

Untracked `Fuel.pdf`, `billions.pdf`, `decimal.pdf` and four flamegraph SVGs in
the repo root. If they are reference material, move them under `docs/` and
track them; otherwise delete. `lcov.info` is tracked — it should be an artifact.
— *Landed 2026-07.* The PDFs/SVGs are gone from the tree, `.gitignore`
carries `*.svg`, `*.pdf`, `lcov.info`, and the tracked
`.claude/scheduled_tasks.lock` runtime artifact is untracked with `.claude/`
ignored. The empty `docs/papers/` directory this cleanup created is removed.

### 5.4 S3 — Feature gating leaks into logic

- `<[T] as ToJson>::write_json` contains two inline `#[cfg(feature = "alloc")]`
  blocks in the middle of the method body.
- `JsonWrite::write_float_f64` is a required trait method that exists only under
  `alloc`, so the trait's shape changes by feature. `float.rs` only needs
  `alloc` for the `Vec`/`String` helpers; `format_finite_to_buf` is core-only.
  Consequence: a `no_std` (no `alloc`) build has no `ToJson` for `f64`/`f32`
  at all, which the feature table does not mention.

Recommendation: un-gate `float.rs` (keep only the `Vec` helpers behind
`alloc`), make `write_float_f64` unconditional, and replace the inline cfg
blocks with thin wrapper methods (`fn flush_float_taint` with an `alloc` impl
and a no-op twin).
— *Landed 2026-07.* `float.rs` is un-gated (only `format_finite` /
`format_finite_to_vec` keep `alloc` gates; the teju core, `POW10`,
`EXP_MASK`, and `format_finite_fmt` are core-only), `write_float_f64` /
`write_float_f64_hinted` are unconditional trait methods, and the `f64` /
`f32` `ToJson` impls moved out of `alloc_impls`. The inline cfg blocks in
`[T]::write_json` were already gone with the §4.5.2 windowing rewrite, so
no wrapper method was needed there. `FmtWriteSink` and `to_fmt` are now
the allocation-free escape hatch and ship in no-std builds. The bare
`--no-default-features` build compiles and passes tests for the first
time — it had been broken at the lexer (`crate::float::POW10`) and the
feature-table rot the audit described; `serde_json` needs its `alloc`
feature for the same reason (`f64`'s `impl` lives behind `alloc` there).
The feature table note follows.

## 6. Structure (SRP) observations

Not defects, but they bear on how safely the fixes above can be made.

- `crates/bourne/src/lib.rs` is 3 199 lines, ~3 000 of which are `#[cfg(test)]`
  unit tests. Move them to `tests/` (or `src/tests/*.rs` modules) so the public
  surface of the crate is readable in one screen.
  — *Landed 2026-09* (it had grown to 3 751 lines by then; now **1 461**, a
  61 % cut). The split follows what each test can actually reach, which is
  the distinction the audit left open:
    - Tests touching crate internals (`lexer::Stack`/`Frame`, `parse_f64_value`,
      `decode_escapes`, `float::format_finite`, the miri-targeted unsafe
      boundaries) became `src/tests/*.rs` — nine modules: `stack_frames`,
      `float_fast_path`, `integer_paths`, `sink_adapter`, `sink_direct`,
      `unsafe_boundary`, `vec_fast_path`, `escape_decode`, `float_uncentred`.
      An integration test cannot see `pub(crate)`, so these had to stay in-crate.
    - Tests using only the public API moved out to `tests/` — `ser_roundtrip`
      (29), `to_json_derive` (26), `derive_roundtrip` (10). This is a strict
      gain: out-of-crate they also pin that the surface is exported and that
      the derives' `::json_bourne::` paths resolve for a real consumer, neither
      of which the in-crate versions could catch.
  The 900-line `unsafe_boundary_tests` module was itself five jobs, already
  self-documented with banner comments; the split follows those seams rather
  than a new grouping. Method throughout: gate the old block `#[cfg(any())]`,
  confirm the suite still reports the same count with it excluded (proving the
  new modules complete), then delete. 281 tests before and after, every step.
- `ser.rs` (2 000 lines) holds the trait, five sinks, the integer formatter, the
  array/object writers, and all impls. Natural split: `sink/{string,bytes,fmt,
  io,pretty}.rs`, `int_format.rs` (`IntFormatter` owning `DIGIT_LUT` +
  `fast_digit_count`), `escape.rs` (shared `EscapeWriter`), `impls/*.rs`.
- `de.rs`: `decode_escapes`, `find_backslash`, `decode_unicode_escape`,
  `decode_surrogate_pair`, `parse_hex4` are free functions with one obvious
  owner — an `EscapeDecoder` — and `parse_hex4` is duplicated in `lexer.rs`,
  where `validate_escapes` wants an `EscapeValidator`.
- `float.rs`: the teju port is free functions over shared tables; a `Teju`
  type with `decompose`/`to_decimal`/`format` methods would carry the
  `debug_assert!` preconditions as type-level state.
- `bourne-derive/src/lib.rs` (1 461 lines, all free functions) mixes attribute
  parsing, generics planning, and six codegen strategies. Split into
  `attrs.rs` (`FieldAttrs`, `ContainerAttrs`, `EnumMode`), `generics.rs`
  (`GenericsPlan`), `from_json/{named,tuple,enum_external,enum_internal,
  enum_adjacent,enum_untagged}.rs`, `to_json/…`. Introduce a `FieldPlan`
  (ident, key expression, acquire expression, finalize expression) built once
  and consumed by *both* the named-struct and struct-variant code paths — that
  single change fixes §3.6 and prevents it recurring.
  — *Landed 2026-09* (1 902 lines by then; `lib.rs` is now **101** — the two
  proc-macro entry points and the empty-tuple rejection tests). Modules:
  `attrs.rs`, `field_plan.rs`, `naming.rs` (`Naming::key_expr`/`key_arm_head`),
  `acquire.rs` (`Acquire::expr`), `generics.rs`, `shape.rs` (`VShape::classify`),
  `from_json/{mod,object,enums}.rs`, `to_json/{mod,object,enums}.rs`. Every
  free function became a method on the type that owns it.
  The `FieldPlan` prediction held exactly: building it for both shapes and
  feeding one `ObjectReader`/`ObjectWriter` fixed two *unreported* §3.6
  instances (`skip_if_none`, `deny_unknown_fields`) with no targeted change.
  A third duplication surfaced while merging the writers — comma placement
  existed in three forms (`to_json_named`'s `StaticFirst`, `write_variant_
  fields`' `__first`, and an open-coded variant inside the internal-mode arm)
  — now one `Comma` enum, with internal mode passing `already_emitted = true`
  rather than hand-writing its leading comma.

## 7. Recommended action plan

Each phase ends with the full matrix: `--no-default-features`, `--features
alloc`, default, `--features derive`, `--all-features`; tests + clippy
`-D warnings` in each; `cargo miri test -p json-bourne --lib` with
`--cfg bourne_no_simd`; fuzz smoke on both targets.

**Phase 0 — make verification real (half a day)**
1. Fix CI package names; enforce bench feature exclusivity; add a local
   `ci` recipe that runs the same commands. (§5.1)
2. Refresh README; move/delete stray artifacts; untrack `lcov.info`. (§5.2, §5.3)

**Phase 1 — soundness (1–2 days)**
3. Seal the reserved-write path behind a crate-private `unsafe` marker trait;
   add the lying-`MAX_SERIALIZED_LEN` regression test under miri. (§3.1)
4. Make `JsonStr::as_str` / `JsonNum::as_str` checked, or replace them with
   `Lexer`-anchored accessors; keep unchecked variants `pub(crate)`. (§3.2)
5. ~~Remove the crate-level `#![allow(unsafe_code)]`.~~ **Done** — seven sites
   now carry their own justified allow; workspace `deny` is live. (§3.2)
6. Extend fuzz `typed` to `String`, `BTreeMap<String,_>`, a derived struct
   with `deny_unknown_fields = false`, and one enum per tagging mode. (§3.15)

**Phase 2 — correctness (2–3 days, one item per commit, each with tests)**
7. `Lexer::parse_u64_value`; use it in `impl_int!` and `acquire_expr`. (§3.3)
8. Pass `rename_all` into internal/adjacent enum parse. (§3.4)
9. Escape (or compile-time-validate) variant tags. (§3.5)
10. ~~`FieldPlan` shared by named structs and struct variants.~~ **Done** —
    both shapes build one `FieldPlan` and share `ObjectReader`/`ObjectWriter`;
    fixed two further §3.6 instances (`skip_if_none`, `deny_unknown_fields`)
    that the first, per-attribute fix had left. (§3.6, §6)
11. `skip_object_body` via `_lex` key methods. (§3.7)
12. Structural methods on `JsonWrite`; derive emits them; pretty sink
    overrides them. (§3.8)
13. Shared `EscapeWriter`; default `write_escaped_str` uses it. (§3.9)
14. Reject zero-field tuple structs at derive time. (§3.10)
15. Duplicate-tag detection; `Parser` state sync for fast-path methods; error
    kind for `&str` map keys; dedicated non-finite error hook. (§3.11–§3.14)

**Phase 3 — performance (measure before/after each, `compare` + `floats`)**
16. Derive: literal-pattern `match` for keys; borrowed-key-first; `f64`/`bool`
    acquire arms; structural `&str` detection. Target: derived ≤ 1.05× hand-written. (§4.3)
17. ~~`QUAD_LUT` vs pair-LUT experiment at n=10 000.~~ **Done — hypothesis
    refuted, no change.** Pair-LUT is 12 % slower and the cliff survives it;
    `perf` attributes the gap to branch misprediction (8.4× serde's rate),
    not cache pressure. Any future work here targets the data-dependent
    shape-selection branches. (§4.2)
18. ~~SIMD whitespace skip; SIMD escape scan in serializer.~~ **Done** — the
    whitespace skipper was implemented and measured with *no detectable win*
    (scalar already cheaper on realistic 1–7 byte runs) and reverted, fixtures
    kept; the escape scan landed as `escape::find_escape`. (§4.4.1, §4.5.1)
19. ~~SWAR digit parsing; fused simple-float path.~~ **Partly done** — SWAR
    measured at a *2.4x regression* and reverted; the fused plain-decimal
    float path landed (1.9x, bit-identical to `str::parse`). (§4.4.2, §4.4.3)
20. ~~Bitset `Stack`; reservation cap.~~ **Done** — `Stack` is a `u128` bitset
    (breaking: depth capped at 128 for all `MAX_DEPTH`); the slice writer
    reserves in 256 KiB windows. (§4.4.4, §4.5.2)

**Phase 4 — structure**
21. Un-gate `float.rs`; cfg wrappers instead of inline cfg blocks. (§5.4)
22. Split `lib.rs` tests, `ser.rs`, `de.rs`, and the derive crate per §6, one
    module per commit, matrix-verified each time.
