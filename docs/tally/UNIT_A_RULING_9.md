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

## 3a. The two lab instances are NOT interchangeable — and the detector found it

Re-running the exit check under the current shape produced different answers per port, which
is how a genuine defect in the recorded assumptions surfaced.

| | port 9000 | port 9001 |
| --- | --- | --- |
| GUID | `75f7566d-…d57d` | **identical** |
| `BooksFrom` / `LastVoucherDate` | 20240401 / 20260702 | **identical** |
| `ALTVCHID` | 252 | **identical** |
| `ALTMSTID` | 218 | **219** |
| Bill-wise ledgers with non-zero opening | **0** | **10**, over ₹15 lakh |
| Exit check | Complete, matches target | **Partial `ledger_opening_bills_not_covered`** |

The corpus notes describe 9001 as a GUI backup/restore of the same company — "same GUID and
same AlterIDs" — and therefore a clean control. **That is wrong in a way that matters.** The
identity is identical and so is the *voucher* high-water; the divergence is entirely on the
**master** axis, where 9001 carries ledger opening balances 9000 does not.

Two consequences worth stating plainly:

1. **Voucher-axis identity is not book identity.** A sync tracking only `ALTVCHID` would see two
   identical books. `ALTMSTID` was the only visible signal, and one increment concealed
   ₹15 lakh of outstandings.
2. **Ruling 8's "both ports agree" was a false agreement.** The two runs matched because both
   computations ignored ledger openings — not because the books agree. The port-9000 number is
   sound; the port-9001 number was never meaningful, and nothing in the previous evidence could
   have revealed that. The detector revealed it on its first live run.

The exit check now asserts **per port**: 9000 completes at the accepted target, 9001 returns
`ledger_opening_bills_not_covered`. That is not a relaxation — demanding Complete on 9001 would
be demanding a wrong answer — and it adds the stronger property that the detector fires exactly
where opening bills exist and nowhere else.

## 3b. The coverage detector is cheap, and its limit is known

The detector reads ledger-level `OPENINGBALANCE`, so **offsetting opening bills are invisible
to it**: a bill-wise ledger holding a 100 debit and a 100 credit opening bill nets to zero and
is classified as fully covered, while both bills exist with no voucher. The scan can then
complete while omitting both receivable and payable exposure.

This is a limitation of the cheap detector, not a bug in it. Closing it needs evidence about the
opening *allocations* rather than the ledger balance — the same read that reconciling opening
bills would require, i.e. Unit B. Tracked in issue #108 and recorded in the code so the detector
is not mistaken for a complete one.

It does not affect the accepted reconciliation: `Bridge Billwise Lab` on port 9000 has **no**
bill-wise openings at all, offsetting or otherwise.

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

## 4a. Ageing ran from the wrong date — the third corpus blind spot

**VERIFIED 2026-08-01.** Tally emits `<BILLDATE>` on bill allocations — **52 of them in the
retained wildcard capture** — and the wire model silently discarded it. Ageing ran from
`voucher.date` instead. When a bill's own date differs from the voucher that carries it, every
ageing bucket and the oldest-bill age are wrong.

This is in the **original** Unit A computation, not in any review repair. It is now fixed:
`BILLDATE` is parsed and a bill is aged from it when Tally supplies one.

One distinction is load-bearing and is encoded in the code:

- **Opening a bill** (previous balance zero) ages from `BILLDATE` — Tally's authoritative date,
  which can precede the voucher carrying it.
- **A sign flip** (an over-settlement creating a fresh exposure in the opposite direction) ages
  from the **voucher** date, because on an `Agst Ref` Tally's `BILLDATE` is the date of the bill
  being *settled*, not of the settlement. Reusing it would age a brand-new exposure from the old
  bill.

The first attempt at this fix collapsed both cases into one branch and was silently inert: a
newly inserted bill has a zero balance, so the "previous balance is zero" branch fired on the
first allocation and overwrote the date. The regression that caught it asserts a bill whose
`BILLDATE` is 90 days before its voucher lands in the 61–90 bucket, not 0–30.

**Why this matters beyond the bug: it is the third time the accepted corpus could not
distinguish a correct implementation from an incorrect one.** After width 252 and the ledger
openings, `Bridge Billwise Lab` opens its bills through Sales vouchers whose `BILLDATE` equals
the voucher date — so the recorded ageing of 4/4/4/36 matched under both the wrong and the right
rule. The pattern is now established and should be treated as a standing risk, not an anecdote:
**a corpus that passes proves the corpus cannot produce the failure, not that the code handles
it.**

> **Re-run 2026-08-01, and the figures are CONFIRMED under the fix — unchanged.** Port 9000:
> 220 vouchers, ₹45,14,597, 48 open bills, ageing **4/4/4/36**, as-of `20260731`. Port 9001:
> Partial `ledger_opening_bills_not_covered`, as expected. Health verified on both gateways
> before and after.
>
> The numbers being identical is the point, not a relief: it means `Bridge Billwise Lab` opens
> every bill through a Sales voucher whose `BILLDATE` equals the voucher date, so the corpus
> produces the same answer under the wrong rule and the right one. **The previous figures were
> correct by coincidence of the corpus, not by correctness of the code.** A corpus that
> exercises a differing `BILLDATE` is required before ageing can be called verified — that gap
> now stands alongside the licensed-instance gap.

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
