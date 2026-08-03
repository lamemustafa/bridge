# Test corpus — state, known defects, and how to fix them

**Purpose.** What test data exists on the lab instances, what it is good for, what it is
**not** good for, and the exact steps to repair the gap. Written because the 101K-voucher
corpus generated on 2026-07-29 has a defect that silently invalidates any outstandings
testing, and that defect must not be rediscovered later.

---

## 0. The two corpora — which to use for what

**There are now two companies, with opposite strengths. Using the wrong one silently
invalidates results.**

| | `Aarav Trading Company Demo` | `Bridge Billwise Lab` |
| --- | --- | --- |
| GUID | `bb8ad19e-6aef-4239-a917-87fec0c6215e` | `75f7566d-7a4f-431a-9642-e93a9d06d57d` |
| Vouchers | ~101,150 | 220 |
| `ALTVCHID` / `ALTMSTID` | **101,603** / 327 | 252 / 218 |
| Bill references | **2 named in 4,894** — degenerate | **216 named of 440** — real |
| `AlterID` ↔ date locality | **none** — one day spans the whole ID range | **strong** — one month ≈ 18 consecutive IDs |
| Scale / payload / failure-mode work | **YES** | no — too small |
| Outstandings correctness | no | **UNQUALIFIED** — historical cross-check only |
| **Segment-size calibration** | **PROHIBITED** (§2.4b) | **UNQUALIFIED** — historical sizing sample only |

Both are loaded on port 9000. `Bridge Billwise Lab` also exists on port 9001 as a GUI
backup/restore of the same company — **same GUID, same AlterIDs** — see §7.

> **CORRECTED 2026-08-01 — the two instances are NOT interchangeable.** Measured: identical
> GUID, `BooksFrom`, `LastVoucherDate` and `ALTVCHID` (252), but **`ALTMSTID` 218 on 9000 vs
> 219 on 9001**, and port 9001 carries **10 bill-wise ledgers with non-zero opening balances
> (over ₹15 lakh)** that port 9000 does not. Those bills have no voucher, so a voucher-only
> scan cannot see them: 9000 reconciles Complete, 9001 correctly returns Partial
> `ledger_opening_bills_not_covered`. **Do not treat 9001 as a clean control for 9000**, and do
> not read matching totals across the two as agreement — see
> [UNIT_A_RULING_9.md](./UNIT_A_RULING_9.md) §3a.

---

## 1. What exists today

**Instance:** TallyPrime Edit Log 7.0 EDU, port 9000. Company `Aarav Trading Company Demo`.
A second instance (standard TallyPrime 7.1 EDU) runs on port 9001.

| Layer | Count | Origin |
| --- | --- | --- |
| Original demo vouchers | ~150 | Created by tooling before Bridge work began |
| Bulk-generated vouchers | 101,000 | Generated 2026-07-29 via XML import |
| Ledgers | 87 | 27 original + 50 generated + 9 Unicode probes + 2 probe ledgers |
| Groups | 28 | Original |
| Voucher types | 24 + 1 | Original, plus `BRIDGE MANUAL JOURNAL` (manual numbering) |

The generated ledgers carry realistic Indian trade names and valid-format GSTINs across six
state codes. Vouchers span every Education-legal date from `20240401` to `20260401`
(live `LASTVOUCHERDATE`, measured 2026-07-31 port 9000 — an earlier revision said "2026-03"),
mixing Sales,
Purchase, Payment and Receipt with 9%+9% CGST/SGST splits and invoice-referencing narrations.

> **Aarav was written to on 2026-08-01 (ruling 9).** It carries one extra ledger
> (`BRIDGE PROBE PARTY OPT`) and two extra vouchers at `AlterID` 101602/101603 dated
> `20260401`, created to measure whether Tally returns optional vouchers from a Collection
> export. It does — see [UNIT_A_RULING_9.md](./UNIT_A_RULING_9.md). `ALTVCHID` is therefore
> **101,603**. Aarav is synthetic and disposable and is prohibited for sizing calibration, so
> this is recorded rather than reversed. **`Bridge Billwise Lab` was deliberately not touched**
> so its reconciliation baseline stays intact.

---

## 2. What the corpus is good for

**VERIFIED** — it has already produced sound measurements for:

- Read performance and payload economics at scale (3,609 B/row curated, 21,532 B/row wildcard)
- The 32 MiB cap and truncation-invisibility finding
- Filter cost by predicate type
- Write throughput and its degradation curve (21/s → 10/s)
- Gateway blocking, serialisation, and recovery behaviour
- Segment sizing and completeness verification

For **protocol and scale** work it is a good corpus and should be kept.

---

## 3. The defect — no bill references

**The 101,000 generated vouchers contain no bill-wise allocations.**

The generator emitted plain `ALLLEDGERENTRIES.LIST` entries with no nested
`BILLALLOCATIONS.LIST`. Tally therefore records every allocation as `On Account` — an
unattributed amount against a party, with **no bill identity**.

Measured on window `20250401`, 1,632 vouchers, 4,894 allocations:

| | `New Ref` | `Agst Ref` | `On Account` | Named bills |
| --- | --- | --- | --- | --- |
| Whole window | 1 | 1 | 1,628 | **2** |

Two named bills in 4,894 allocations — and both come from the *original* demo data, not the
generated set.

**Why this matters.** `New Ref` opens a bill; `Agst Ref` settles one. Outstandings and ageing
are computed entirely from that pairing. A corpus of `On Account` entries cannot exercise the
logic, cannot validate ageing buckets, and cannot be hand-reconciled against Tally's native
outstandings report in any meaningful way.

**This was a generation error, not a Tally limitation.** The corpus is realistic in volume,
value distribution and naming, and degenerate in the one field the outstandings feature
depends on.

---

## 4. The fix — verified, no new company required

**VERIFIED 2026-07-30:** vouchers can be created with real bill references. The shape is
`BILLALLOCATIONS.LIST` nested **inside** the party's `ALLLEDGERENTRIES.LIST`:

```xml
<ALLLEDGERENTRIES.LIST>
  <LEDGERNAME>Bright Retail Pvt Ltd</LEDGERNAME>
  <ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>
  <AMOUNT>-1180.00</AMOUNT>
  <BILLALLOCATIONS.LIST>
    <NAME>BRIDGE-INV-0001</NAME>
    <BILLTYPE>New Ref</BILLTYPE>
    <AMOUNT>-1180.00</AMOUNT>
  </BILLALLOCATIONS.LIST>
</ALLLEDGERENTRIES.LIST>
```

Returned `CREATED=1`; readback confirmed `NAME=BRIDGE-INV-0001`, `BILLTYPE=New Ref`.

### Historical 2026-07-31 reconciliation — `Bridge Billwise Lab` is currently UNQUALIFIED

The company below was built through the Tally GUI on the Windows host and historically
reconciled. It is **not accepted for current corpus qualification**: the paired partition,
opening-coverage, and extent-bound proof requirements below were not retained. The recipe that
follows it is retained because it is the procedure to repeat for any future corpus.

| Property | Value |
| --- | --- |
| Name / GUID | `Bridge Billwise Lab` / `75f7566d-7a4f-431a-9642-e93a9d06d57d` |
| `ALTVCHID` / `ALTMSTID` | 252 / 218 |
| Books & FY from | `20240401` |
| Ledgers | 13 (10 sundry debtors, bill-by-bill enabled) |
| Vouchers | 220 — Sales 120 (`INV-0001`…`INV-0120`), Receipts 100 (4 `On Account`) |
| Allocations | 440 — `New Ref` 120, `Agst Ref` 96, `On Account` 4, **216 named** |
| Date span | `20240401`…**`20260702`**, **0 illegal entry dates** |
| Whole-book wildcard read | **1.4 s / 3.25 MB** |

**Reconciliation target — agreed by two independent methods:**

| | Value |
| --- | --- |
| Open bills | **48** |
| Total receivable | **₹45,14,597** |
| Ageing as of 31-Jul-2026 | 0–30: **4** · 31–60: **4** · 61–90: **4** · 90+: **36** |

Tally's own *Bills Receivable* export is the authority. The figures above were computed
**separately, from raw voucher XML**, and matched it exactly. Bridge must match both.

> **The as-of date is a choice, not a property of the book — ruling 7.** The live company
> extent returns `LASTVOUCHERDATE = 20260702` (measured 2026-07-31, port 9000). An earlier
> revision of this table said the span ended `20260731`; that was the intended fiscal
> boundary, never a voucher date. Education restricts voucher dates to day 01/02/31, so
> July-2026's 18 vouchers sit on `0701`/`0702`.
>
> The ageing row above is therefore **ageing as of 31-Jul-2026 against a book that ends
> 2-Jul-2026**. Bridge cannot derive `31-Jul` from the data and must be given it. Open bills
> (48) and total receivable (₹45,14,597) are as-of-independent; **only the four ageing
> buckets move with as-of.** A run that reports as-of `20260702` is not a compute defect —
> it is the wrong as-of source, and ruling 7 §4 requires as-of to be an explicit input.

#### Historical locality measurement — not an acceptance criterion

`AlterID` locality is what makes a corpus usable for segment-size calibration. Measured here:
each month occupies a **contiguous, non-overlapping band of ~18 IDs** (`202404` → 1..19,
`202407` → 20..37, … `202607` → 235..252); worst month spans **19.8%** of the ID range.

> **The check must measure locality, not inversions.** A first version of the acceptance script
> failed this corpus for having **1 inversion in 219 adjacent pairs** — a receipt dated 2-Jan
> entered just before a sale dated 1-Jan, inside the same month band. That is harmless. Had it
> been trusted, a perfectly good corpus would have been deleted and re-entered.
>
> Criterion: **worst month's `AlterID` span ≤ 40% of the book's ID range.** Aarav fails this by
> orders of magnitude — one *day* spans the whole range.

The corpus is currently **unqualified**. Do not treat a single voucher response,
a hand-maintained fixture label, or gateway reachability as acceptance evidence.
Qualification requires independently captured paired responses for every narrow
date partition, paired ledger-opening coverage, and an extent-bound full-book
scan. Until those captures are available, [`scripts/verify-tally-test-corpus.py`](../../scripts/verify-tally-test-corpus.py)
fails closed and cannot emit an acceptance token.

**Check locality during generation, not only at the end** — it is a non-accepting diagnostic
because it cannot be repaired afterwards, and catching it at 50 vouchers is far cheaper than at
500. Run it only against an already captured response; it does not contact Tally or qualify the
corpus. It repairs observed XML-1.0-illegal numeric references before strict parsing, but requires
the product's canonical eight-ASCII-digit voucher dates. It is **inconclusive** until at least two
month bands are present, because one month cannot measure date-to-AlterID locality:

```bash
python3 scripts/verify-tally-test-corpus.py --locality-xml /safe/local/vouchers.xml
```

> Capture remains an **operator-only live Tally activity**. It must never be imported or invoked
> from an automated test — no test in this repository contacts a live Tally, a government portal,
> or an external provider.

---

### The recipe (retained for future corpora)

**Do not regenerate inside `Aarav Trading Company Demo`.** The 101,000 existing `On Account`
allocations would remain and would dominate any outstandings total, making hand-reconciliation
against Tally's native report impractical. Keep Aarav for protocol and scale work.

Instead:

1. **Create a second company through the Tally UI** — company creation over XML is still
   unsolved (protocol reference §9.10a). The data path (`SVCURRENTPATH`) and financial year
   (`STARTINGFROM`/`BOOKSFROM`) are now solved, but the **currency formal-name element remains
   unknown after 19 candidates and 4 structural shapes**, and it is the one field no export
   reveals. Do not spend more time on it for this purpose — the UI takes two minutes.
   Enable **Bill-wise entry**.
2. **Generate 200–500 vouchers**, small enough to reconcile by hand:
   - Sales with `New Ref` opening a bill per invoice
   - Receipts with `Agst Ref` settling some of them, fully and partially
   - A deliberate spread of bill dates across the ageing buckets (0–30 / 31–60 / 61–90 / 90+)
   - Some parties left fully open, some fully settled, some partially
   - A few `On Account` receipts, because real books contain them
3. **Use a voucher type with Manual numbering** and `PREVENTDUPLICATES=Yes` (guide §3.3) so
   supplied voucher numbers survive.
4. **Supply a client `REMOTEID` on every voucher** (guide §3.3a) so the generator is re-runnable
   without duplicating.
4b. **Generate strictly in ascending date order.** Guide §2.4b: Aarav's bulk load inserted
   vouchers in an order unrelated to their dates, so `AlterID` and `$Date` are uncorrelated and
   **no valid segment size can be derived from it**. Real books are entered roughly in date
   order. Emitting oldest-first is the only way this corpus can tune segmented reads.
5. **Open Tally's own outstandings report** for that company and save it as the expected result.
   That becomes the reconciliation target no automated test can substitute for.

### Constraints to respect while generating

- Voucher dates must be day **1, 2 or 31** (Education mode).
- Both `SVFROMDATE` and `SVTODATE` on any read must also be day 1, 2 or 31 (guide §2.7).
- Escape `&`, `<`, `>` in every emitted value — party names like `Ram & Sons` are common
  (guide I8, and this rule was violated during testing by the author of this document).
- Batch imports of 250 objects work and run ~10–21 vouchers/sec depending on book size. That is
  fine for test-data generation; production writes remain batch-of-one.

---

## 5. Other corpus caveats

- **Probe residue.** Aarav contains roughly a dozen `BRIDGE-PROBE-*` vouchers, two probe
  ledgers, several `UDFTEST`/`UDFIDX`/`ledgermatch`/`billtest` vouchers, one cancelled voucher,
  and nine Unicode-named ledgers (Devanagari, Gujarati, Tamil, Bengali, `₹`, curly quotes,
  em-dash, accented Latin, ampersand). All are harmless but will appear in reads.
- **One voucher was edited through the Tally UI** on 2026-07-30 (narration
  `EDITIED BY OPERATOR TEST`) to test the Edit Log. It is recorded in Tally's Edit Log as
  Version 2, Altered, with username `Unknown (Security not enabled)`.
- **Tally security is not enabled**, so no Edit Log entry carries a real username. If user
  attribution ever needs testing, Tally users must be configured first (guide §4.4).
- **Neither instance is licensed.** Nothing measured here can be marked `Verified` in the
  compatibility matrix, and Education-mode restrictions may not apply to licensed Tally.

---

## 7. Traps discovered while validating the new corpus

### 7.1 The `Company` collection ignores `SVCURRENTCOMPANY` — **TRAP**

**VERIFIED 2026-07-31.** A `Company` collection read with
`SVCURRENTCOMPANY = Bridge Billwise Lab` returned **`Aarav Trading Company Demo`** as its first
row. The collection enumerates **every loaded company**; `SVCURRENTCOMPANY` sets evaluation
context, it does **not** filter the collection.

Any code that reads "the company" as the first row of a `Company` collection **will silently
bind to whichever company Tally happens to list first** — and with two companies loaded, that is
not the one requested. This is I3 in a new disguise: identity must come from matching the
**GUID in the response**, never from position or from what the request asked for.

### 7.2 `ALTVCHID` is missing from the single-object company export

The `TYPE=Object / SUBTYPE=Company` export of §9.11a — 565 tags, the recommended
company-identity probe — **does not carry `ALTVCHID` or `ALTMSTID`**. Those come back only from
a `Company` **collection** read.

So the two company reads are complementary, not interchangeable: the object export gives GUID
and configuration; the collection gives the change-detection high-water marks. Change detection
needs the collection read, subject to §7.1.

### 7.3 Backup/restore duplication preserves the GUID across instances — **TRAP**

`Bridge Billwise Lab` was placed on port 9001 by Tally's GUI backup/restore of the 9000 copy.
The restored company keeps the **same GUID and the same `AlterID` values**.

**Consequence: company identity cannot tell you which instance you reached.** A GUID check
confirms the right *book*, not the right *server*. Any test asserting SKU-specific behaviour
must bind its conclusion to the **port**, not to the company.

**This is also a benefit, deliberately.** Identical data on both SKUs makes port 9001 a clean
control: any behavioural difference between the two is attributable to the SKU rather than to
the data. That is the right setup for closing the open Alter/Cancel question (reference
§9.5/§9.6).

---

## 6. Changelog

| Date | Change |
| --- | --- |
| 2026-07-30 | Created. Records the missing-bill-reference defect and the verified fix. |
| 2026-07-31 | `Bridge Billwise Lab` created and historically reconciled: GUID, extents, reconciliation target, and locality measurement. It is now explicitly unqualified pending paired partitions, opening coverage, and extent-bound proof. Added §0 (which corpus for what) and §7 (three traps found while validating). Recorded that the first acceptance script used the wrong criterion and would have condemned a good corpus. |
