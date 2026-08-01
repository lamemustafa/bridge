# Owner ruling 5 — accepted corpus calibration

**Date:** 2026-07-31. **Unit A is unblocked for calibration. Reconciliation remains withheld
until the owner resolves the empty-partition conflict below and rules on an initial `AlterID`
width.**

## Company-selection correction

The `Company` collection enumerates every loaded company; `SVCURRENTCOMPANY` does not filter
the returned rows. The Unit A company-extent boundary now deserializes every `<COMPANY>` row,
selects exactly one row by the expected GUID, and only then validates the response name and
`NAME` attribute. A missing GUID match is `CompanyIdentityMismatch`; duplicate matching GUIDs
fail closed as `company_identity_ambiguous`.

The regression input is derived from the retained real company-extent capture. It proves that
an unrelated first row cannot displace the expected GUID and that duplicate expected-GUID rows
cannot produce a `PinnedCompany`.

The ignored calibration harness is bound before endpoint contact to:

- port `9000`;
- company `Bridge Billwise Lab`;
- GUID `75f7566d-7a4f-431a-9642-e93a9d06d57d`.

It still reserves each evidence identity before contact, performs exactly one paired wildcard
read per invocation, checks health before/between/after, does not retry, and does not mutate the
production sizing policy.

## Comparable live samples

All three samples used the identical narrow window `20260701..20260731`, wildcard request
shape, and `AlterID` range `0..252` on port 9000. Each row below is one separate manual test
invocation.

| Sample | Rows | Encoded bytes | First read | Second read | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 18 | 280,221 | 106 ms | 114 ms | Complete |
| 2 | 18 | 280,221 | 113 ms | 135 ms | Complete |
| 3 | 18 | 280,221 | 111 ms | 130 ms | Complete |

All six response files have SHA-256
`989c9cffed082fea549b61444e9eb918862e7f19ff3e68cfccc8c8152aea59a5`. The three rendered
requests and three company-extent responses are likewise byte-identical within their artifact
class. Evidence is retained under `.bridge-live/calibration/` and remains gitignored.

## Initial width proposal withdrawn after offline exit audit

The first report proposed `AlterID` width **252** because it was the largest directly measured
span and had substantial time/byte margin. That proposal is **withdrawn before encoding**.

The offline exit-path audit exposed a structural conflict with the already-approved empty-range
rule:

1. Width 252 produces the single range `0..252`. If a narrow date partition is genuinely empty,
   the paired read produces `EmptySegmentCandidate`, but there is no adjacent range inside the
   high-water mark. Runtime must return `empty_segment_has_no_adjacent_corroboration_window`.
2. A smaller width does not solve a genuinely empty date partition. Its one permitted wider
   adjacent pair is also empty, which must return `empty_corroboration_wider_window_empty`.
3. The accepted corpus has date-local bands at `202404`, then `202407`; the fixed Education
   partition `20240502..20240601` therefore has no vouchers. More generally, real books can
   legitimately have empty reporting periods.

Focused regressions now prove both mechanical facts: a width equal to the high-water has no
adjacent range, and an empty wider pair remains Partial. Consequently **no initial width can
make this complete-book exit pass under the current combination of fixed date partitions,
exact `0..ALTVCHID` coverage per partition, and the approved empty-pair policy**.

No production constructor or width has been added. The runtime continues to return
`outstandings_segment_sizing_uncalibrated` before endpoint admission. The unweakened
reconciliation exit checks on ports 9000 and 9001 remain ignored. An owner ruling is required
on how a truly empty date partition can be proven complete before a width can be proposed
again; no live request can settle this type-level conflict.

## Exit-check strengthening

The earlier ignored exit check was not actually a reconciliation check: it accepted any
200–500-voucher report with non-zero totals and non-zero monetary ageing buckets. It could pass
while disagreeing with every accepted native-report number.

The report now carries exact receivable bill counts alongside monetary ageing totals. The
ignored exit is pre-contact bound to ports 9000/9001, the accepted company name/GUID, 220 source
vouchers and as-of `20260731`; completion must equal ₹45,14,597, 48 open receivable bills, and
ageing counts 4/4/4/36. Monetary ageing buckets must also sum exactly to receivable total.

## Offline verification

- `cargo test --workspace`: pass, including three compile-fail doctests.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- `git diff --check`: pass after the calibration report updates.
- Focused empty-corroboration, high-water range, exact bill-count and exit-preflight tests: pass.

No reconciliation request was run by this ruling.
