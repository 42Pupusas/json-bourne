# Final pre-release audit (third pass)

Audited revision: `7a6f922` (workspace version `0.2.2`), working tree clean
apart from the unrelated untracked `akita/` directory, which is excluded.
Scope: everything a `cargo publish` of `bourne-derive` and `json-bourne`
would ship, plus the CI that gates it. No source or configuration was
changed during this pass; the only mutation was `git checkout --
scripts/ci.sh` to restore a mode bit (see M5).

Every claim below was re-derived from the checkout or the registry, not
carried over from earlier audit documents.

## Verdict

**Hold — metadata only.** The code is release-ready: every local gate and
13 of 14 CI jobs are green on `7a6f922` (miri was still running, ~20 min
job, and passed on the two previous runs of the same code). Publishing as
`0.2.2` is not possible and would be wrong even if it were, see M1. Fix M1
and M2, then cut.

## Findings

| # | Severity | Finding |
|---|---|---|
| M1 | **Blocker** | `0.2.2` is already on crates.io. The unreleased changes are breaking. The release must be `0.3.0`. |
| M2 | High | `repository` metadata and CHANGELOG links point at `github.com/illuminodes/bourne`, which returns 404. The remote is `github.com/42Pupusas/json-bourne`. |
| M3 | Medium | No git tags exist, locally or on the remote. The CHANGELOG's compare links reference `v0.1.0`…`v0.2.1`, none of which exist. |
| M4 | Low | `cargo doc` emits 8 rustdoc warnings: 4 broken intra-doc links, 3 links to a private module, 1 bare URL. These render as literal text on docs.rs. |
| M5 | Low | `scripts/ci.sh` keeps losing its executable bit in the working tree (index has `100755`, tree had `100644` again at the start of this pass). CI is immune since `ff09da7` (`bash scripts/ci.sh`); local `./scripts/ci.sh` is not. |
| M6 | Low | `README.md` says "Licensed under MIT", `Cargo.toml` says `license = "MIT"`, but the tree also ships `LICENSE-APACHE`, so GitHub reports the repo as dual-licensed. One of the two is wrong. |

### M1 — the version

Registry state, fetched from the crates.io API during this pass:

| crate | max version | published | by |
|---|---|---|---|
| `json-bourne` | `0.2.2` | 2026-07-23T14:54:53Z | 42Pupusas |
| `bourne-derive` | `0.2.2` | 2026-07-23T14:54:51Z | 42Pupusas |

This matches commit `de73ec9` ("Bump bourne-derive to syn 3; release
0.2.2", 2026-07-23) and the CHANGELOG's released section `## [0.2.2] -
2026-07-23`. The premise that `0.2.2` "was never released" is false;
`AUDIT-RELEASE.md` R6 recorded the version question as open, and the
"no bump" decision was made against that stale premise.

Two consequences:

1. `cargo publish` will reject `0.2.2` outright ("crate version already
   uploaded"). There is no path that ships this tree under the current
   number.
2. The Unreleased section contains four changes marked **Breaking**
   relative to the published `0.2.2`: `Lexer::new`/`Parser::new` no longer
   accept a custom depth (`with_depth` is the replacement); the `indexmap`
   feature and impls are removed (the published `0.2.2` feature list on
   crates.io still has `indexmap`); `MAX_DEPTH` is capped at 128; float
   `ToJson`/`JsonWrite` methods are no longer `alloc`-gated on `no_std`.
   Under 0.x semver, breaking means a minor bump: **`0.3.0`**.

Required edits (three lines plus a changelog heading):

- `Cargo.toml` `[workspace.package] version = "0.3.0"`.
- `crates/bourne/Cargo.toml` `bourne-derive = { …, version = "=0.3.0" }`.
  The exact pin is correct for a proc-macro companion; it just has to
  move in lockstep.
- `CHANGELOG.md`: `## [Unreleased]` → `## [0.3.0] - <date>`, add a fresh
  empty `## [Unreleased]`, and add the `[0.3.0]` compare link.

Publish order is forced by the pin: `bourne-derive` first, then
`json-bourne` (the latter's verify build resolves `=0.3.0` from the
registry). Re-run `cargo package` for both after the bump; both verified
clean at `0.2.2` in this pass (`json-bourne` 22 files, `bourne-derive`
17 files, both verify builds OK).

### M2 — the repository URL

`Cargo.toml` line 10 and `CHANGELOG.md` lines 486–489 use
`https://github.com/illuminodes/bourne`. That URL 404s. `git remote -v`
gives `git@github.com:42Pupusas/json-bourne.git`, and the published
`0.2.2` crate already carries the dead link on its crates.io page. Fix
the workspace `repository` and the four changelog link definitions.

### M3 — tags

`git tag --list` and `git ls-remote --tags origin` both return nothing.
Every prior release went out untagged. Tag `v0.3.0` at the release
commit and push it; consider back-tagging `v0.2.2` at `de73ec9` so the
changelog's compare links resolve.

### M4 — rustdoc

`cargo doc -p json-bourne --all-features --no-deps`:

- `lexer.rs:66` `[Parser::with_depth]` — `Parser` not in scope from
  `lexer`; needs `crate::Parser::with_depth`.
- `lexer.rs:243`, `parser.rs:83` `[MAX_DEPTH]` — a const generic
  parameter, not an item; use backticks without brackets.
- `lexer.rs:566` `[parse_i64_value]` — needs `Self::parse_i64_value`.
- `ser.rs:89, 301, 399` `[crate::escape]` — private module linked from
  public docs; drop the brackets.
- `float.rs:10` bare URL — wrap in `<…>`.

Not a blocker. Worth folding into the release commit since docs.rs
builds from the published tarball.

### M5 — the mode bit

The index has `scripts/ci.sh` at `100755` since `ff09da7`, but the
working tree showed `100644` at the start of this pass with no content
change. Restored with `git checkout -- scripts/ci.sh`. Something in the
local editing path rewrites the file without preserving its mode; the
`bash scripts/ci.sh` call sites in `ci.yml` make CI indifferent to it.
If it recurs, the fix is in the tool, not the repo.

### M6 — license files

Pick one: delete `LICENSE-APACHE`, or change `license` to
`"MIT OR Apache-2.0"` and update the README. The published crate says
MIT; the GitHub repo page says both.

## Verification performed

All at `7a6f922`, local toolchain 1.97.1 unless stated.

| Gate | Result |
|---|---|
| `cargo test -p json-bourne --all-features` | 132 lib + 109 api_smoke + all integration suites + doctests, green |
| `cargo test -p json-bourne --no-default-features` | green (alloc-gated suites correctly skipped) |
| `… --features alloc` | green |
| `… --features alloc,derive` | green |
| `cargo +1.98 clippy --workspace --all-targets --features json-bourne/derive -- -D warnings` | clean |
| `cargo +1.98 clippy -p json-bourne --all-targets --no-default-features -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `cargo +1.85 build --workspace` (MSRV) | clean, no warnings |
| `cargo crappy --threshold 21 --exclude-path benches/ --exclude-fn BigUint::add_u64` | 389 functions, no warnings |
| `cargo package -p json-bourne` | 22 files, 356 KiB / 100 KiB, verify build OK |
| `cargo package -p bourne-derive` | 17 files, 81 KiB / 18 KiB, verify build OK |
| `cargo doc -p json-bourne --all-features --no-deps` | builds; 8 warnings (M4) |
| CI run #4 on `7a6f922` | 13/14 jobs green: test ×3 OS, no_std ×2 targets, MSRV, clippy 1.98, clippy beta, fmt, crap, fuzz ×3. `miri` in progress at time of writing |

Not re-run this pass (unchanged code since they last ran green on
`1023029`/`b9077cd`): full Miri `--lib` under `--cfg bourne_no_simd`
(110/110), fuzz 20k runs per target. The CI `miri` job on `7a6f922` will
supersede the first of these when it reports.

## Release sequence

1. Bump to `0.3.0` in the three places listed under M1; fix the
   repository URL (M2); fix the 8 rustdoc warnings (M4); resolve M6.
2. `cargo package -p bourne-derive && cargo package -p json-bourne`, then
   `bash scripts/ci.sh all`.
3. Commit, push, wait for a fully green run including `miri`.
4. `cargo publish -p bourne-derive`, wait for the index, then
   `cargo publish -p json-bourne`.
5. `git tag v0.3.0 && git push origin v0.3.0`.
