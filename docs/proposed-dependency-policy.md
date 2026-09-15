# Proposed dependency-security and compatibility-surface CI changes

This document proposes workflow changes that are **not implemented**. They
are written here, rather than under `.github/`, because this change was
scoped to leave that directory untouched -- an owner who wants any of the
following adopted needs to actually add it to the relevant workflow file and
review the runner cost and required-check implications, which is a
maintainer decision this document does not make for them.

Everything referenced below (`.cargo/audit.toml`, `scripts/reseal.sh`,
`scripts/check-advisory-delta.mjs`, `scripts/reseal-merge-driver.mjs`, and
their `*.test.mjs` siblings) is implemented and tested; only its CI wiring is
proposed rather than applied.

## 1. Differential dependency-audit gate for pull requests

### What `cargo audit` alone cannot do

Be precise about the boundary: `cargo audit` (with `.cargo/audit.toml`, see
that file) answers "does the lockfile in front of me right now contain an
accepted vulnerability". It has no concept of "before" and "after", and no
flag turns it into one -- `--ignore` operates on advisory ids, not on git
history. Nothing in cargo-audit 0.22.2 (checked against its own source,
`src/config.rs` and its CLI `--help`) resolves a merge base, diffs two
lockfiles, or compares two runs. **Differential auditing needs the custom
script this change adds** (`scripts/check-advisory-delta.mjs`); it is not a
built-in capability this proposal merely exposes.

### What the script does and does not change about the existing gate

`.github/workflows/dependency-security.yml`'s existing `cargo audit --file
src-tauri/Cargo.lock` step is unchanged by this proposal -- it still runs,
still fails on a real accepted-severity vulnerability, and still benefits
from `.cargo/audit.toml`'s dated ignore list on its own (that file alone
already stops the 9 standing warnings from being noise, independent of
anything below). The addition proposed here is a **second, additional**
step for the `pull_request` trigger only:

```yaml
# .github/workflows/dependency-security.yml -- proposed addition to the
# existing `audit` job, after the current "Audit locked Rust dependencies"
# step, PULL REQUEST TRIGGER ONLY (the differential check has no meaning on
# a `push: master` run, which has no "other side" to diff against -- that
# run should keep depending on the unconditional `cargo audit` step alone).
- name: Fail only on advisories this PR introduces
  if: github.event_name == 'pull_request'
  run: node scripts/check-advisory-delta.mjs
  # Needs full history to resolve a merge base, not the default shallow
  # clone -- add to the existing actions/checkout step's `with:`:
  #   fetch-depth: 0
```

With this in place: the existing unconditional `cargo audit` step is what
would have blocked all 9 PRs on 2026-09-15 (and did, until PR #381 fixed it
on master). This second step does not replace that -- master itself still
needs to be kept clean by whoever merges the fix, exactly as happened. What
it changes is a **different** scenario: a PR whose *own* Cargo.lock is
unaffected by a newly-published advisory (because master already carries the
fix, or because the advisory doesn't apply to that PR's dependency versions)
would, without this step, still show the unconditional `cargo audit` step as
red for as long as ANY accepted-severity advisory is unresolved anywhere in
the tree -- coupling every PR's status to master's current audit state
regardless of whether the PR's own diff has anything to do with it. The
differential step reports pass/fail based only on what the PR's Cargo.lock
diff introduced.

Do not make the differential step a *replacement* for the unconditional one,
and do not make it the only required check: a PR opened while master is
genuinely broken (a real, unfixed vulnerability on master) should still be
visible as blocked by *something*, and the scheduled audit in section 2
below is what should be driving that fix, not individual PRs discovering it
piecemeal.

### Proof this actually fires (from this session, not asserted)

Using the real commit pair for RUSTSEC-2026-0285 (rustls, fixed by PR #381):

```
$ node scripts/check-advisory-delta.mjs --base 9558a316... --head 8b17f95c...
check-advisory-delta: this change introduces 1 new advisory finding(s) not present at the merge base (9558a316...):
  - RUSTSEC-2026-0285 (vulnerability) rustls@0.23.43 -- TLS 1.3 handshake messages incorrectly accepted across encryption level boundaries
[exit 1]

$ node scripts/check-advisory-delta.mjs --base 8b17f95c... --head 9558a316...
check-advisory-delta: 1 advisory finding(s) present at the base are no longer present at HEAD:
  - RUSTSEC-2026-0285 (vulnerability) rustls@0.23.43 -- ...
check-advisory-delta: no new advisory findings (base has 2, head has 1, both filtered through .cargo/audit.toml).
[exit 0]

$ node scripts/check-advisory-delta.mjs                     # default: merge-base(HEAD, origin/master)
check-advisory-delta: lockfile is byte-identical between base and head -- no delta to check.
[exit 0]
```

## 2. Scheduled daily audit of master, with a tracking issue

The differential gate in section 1 only ever looks at what a PR's own diff
introduced. Nothing above makes master's own drift visible on a day nobody
happens to merge a PR -- exactly the gap that let RUSTSEC-2026-0285 sit
undiscovered until whoever next tried to merge found the whole pipeline
frozen. Proposed: a scheduled workflow, independent of any PR, that audits
master daily and files or updates a tracking issue on a new finding.

```yaml
# .github/workflows/dependency-security-scheduled.yml (proposed, new file)
name: Dependency security (scheduled)

on:
  schedule:
    - cron: "17 6 * * *" # daily; avoid :00 to dodge the top-of-hour scheduler pile-up
  workflow_dispatch:

permissions:
  contents: read
  issues: write

jobs:
  audit-master:
    name: Audit master and file a tracking issue on new findings
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@<pinned-sha> # v7, matching ci.yml's pin
        with:
          persist-credentials: false
      - uses: taiki-e/install-action@<pinned-sha>
        with:
          tool: cargo-audit@0.22.2
          fallback: none
      - name: Audit (JSON, always succeeds so the next step can inspect it)
        run: |
          cargo audit --file src-tauri/Cargo.lock --json > audit-report.json || true
      - name: Open or update a tracking issue on a new finding
        uses: actions/github-script@<pinned-sha>
        with:
          script: |
            const fs = require('fs');
            const report = JSON.parse(fs.readFileSync('audit-report.json', 'utf8'));
            if (report.vulnerabilities.found) {
              // Search for an existing open issue labeled dependency-security
              // before opening a new one -- update its body/comment instead
              // of creating a duplicate on every subsequent day the same
              // advisory remains unresolved. Label: "dependency-security".
              // (Full script omitted here; this is a proposal, not a
              // committed implementation -- see the note at the top of this
              // document.)
            }
```

Design notes for whoever implements this:

- **Idempotency matters more than the exact script.** A naive
  "always open a new issue" would file a fresh issue every single day an
  advisory stays unresolved (which, given a severity/dependency-upgrade
  investigation can take days, is the normal case, not the exception). Search
  for an existing open issue by label + a stable identifier (the advisory
  ID(s), sorted, embedded in the issue body or a hidden marker) and update it
  instead of duplicating.
- **Closing is as important as opening.** When a subsequent day's run finds
  the previously-tracked advisory no longer present (fixed, or the affected
  crate removed), the same script should comment and close the issue rather
  than leaving it open forever once the underlying problem is gone.
- This uses `cargo audit`'s own severity handling (post-`.cargo/audit.toml`
  ignores) directly -- it does not need `scripts/check-advisory-delta.mjs`,
  because there is no "PR diff" here to be differential *against*; the whole
  point is master's absolute state on a given day.
- `schedule` triggers run against the default branch only and do not need
  `pull_request` permissions; `issues: write` is the one extra permission
  beyond the existing workflow's `contents: read`.

## 3. CI for the new toolchain-dependent test files

`scripts/reseal.test.mjs`, `scripts/check-advisory-delta.test.mjs`, and
`scripts/reseal-merge-driver.test.mjs` are real, and were run for real in
this session (see the branch's commits and this change's PR description for
output) -- but as `.github/workflows/ci.yml` is currently laid out, no job
runs both Node's `scripts/*.test.mjs` glob (`corepack pnpm test`, in the
"Frontend build" job) **and** has the Rust toolchain and `cargo-audit`
these three specifically need. All three detect this at runtime and `skip`
themselves with a clear reason (visible in `pnpm test`'s own output) rather
than failing a job that was never given the tools -- see each file's own
header comment.

The `workflow-consistency` job already has exactly what's missing: it
installs the pinned Rust toolchain via `dtolnay/rust-toolchain` (for
`scripts/check-protocol-section-numbers.mjs`'s own needs) and already runs
individual `*.test.mjs` files directly (`node
scripts/check-protocol-section-numbers.test.mjs`), not through the `pnpm
test` glob. Proposed addition to that job:

```yaml
# .github/workflows/ci.yml -- proposed additions to the existing
# `workflow-consistency` job, after its current `dtolnay/rust-toolchain`
# step (which already pins 1.96.0 -- no separate toolchain setup needed)
- uses: taiki-e/install-action@<pinned-sha> # match dependency-security.yml's pin
  with:
    tool: cargo-audit@0.22.2
    fallback: none
- run: node scripts/reseal.test.mjs
- run: node scripts/check-advisory-delta.test.mjs
- run: node scripts/reseal-merge-driver.test.mjs
```

`scripts/reseal-merge-driver.test.mjs` creates and deletes real git branches
against the checkout (see its own header) and needs full history to compute
a merge base -- the existing `workflow-consistency` job already sets
`fetch-depth: 0` for `check-protocol-section-numbers.mjs`'s sake, so no
further change is needed there. `taiki-e/install-action` for cargo-audit is
the same action and version already used in
`.github/workflows/dependency-security.yml`; if that job's SHA pin is bumped,
this one should move with it (see `scripts/check-ci-workflow-consistency.mjs`,
which already exists to catch exactly this kind of drift between workflow
files -- confirm it covers action-version consistency across the two files
before relying on it here).

## 4. Structural option for the compatibility-surface conflict (NOT implemented)

This section is **analysis only**. It changes a security-relevant
tamper-evidence artifact and needs the owner's explicit decision, not an
agent's; nothing here is applied to `compatibility-surface.json`,
`compatibility-matrix.json`, or `tools/bridge-tally-compatibility`.

### The idea

`compatibility-surface.json` currently holds both the stable, large,
disjoint-by-nature list (`files`, 212 entries, ~850 of the file's 853 lines)
and one small, single-line, EVERY-CHANGE-TOUCHES-IT field
(`manifest_sha256`). The merge driver in this change works around that by
reconciling the two halves separately at merge time; the structural
alternative is to not need that workaround by splitting the file so the
volatile digest cannot collide with the stable list textually in the first
place:

```
docs/tally/compatibility/
  compatibility-surface.json        # schema_version, files[] -- NO checksum field
  compatibility-surface.digest.json # { "manifest_sha256": "..." } -- tiny, single-purpose
```

Two PRs touching different pinned files would then produce a real textual
conflict ONLY on the two one-line digest files (still a conflict -- the
digest still has to change on any content change -- but now isolated to
files that carry no other information, so a wholesale "take either side and
recompute" is unambiguously correct with no authored-content risk at all,
and no case exists anymore where a naive resolution could silently drop a
pin, because the file that could hold a dropped pin never conflicts on the
digest anymore).

### Honest analysis of what this weakens

**This is not free.** The current design's whole point, per
`docs/release-process.md`'s own description of the gate, is that
`manifest_sha256` is a checksum **over the file list**, computed with a
domain-separated hash (`checksum(b"bridge.tally.compatibility-surface/1\0",
&self)` in `tools/bridge-tally-compatibility/src/lib.rs`) that binds
`schema_version` and every entry together into one attestation. Splitting the
digest out into a sibling file does not remove that binding by itself -- the
digest can still be computed over the *same* serialized content, just read
from and written to a different file -- but it does introduce a **new**
integrity question the current single-file design does not have: are the two
files always updated atomically, together, as a single unit? A single JSON
file cannot be partially applied by any normal git operation short of a
hand-edit (which this tooling already forbids and CI checks against); two
files that reference each other CAN be independently reverted, cherry-picked,
or hand-edited to leave one stale while the other is current, without
`validate()`'s own internal consistency check ever seeing it -- because that
check currently lives *inside* one file's own structure. A cross-file
consistency check equivalent to today's would have to be added and covered
by `gate` for the split design to keep the same guarantee it has today,
and that is new surface for a mistake (e.g. a partial commit, or a
`cherry-pick` that picks one file and not the other) that the current design
structurally cannot have.

**It also doesn't fully solve the matrix side.** `compatibility-matrix.json`
has the identical problem in miniature (`compatibility_surface_sha256`
conflicts on every change) and the same split could apply there too -- but
matrix's `claims` array is not merely disjoint-by-path the way `files` is; a
claim can be legitimately *promoted* (its own fields changed) by two
different PRs working on different claims, which the merge driver in this
change already handles via real per-entry reconciliation (see
`reconcileKeyed()` in `scripts/reseal-merge-driver.mjs`) rather than treating
the whole list as append-only. Splitting the digest out of matrix.json would
still leave that reconciliation need in place; it only removes the *digest
line's* conflict, not the whole category of matrix-merge conflict this
change already handles.

### Recommendation

Given the merge driver in this change already resolves the common case
(disjoint pinned-file or claim changes) automatically, and given the
structural change would need its own new cross-file consistency
enforcement to not regress the gate's guarantee, this is a genuine
maintainer trade-off (marginally simpler merges for two brand-new files and
a new invariant to enforce and test, vs. the current design's one-file
atomicity for no incremental integrity work) rather than a strict
improvement. Not implemented here for that reason.
