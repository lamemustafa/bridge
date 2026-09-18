# Proposed CI gates — deterministic review coverage

This repository's only automated code reviewer went away. Analysis of 880 real
review findings found five classes that are mechanically catchable. This
document is the reviewable patch a human applies to `.github/workflows/ci.yml`
— nothing under `.github/` was touched directly (hard boundary for this
change). Every snippet below is marked **BLOCKING** (fails the run) or
**REPORTING** (`continue-on-error: true`, always green, visible in logs) per
the rule: only a gate that already passes clean against this repository today
is proposed as blocking.

All new scripts live in `scripts/` and each has a `*.test.mjs` sibling
(`node --test scripts/*.test.mjs`, already wired into `pnpm test`, already
covers them).

## Read this first: the clippy `[lints]` table has a side effect on existing CI

`src-tauri/Cargo.toml` and `tools/Cargo.toml` now carry a `[workspace.lints.clippy]`
table turning on the `pedantic` group at `warn` (see those files' comments for
the full reasoning and the two lints — `missing_errors_doc`, `must_use_candidate`
— deliberately left off). **`warn`, not `deny`** — a plain `cargo check`/`cargo
build`/IDE run is unaffected.

But this repository's existing CI already runs several `cargo clippy ... --
-D warnings` steps, and `-D warnings` escalates *every* enabled warning to a
hard error, including ones newly enabled by this table. Measured on this
branch: merging the `[lints]` table as-is, with no other change, turns every
existing `-D warnings` clippy step in `ci.yml` red — 1,150 new warnings in the
src-tauri workspace, 49 in the tools workspace (1,199 total; full breakdown by
lint below). That is **not proposed** — it would break `tally-portable` and
`native` on the next push to master. The fix is mechanical: every existing
`-- -D warnings` invocation gets `-A clippy::pedantic` appended, which keeps
today's zero-warning bar on the default lint groups exactly as strict as it is
now, while the newly-enabled pedantic group stops being escalated by that
specific invocation. A **separate, new, REPORTING** job then surfaces the
pedantic count without gating anything. Both parts are below, and neither is
optional — shipping the `[lints]` table without the `-A clippy::pedantic`
additions breaks CI; shipping the additions without a reporting job means the
1,199 pedantic findings this table exists to surface are invisible again.

### 1a. BLOCKING (preserves current behaviour) — append `-A clippy::pedantic` to every existing `-D warnings` step

Eight existing lines in `ci.yml`, each ending `-- -D warnings`, need
` -A clippy::pedantic` appended so they keep testing only what they test
today:

- `tally-portable` job, step "Lint portable Tally truth layer"
- `tally-portable` job, step "Lint portable Tally qualification tools"
- `tally-portable` job, step "Lint disabled protocol evidence features"
- `tally-portable` job, step "Lint isolated native outstandings qualification path" (all three `cargo clippy` lines in that step's `run: |` block)
- `native` job, the bare `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets --timings -- -D warnings` step
- `native` job, step "Lint Tally qualification tools workspace"
- `native` job, step "Lint isolated native outstandings qualification feature"

Example (one of the eight — the pattern is identical for the rest: append
` -A clippy::pedantic` to the existing command, nothing else changes):

```diff
       - name: Lint portable Tally truth layer
         working-directory: src-tauri
         run: >-
           cargo clippy --locked
           -p bridge-tally-core
           -p bridge-tally-protocol
           -p bridge-tally-transport
           -p tally-protocol-simulator
-          --all-targets -- -D warnings
+          --all-targets -- -D warnings -A clippy::pedantic
```

### 1b. REPORTING — new pedantic-lint advisory job

```yaml
  lint-pedantic-advisory:
    name: Clippy pedantic advisory (non-blocking)
    needs: changes
    if: github.event_name != 'pull_request' || needs.changes.outputs.native == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 15
    permissions:
      contents: read
    continue-on-error: true
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable
        with:
          toolchain: 1.96.0
          components: clippy
      - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2
        with:
          workspaces: |
            src-tauri -> target
            tools -> target
      - name: Report pedantic findings (src-tauri workspace)
        run: cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets
      - name: Report pedantic findings (tools workspace)
        run: cargo clippy --locked --manifest-path tools/Cargo.toml --workspace --all-targets
```

No `-D warnings`: this step's exit code is not the point (`continue-on-error`
covers it regardless), the printed warning list is. `too-many-lines-threshold`
is set to 150 in `src-tauri/clippy.toml` / `tools/clippy.toml` (default 100;
see those files' comments for why 150 was chosen as the first, deliberately
loose bar rather than tightened further).

**Measured counts (2026-09-15, this branch, both workspaces, after the two
lints above are dropped):**

| Workspace | Warnings |
| --- | ---: |
| src-tauri | 1,150 |
| tools | 49 |
| **Total** | **1,199** |

Top contributors (src-tauri; tools follows the same shape at smaller scale):
`needless_pass_by_value` (153), `cast_possible_truncation` (146),
`too_many_lines` (73), `doc_markdown` (72), `large_futures` (70),
`redundant_closure_for_method_calls` (54), `format_collect` (52),
`semicolon_if_nothing_returned` (51) — full breakdown (55 distinct lints) was
produced by running `cargo clippy --message-format=json` and grouping by
`message.code.code`; available on request, omitted here for length.

## 2. `cargo fmt --check` — already effectively covered, no new YAML needed

`rustfmt.toml` (edition 2021, every other option left at rustfmt's default)
was added at the repo root. The existing `rust-format` job already runs
`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` on every PR
— **no new YAML is proposed here**, since adding the file changes nothing
about what that step does. Measured: `cargo fmt --check` reports **0 files
that would change** in either workspace (src-tauri and tools) with this file
present — the whole tree was already conformant to rustfmt's defaults.
`tools/Cargo.toml`'s workspace has no equivalent CI step today; add one if
that workspace should be checked too (out of scope for this change — flagging
the gap rather than silently leaving it uncovered):

```yaml
      - run: cargo fmt --manifest-path tools/Cargo.toml --all -- --check
```

## 3. Fixture provenance — BLOCKING (wired)

`scripts/check-fixture-provenance.mjs` generalises the
`tests/fixtures/*/PROVENANCE.md` pattern (a per-fixture line naming where the
bytes came from, and — for a fixture asserted as byte-exact captured evidence
— its size and SHA-256) to every directory `check-fixture-byte-integrity.mjs`
already covers. When first proposed it failed on 51 of 125 fixtures (28/101 in
`bridge-tally-protocol/tests/fixtures`, all 20/20 in
`tally-protocol-simulator/fixtures`, both 2/2 in
`docs/tally/compatibility/fixtures`, 1/3 in `scripts/fixtures`), so it was
proposed as REPORTING. Those 51 are now backfilled and it passes clean, so it
is wired into `ci.yml` as a blocking step in the `tally-portable` job, directly
after `Enforce fixture byte integrity`:

```yaml
      - name: Enforce fixture provenance
        run: node scripts/check-fixture-provenance.mjs
```

## 4. Unbounded reads — BLOCKING

`scripts/check-unbounded-reads.mjs` requires every `Read::read_to_end`/
`read_to_string` call site (outside tests) to be capped with `.take(N)`, the
pattern already used in `src-tauri/src/dsc.rs`, `agent_desktop_journal.rs`,
`source_draft/files.rs`, and `tools/bridge-tally-qualification`. Passes clean
today: 15 call sites scanned, 10 bounded, 3 reviewed exceptions (zip entries
read from a bundled XLSX/PDF template, not untrusted input — named in the
script's `ALLOWED_UNBOUNDED`), 0 unbounded.

```yaml
      - name: Enforce read bound coverage
        run: node scripts/check-unbounded-reads.mjs
```

Place in the `tally-portable` job, alongside
`check-tally-live-read-boundary.mjs`. No Rust toolchain needed (pure Node +
`git ls-files`), so it can equally go in `workflow-consistency`.

## 5. PII regex regression coverage — BLOCKING

`scripts/check-pii-regex-regression-coverage.mjs` enforces: a diff editing a
PII-matching regex inside a file named like a scanner/sanitizer
(`sanit*`/`redact*`/`scrub*`/`*pii*`) must touch that file's `*.test.<ext>`
sibling in the same diff. Diff-based (compares against
`origin/${{ github.base_ref }}` / `origin/master`), so it needs
`fetch-depth: 0`, same as `check-protocol-section-numbers.mjs`. Passes clean
today (no PII regex edits in this branch's diff — the gate has not yet been
exercised against a real regex-editing PR; noted as a real limit on how much
confidence "passes clean" should carry here).

```yaml
      - run: node scripts/check-pii-regex-regression-coverage.mjs
```

Place in `workflow-consistency` (already checks out with `fetch-depth: 0`),
immediately after `check-protocol-section-numbers.mjs`.

## 6. Parser accept/reject test symmetry — BLOCKING

`scripts/check-parser-test-symmetry.mjs` enforces: a parser module (any `.rs`
file defining a `fn` whose name contains `parse`) gaining a new
accept-path-named test in a diff must gain a new reject-path-named test in the
same diff. Diff-based, same `fetch-depth: 0` requirement as above. Passes
clean today (no parser files touched in this branch's diff — same caveat as
§5: not yet exercised against a real parser-test-adding PR).

```yaml
      - run: node scripts/check-parser-test-symmetry.mjs
```

Place in `workflow-consistency`, same job as §5.

## 7. `gh api` pagination — BLOCKING

`scripts/check-gh-api-pagination.mjs` scans workflow YAML, `scripts/**`, and
`docs/**` for `gh api` REST calls that look like a collection listing
(`.../pulls`, `.../issues`, `.../comments`, ...) without `--paginate`, and
`gh api graphql` calls whose query requests a `nodes`/`edges` connection
without `hasNextPage` appearing anywhere in the same file. Passes clean today:
2 `gh api` invocations found repository-wide (one real, in
`release-mcpb-preview.yml`; one a test assertion string matching it), both
`--method POST` mutations, neither a list call — this repository does not yet
have a `gh api` listing call to positively exercise the rule against, which is
exactly the situation the brain note this gate is named after describes: the
hazard is invisible until someone adds one.

```yaml
      - run: node scripts/check-gh-api-pagination.mjs
```

Place in `workflow-consistency`. No `fetch-depth: 0` requirement (scans the
working tree, not a diff).

## 8. File size report — REPORTING (never blocking, by design)

`scripts/report-file-sizes.mjs` prints line-count distribution and the
largest tracked source files. There is no defensible industry line-count
standard, so this intentionally has no threshold and always exits 0 — wiring
it as `continue-on-error: true` is really just documentation; nothing in this
step can fail.

```yaml
      - name: Report file sizes
        run: node scripts/report-file-sizes.mjs | tee -a "$GITHUB_STEP_SUMMARY"
```

Place in `workflow-consistency`, or its own trivial job — it has no
dependencies beyond Node and `git ls-files`.

## Summary

| Gate | Script | Classification | Today's result |
| --- | --- | --- | --- |
| rustfmt | `rustfmt.toml` (no new step; existing `rust-format` job covers it) | BLOCKING (already is) | 0 files would change |
| clippy default groups | existing `-D warnings` steps, `-A clippy::pedantic` appended | BLOCKING (already is, unchanged) | 0 warnings (unchanged by this PR) |
| clippy pedantic | `lint-pedantic-advisory` job (new) | REPORTING | 1,199 warnings (1,150 + 49) |
| Fixture provenance | `check-fixture-provenance.mjs` | BLOCKING (wired in `tally-portable`) | 0 undocumented (51/125 backfilled) |
| Unbounded reads | `check-unbounded-reads.mjs` | BLOCKING | 0 unbounded (3 reviewed exceptions) |
| PII regex regression coverage | `check-pii-regex-regression-coverage.mjs` | BLOCKING | clean (0 regex edits in this diff) |
| Parser accept/reject symmetry | `check-parser-test-symmetry.mjs` | BLOCKING | clean (0 parser files in this diff) |
| `gh api` pagination | `check-gh-api-pagination.mjs` | BLOCKING | clean (2 invocations, both excluded mutations) |
| File size report | `report-file-sizes.mjs` | REPORTING (never fails) | 559 files, 213,322 lines |

Three of the "BLOCKING, passes clean today" gates (§5, §6, §7) have not yet
been exercised by a real PR that trips them — "passes clean" here means "the
current tree has nothing to flag," not "has been proven to correctly flag a
real violation in production." Each has a contract-test suite
(`scripts/*.test.mjs`) that does exercise the failure path on synthetic input;
that is the evidence available before the first real trip.
