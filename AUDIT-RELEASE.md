# Final pre-release audit

Audited revision: `038e832` (workspace version `0.2.2`).
Scope: `json-bourne`, `bourne-derive`, release configuration, existing tests and float-write performance. `akita/` is excluded.

## Status after remediation pass

| Finding | Severity | Status |
|---|---|---|
| R1 pretty separators | High | **Fixed.** `runtime_comma` no longer gates on `__FUSED`; separator emission is structural on every sink. 6 regression tests added to `tests/pretty_derive.rs` (rename/conditional × present/absent × first/middle/trailing × adjacent/internal/untagged), exact-output + round-trip pinned. External reproductions now pass; compact output pinned byte-identical. Workspace suite green; `derived` fuzz 10k runs clean. |
| R3 contradictory derive config | Medium | **Fixed.** `enum_mode` validates: identical tag/content rejected, `content` without `tag` rejected, `untagged`+`tag`/`content` rejected, each with a targeted message. 5 derive unit tests added; valid combinations still derive. |
| R2 CI target mismatch | High | **Fixed (script verified).** Host matrix runs host suites only; bare-metal builds live in the `no_std` job, which passes its matrix target to `scripts/ci.sh no_std <target>`. Windows step gets an explicit bash shell. Verified locally: both targets installed, all four bare-metal builds pass, host `test` recipe passes end-to-end. The workflow file itself needs one green Actions run to close. **See R9 — that run had never happened, and could not have.** |
| R9 CI trigger names a branch that does not exist | High | **Fixed.** `on:` filtered `push`/`pull_request` to `branches: [main]`, but this repository has no `main` — `mera` is the trunk and the only ref on the remote. No job in this workflow had ever executed, on any commit. Trigger repointed at `mera`, plus `workflow_dispatch` for manual runs. This invalidated the "verified on runners" half of R2 and F7 for the whole remediation pass; every such claim rested on local reproduction alone. |
| R4 feature masking | Medium | **Fixed.** Self dev-dependency is `default-features = false`; `derive` now implies `alloc` (generated readers need the alloc-gated key API — a documented fact, previously implicit); alloc-dependent integration tests declared via `required-features`; five alloc-API doctests gated. All four combos now run their claimed feature set and pass. |
| R5 float attribution | Medium | **Corrected.** Dated correction note added to `AUDIT-2026-09.md` §4.3; §4.4 recommendation no longer cites the retracted percentages. |
| R7 MSRV unsafe warnings | Low | **Fixed.** Explicit `unsafe` blocks around register-only intrinsics; `unused_unsafe` allowed (redundant on current rustc, required on 1.85). 1.85 workspace build is now warning-free. |
| R8 stale docs | Low | **Partially fixed.** Serializer header describes the teju-jagua formatter accurately; Duration/SystemTime adapters document binary64 rounding instead of promising nanosecond preservation. Graph back-edges (`EnumReader → FromJsonDerive`, `EnumWriter → ToJsonDerive`) and `ser.rs` modularity intentionally deferred to post-release per the original plan. |
| R6 version/registry | Medium | **Open.** Version still `0.2.2`; breaking changes in Unreleased imply a minor bump. Changelog entries for this remediation pass added under Unreleased. Publish-order and packaged-consumer checks remain. |

Remaining verification gaps (unchanged from the original pass): typed/`stream` sustained fuzz campaigns beyond smoke, full configured Miri with default isolation on `tests::*`, dependency advisory checking, and registry-consumer validation of final archives.

---

## Original audit (pre-remediation)

The existing suite passed, but external reproductions exposed invalid JSON from derived pretty serialization. Release CI also had a target-installation mismatch. Fix R1 and R2 before publishing; resolve or explicitly document R3 and select a version consistent with the breaking changes. This is a bounded source-and-execution audit, not a proof of correctness or an exhaustive review of every numeric input and architecture.

## Findings

### R1 — High: derived pretty serialization omits required commas

**Confirmed by execution.** `crates/bourne-derive/src/to_json/object.rs`, `ObjectWriter::write` and `runtime_comma`; `crates/bourne/src/ser.rs:1125–1143`.

The conditional and non-plain field branches use `runtime_comma`, which emits separators only when `__FUSED` is true. Pretty sinks set this false. Unlike a separator, `PrettyStringSink::object_key` only flushes the pending opening delimiter and writes the key: it does not insert a comma between members.

Reproduction types (derive both `FromJson` and `ToJson`):

```rust
struct Renamed {
    a: u8,
    #[bourne(rename = "second")]
    b: u8,
}

struct Conditional {
    a: u8,
    #[bourne(skip_if_none)]
    b: Option<u8>,
}
```

With values `a = 1, b = 2` / `Some(2)`, compact serialization is valid. Pretty serialization returns `Ok` containing respectively:

```text
{\n  "a": 1"second": 2\n}
{\n  "a": 1"b": 2\n}
```

Both fail the crate's own parser. Existing `tests/pretty_derive.rs` tests plain field names but not these attribute combinations. Shared code generation also makes enum struct variants and `rename_all` combinations important regression targets.

**Action:** make separator emission correct for both structural and fused paths. Add exact-output and parse-back tests for rename, rename_all, conditional fields (present/absent, first/middle/last), and enum struct variants under each tagging mode. Check compact output remains unchanged. This is a release blocker, not merely formatting aesthetics.

### R2 — High: normal CI test jobs request targets they do not install

**Confirmed configuration defect; local missing-target failure reproduced.** `.github/workflows/ci.yml` test job and `scripts/ci.sh::run_test`.

The Ubuntu/macOS/Windows test matrix installs the stable host toolchain, then invokes `scripts/ci.sh test`. That script runs the host suites and then builds both `thumbv7em-none-eabihf` and `aarch64-unknown-none`. Those jobs do not install either target. The separate `no_std` matrix installs its selected target, but jobs do not share toolchain installations.

The local thumb build fails with E0463 (missing `core`). This is an environment failure, not evidence that the crate fails to support the target. The workflow nevertheless cannot reliably pass on clean runners as written. Windows shell handling of the Bash script also needs explicit verification.

**Action:** keep host tests in the ordinary matrix and bare-metal builds in the existing target-installing matrix, or install all required targets in each consuming job. Use an explicit Bash shell for shell scripts where needed. Require green runs for the exact release revision; workflow presence is not evidence of a successful run.

### R3 — Medium: contradictory derive configuration can generate unreadable output

**Confirmed by execution.** `crates/bourne-derive/src/attrs.rs::ContainerAttrs::parse` / `enum_mode`.

```rust
#[derive(Debug, FromJson, ToJson)]
#[bourne(tag = "kind", content = "kind")]
enum Collision { Value(u8) }
```

This compiles and serializes `Collision::Value(2)` as `{"kind":"Value","kind":2}`. The derived reader rejects the result. Duplicate member names are an interoperability hazard even where JSON grammar permits them.

**Action:** reject equal tag/content keys at derive time with a targeted diagnostic. Extend compile-fail coverage to internal tag/field collisions, duplicate renamed fields/variants, content without tag, and contradictory untagged/tag configuration. Those additional cases are review targets, not all independently reproduced findings.

### R4 — Medium: reduced-feature tests do not exercise genuine reduced-feature builds

**Confirmed manifest configuration.** `crates/bourne/Cargo.toml` self dev-dependency enables `derive` and implicitly enables defaults (`std`). Feature unification therefore means the nominal `--no-default-features` test commands still run the std-enabled suite.

Host library builds with no defaults, and with alloc only, passed separately. They do not replace cross-target verification or behavioral tests in genuinely reduced-feature consumers.

**Action:** use an external consumer fixture or dev-dependency arrangement that does not re-enable std; test core-only, alloc, alloc+derive, std and std+derive deliberately. Keep bare-metal build gates. Do not report current no-default test results as proof of no_std test coverage.

### R5 — Medium: prior performance attribution is overstated

**Evidence-quality finding.** `AUDIT-2026-09.md` §4.3 addendum and `benches/bourne-bench/benches/floats.rs` commentary.

The earlier claim that exactly ~83% of misses belong to teju and ~17% to shape selection is not established by its probes:

- Pinning `point` changes emitted work, output length, compiler optimization and branch history, not solely one branch's predictability.
- Pinning a 17-digit mantissa while leaving the original `digits` argument violates `write_digits_at_ptr`'s documented safety precondition whenever the original digit count differs. Its backwards unsigned cursor can underflow; that diagnostic run is not trustworthy performance evidence.
- Same/four-value inputs change mantissa lengths, formatter paths and output sizes. Their cycle differences cannot all be assigned to branch misses, nor establish a 14% optimization ceiling or a 21-cycle miss penalty.
- A boolean sum in Rust does not guarantee branch-free machine code without inspecting generated assembly. Post-teju integer mantissas are not always 15–17 digits: short exact values exist.

The diagnostic edits were reverted before this audit. This finding does **not** identify that precondition violation in the current production formatter.

**Action:** retain the observed counter totals as historical observations, retract precise causal attribution, and use branch-address sampling/annotation plus invariant-preserving experiments before restructuring teju. Repeat paired benchmarks under controlled load; even within one sequential harness run conditions can drift. No last-minute float rewrite is justified by this evidence.

### R6 — Medium: release version and artifact validation need explicit completion

`Cargo.toml` still declares `0.2.2`, while `CHANGELOG.md` Unreleased explicitly lists breaking constructor and feature removals. The derive dependency is pinned to `=0.2.2`.

Both crates passed offline packaging and default package verification. That does not establish registry availability, publish order, or all-feature behavior from the final archives. The JSON package excludes integration tests; its default package build does not verify the optional derive consumer surface.

**Action:** select the next semver-compatible release number (normally a minor bump for breaking changes in 0.x), update the exact derive dependency in tandem, finalize migration notes, and verify the packaged crates with an external consumer and derive enabled. Establish whether the derive version must be published first. No publication was attempted.

### R7 — Low: MSRV build succeeds with nine unsafe-operation warnings

`cargo +1.85 build --workspace` succeeds, but reports nine E0133 warnings in `lexer.rs:1464–1479`, in `scan_ascii_string_run_sse2`. These intrinsics require explicit unsafe blocks on that compiler, while current-toolchain clippy is clean.

**Action:** make the unsafe blocks explicit and justified across the supported compiler range; verify warning-clean MSRV compilation. Warnings alone do not demonstrate undefined behavior on x86_64, where SSE2 is baseline.

### R8 — Low: structural and documentation debt remains

`cargo graph --report crates`: 56 types, 76 edges, 36 skip edges; reports zero SCC cycles but two dropped back-edges: `EnumReader → FromJsonDerive` and `EnumWriter → ToJsonDerive`. Do not describe this graph as clean. Shared error/position types account for some long edges; not every skip is a defect.

`ser.rs` remains 1,810 lines and its introductory documentation still says float implementations are a future ryu follow-up. The Duration/SystemTime parser docs promise nanosecond preservation, while serialization uses `as_secs_f64`: arbitrary large timestamps/durations cannot preserve every nanosecond through binary64 seconds.

**Action:** defer structural refactors until after the correctness release; move shared codegen responsibilities to appropriate modules rather than suppressing graph results. Correct stale algorithm and precision claims. Document adapter precision/range limitations and add boundary round-trip tests.

## Executed checks and limitations

| Check | Result |
|---|---|
| `cargo test --workspace` | Passed; includes 124 runtime unit tests, 104 API smoke tests, derive tests, conformance, properties, stress, allocation and integration tests; 1 doctest ignored |
| `cargo test -p json-bourne --all-features --quiet` | Passed |
| `cargo test -p json-bourne --no-default-features --quiet` | Passed, subject to R4 |
| Same with `--features alloc,derive` | Passed, subject to R4 |
| Host `cargo build -p json-bourne --no-default-features` | Passed |
| Same with `--features alloc` | Passed |
| Published crates clippy, all targets/all features, `-D warnings` | Passed |
| Runtime crate clippy, no defaults, `-D warnings` | Passed |
| Workspace fmt check | Passed |
| `cargo +1.85 build --workspace` | Passed with R7 warnings |
| Thumb bare-metal build | Blocked by missing target/core |
| Offline package and default verification, both published crates | Passed |
| `cargo crappy --threshold 21 --exclude-path benches/ --exclude-path akita/ --exclude-fn BigUint::add_u64` | 385 functions, no warnings; exclusions and source suppressions still apply |
| `cargo graph --report crates` | R8; initial root scan was contaminated by accidental directory and discarded |
| Miri `--lib tests::unsafe_boundary` | 15 passed |
| Miri full `--lib` with default isolation | Stopped at generator test filesystem write; not a memory-safety finding or full pass. CI uses disable-isolation; not rerun here with that setting |
| Stream fuzz, explicit GNU target, ASan default | 10,000 runs completed without crash, starting from an empty temporary corpus; very short smoke only |
| External derive reproductions | 3 expected failures confirming R1/R3 |
| Full-sample floats benchmark | Completed; see below |

Fuzz command after creating the temporary corpus directory:

```text
cargo +nightly fuzz run --fuzz-dir fuzz --target x86_64-unknown-linux-gnu stream /tmp/bourne-final-release-audit/corpus -- -runs=10000 -max_total_time=30 -artifact_prefix=/tmp/bourne-final-release-audit/
```

The explicit GNU target worked: fuzzing is not unavailable simply because the installed tool defaults to musl. Typed/derived fuzz campaigns, sustained serialization differential fuzzing, full configured Miri, other bare-metal targets, non-Linux runtime tests, dependency advisory checking and final registry-consumer validation remain unverified in this pass. No current remote CI results were inspected.

## Performance assessment

Command: `cargo bench -p bourne-bench --bench floats`, 100 samples, current source unchanged. Median microseconds:

| Workload | 100 | 1,000 | 10,000 |
|---|---:|---:|---:|
| Bourne mixed | 2.073 | 28.39 | 277.3 |
| Bourne four-value | 2.444 | 23.53 | 239.0 |
| Bourne same-value | 2.392 | 24.70 | 254.5 |
| Bourne reused buffer | 2.151 | 22.99 | 276.1 |
| serde_json mixed | 2.732 | 26.01 | 252.9 |
| ryu direct | 3.064 | 29.99 | 372.6 |

The mixed 10k median is ~9.6% slower than serde_json in this run, reversing the prior run's ordering. Wide timing spreads and inconsistent reuse results show substantial noise. This is a current observation, not proof of a new regression. No throughput SLA was supplied, and no before/after implementation change was made. Parsing, escaping, allocation and representative-struct benchmark suites were not rerun in this final pass; do not generalize this float table to overall crate performance.

**Recommendation:** prioritize valid output and release gates. Retain QUAD_LUT and the current formatter pending properly controlled evidence. Before making comparative release claims, archive repeated paired results for parsing, compact/pretty serialization, integer and float arrays, Unicode/escape-heavy strings, and representative derived structures, including compiler/CPU/configuration and output-byte counts.

## Recommended action plan

1. ~~**Block release on R1**~~ **Done:** separators fixed; attribute × sink × enum regression matrix added; external reproductions pass against the fix.
2. ~~**Repair R2**~~ **Script side done:** recipes separated; both bare-metal targets verified locally on the fixed source. Remaining: one green Actions run for the exact release revision (Ubuntu/macOS/Windows + both bare-metal jobs). **Blocked until R9 shipped** — the trigger named a nonexistent branch, so no run was reachable.
3. ~~**Resolve R3**~~ **Done:** conflicting tags rejected at expansion; negative configuration tests added.
4. ~~**Close verification gaps**~~ **Partially done:** genuine reduced-feature consumers now real (R4); targeted Miri full `--lib` without disable-isolation and sustained fuzz campaigns remain; package consumer tests and advisory checks remain.
5. **Finalize release metadata:** version/dependency coordination (R6), migration notes, package contents and publish order. Re-run package verification after metadata changes.
6. ~~**Correct evidence and docs**~~ **Done:** R5 attribution retracted, R7 warnings fixed, adapter precision claims corrected. No speculative optimizations were added during stabilization.
7. **After release:** address graph back-edges and serializer modularity incrementally, and establish repeatable performance baselines.

Acceptance: ~~valid compact and pretty output for the expanded regression matrix~~ **verified**; matching round-trip behavior or compile-time rejection for supported derives **verified**; green exact-revision platform/feature gates (CI run outstanding); verified final archives (outstanding); explicit sign-off on any remaining limitations (this document). Passing the existing suite alone is insufficient, as R1 demonstrated.
