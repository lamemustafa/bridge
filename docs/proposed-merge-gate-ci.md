# Proposed CI wiring for the shrunk merge gate

`scripts/merge-gate.sh` was cut from a 3977-line manual CLI tool that gated
nothing automatically down to three checks that can run as real, required CI
status contexts: compatibility-surface validation, the privacy/PII scan, and
a minimal "a review exists naming the current head SHA" check.

This document is a **proposal only**. Per the task boundary, nothing under
`.github/` was modified. A maintainer who agrees with this design should add
a new workflow file at `.github/workflows/merge-gate.yml` with the content
below (or fold it into `ci.yml`; see "Why a separate workflow file" below),
then add the job's context to branch protection's required status checks for
`master`.

## Why a separate workflow file, not a job inside `ci.yml`

`ci.yml` triggers only on `push`/`pull_request` (`opened, reopened,
synchronize, ready_for_review`) plus `workflow_dispatch`. That is enough for
the compatibility-surface and privacy checks, which only need the current
diff and are naturally re-evaluated on every push. It is **not** enough for
the review-evidence check: per issue #317, the entire point is to catch a PR
merged before any review artifact exists. A required check that can only
fire on `push`/`synchronize` would stay red forever once a reviewer finally
does leave a review or a completion summary comment, because neither of
those events re-triggers `ci.yml` — there would be no way to turn the check
green without an empty commit.

So the merge-gate job needs to also listen for `pull_request_review` and
`issue_comment` (the two ways review evidence can appear: a real review
object, or — per the "clean review, no review object, only a reaction plus a
summary comment" case the check is written to handle — a completion comment
on the PR). Adding those events to `ci.yml`'s single `on:` block would
re-trigger the entire native/bundle/frontend build matrix on every PR
comment, which is wasteful and unrelated to what this gate checks. A
dedicated workflow file scopes the extra trigger events to only the cheap
job that needs them.

## Proposed workflow

```yaml
name: Merge gate

on:
  pull_request:
    branches: [master]
    types: [opened, reopened, synchronize, ready_for_review]
  pull_request_review:
    types: [submitted]
  issue_comment:
    types: [created]
  workflow_dispatch:
    inputs:
      pr_number:
        description: PR number to re-check (defaults to the PR this run was triggered from)
        required: false
      binary_review_sha:
        description: >-
          Full 40-hex commit SHA a maintainer has manually reviewed for binary
          byte/ownership/license/NOTICE content. Only needed when the PR adds
          or changes a binary (or Git-LFS-pointer) file; leave blank otherwise.
        required: false
      independent_review_sha:
        description: Full 40-hex commit SHA of the matching independent review attestation.
        required: false

concurrency:
  group: merge-gate-${{ github.event.pull_request.number || github.event.issue.number || github.event.inputs.pr_number }}
  cancel-in-progress: true

jobs:
  merge-gate:
    name: Merge gate
    runs-on: ubuntu-latest
    timeout-minutes: 10
    # issue_comment fires for both issues and PRs; only run for PR comments.
    if: ${{ github.event_name != 'issue_comment' || github.event.issue.pull_request != null }}
    permissions:
      contents: read
      pull-requests: read
      issues: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false
      - name: Resolve PR number
        id: pr
        env:
          EVENT_NAME: ${{ github.event_name }}
          PR_NUMBER: ${{ github.event.pull_request.number }}
          ISSUE_NUMBER: ${{ github.event.issue.number }}
          DISPATCH_PR_NUMBER: ${{ github.event.inputs.pr_number }}
        run: |
          set -euo pipefail
          number="${PR_NUMBER:-${ISSUE_NUMBER:-${DISPATCH_PR_NUMBER:-}}}"
          if [ -z "$number" ]; then
            echo "could not resolve a PR number for this event" >&2
            exit 2
          fi
          echo "number=$number" >> "$GITHUB_OUTPUT"
      - name: Run merge gate
        env:
          GH_TOKEN: ${{ github.token }}
          BINARY_REVIEW_SHA: ${{ github.event.inputs.binary_review_sha }}
          INDEPENDENT_REVIEW_SHA: ${{ github.event.inputs.independent_review_sha }}
        run: |
          set -euo pipefail
          args=(scripts/merge-gate.sh "${{ steps.pr.outputs.number }}" --repo "${{ github.repository }}")
          [ -n "$BINARY_REVIEW_SHA" ] && args+=(--binary-review-sha "$BINARY_REVIEW_SHA")
          [ -n "$INDEPENDENT_REVIEW_SHA" ] && args+=(--independent-review-sha "$INDEPENDENT_REVIEW_SHA")
          "${args[@]}"
```

Notes on this draft:

- Exit codes line up with what a required check needs: `merge-gate.sh` exits
  `0` (pass), `1` (block), or `2` (indeterminate) — both `1` and `2` are
  non-zero, so the job step fails and the required check goes red for either
  outcome. There is no separate "neutral" state; INDETERMINATE is treated as
  blocking, matching the script's own stance that unknown evidence is never
  converted into an empty successful set.
- `pull-requests: read` and `issues: read` are enough: every `gh api`/`gh pr`
  call the shrunk script makes is a read against the same repository, so the
  default `GITHUB_TOKEN` (`github.token`) needs no elevated scope.
- The `--binary-review-sha`/`--independent-review-sha` attestations are
  inherently a human judgment call (someone read the actual binary bytes,
  ownership, license, and NOTICE obligations) and cannot be automated. The
  `workflow_dispatch` inputs above are the proposed escape hatch: a
  maintainer who has done that review re-runs this workflow by hand with
  both SHAs filled in. Until that happens, any PR that adds or changes a
  binary (including a Git LFS pointer, per the #360 fix) correctly stays
  blocked on the automatic `pull_request`/`pull_request_review`/
  `issue_comment` triggers.
- Add the job's status context — **"Merge gate"** — to `master`'s branch
  protection required status checks alongside the existing "Frontend build",
  "Rust format", "GitGuardian Security Checks", "Dependency security", and
  "Required checks" contexts. It intentionally is **not** folded into the
  existing `required-checks` rollup job in `ci.yml`, both because it lives in
  a different workflow (a rollup job can only depend on jobs in its own
  workflow run) and because keeping it as its own context makes it possible
  to see at a glance, on the PR, exactly which of the three concerns
  (surface reseal, privacy scan, review evidence) failed — `merge-gate.sh`
  prints a `BLOCK`/`INDETERMINATE`/`ok` line per check in its log output.
- `concurrency` is keyed by PR number (falling back across the three event
  shapes) so a review submitted while a push-triggered run is still in
  flight doesn't race it; the newer run's result is what branch protection
  sees.

## What this intentionally leaves for GitHub itself

`scripts/merge-gate.sh` no longer re-derives PR/head/base identity binding,
branch-protection required-check contexts, or a checks rollup — that logic
(roughly 900 of the cut lines) duplicated what `master`'s branch protection
already enforces natively once "Merge gate" is added as a required context:
GitHub itself refuses to merge a PR that is behind, has an unresolved
required context, or whose head has moved since a context last reported
success. Required status checks, not this script, are the source of truth
for "is everything else green."

## Design constraint if this is applied: the gate becomes force-push sensitive

Today `scripts/merge-gate.sh` is a manual command, and it reads review state
**live against whatever `$head` currently is**. That is what keeps it immune to
a hazard that has cost a sibling repository four pull requests.

A GitHub force-push records `before_commit_id: null`. The discarded head is
therefore not named anywhere in the pull request's record, and any state
addressed by that commit SHA becomes permanently unreachable. In the sibling
repository this stranded a scheduled gate's durable review state: the head it
had recorded no longer existed, so the gate could not evaluate at all and
failed closed, correctly but unrecoverably.

The distinction that matters is **storage, not addressing**. Reading by SHA is
safe; persisting state keyed to a SHA is not. This script reads by SHA
throughout and persists nothing, so a force-push makes it refuse rather than
strand:

- the review-evidence check asks whether a review or comment names the
  *current* head, so after a force-push it finds none and fails closed;
- `--independent-review-sha` and `--binary-review-sha` are compared for
  equality against the current `$head`, so a prior attestation stops matching
  the moment the head moves.

Both are the behaviour you want. Neither would survive being cached.

**Applying this proposal changes that.** Once "Merge gate" is a required
context, the gate stops being something an operator runs at a chosen moment
and becomes something that must evaluate on every push, asynchronously, with a
result recorded against a specific commit. Two consequences follow:

1. **A force-push during evaluation opens a check-of-time / use-of-time
   window.** The run was dispatched for head *A*; by the time it reports, the
   PR's head may be *B*. A result recorded against *A* is not evidence about
   *B*, and GitHub will not treat it as such for the required context — but the
   gate must not report success in a way that could be read as covering *B*.
   Re-read the head immediately before emitting a verdict and abort if it
   moved, rather than reporting against the head the job started with.

2. **Do not introduce a cache to make it cheaper.** The obvious optimisation —
   remembering that head *A* passed so a later run can skip work — is exactly
   the persistence that strands. If evaluation cost becomes a problem, shrink
   what is evaluated; do not store verdicts keyed by SHA.

Fail closed in both cases. A gate that cannot establish which commit it is
talking about must refuse, not assume.

Related: #317 records the failure from the other direction — checks read from a
head that was not the one being merged.
