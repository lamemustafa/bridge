# Owner ruling 9 — amending ruling 8, and what live writes proved

**Date:** 2026-08-01. Amends ruling 8 with the owner's approval, and records what a
write-and-read-back experiment on live Tally established that no amount of code review could.

---

## 1. Ruling 8 was premature. It is amended, not withdrawn.

Ruling 8 declared Unit A "complete for its scope" on the strength of a reconciliation that
matched 48 open bills / ₹45,14,597 / ageing 4/4/4/36 on two SKUs. That reconciliation still
stands and was not weakened. **What was wrong was the inference drawn from it.**

The corpus it ran against, `Bridge Billwise Lab`, opens every bill through a Sales `New Ref`
voucher. It contains **no optional vouchers** and — now measured — **no bill-wise ledger opening
balances at all** (0 of 13 bill-wise ledgers carry a non-zero opening). So the corpus *cannot
produce* the cases that were later found to be mishandled. A clean pass against it was evidence
that the read, pairing, tiling and arithmetic are right; it was **not** evidence that the
computation is right for books unlike it.

This is the same error as the withdrawn width 252: a clean measurement on a corpus incapable of
exhibiting the phenomenon in question. Recorded here because it is the second instance, and the
pattern — *"the corpus passed" is not "the property holds"* — is the durable lesson.

**Amendment.** Unit A is complete for its scope **on books whose bills all originate in
vouchers**. Where that does not hold, it must now say so rather than compute (§3).

## 2. What the live writes proved — the finding was real, and worse than described

Review claimed optional vouchers would inflate totals. Rather than accept or reject that, two
vouchers were written into the disposable `Aarav Trading Company Demo` corpus and read back
through the production fetch shape.

| Step | Result |
| --- | --- |
| Create Receipt, `ISOPTIONAL` absent | `CREATED=1 ERRORS=0 EXCEPTIONS=0` — accepted |
| Create identical Receipt, `ISOPTIONAL=Yes` | `CREATED=1 ERRORS=0 EXCEPTIONS=0` — accepted |
| Read back the date through the sealed FETCH list | 16 rows, **exactly one `ISOPTIONAL=Yes`** |

**Optional vouchers are returned by the Collection export.** They are not filtered by Tally, so
a voucher-only scan sees them. Tally's own help states an optional voucher "does not get
posted", so those allocations are not in the books.

Two details matter more than the headline:

- Tally **converted the second voucher's `New Ref` into `Agst Ref`** because the reference
  already existed. So the optional row arrived as a *settlement*.
- With the exclusion removed, the captured pair computes **19,998 against a posted 9,999** — the
  optional allocation is applied on top of the posted one and **doubles** the balance.

Depending on sign, an unexcluded optional row either doubles a balance or settles a bill the
books still show as open. The second is worse: Bridge would report a client's invoice as **paid
when Tally says it is outstanding**. Understating receivables is a harder error to catch than
inflating them, because nobody goes looking for money they have been told they already have.

The capture is retained as `unit_a_optional_voucher_live.xml` and drives a regression that fails
with 19,998 when the exclusion is removed — verified by removing it.

## 3. Ledger-opening bills — measured, then handled by refusing to answer

A bill-wise ledger with a non-zero **opening balance** carries bills that exist with no voucher
at all. A voucher-only scan cannot see them, and would silently under-report.

Measured on `Bridge Billwise Lab`: **0 of 13** bill-wise ledgers carry a non-zero opening. So
this defect does not touch the accepted reconciliation — the numbers are right, and right for
the right reason on that corpus.

**Ruling: detect, and refuse to complete. Do not implement reconciliation of opening bills in
Unit A.** One extra paired ledger read per scan now reports coverage; any bill-wise ledger with
a non-zero opening returns typed `Partial` `ledger_opening_bills_not_covered`. Full
opening-bill support is Unit B.

This is the right shape for Unit A specifically: its contract is *complete or explicitly
partial*, so "there are bills I cannot see" is an answer it is already built to give. Computing
anyway would produce a plausible, wrong receivables total with no error — the exact failure this
whole design exists to prevent.

## 4. A better approach was looked for, and rejected on evidence

Tally exposes a `Bills` collection that reportedly returns outstanding bills directly, which
would sidestep reconstructing them from vouchers — and would also see opening bills. It was
**not** probed, deliberately:

- `Bills` is not a verified collection type here, and §5.3a records that an unknown collection
  `TYPE` returns **no HTTP response at all** and raises a **modal dialog** that blocks the
  gateway until a human clicks it. That is the failure that once cost ~44 minutes.
- The native outstandings path is `<TYPE>Data</TYPE>`, which standing policy forbids extending.
- Even if it worked, §6.4 already proves Tally's own aggregates ignore the requested window, so
  a native read would need independent verification before it could be trusted.

It remains a live candidate, but it needs a deliberate isolated experiment with someone at the
Windows box to dismiss a dialog — not an unattended probe while a reconciliation run is pending.

## 5. Corpus mutation, recorded

`Aarav Trading Company Demo` was written to. It now carries one extra ledger
(`BRIDGE PROBE PARTY OPT`) and two extra vouchers at `AlterID` 101602/101603, dated 20260401.
Its `ALTVCHID` is therefore **101,603**, not the 101,601 previously recorded. Aarav is
documented as synthetic and disposable and is prohibited for sizing calibration, so this is
recorded rather than reversed — deletions are themselves an unproven path (§4.3).

**`Bridge Billwise Lab` was deliberately not touched**, so the agreed reconciliation baseline
remains intact for the exit check.

## Required

1. Re-run the ignored reconciliation exit check on port 9000, then 9001. The request template
   changed (`ISOPTIONAL` added), so the previous runs no longer evidence the current shape.
   Expect the same numbers — Billwise Lab has neither optional vouchers nor opening bills — but
   **expect is not evidence**.
2. Do not relax `ledger_opening_bills_not_covered` to make a book complete. If it fires, that
   book genuinely has bills Unit A cannot see.
3. Unit B inherits: opening-bill reconciliation, the `Bills` collection experiment, conditional
   AlterID subdivision, and licensed-mode coverage.
