# Owner ruling 2 — Unit A live exit check

**Date:** 2026-07-31. **Supersedes** the pending request for a resource-policy revision.

---

## Decisions

| Request | Ruling |
| --- | --- |
| Revise the resource policy / raise the 20 s deadline | **Denied** |
| Change the outstandings fetch shape | **Denied** |
| Add failed-window diagnostics + one measurement on Aarav | **Denied — the failing request is already isolated** |
| Segment on `AlterID` instead of dates | **Required** |
| Re-target the live exit check away from Aarav | **Required** |

---

## Evidence

Measured directly against port 9000 with Bridge's exact rendered request, window
`20240401..20240401`, 1,632 vouchers, 4,894 allocation blocks in every case.

| Fetch shape | Time | Bytes | `BILLTYPE` returned | Correct |
| --- | --- | --- | --- | --- |
| `ALLLEDGERENTRIES.*` (current) | **61.7 s** | **33.7 MB** | `On Account 1629`, `New Ref 1`, `Agst Ref 1` | **yes** |
| `…LIST.BILLALLOCATIONS.*` | 5.5 s | 5.39 MB | empty on all 4,894 | no |
| `…LIST.BILLALLOCATIONS.LIST.*` | 2.0 s | 5.32 MB | absent on all 4,894 | no |

1. **The 20 s deadline is not the defect.** 61.7 s cannot fit 20 s. The typed `Partial` at
   20.06 s / 20.09 s is fail-closed behaviour working exactly as designed, on both SKUs.
2. **The wildcard is correct and must stay.** Both cheaper shapes are 11–30× faster and
   silently empty `BILLTYPE`, destroying the `New Ref`/`Agst Ref` pairing that outstandings is
   computed from. The mid shape additionally loses ₹163,382.80 of allocation value
   (₹182,030,529.77 vs ₹182,193,912.57) while returning the full row count and no error.
   Guide §2.4a's open question "does a correctly-resolving named path exist" is now answered:
   **no**.
3. **Per-row cost of the only correct shape is ~20,650 B/row**, independently confirming the
   21,532 B/row wildcard figure in §2.6.

---

## The structural finding

Per-request budget is **~530 vouchers** by time (~1,600 by the 32 MiB cap — **time binds
first**). `SVFROMDATE`/`SVTODATE` cannot address anything finer than **one day**, and
`20240401` alone holds 1,632 vouchers.

**No date-based segmentation can succeed on this book at any segment size.** The minimum
addressable window exceeds the per-request budget. This is not a tuning problem.

**Do not raise the deadline in response.** A longer deadline walks into the 32 MiB cap and
I4's invisible truncation — trading a loud, correct failure for a silent wrong answer.

---

## Required work

### R1 — Segment on `AlterID`, not on dates

Guide §10.1 **verifies** that `$AlterID > N AND $AlterID <= M` filters server-side. It
partitions a book at arbitrary granularity, independent of the calendar, and has **no floor**.

- Keep the date window as the *reporting* period (`as_of`), not as the *segmentation* axis.
- Size segments from measured wildcard cost (~20,650 B/row), targeting well inside 20 s —
  start at **400 rows** and let the existing trend guard shrink it.
- Keep every existing completeness, digest and fail-closed guarantee. Segmentation changes
  which rows a request asks for, nothing about how results are verified.
- **Untested at outstandings scale** — treat first live numbers as measurements, not
  confirmation.

### R2 — Re-target the live exit check

The exit check **cannot pass on `Aarav Trading Company Demo` regardless of R1**. Confirmed on
`20240401`: **2 named bills in 4,894 allocations** (1,629 `On Account`), matching
`TEST_CORPUS.md` §3. A fully successful read yields essentially nothing to reconcile against
Tally's native outstandings report, so the reconciliation criterion is unmeetable there.

- Point the exit check at the purpose-built bill-bearing company specified in
  `TEST_CORPUS.md` §4 (200–500 vouchers, `New Ref`/`Agst Ref` pairs across ageing buckets).
- **That company does not exist yet** and is being created through the Tally UI. Until it
  does, keep the live exit check `#[ignore]`d and blocked — do not weaken its assertions to
  make it pass.

### R3 — Keep Aarav for what it is good for

Aarav stays the protocol- and scale-testing corpus: R1's segmentation behaviour, throughput,
trend-guard shrinkage and completeness verification should all be exercised against it. Only
the **reconciliation** criterion moves.

---

## Constraints

- **No live retry loops.** One request at a time, health-check between. A loop of failing
  requests has previously blocked the operator's Tally for 44 minutes.
- Do not rerun a live command that timed out or returned `Partial` without an explicit reason
  to expect a different result.
- No push, PR, merge or release without separate owner authorization.
- Record deviations in `EXECUTION_LOG.md` and `UNIT_A_REPORT.md` as already established.

---

## Deliverable

Offline first: R1 implemented with tests, full workspace suite green, clippy clean. Report
back **before** any live run, stating what live evidence remains and that R2 is blocked on the
new company.
