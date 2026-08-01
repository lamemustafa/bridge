# P0b live-evidence defect findings — 2026-07-29

This is a redacted engineering record. Raw request/response pairs remain only
in ignored local capture storage. It is not a compatibility receipt, an
attestation, a matrix row, or a support claim.

## D1 — import success detection

Question: can a bare import response with `ERRORS=0` but an exception and a
line error be treated as successful?

Evidence source: local `w1`, `w4`, and `w7` captures. Their raw SHA-256 values
are, respectively, `b62c37622424dc545228a3cc9319ab0ee9931737b7ee1e4b2c511e5a1737e9d9`,
`969bcbd6064c1018e702d24d6bfe2ed471e154fe26c2d79883714817d755568f`, and
`faee23ffca0d169e6a6f03179997ce4ad939ea05ac91fdf32fb3a5b2be7477e1`.

Exact parser-relevant request/response record: imports returned a bare
`RESPONSE` result. `w1` and `w4` each had `CREATED=1`, `ERRORS=0`,
`EXCEPTIONS=0`, and no `LINEERROR`. `w7` had `CREATED=0`, `ERRORS=0`,
`EXCEPTIONS=1`, and one non-empty `LINEERROR`. The checked-in golden fixtures
preserve this structure while redacting the raw error text. The observed w4
`LASTVCHID=295` value is retained because its numeric shape is parser evidence.

Verdict: **OBSERVED**. The parser preserves all three responses as evidence.
The reviewed public success predicate was defective because it returned true
for an all-zero response; only production write callers added an intended-
counter comparison. The predicate now requires exact, non-zero intended
`CREATED`/`ALTERED`/`DELETED` counts and rejects ignored, error, cancelled,
exception, or line-error counters.

Consequence: the protocol primitive and its write callers are rectified. The
golden test makes the real `w7` structural case a must-reject for success and
`w1`/`w4` must-accept clean results. Independent tests reject an absent
intended increment, `ERRORS>0`, `EXCEPTIONS>0`, and any `LINEERROR`. This
enforces the §8.2 invariant at the shared boundary.

## D2 — Bridge V2 one-day period boundary

Question: does Bridge's exact `bridge.tally.vouchers/2` `TYPE=Data`/`REPORT`
request honor a one-day window?

Exact request: the closed Bridge V2 renderer produced the request for the
operator-specified company and `20260403` through `20260403`; the exact 6,832
request bytes remain only in ignored local capture storage. The request was a
read-only `bridge.tally.vouchers/2` `TYPE=Data`/`REPORT` export. No import or
write request was sent.

Exact response summary: the durable captures show HTTP `000` and zero
response bytes. Private operator diagnostics and isolation details remain only
under gitignored `.bridge-live/`. The public engineering conclusion is that
no `$$` function may reference a TDL identifier containing spaces; §8.10
identifies four of seven shipped read profiles that violate that rule.

Verdict: **OBSERVED**. The exact Bridge profile failed to produce a response.
Distinct `DATE` count: not available.

Consequence: Bridge's V2 report profile cannot settle the period-boundary
question because it cannot produce a response on this SKU. The shipped read
profile family is nevertheless confirmed defective, with a four-of-seven
identifier-rule blast radius. Phase 2 must replace the affected profiles as
specified by §8.12; this unit does not alter them. ADR 0015 is flagged as
contradicted, not silently edited: its exact-scope evidence claim cannot be
established by the non-functional V2/V3 profile family.

## D3 — import versus export application status

Question: can one `STATUS=1` rule govern both exports and imports?

Exact response summary: the live import captures have a bare `RESPONSE` root
and no application `STATUS` field. The parser reports that form as
`NotReported`, not as an export success.

Verdict: **OBSERVED**.

Consequence: `PROMPT_PLAYBOOK.md` now requires `STATUS=1` for exports only;
bare imports use the §8.2 clean-counter and intended-increment rule, while a
reported import `STATUS=0` fails regardless of counters. A reported
`STATUS=1` does not replace the counter or readback checks. This implements the
§8.3 amendment without rejecting valid bare import responses.

## D4 — Edit Log SKU representation

Question: can the compatibility DTO represent the lab product family?

Exact response summary: before this change `ProductFamily` had only
`tally_prime`, `tally_erp9`, and `unknown` values.

Verdict: **OBSERVED**.

Consequence: `tally_prime_edit_log` is now a distinct serializable product
family. A mode-level compatibility constraint now prevents Education receipts
for every product family from reaching `Observed` or `Supported`, including a
valid signed Edit Log receipt. Licensed promotion remains available. No matrix
row or receipt was minted from this exploratory evidence.

## Fixture-manifest limitation

The existing live fixture contract requires an empty date window. §8.8 reports
that an empty-range request can return the company period instead. The fixture
manifest is therefore intentionally unchanged; this unit does not work around
that contract failure.

## Plan clauses contradicted by this unit

- The unsplit `STATUS=1` clause in `PROMPT_PLAYBOOK.md` §1 conflicted with the
  observed import shape and §8.3; it is corrected here.
- The assumption that absence of negative import counters was a sufficient
  reusable success predicate conflicted with §8.2; an intended non-zero
  mutation is now part of the protocol predicate.
- `IMPROVEMENT_PLAN_2026H2.md` §2.5 said date-unbounded scans catch back-dated
  vouchers. Section 8.8 contradicts that claim because the loaded company
  period is a hard visibility boundary.
- The earlier §3.1.7 framing treated segmented scans as a performance design.
  Section 8.8 makes period segmentation a correctness requirement.
- The original Phase 2 custom-report extension plan is contradicted by
  §§8.9–8.11: four of seven shipped profiles violate the identifier rule, the
  report family depends on unmodelled rendering geometry, and its output shape
  is rejected by Bridge's parser. Section 8.12 supersedes that plan.
- ADR 0015's exact-scope selected-read authority is contradicted. The V2/V3
  family cannot emit the required evidence, and §8.8 separately shows that the
  requested date window was not enforced by the direct collection probe. The
  ADR is flagged here for explicit rectification; it is not silently edited.
