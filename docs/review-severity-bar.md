# Review severity bar

This defines review-finding severity — what a reviewer (human or agent) calls
out in a PR. It is a different scale from the issue-tracker `severity:p1..p4`
labels in [AGENTS.md](../AGENTS.md#rectification-expectations); those grade
production bug impact after the fact, this grades a finding at review time.

**The rule: P1 blocks the merge. P2 and P3 do not block, but every P2/P3
finding needs an explicit disposition — `fixed`, `stale`, `rejected`, or
`accepted-follow-up` — each stated with the evidence behind it.** Filing a
tracking issue is one disposition among four, not the default one. The old
unwritten rule ("P1 blocks, log everything else as an issue") produced 27
tracking issues and two duplicate pairs out of a single PR review. Do not
repeat that: `stale` and `rejected` are legitimate, cheap outcomes and should
be used whenever they are true.

## Area sensitivity first

Before applying a severity, classify the area the diff touches:

- **Strict bar** — anything that reads or writes Tally (transport, protocol,
  parser, `master_binding.rs`, `book_presence.rs`, sync/reconciliation,
  migrations), anything handling money (ledger amounts, GST, trial balance,
  reports), anything handling DSC/credentials, or anything handling personal
  or customer data.
- **Loose bar** — one-off scripts, dev tooling, CI convenience helpers,
  internal test fixtures, and docs that carry no compatibility claim. A gap in
  a manual helper script is not a P1 even if the same defect class would be
  P1 in `master_binding.rs`.

The same bug shape gets a different severity depending on which side of this
line it lands on. Say which side you put it on when you file the finding.

## P1 — blocks the merge

A P1 is a defect in the strict-bar area that would misbind, miswrite, leak,
or silently corrupt — or a claim that would let someone trust evidence that
was not actually produced.

- **Wrong-ledger / wrong-master financial writes.** Binding a document entity
  to the wrong existing master, or creating a master that silently shadows an
  existing one. Example calibration: a compact `2025-2026` fiscal-year range
  parsed as an 8-digit numeric identifier and bound to the wrong ledger — this
  is exactly the class ADR 0016 exists to close off.
- **Privacy leak or new category of sensitive data** committed, logged, or
  exposed — real client data, credentials, certificate material, personal
  identifiers, absolute developer paths.
- **Scope/qualification errors written into shared code.** A rule proven true
  under one configuration (e.g. TallyPrime 7.1 Silver only, or one voucher
  type, or one license tier) applied unconditionally in code or docs with no
  scope tag, so it silently mis-fires on an untested configuration. This class
  was 78% P1 in the historical corpus — the highest of any class except
  privacy. Calibration: "SVCURRENTCOMPANY guards imports" proven on 7.1 Gold
  and then stated as a general Bridge behavior.
- **Overclaimed evidence that would be relied on.** A doc or comment asserting
  something was measured, verified live, or observed against a real Tally
  instance when the actual evidence is synthetic, simulated, or absent.
  Calibration: a paragraph claiming "verified against live Tally" when the
  receipt is a fixture the codebase generated for itself (AGENTS.md P1: "it
  verified that Bridge agreed with Bridge").
- **Silent data loss or silent write failure** — a fallback that swallows an
  error instead of failing loud and in-band (AGENTS.md P7).
- Domain-logic bugs in `master_binding.rs` or `book_presence.rs` that would
  change what gets bound, tombstoned, or posted.

## P2 — needs a disposition, does not block

A real defect or gap, but in a place where being wrong costs a fix-forward,
not a corrupted book or a leaked secret.

- A domain-logic edge case in strict-bar code that cannot reach a wrong write
  (e.g. a display-only formatting slip in an evidence drawer).
- A doc paragraph that is imprecise but not load-bearing — nobody would build
  on the imprecision to skip a real check.
- A scope gap in loose-bar tooling (a script that mishandles one input shape).
- Missing test coverage for a case that is unlikely but plausible.
- A fail-open fallback whose blast radius is confined and already logged.

Disposition options, each stated with evidence:

- `fixed` — link the commit/diff that fixes it.
- `stale` — say what already covers it (a later commit, an existing check)
  and where.
- `rejected` — say why it's not a real problem here (e.g. the configuration
  it worries about doesn't exist in this codebase).
- `accepted-follow-up` — only when the fix is genuinely out of scope for this
  diff; link the tracking issue you filed and say why it can't be `fixed` now.

## P3 — note it, no disposition required

Style, naming, minor duplication, or a suggestion with no correctness impact.
Mention it if useful; do not block review completion on it.

## What does NOT get a severity

An observation about code the diff didn't touch is out of scope for review —
flag it separately (e.g. a spawned follow-up task), don't fold it into this
PR's findings list.
