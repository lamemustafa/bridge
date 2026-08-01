# Owner ruling 4 — date-boundary compatibility profile

**Date:** 2026-07-31. **Unit A remains blocked. No live request is authorized by this ruling.**

## Decision

The day `01`/`02`/`31` period-boundary rule is verified only for TallyPrime Edit Log 7.0
Educational mode. It is not a universal `DateWindow` invariant.

- A detected Educational-mode compatibility profile retains the verified strict boundary rule.
- Licensed or unknown mode accepts ordinary calendar boundaries.
- Unknown/incomplete product-mode evidence takes the mode-agnostic path; Bridge must not invent
  Educational compatibility evidence.
- Every returned voucher must still lie inside the requested date span. A mismatch remains a
  typed `Partial` under I12.
- `NarrowDateWindow` remains capped at 31 calendar days for payload and latency safety.

## Offline implementation evidence

`DateBoundaryProfile` is now a mandatory `DateWindow` constructor argument and travels with the
window. The runtime selects it from cached detected
product/mode evidence: a recognized Tally product plus explicit Education/Educational mode is
strict; licensed, unknown, absent or inconsistent evidence is mode-agnostic. The uncalibrated
outstandings path still returns before endpoint admission.

Regression tests prove an ordinary day-15 boundary is accepted by the mode-agnostic profile,
the same boundary is rejected by the Educational profile, and a live-capture-derived voucher
outside the requested span still produces `Partial(voucher_outside_requested_window)`.
