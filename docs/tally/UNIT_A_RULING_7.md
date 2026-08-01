# Owner ruling 7 — the initial AlterID width, the segment budget, and the as-of date

**Date:** 2026-07-31. Answers Codex's re-proposal of initial `AlterID` width **252**
(ruling 6 §"Consequence for calibration"; `UNIT_A_REPORT.md`).

## Rulings at a glance

1. **Width 252 is REJECTED as a production initial width.** It is **ACCEPTED** as a
   corpus-bound constant for the ignored reconciliation exit check only.
2. **A pre-flight segment budget is now required**, refused before endpoint contact.
   The scan loop is currently unbounded; that is a live-safety defect independent of width.
3. **The as-of date must become an explicit parameter.** As wired today the exit check
   cannot pass, for a reason unrelated to outstandings correctness.
4. **`TEST_CORPUS.md` is corrected**: `Bridge Billwise Lab`'s last voucher date is
   `20260702`, not `20260731`.

---

## Live measurement taken for this ruling

One request. Port 9000. `<TYPE>Company</TYPE>` with the exact `render_company_book_extent`
fetch shape (`Name, GUID, BooksFrom, LastVoucherDate, ALTVCHID`). Gateway health checked
before and after — both alive. `STATUS=1`, 0.06 s, 2,202 bytes. No loop, no retry.

| Company | GUID | `BOOKSFROM` | `LASTVOUCHERDATE` | `ALTVCHID` |
| --- | --- | --- | --- | ---: |
| Aarav Trading Company Demo | `bb8ad19e-…6215e` | 20240401 | **20260401** | 101,601 |
| Bridge Billwise Lab | `75f7566d-…3a57d` | 20240401 | **20260702** | 252 |

Both differ from the written record (`TEST_CORPUS.md` says Billwise spans "…`20260731`";
Aarav "to 2026-03"). The `ALTVCHID` values are unchanged, so neither corpus has drifted in
size — but the **reporting period the runtime will actually build is not the one the
documents assert**, and ruling 4's Education day rule explains why: voucher dates are
restricted to day 01/02/31, so July-2026's 18 vouchers sit on `0701`/`0702` and the book
genuinely ends on `20260702`. `20260731` was the intended fiscal boundary, never a voucher
date.

---

## 1. Why 252 is not a width

`Bridge Billwise Lab` has `ALTVCHID = 252`. The three calibration samples used the range
`0..252`. Therefore **width = high-water = whole book**, and
`SegmentTrendGuard::next_range` emits **exactly one range** — Codex's own test
`a_full_high_water_width_tiles_the_budget_axis_once` asserts precisely this.

**No segmentation occurred in any of the three samples.** They measure the repeatability of
an unsegmented whole-book read. Three repetitions of a read that never splits cannot
calibrate the mechanism whose only purpose is splitting. The measurement has no leverage on
the quantity being decided.

`TEST_CORPUS.md` states the corroborating fact directly: the whole Billwise Lab book reads
in **1.4 s / 3.25 MB in a single request**. A corpus that fits in one request cannot size a
segment.

Two properties of the code make this worse than an inert placeholder:

- **The initial width is a permanent ceiling.** `next_width` is only ever `min`-ed
  ([outstandings_runtime.rs:129](../../src-tauri/src/tally/outstandings_runtime.rs:129));
  it never grows, and the guard is constructed fresh per scan from `initial_width`. A width
  chosen on a book that never needed segmenting becomes the hard upper bound for every book
  that does.
- **The adaptive shrink often cannot engage.** It needs three *comparable* observations, and
  `segments_are_comparable` requires width, rows **and** encoded bytes each within 25%. Where
  row density varies between partitions the comparison rarely holds, so the initial width is
  frequently the **only** width. It is not a value runtime will correct.

The proposal is withdrawn for the second time, and this time the reason is not the emptiness
rule — it is that the accepted corpus is structurally incapable of producing the number.

## 2. What the width actually is

Let `D` = date partitions, `H` = `ALTVCHID`, `W` = width.

```
segment pairs = D × ceil(H / W)          live reads = 2 × that
total AlterID span scanned = D × H       — independent of W
```

Ruling 3 established that cost tracks the **span scanned, not rows returned**. So `W` does
not reduce total work at all; it only slices a fixed total into deadline-sized pieces.

> **The width is a deadline-fitting device, not a performance knob. It cannot make a scan
> fast; it can only make a scan possible.**

Applying this to the two corpora, using ruling 3's one-day measurements (`0..400` = 0.7 s,
`0..25,000` = 69.3 s, `0..50,000` = 41.9 s — non-monotonic, so order-of-magnitude only,
≈2 ms per ID scanned):

| Corpus | `D` | `H` | at `W`=252 | live reads | rough wall time |
| --- | ---: | ---: | ---: | ---: | --- |
| Bridge Billwise Lab | ~27 | 252 | 1 range/partition | **~54** | a few seconds |
| Aarav Trading Company Demo | ~24 | 101,601 | 404 ranges/partition | **~19,392** | **~4 hours** |

And no width rescues Aarav. The 20 s deadline caps `W` at roughly 10,000 (`0..25,000`
already costs 69.3 s), which still leaves ≥11 ranges × 24 partitions ≈ 528 reads and an
**~80-minute floor** set by `D × H` alone.

This is the durable finding, and it is larger than Unit A: **the `AlterID` axis cannot
rescue a book with no `AlterID`↔date locality.** For such books the fix is a different read
strategy, not a different width. That is explicitly out of Unit A's scope — but it must be
recorded, because the next person to see "width 252 is slow" will otherwise try to tune the
width, and no value works.

## 3. The missing segment budget — a live-safety defect

The scan loop at [runtime.rs:1160](../../src-tauri/src/tally/runtime.rs:1160) iterates
`next_range` until the high-water is covered, inside every date partition, with **no cap on
total requests**. The only stop conditions are the latency-trend guard (which fires only on
*rising* times near 15 s) and a typed failure.

A scan of ~19,392 uniformly fast reads trips neither. Under I7 and the standing "never loop"
rule, an unbounded segment loop **is** a loop — the same failure mode that previously
occupied the operator's machine for ~44 minutes, but by design rather than by accident.

`D`, `H` and `W` are all known **before the first segment request**, so the planned request
count is computable at zero live cost.

## 4. The as-of date is wired to the wrong source

`fetch_outstandings` computes with `extent.last_voucher_date()` as the as-of date
([runtime.rs:1215](../../src-tauri/src/tally/runtime.rs:1215)). On Billwise Lab that is now
measured to be **`20260702`**. The exit check asserts `report.as_of_yyyymmdd == "20260731"`
([unit_a_live.rs:12](../../src-tauri/tests/unit_a_live.rs:12)). These cannot both hold: the
check fails on as-of before it compares a single rupee, and the ageing assertion
(4/4/4/36, computed as of 31-Jul-2026) is 29 days adrift and would also fail.

Note which numbers this does and does not touch. **48 open bills and ₹45,14,597 are
as-of-independent** — a bill is open or settled regardless of when you ask. **Only the four
ageing buckets move.** So the reconciliation target is sound; the wiring is not.

Deriving as-of from `LastVoucherDate` is also wrong on the product: it means the newest bill
always ages 0 days, and a book quiet for three months reports its debt as three months
younger than it is. An accountant asking "what is overdue" means overdue **now**. The scan's
data ending earlier is a *freshness* disclosure — which the screen already owes — not an
ageing input.

---

## Required

1. **Do not encode 252, or any width, as `CalibratedSegmentPolicy` in production.**
   `fetch_outstandings` keeps returning `outstandings_segment_sizing_uncalibrated`.
2. **Admit a width to the ignored exit check only.** `outstandings_segment_policy` is
   hard-coded `None` at [runtime.rs:540](../../src-tauri/src/tally/runtime.rs:540) with no
   setter, and `unit_a_live.rs` is an integration test, so `#[cfg(test)]` items in the lib
   are not visible to it — the check cannot run today even with an approved width. Add an
   admission path (a non-default cargo feature such as `live-calibration-harness` is the
   obvious mechanism; pick what you can prove). The invariant to hold and to test: **no code
   path reachable from a default build can set a width.** Add a compile-fail or
   feature-off regression proving it.
3. **Implement a pre-flight segment budget.** Before the first segment request, compute
   `D × ceil(H / W)` and refuse above the ceiling with a typed `Partial` reason
   (`outstandings_segment_plan_exceeds_budget`) and **zero live requests spent**. Ceiling:
   **128 segment pairs (256 reads)** per scan.
   *Rationale, and it is a policy choice, not a measurement:* it admits Billwise Lab's ~27
   with ~5× headroom; at the calibration read cost (~0.12 s) the worst admitted scan is
   ~30 s, and at the slowest cheap-shape cost measured (0.7 s) ~3 minutes. Above that Bridge
   should say the book is too large for this read strategy rather than occupy Tally for
   hours. Revisit when a real (non-synthetic) book has been measured.
   **Log the computed `D`, `H`, `W` and planned pair count** — that turns my "~27" estimate
   into evidence.
4. **Make as-of an explicit input** to `compute_outstandings`, not a derivation from
   `LastVoucherDate`. Production passes the current date; the exit check passes `20260731`
   explicitly, which keeps it deterministic and reproducible. Keep the existing guard that
   as-of may not precede the scan window's end. Add a regression proving two different
   as-of dates against the identical scan produce identical totals/bill counts and
   *different* ageing buckets — that is the property that was silently untested.
5. **Correct the record.** `TEST_CORPUS.md` is updated by this ruling (below). Fold §1–§2
   of this document into `IMPLEMENTATION_GUIDE.md` §2.5b yourself — I deliberately left that
   file alone because you have uncommitted edits in it.

## Blocked until the above lands

- The live reconciliation run on port 9000, then 9001. It is authorised **in principle** at
  width 252 for `Bridge Billwise Lab` only, and only once (2)–(4) are in place. It is not
  authorised while the exit check would fail on as-of.
- Any production width. That needs a corpus whose `ALTVCHID` is large enough that a width
  actually splits it, with `AlterID`↔date locality — neither existing corpus qualifies.
  Do not build one as part of Unit A; record it as the blocker for the production path.

## Constraints unchanged

- Do not weaken the reconciliation exit check to make it pass. The as-of fix is a correction
  to the *runtime*, not a relaxation of the target; the money and bill-count assertions stay
  exactly as they are.
- Do not reinstate `AlterID` adjacency as an emptiness proof (ruling 6).
- Aarav remains prohibited for sizing calibration. §2's Aarav figures are **arithmetic from
  its already-measured extent**, not new timing calibration, and do not license a rerun.
- One live request at a time, health check between, never loop. No commit, push, PR or merge
  without explicit owner authorisation.
