# Owner ruling 6 — proving an empty date partition complete

**Date:** 2026-07-31. Answers the blocker raised after ruling 5: *how may Bridge prove a
genuinely empty date partition complete, when its verification requires an adjacent non-empty
`AlterID` range and none exists?*

---

## The conflict, restated

Ruling 5's calibration was withdrawn after Codex proved offline that **no `AlterID` width can
complete the accepted corpus** under the current empty-period rule:

- The corpus is quarterly-ish; most months are empty (e.g. the whole span between the `202404`
  and `202407` bands).
- The completeness proof corroborated each partition against an **adjacent `AlterID` range**.
- An empty partition has no adjacent non-empty `AlterID` range, so it can never be corroborated,
  so the scan is permanently `Partial`.

This is a design flaw in the **verification axis**, not a corpus defect. Empty months are normal
in every real book. The fix is to corroborate emptiness on the axis where a witness always
exists: **dates**.

## Measured facts this ruling rests on (live, port 9000, `Bridge Billwise Lab`)

| Read | Result |
| --- | --- |
| `20240501..20240531` (empty May) | `STATUS=1`, **0 rows**, 0.08 s |
| `20240401..20240501` (encloses April) | `STATUS=1`, 19 rows, dates only `0401`/`0402` |
| `20240401..20240701` (encloses Apr–Jun) | `STATUS=1`, 29 rows, dates `0401`/`0402`/`0701` — **nothing in May/Jun** |
| `20240501..20240701` (empty May+Jun) | `STATUS=1`, 10 rows, all dated `0701` |

An empty window returns a clean, fast `STATUS=1`/0-row response, and a **strictly wider date
window** returns dated rows that positively bracket the empty span. Date filters are cheap
(§2.3), so this corroboration is nearly free.

---

## Ruling

**Completeness is proven on the DATE axis for the whole scan, not per-partition on the
`AlterID` axis. `AlterID` ranges are an execution device for staying inside the request budget;
they are not the completeness witness.**

A scan of reporting period `[BooksFrom, LastVoucherDate]` is **Complete** when all hold:

1. **The date partitions tile `[BooksFrom, LastVoucherDate]` with no gap and no overlap.** This
   is the completeness backbone: every voucher's date falls in exactly one partition, so no
   voucher can be missed. `BooksFrom` and `LastVoucherDate` come from the extent probe (§4.1).
2. **The company GUID is confirmed** from the extent probe (I3), bound to the **port**, not
   assumed from the request (corpus §7.3).
3. **Every request is proven live** — connected, `STATUS=1`, distinguishable from `NoResponse`
   / `BadShape` (the harness already does this).
4. **Every non-empty partition passes I12** — returned date span ⊆ requested span.
5. **Within a partition, `AlterID` sub-ranges tile `[0, ALTVCHID]`** so no in-budget slice is
   skipped. This is a *budget* mechanism; it does not need a non-empty neighbour.

**An empty partition is admissible under (1)–(3) with no further proof of its own**, because the
date-axis tiling already guarantees no voucher was skipped and the live+GUID checks rule out the
false-empty routes of §2.8. It must **not** require an adjacent non-empty `AlterID` range.

### Optional hardening — the widening witness

Where stronger corroboration of a specific empty partition is wanted (e.g. a partition adjacent
to a deletion the sync must tombstone), read the **enclosing wider date window** and require:

- it returns `STATUS=1` with rows (proving the filter functions and the book has nearby data),
  and
- **none** of those rows are dated inside the empty sub-window.

This is I5-compliant ("absence corroborated by a strictly wider query"), on the cheap date axis,
and it works precisely when the `AlterID`-adjacent method fails. Prefer it over the withdrawn
mechanism; do not reinstate `AlterID`-adjacency as an emptiness proof.

### Degenerate cases

- **`ALTVCHID > 0` but the full `[BooksFrom, LastVoucherDate]` span returns 0 rows** → a
  contradiction (the extent says vouchers exist, the whole-book read says none). **Fail closed**
  — this is a false-empty, not an empty book.
- **`ALTVCHID = 0`** → the book is genuinely empty; an all-empty scan is Complete.

---

## Consequence for calibration (ruling 5's open width)

Width selection is **decoupled from emptiness** by this ruling: an empty partition no longer
forces `Partial`, so a width can be proposed again. Ruling 5's measured samples stand
(18 vouchers / 280,221 bytes / 106–135 ms per paired read). **The width ruling itself is still
pending and is now unblocked** — propose it from the calibration samples once (1)–(5) are
implemented, per ruling 3's requirement that the initial width needs a separate owner decision
before it is encoded.

---

## Required

1. Replace per-partition `AlterID`-adjacency emptiness proof with the date-axis completeness of
   (1)–(5). Keep the `Complete`/`Partial` typestates.
2. Implement the degenerate-case fail-closed on `ALTVCHID > 0` vs whole-book-empty.
3. Add tests: a scan with interior empty partitions reaches `Complete`; a forged whole-book-empty
   read against `ALTVCHID > 0` stays `Partial`/fails closed; the widening witness rejects a row
   dated inside the supposedly-empty window.
4. Do **not** weaken I5 or I12. The date tiling is an addition to them, not a replacement.

Offline only. No live reconciliation, no width encoding, no commit/push/PR until the width
ruling. Report back before any live run.
