# Release audit — correctness and performance

Audited revision: `b9077cd` (workspace version `0.2.2`, unreleased).
Scope: `json-bourne` and `bourne-derive` — correctness of the parse and
serialize paths, the `unsafe` surface, the derive codegen, and measured
performance. `akita/` is excluded. **No implementation, test, manifest,
or workflow changes were made for this audit.** External reproduction
code lives in `/tmp/bourne-audit-2`.

## Status

**A1, A2 and A4 are fixed; the coverage gaps they exposed are closed.**
A3 remains open and needs a decision before the cut. See "Remediation"
at the end for what changed and how it was verified.

The findings below are written as they were found, so the reasoning
stays auditable against the pre-fix revision `b9077cd`.

## Release decision (at time of audit): HOLD

Two defects reject valid JSON that the crate itself produces. Both are
in derive-generated readers, both are invisible to the current suite
(281 tests, cargo-crappy, and three fuzz targets all pass at this
revision), and one of them makes `to_string` → `parse_str` fail for an
ordinary `#[bourne(rename = "...")]` field. Fix A1 and A2 before
cutting. A3 is a silent-misconfiguration issue that should be decided
before release because the fix is plausibly breaking.

Performance is not a blocker: bourne leads `serde_json` on 14 of 15
head-to-head benchmarks, by 1.2×–4.3×. One case (P1) is a measured
regression against `serde_json` and is worth scheduling, not blocking.

This is a bounded source-and-execution audit. It is not a proof of
correctness, and it does not exhaustively cover every numeric input,
target architecture, or attribute combination.

---

## Correctness findings

### A1 (High) — an escaped object key is rejected when it appears first

`ObjectReader::key_walk` (`crates/bourne-derive/src/from_json/object.rs`)
reads the first key with `__lex.object_first_key_str()?`. The `?`
propagates, so an escape-bearing first key returns `InvalidEscape`. Every
*subsequent* key goes through the loop below it, which catches exactly
that error and retries via `object_key_cow`. The result is that identical
JSON parses or fails depending on key order:

```rust
#[derive(FromJson, ToJson)]
struct EscKey {
    #[bourne(rename = "user-id")]  user_id: u32,
    #[bourne(rename = "x\ny")]     x_newline_y: u32,
}
```

| Input | Result |
|---|---|
| `{"user-id":1,"x\ny":2}` (escape second) | `Ok(EscKey { .. })` |
| `{"x\ny":2,"user-id":1}` (escape first) | `Err(InvalidEscape @ 3)` |

Both documents are valid JSON and `serde_json` accepts both.

The severity comes from the round trip. Field order in the output follows
declaration order, so a struct whose escaped-key field is declared first
**emits JSON it cannot read back**:

```
to_string  -> {"x\ny":2,"user-id":1}     (valid JSON)
parse back -> Err(InvalidEscape @ 3)
```

A single-field struct with a quote in its key fails the same way
(`{"a\"b":5}`), so this is not limited to multi-field types.

`key_walk` is the one walk shared by named structs, enum struct variants,
and every tagging mode, so all derived object readers inherit it. The
non-derive paths (`HashMap`, `BTreeMap`) handle both orders correctly —
this is derive-only.

The lexer already provides everything a fix needs:
`object_first_key_str` deliberately **rewinds to the key's opening quote**
before returning the error (`lexer.rs`), which is precisely where
`object_key_cow` expects to start. The fix is to give the first key the
same match-and-retry shape the subsequent-key loop already has; no
library change is required.

The method's own doc comment claims the walk is "equivalent to the `_lex`
+ `key_to_cow` sequence for every input". That is the invariant the first
key breaks, so the comment should be re-verified once the code matches it.

### A2 (Medium) — `f32` overflow is silently `inf` in a derived field

`Acquire::expr` maps `"f32"` to `__lex.parse_f64_value()? as f32`. A bare
`as` cast on an out-of-range double saturates to `±inf` with no error.
The `FromJson for f32` impl in `de.rs` does the opposite, and says so:
"Reject literals whose magnitude can't fit `f32` instead of silently
coercing to `±inf`."

| Path | `1e300` |
|---|---|
| `parse_str::<f32>` (library impl) | `Err(NumberOutOfRange)` |
| derived `struct { v: f32 }` | `Ok(v: inf)` |

The integer narrowings in the same file (`int_narrow` / `uint_narrow`)
correctly use `try_from` and return `NumberOutOfRange`; `f32` is the only
lossy one. Recommended fix: have the `f32` arm reuse the library impl's
finite check rather than casting, so the two paths agree by construction.

JSON has no infinity literal, and the serializer rejects non-finite
floats with `NonFiniteFloat` — so this produces a value that the crate
would refuse to serialize back.

### A3 (Medium) — `skip_if_none` silently does nothing behind a type alias

`FieldPlan::is_conditional` calls `Naming::is_option(self.ty)`, a
syntactic match on the token `Option<...>`. With `type MaybeName =
Option<String>`, the attribute is accepted and then ignored:

| Field type | `to_string` of `name: None` |
|---|---|
| `Option<String>` | `{"id":1}` |
| `MaybeName` (alias) | `{"id":1,"name":null}` |

A proc macro cannot resolve aliases — it never sees through the type — so
detecting this properly is not possible at expansion time. The realistic
options are to document the limitation prominently, or to reject
`skip_if_none` on a field whose type is not syntactically `Option<_>`
(a compile error is louder than silently wrong output, but it is a
breaking change for anyone relying on the current no-op, which is why it
should be settled before the release rather than after).

The same syntactic assumption governs the `finalize` default for missing
`Option` fields, so an aliased `Option` also becomes *required* rather
than defaulting to `None`. Worth confirming as part of the same fix.

**Both paragraphs above were wrong, and the second understated the
severity.** See the A3 resolution below: the reader half rejects valid
JSON, which is the same class of defect as A1 rather than a cosmetic
output difference; and the premise that expansion-time detection is
impossible does not survive contact with the type checker.

### A4 (Low) — two stale doc comments describe code that no longer exists

Neither affects behavior; both mislead a reader auditing the `unsafe`.

- `ser.rs` (`ByteSink`): "the caller can convert to `String` via
  `from_utf8_unchecked` after serialization completes". `to_string`
  actually uses **checked** `String::from_utf8` and maps failure to
  `InvalidUtf8` — deliberately, per the comment at that call site. The
  `ByteSink` comment describes an abandoned design and reads as a claimed
  UB invariant that the code does not rely on.
- `ser.rs` (`StringSink::write_float_f64`): "Currently dispatches to the
  `write!`-based formatter — see the `float` module below for the
  alternate ryu path and the bench that picks between them." There is no
  ryu path; the crate uses an in-tree teju-jagua formatter, and the choice
  is not bench-selected.

---

## Performance findings

Measured on this machine at `b9077cd` via the divan suites (`compare`,
`realistic`). Ratios are median-vs-median against `serde_json` in the
same run. Single-machine numbers with visible run-to-run spread — treat
them as directional.

Bourne leads on 14 of 15 head-to-head cases:

| Workload | bourne | serde_json | ratio |
|---|---|---|---|
| `ints_10000` stream | 232.3 µs | 978.6 µs | **4.21×** |
| `giant_geojson_25000` | 16.25 ms | 106.2 ms | **6.53×** |
| `metric_events_1000` derived | 151.7 µs | 281.7 µs | **1.86×** |
| `vec_borrowed_str` 10000 | 58.66 µs | 211.1 µs | **3.60×** |
| `vec_i64` 10000 | 80.14 µs | 108.3 µs | **1.35×** |
| `pretty_ints_10000` | 228.6 µs | 1.00 ms | **4.38×** |
| `mixed_length_strings` typed borrowed | 37.03 µs | 131.5 µs | **3.55×** |
| `unicode_strings_1000` typed borrowed | 60.35 µs | 54.10 µs | **0.90×** |

(The `unicode_strings` row is the pre-fix measurement; see the P1
resolution below — it now runs at **1.21×**.)

### P1 (Medium) — unicode-dense strings are the one measured loss

`unicode_strings_1000_typed_borrowed` is the only benchmark where bourne
trails `serde_json` (60.35 µs vs 54.10 µs, ~1.12× slower). The same
corpus through the streaming layer runs at 1.64 GB/s while the typed
borrow drops to 1.30 GB/s, which places the cost in the string read
rather than the lexer's structural walk.

Mechanism, from the source: `scan_ascii_string_run_sse2` uses a signed
byte compare that treats every byte ≥ 0x80 as a stop, so the 16-byte
vector loop halts at the first multibyte lead. `consume_utf8_multibyte`
then advances 2–4 bytes one at a time through `peek`/`bump` before the
vector loop restarts. On text that is mostly non-ASCII the scanner spends
its time paying vector-loop entry costs for very short ASCII runs.

This is a correctness-preserving design (the signed compare is what makes
the `from_utf8_unchecked` in `parse_str_value` sound — every non-ASCII
byte is routed to the validating scalar path). Any change must keep that
property. Worth scheduling as a focused optimization with the existing
`unicode_strings` bench as the gate, not as release-blocking work.

#### P1 — fixed, and the diagnosis needed correcting

The finding blamed "vector-loop entry costs for very short ASCII runs",
which implied the SIMD scanner was the problem. `perf record` on the
`vec_borrowed_unicode_10k` workload says otherwise:

| symbol | cycles |
|---|---|
| `consume_utf8_multibyte` | **52.5%** |
| `parse_str_value` | 44.7% |
| everything else | 2.8% |

IPC was 5.0 with a 0.001% branch-miss rate — no stalls, no
misprediction, just too many instructions retired. And a control build
with `--cfg bourne_no_simd` ran *worse* (1.47× serde vs 1.19×), so the
SSE2 scanner was earning its keep even on this corpus. The cost was the
round trip: each character returned to the string loop, which re-entered
the scanner — splatting three compare constants — for a run often only
one character long.

The fix consumes a *run* of multi-byte characters per call and walks the
cursor in a local instead of through `peek`/`bump`. Both halves matter;
the local-cursor change alone moved the ratio only 1.19× → 1.15×.

| `unicode_strings_1000_typed_borrowed` | vs serde, same run |
|---|---|
| before | 52.50 µs — **1.19× slower** |
| after | **38.25 µs** — **0.83×, i.e. 1.21× faster** |

That is a 27% improvement on the target workload, and it inverts the
finding: **bourne now leads on all 15 head-to-head cases.** The typed
borrow (38.25 µs) is now faster than bourne's own streaming drain of the
same corpus (42.29 µs). No ASCII-path regression — `mixed_length_strings`
typed borrowed still runs 3.87× serde_json.

Every byte is still bounds-checked and range-validated individually, so
the `from_utf8_unchecked` invariant and error positions are unchanged;
four new tests in `api_smoke.rs` pin the run boundaries (quote, ASCII,
escape, control byte, invalid continuation) and one asserts the error
position lands on the offending byte. Verified under Miri (118/118),
`--cfg bourne_no_simd`, and 30k fuzz runs.

### P2 (Low) — `vec_string` gains least, and the reason is worth confirming

`vec_string` (owned `String` elements) is bourne's narrowest win:
365.7 µs vs 438.6 µs, **1.20×**, against **3.60×** for the borrowed-`&str`
equivalent on comparable data. The gap between the borrowed and owned
paths is much larger than the allocation cost alone would suggest. The
`profile` binary already has a `vec_string_10k` workload for exactly this
question; a flamegraph would confirm whether the cost is allocation,
the escape-decode copy, or both before any work is attempted.

#### P2 — investigated, no change recommended

The profile answers the question the finding posed: it is allocation,
and it is irreducible. Counters for the same parse work, owned versus
borrowed:

| | borrowed `&str` | owned `String` |
|---|---|---|
| cycles | 11.9 G | 40.9 G |
| instructions | 62.8 G | 181.1 G |
| IPC | 5.3 | 4.4 |

The owned path burns 3.4× the cycles for identical parsing, and IPC stays
high — it is not stalling, it is executing allocator code. `perf record`
agrees: `String::from_lex`, `read_string_inner` and `vec_from_lex`
together account for ~33%, while `malloc`, `cfree`, `try_allocate_in` and
libc internals take most of the rest.

The decode path has no fat left to trim. `decode_owned` allocates once at
exact capacity (the raw byte length is a valid upper bound because no
escape decodes to more bytes than it occupies), so there are no growth
reallocations; `read_string_no_validate` already avoids a redundant
validation pass; and the escape-free case is a single sized copy.

The premise that "the gap is larger than allocation alone would suggest"
does not survive measurement — 10 000 heap allocations and frees is
simply what the workload costs. Bourne still wins the head-to-head
(546.8 µs vs 629.4 µs). Callers who want the difference should use
`&str` or `Cow<str>`, which the crate already documents as the default
choice for string fields. **Closed: measured, no code change.**

---

## Test-coverage gaps this audit exposed

The suite is green at this revision while A1 and A2 are both live. Three
specific gaps let that happen:

1. **No derive-side escaped-key round-trip property.**
   `properties.rs` has `bourne_serialized_map_key_round_trips`, which
   covers escaped keys through the `BTreeMap` path — the path that works.
   There is no derived-struct equivalent, which is the path that fails.
2. **The escaped-key unit tests only cover second position.**
   `struct_dispatch_handles_escaped_key` puts the escaped key second,
   which takes the retry loop. No test places one first. Relatedly,
   `struct_dispatch_handles_unicode_escape_in_key` parses `{"id":7}` —
   it contains no escape at all and does not test what its name claims.
3. **The `derived` fuzz target asserts only "does not panic".**
   Its round-trip block re-parses but discards the result, so a
   serialize-then-fail-to-parse cycle passes. None of its types use a
   `rename` with an escape-worthy key. Asserting that a successful
   `to_string` must re-parse would have caught A1 directly.

`JSONTestSuite` conformance passes but exercises only the streaming
parser (it drains `Parser::next_event`), so it cannot see derive-layer
defects by construction.

---

## Recommended action plan

1. **Block the release on A1.** Give the first key the match-and-retry
   shape the subsequent-key loop already uses. The lexer's rewind
   contract already supports it. Add the regression matrix the gap
   analysis implies: escaped key in first / middle / last position, as
   the sole field, and for each enum tagging mode.
2. **Fix A2** by routing the derived `f32` arm through the same finite
   check as `FromJson for f32`, and add a test asserting the two paths
   agree on `1e300`.
3. ~~**Decide A3 before cutting**~~ — no decision was needed. Probing
   showed the reader half rejects valid JSON (A1's class, not cosmetic)
   and that the type checker resolves aliases at the concrete call site,
   so neither documenting nor breaking was necessary. Fixed.
4. **Close the coverage gaps** — a derived escaped-key round-trip
   property, a first-position escaped-key test, and a `derived` fuzz
   target that asserts successful serialization re-parses. Fix the
   mis-named unicode-escape test so it exercises an escape.
5. **Correct the A4 comments** in the same pass; the `ByteSink` one
   describes an invariant the code does not maintain, which is the kind
   of comment that gets trusted during a future refactor.
6. ~~**Schedule P1** … and settle P2 with a flamegraph~~ — both done.
   P1 fixed (27% faster on the target workload; bourne now leads all 15
   head-to-head cases). P2 measured and closed with no code change: the
   cost is irreducible allocation, and the decode path is already
   single-allocation at exact capacity.
7. **After the fixes land**, re-run the full battery (workspace suite in
   every feature combo, clippy, fmt, MSRV, bare-metal, Miri, all three
   fuzz targets, `cargo graph --report`, cargo-crappy) and push to CI for
   a green run on the exact release revision.

Acceptance for the cut: bourne's own serializer output re-parses for
every supported attribute shape including escaped keys in any position;
derived and library readers agree on numeric range rejection; a green
Actions run on the release revision; and explicit sign-off on any
remaining limitation — for A3, that generic `field: T` behaves as
non-`Option`, which is documented in `option_shape.rs`.

## Verification performed for this audit

Read: the `unsafe` surface (float formatter bounds, `Vec::set_len` sites,
both SSE2 scanners, `from_utf8_unchecked` justifications), the derive
codegen (`object`, `field_plan`, `acquire`, `key_walk`), the lexer's key
and string paths, and the sink implementations.

Ran: the workspace test suite (green), cargo-crappy at threshold 21
(385 functions, no warnings), `cargo graph --report` (unchanged: 56
types, the same 2 known back-edges), the `compare` and `realistic` divan
benches, and standalone reproduction programs for A1, A2, and A3 built
against the crate as an external consumer.

Checked and found correct: float round-trip through `str::parse` for
subnormals, `f64::MAX`, and the `0.1+0.2` case; `u64::MAX` / `i64::MIN`
boundaries; `1e309` overflow rejection; `-0.0` sign preservation;
duplicate-key rejection on both derive and map paths; the `format_finite_to_ptr`
bounds arithmetic (worst case 26 ≤ 32 bytes); and the SSE2 signed-compare
argument that makes the string-path `from_utf8_unchecked` sound.

---

## Remediation

### A1 — fixed

`key_walk` now runs one loop in which every key, first included, is
acquired by the borrowing call and retried through `object_key_cow` on
`InvalidEscape`. Position no longer decides anything. The rewrite also
removed the duplicated match arms the old two-site structure required.
No library change was needed: the lexer's rewind contract already
supported this, as its own `first_key_escape_rejects_then_decodes` test
showed.

Verified: the two new property/round-trip tests fail on the pre-fix
codegen with exactly the reported `InvalidEscape at byte 3` and pass
after; escaped keys parse in first, middle, last and sole position and
in any permutation; `to_string` output re-parses for plain structs and
internally-tagged enums; malformed escapes (`\q`, truncated `\u`, lone
surrogate) in first position are still rejected, as are duplicate and
unknown escaped keys, so the retry did not become permissive.

### A2 — fixed

The derived `f32` arm applies the same finite check as `FromJson for
f32` rather than a bare `as` cast. A test asserts the two paths agree
on `1e40`, `-1e40` and `1e300`, and that `f32::MAX`, underflow to zero
and ordinary values still parse.

### A3 — fixed, and the finding was understated

Probing before implementing corrected the finding twice.

**The severity was wrong.** `is_option` gates two decisions, and the
original write-up only examined the writer. The reader's `finalize`
branches on it too, so an aliased `Option` whose key is *absent* is a
hard `MissingField` error:

```text
struct S { name: MaybeName }   // type MaybeName = Option<String>;

{"name":"x"}   -> Ok
{}             -> Err(MissingField)   // valid JSON, rejected
```

That is A1's class of defect — valid input refused — not a cosmetic
`"name":null`. It went unnoticed because the type still round-trips
against *itself*: the writer emits the explicit null that the reader
then accepts. Against any other producer, which simply omits the key,
it fails.

**The premise was wrong.** "A proc macro cannot resolve aliases" is
true and irrelevant: the macro does not need to, because the type
checker already has. Rust prefers an inherent method over a trait
method, so an inherent impl on an `Option`-shaped wrapper wins whenever
the field really is an `Option`, however spelled, and a blanket trait
impl catches everything else. This is specialization that works on
stable, resolved at the concrete call site the derive expands to. A
standalone probe confirmed 19/19 cases before any crate code changed,
including both alias forms and `Option<T>` with `T` a type parameter.

So the release decision the finding asked for — document the limitation,
or break the API by rejecting the attribute — was a false choice. There
was a third option that is neither.

The fix is `crates/bourne/src/option_shape.rs` (`OptionShape` for the
writer's skip decision, `OptionSlot` for the reader's absent-key rule),
with `Naming::is_option` and `FieldPlan::option_scrutinee` deleted: the
syntactic test is gone from the derive entirely rather than left as a
second, disagreeing implementation.

One genuine limit remains, documented in the module: a fully generic
field (`field: T` where `T` is a type parameter) takes the fallback and
behaves as non-`Option`, since inherent impls are not selectable in a
generic context. `Option<T>` *is* detected, because the outer
constructor is concrete — which is the case that occurs in practice.

Verified: 21 tests in `tests/option_aliases.rs` covering both directions
for direct `Option`, plain aliases, generic aliases, `Option<T>`,
non-`Option` fields, renamed fields, and enum variants (external and
internally tagged). Against the pre-fix codegen, 11 of them fail with
exactly the reported signatures — `"name":null` on the writer,
`MissingField` on the reader — and the 10 that pass are the control
cases that were never broken.

### A4 — fixed

The `ByteSink` and float-module comments now describe what the code
does. Auditing them turned up a third stale comment the original pass
missed: the float module claimed a Grisu3 formatter with a libstd
fallback, while the crate uses teju-jagua, which always succeeds and
has no fallback. A test named `fmt_sink_handles_floats_with_grisu3` was
renamed to match.

### Coverage gaps — closed

All three are addressed: a derived escaped-key round-trip property over
key permutations, unit tests for every escaped-key position including
first, and a `derived` fuzz target that now asserts a successful
`to_string` re-parses to an equal value. The vacuous unicode-escape test
now contains an actual escape.

### Verification of the fixes

Workspace suite green in default, `--all-features`, bare `no-default-features`
and `alloc,derive`; clippy `-D warnings` clean; fmt clean; MSRV 1.85
green; Miri 110/110 with no UB (the `teju_gen` table emitter is excluded
— it writes a file, which Miri's isolation blocks, and it is unrelated
to these changes); `derived` and `typed` fuzz targets 20k runs each with
no crash and rising coverage; `cargo graph --report` unchanged (same two
known back-edges); cargo-crappy 385 functions clean.

The `compare` bench was re-run because A1's fix rewrote a hot path:
derived-vs-`serde_json` is 1.92× (1.86× before), with the whole run
including `serde_json` and the non-derive path drifting by a similar
margin — machine noise, not a regression. The uniform walk costs nothing
measurable.

P1 and P2 remain open as scheduled optimization work.
