# Bridge × Tally — implementation guide

**Purpose.** The build-oriented companion to
[TALLY_PROTOCOL_REFERENCE.md](./TALLY_PROTOCOL_REFERENCE.md). The reference records *what
Tally does*; this document records *what to build, in what order, and which trap each rule
exists to prevent*. Every rule below traces to a live observation — no rule is here on
principle alone.

**Reading order for an implementer:** §1 (invariants) → the part you are building → §8 (trap
index) before you write the tests.

**Provenance.** All observations are from TallyPrime **Edit Log 7.0, Educational mode**, one
company, no third-party TDL, ~101K vouchers. Nothing is established for licensed mode,
standard TallyPrime, Tally.ERP 9, or companies carrying custom TDL. §7 lists what must not be
built on.

---

## 1. Non-negotiable invariants

These are not style preferences. Each one prevents a failure observed on a live instance.

| # | Invariant | Prevents |
| --- | --- | --- |
| I1 | No `$$` function may reference an identifier containing spaces, **and no collection may reference itself in a `$$` function** | Terminates the Tally process / hangs the gateway |
| I2 | `SVCURRENTCOMPANY` present on **every** company-scoped request | Silently reading/writing the wrong company |
| I3 | Verify company identity from the **GUID in the response**, never from the request | A mistyped company reads as an empty company |
| I4 | A read is complete only when the **client** can prove it | Truncated responses carry `STATUS=1` |
| I5 | An empty result is never evidence of absence without corroboration | Three separate routes produce false-empty reads |
| I6 | Import success = intended counter incremented **AND** `ERRORS=0` **AND** `EXCEPTIONS=0` **AND** no `LINEERROR` | Three distinct silent-failure modes |
| I7 | Never issue a request you cannot afford to wait out | No way to cancel server-side work |
| I8 | Escape `&`, `<`, `>` in every emitted value | A stock Tally group name breaks the request |
| I9 | Parse tolerantly for invalid character references; validate strictly everywhere else | Tally emits XML that strict parsers reject |
| I10 | Batch size exactly 1 for production writes | Counters are unattributable at N>1 |
| I11 | Set `SVFROMDATE`/`SVTODATE` explicitly on **every** collection read | Omitting them silently collapses scope to the current display period |
| I12 | Compare the returned date span against the requested span on every read | Catches both rejected-boundary failure modes without knowing the rule |

**I11 in detail.** A collection with no date variables returns **only the current display
period**, not the whole book. This is not a filter — it is a silent scope collapse, and it
produced two false conclusions during this investigation before being identified. Verified
consequence: `$AlterID > 100000` returned **0 rows** without date variables and **1,587 rows**
with them, against identical data.

---

## 2. Reading

### 2.1 Use collection exports, not custom reports

`<TYPE>Collection</TYPE>`. It returns `HEADER/STATUS=1`, matches the existing collection
parsers, returns strictly more data, and needs roughly a tenth of the request size.

Custom `<TYPE>Data</TYPE>` reports are the legacy path and carry three defects: they can crash
Tally (I1), they render nothing without display geometry, and they emit **no `STATUS`** at
all. Do not extend them. See reference §2.

**Canonical request:** reference §3. `<ID>` must equal `COLLECTION NAME`.

### 2.2 Scope with `<FILTERS>`, never with date variables alone

`SVFROMDATE`/`SVTODATE` do **not** filter collection membership — they select which period
Tally loads, which is worse than being inert because it looks like it worked.

Use a `<SYSTEM TYPE="Formulae">` predicate plus `<FILTERS>`. Verified working for dates,
booleans, strings, numerics and compound expressions.

**Required test:** a negative bounding test that **fails when the `<FILTERS>` clause is
removed.** A bounding test that still passes without the filter is not a test.

### 2.3 Server-side filtering is broader than published guidance claims

Verified working: `$Date`, `$AlterID`, `$IsCancelled`, `$IsOptional`, `$VoucherTypeName`,
`$PartyLedgerName`, and compound `AND` expressions.

Published community guidance states Tally "does not respond to any filtering criteria other
than date duration" and recommends downloading the whole book and diffing client-side. **That
is wrong on this release.** Do not adopt the expensive workaround.

**Filter cost differs sharply by predicate type — an earlier revision of this section
flattened this and it misled an implementer.** Measured on the 101K book:

| Predicate | Rows returned | Elapsed |
| --- | --- | --- |
| `$Date` window, one month | 4,891 | **7.3 s** |
| `$AlterID > n` | 1,583 | 24.1 s |
| `$AlterID > n` (no matches) | 0 | 22.1 s |
| `$IsCancelled` | 1 | 22.6 s |

**Date predicates are cheap** — Tally can use the loaded period to narrow the work. **Non-date
predicates cost ~22 s regardless of how little they return**, because Tally evaluates them
across the whole collection.

So: a date-windowed read of a day or a month is fast even on a large book. An `AlterID` or
boolean scan is not, and must be gated behind the cheap company-level `ALTVCHID`/`ALTMSTID`
probe (§4.1).

### 2.3a `<COMPUTE>` inside a collection is evaluated per row — do not use it for constants

**OBSERVED 2026-07-30, cause not fully isolated.** A one-day date-windowed voucher read — which
should cost well under the 7.3 s measured for a whole month — exceeded a 20 s deadline. The
request carried:

```xml
<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>
<COMPUTE>BRIDGECOMPANYNAME:##SVCurrentCompany</COMPUTE>
```

A `<COMPUTE>` on a collection is evaluated **for every object in that collection**. The first
one resolves a *company lookup* per voucher. On a 101K-voucher collection that is ~101,000
company lookups to produce a value that is constant for the entire request.

> **Rule: never use `<COMPUTE>` to carry a per-request constant.** Company name, company GUID,
> schema identifiers and record counts are all obtainable from a separate, cheap request — or
> from the client that built the request in the first place. Bridge's existing
> `company_book_extent_v1` profile already returns the verified company GUID in milliseconds.

This is the same category of error as the self-referential `$$NumItems` in §5.3b: a TDL
construct that looks like free metadata and is actually per-row work. Both were inherited from
the original report profiles, which used exactly this pattern.

### 2.4 Curate the `FETCH` list — never use a wildcard

Measured **3,609 B/row curated versus 21,532 B/row wildcard — 6.0×**.

Dotted sub-paths resolve **one level deep** (`ALLLEDGERENTRIES.LEDGERNAME` works). **Two
levels do not** — GST rate data at `ALLLEDGERENTRIES.LIST > RATEDETAILS.LIST > GSTRATE` is
reachable only via the wildcard.

**Recommended handling:** most voucher types carry no GST lines. Apply the wildcard only to
Sales and Purchase types via a `$VoucherTypeName` filter, and use the curated fetch for
payments, receipts, contra and journals.

### 2.5 Segment by duration, not only by size

| Constraint | Value |
| --- | --- |
| Bytes per row (curated) | ~3,600, stable across scales |
| 32 MiB cap reached at | ~9,300 vouchers |
| **Recommended segment** | **~5,000 vouchers** |

The byte cap alone would allow ~9,300, but a whole-book read on 101K occupied Tally for ~19
minutes and blocked every other caller (§5.2). A one-month segment — 4,891 rows, 17.7 MB,
7.3 s — is the right order of magnitude. **Derive segment size from an observed sample read at
runtime**, not from a constant: inventory lines and long narrations run heavier.

Tally does **not** enforce a size cap. It streamed 282 MB without complaint. The 32 MiB limit
is Bridge's own rule; Tally will not refuse an oversized request on your behalf.

### 2.4a Curated `BILLALLOCATIONS` paths return WRONG values — **P0 for outstandings**

**VERIFIED 2026-07-30.** A/B on an identical window (`20250401`, 1,632 vouchers, 4,894
allocations in both cases):

| Fetch | Named bills | `BILLTYPE` distribution | Bytes |
| --- | --- | --- | --- |
| `BILLALLOCATIONS.NAME, .BILLTYPE, .AMOUNT` (curated) | **0** | `On Account: 1628` | 5.5 MB |
| `ALLLEDGERENTRIES.*` (wildcard) | **2** | **`New Ref: 1`, `Agst Ref: 1`**, `On Account: 1628` | 33.6 MB |

**The curated path does not merely omit the bill reference — it misreports the bill type.**
Allocations that are genuinely `New Ref` (creating a bill) and `Agst Ref` (settling one) are
returned as `On Account`. `<NAME/>` comes back empty in every case.

This is a **silently wrong answer**, not a missing field, and it is fatal specifically for
outstandings: `New Ref` and `Agst Ref` are what make a bill open or closed. A computation fed
the curated data would see every allocation as unattributed `On Account`, produce plausible
totals, and be wrong in a way no test against that same data could detect.

**Consequence:** bill-level outstandings requires `ALLLEDGERENTRIES.*`. §2.4's "never use a
wildcard" rule has one documented exception, and this is it.

**Cost.** The wildcard measured 6.4× the curated payload on this window. Note the density here
is artificial — a bulk-load placed 1,632 vouchers on a single date, whereas real books spread
across the calendar — so do not take "one day = 33.6 MB" as representative. What *is*
representative: **segment sizing for outstandings must be recomputed against wildcard payload,
not curated**, and the ~5,000-voucher default in §2.5 is derived from curated cost and is
therefore too large for this profile.

#### Resolved 2026-07-30: no cheaper shape is correct — the wildcard is mandatory

The "is there a correctly-resolving named path" question is now **answered: no.** Three fetch
shapes were A/B'd on window `20240401` (1,632 vouchers, 4,894 allocation blocks in all three):

| Fetch shape | Time | Bytes | `BILLTYPE` returned | Correct? |
| --- | --- | --- | --- | --- |
| `ALLLEDGERENTRIES.*` | **61.7 s** | **33.7 MB** | `On Account 1629`, `New Ref 1`, `Agst Ref 1` | **yes** |
| `…LIST.BILLALLOCATIONS.*` | 5.5 s | 5.39 MB | **empty on all 4,894** | no |
| `…LIST.BILLALLOCATIONS.LIST.*` | 2.0 s | 5.32 MB | **absent on all 4,894** | no |

The cheap shapes are 11–30× faster and 6× smaller, and both **silently destroy the field
outstandings depends on**. The mid shape also loses ₹163,382.80 of allocation value
(₹182,030,529.77 vs ₹182,193,912.57) while returning the full row count and no error.

**Per-row cost of the only correct shape: ~20,650 B/row** — matching the 21,532 B/row wildcard
figure in §2.6 and confirming that measurement independently.

#### The hard consequence: date segmentation has a floor that can be exceeded

At 61.7 s for 1,632 rows against a 20 s request deadline, the per-request budget is roughly
**530 vouchers**; against a 32 MiB cap it is roughly **1,600**. **Time binds first.**

But `SVFROMDATE`/`SVTODATE` cannot address anything finer than **one day**. If a single day
holds more vouchers than the time budget — as `20240401` does here at 1,632 — **no date-based
segmentation can succeed, at any segment size.** This is not a tuning problem; the minimum
addressable window exceeds the budget.

**Do not respond to this by raising the deadline.** A longer deadline walks into the 32 MiB
cap and I4's invisible truncation, which is a silently-wrong result rather than a loud one.

**The escape hatch is `AlterID` range segmentation**, which §10.1 already verifies works
server-side: `$AlterID > N AND $AlterID <= M` partitions a book at arbitrary granularity,
independent of the calendar, and has no floor.

#### 2.4b Measured 2026-07-31 — filter cost tracks the `AlterID` range *width*, not rows

**A first ruling recommended `AlterID` segmentation on its own. That was wrong, and these
numbers retract it.** All with the outstandings wildcard fetch:

| Date window | `AlterID` range | Time | Bytes | Rows |
| --- | --- | --- | --- | --- |
| Whole book | `0..400` | **31.5 s** | 2.79 MB | 147 |
| One day | `0..400` | **0.7 s** | 0.06 MB | 3 |
| One day | `0..25,000` | **69.3 s** | 8.20 MB | 397 |
| One day | `0..50,000` | **41.9 s** | 16.53 MB | 800 |

1. **The date window is what makes any filter cheap.** Identical `AlterID` range, whole-book
   dates vs one day: **31.5 s → 0.7 s**. This is the §2.3 result generalised — Tally narrows
   using the loaded period first, then evaluates predicates over what remains. **Never issue an
   `AlterID` filter without a narrow date window.**
2. **Cost scales with the `AlterID` span scanned, not the rows returned.** 397 rows cost 69.3 s
   because the scan covered 25,000 IDs. Elapsed is also non-monotonic (69.3 s then 41.9 s for a
   *wider* range), consistent with §2.5a degradation — **do not tune segment size from single
   samples.**

#### Why this corpus cannot size a segmentation policy — **do not tune against Aarav**

`ALTVCHID` is **101,601**, and day `20240401`'s 1,632 vouchers are spread across essentially
that whole span: `AlterID 0..400` on that day returns **3 rows**. Bulk XML generation inserted
vouchers in an order unrelated to their dates, so **`AlterID` and `$Date` are uncorrelated
here**.

In a real book they correlate strongly — vouchers are entered roughly in date order, so a date
window maps to a *compact* `AlterID` band and the two predicates reinforce each other. On
Aarav they fight: covering one day requires sweeping the entire ID space.

**Consequence: Aarav cannot produce a valid segment size, and measurements taken from it would
mis-tune the policy for every real book.** It remains valid for protocol, completeness and
failure-mode work. It is **invalid for performance tuning of segmented reads.**

**Test corpora must be generated in date order** so that `AlterID` locality matches reality.
`TEST_CORPUS.md` §4 now requires this.

### 2.5a Tally degrades over a long run — identical requests are not equally fast

**OBSERVED 2026-07-30.** During a 611-second segmented run, the window
`20251002..20251101` completed **within** the 20 s deadline on its first read and **exceeded
it** on an identical second read. Same request, same data, different outcome.

Earlier segments in the same run had completed normally. `tally-database-loader` independently
documents that Tally "fails to return updated/latest data" after prolonged running and
requires a restart.

**Design consequences:**

1. **A fixed segment size is not sufficient on its own.** Whatever size works at the start of a
   sync may exceed the deadline later in the same sync.
2. **Do not respond by retrying** (I7 — a timed-out request is still occupying Tally) **and do
   not respond by raising the deadline.** Both convert a slow sync into a blocked instance.
3. **Track elapsed time per segment and watch the trend.** If comparable segments are getting
   slower, the correct response is to stop cleanly, report the sync as `Partial` with a
   reason, and surface "Tally may need a restart" to the operator — not to push on.
4. **AlterID segment boundaries must never overlap or gap.** The verified filter is
   `$AlterID > A AND $AlterID <= B`, so the adjacent segment is `$AlterID > B AND $AlterID <= C`.
   The shared numeric boundary is included only by the first range and excluded by the second.
   Scan assembly must prove exact `0..ALTVCHID` coverage before producing `CompleteScan`.
5. **Every segment is two-dimensional.** Partition the reporting period into narrow date
   windows whose `SVFROMDATE` / `SVTODATE` remain valid `01` / `02` / `31` boundaries (§2.7),
   then apply an AlterID range inside each window. A whole-book date period with a narrow
   AlterID range is still expensive (§2.4b) and is not an admissible segment request.

### 2.5b Outstandings-only resource and empty-boundary policy

**OWNER-RULED 2026-07-30.** The wildcard outstandings profile has one narrow response-bound
exception because a live 1,632-voucher boundary produced approximately 33.6 MiB:

- the general XML response cap remains **32 MiB**;
- only the closed `VoucherOutstandingsV1` request type may use a **40 MiB** response cap;
- the runtime sizing target remains **28 MiB**, derived from multiple comparable completed
  date-plus-AlterID segments using the same wildcard request shape; there is **no production
  initial width** until the ordered bill-bearing corpus calibrates one, and later ranges may
  only shrink;
- the transport deadline remains **20 seconds**, with one attempt and no retry.

A paired, byte-stable zero-row response is only an `EmptySegmentCandidate`. It may receive
exactly one paired wider read consisting of that AlterID range plus the next adjacent AlterID
range. If the wider pair is identical, contains rows only in the adjacent range, and does not
contradict the candidate, promote both ranges and **reuse the adjacent rows**;
do not fetch that adjacent boundary again. Do not retry, widen recursively, or shrink around
the empty result. Any transport failure, pair mismatch, second empty result, target-date row,
scope ambiguity, non-single-boundary candidate, or missing in-range adjacent boundary returns
`Partial` and no totals are computed.

### 2.6 Completeness must be established by the client — I4

`STATUS` appears in the `HEADER` at the **start** of the document, before Tally knows whether
the response will finish. There is **no trailer, no row count, no completeness marker.** A
truncated read carries `STATUS=1` and parses cleanly.

Observed: a whole-book read returned **78,320 of 101,287 rows** looking entirely successful.

**Required:** establish completeness from something outside the response — an expected count
from a prior probe, byte-length agreement, or an explicit second read. Treat `STATUS=1` as
proof the request *started*, not that it *finished*.

### 2.7 Rejected date boundaries — both variables, different wrong answers

**Both `SVFROMDATE` and `SVTODATE` must have a day-of-month of 1, 2 or 31** (Education mode).
Any other day is silently ignored — but the *wrong answer you get* differs by variable:

| Rejected variable | What comes back |
| --- | --- |
| `SVTODATE` | **Widens to the whole book** — far too many rows |
| `SVFROMDATE` | **Collapses to the current display period only** — far too few rows |

Verified, `SVTODATE` held at a known-good `20250601`:

| `SVFROMDATE` | day | rows | span returned |
| --- | --- | --- | --- |
| `20250501` | 01 | 6,522 | `20250501..20250601` ✓ |
| `20250502` | 02 | 4,891 | `20250502..20250601` ✓ |
| **`20250515`** | 15 | **7** | **`20260401..20260401`** ✗ |
| **`20250530`** | 30 | **7** | **`20260401..20260401`** ✗ |
| `20250531` | 31 | 3,259 | `20250531..20250601` ✓ |

Both failure modes return `STATUS=1` with no error, so neither is detectable from the status
or from the row count alone — a plausible-looking count can come from entirely the wrong
period.

> **The one check that catches both: compare the returned date span against the requested
> span on every read.** If the span exceeds or falls outside the request, the period was not
> honoured. This is cheap, requires no knowledge of the underlying rule, and survives whatever
> licensed mode turns out to do differently.

### 2.8 The four routes to a wrong read — I5

Tally never reports "I could not do what you asked" on a read. It reports success with
different data. Four confirmed routes:

| Route | Presentation |
| --- | --- |
| Rejected `SVTODATE` (day ∉ {1,2,31}) | too many rows, `STATUS=1` |
| Rejected `SVFROMDATE` (day ∉ {1,2,31}) | too few rows, `STATUS=1` |
| Mistyped or non-existent company name | 0 rows, `STATUS=1` |
| Response truncated mid-stream | partial rows, `STATUS=1` |
| **Impossible calendar date that passes the day rule** (e.g. `20240631`) | **0 rows, `STATUS=1`** |

**The fifth route, added 2026-07-31.** `SVTODATE=20240631` — June has no 31st, but `31` passes
the day-1/2/31 boundary rule — returned **0 rows** on a window whose April portion holds 19
vouchers. It is a *distinct* failure from the §2.7 rejected-boundary widening: the day passes
the rule, the date does not exist, and the whole read collapses to zero rather than widening.
A naïve "month end = day 31" partitioner would emit `0631`, `0931`, `1131`, `0231` and read
four false-empty months a year.

> **Bridge is already immune by construction.** `TallyDate::parse` (`pack_models.rs`) rejects
> any impossible calendar date via `is_valid_yyyymmdd`, so Bridge code cannot emit `20240631` —
> the value has no representation. This is a concrete **P2 win**: the illegal state is
> unconstructable, so the false-empty route is closed before any request is built. The trap
> only bites hand-built probes (it bit this author's probe first).

**Consequence for change detection: a zero-row or short scan may never produce a deletion
tombstone without corroboration from a strictly wider query.**

---

## 3. Writing

### 3.1 Operation support matrix

| Object | Create | Alter | Cancel | Delete |
| --- | --- | --- | --- | --- |
| **Master** (Ledger) | ✓ | ✓ by name | n/a | ✓ by name |
| **Voucher** | ✓ | ✗ **creates a duplicate** | ✗ **creates a duplicate** | ✓ by `REMOTEID` |

Voucher Alter was tested with `MASTERID`, `GUID`, `REMOTEID` element, and `REMOTEID`+`MASTERID`
— all four produced duplicates. `REMOTEID` is a valid *match* key (Delete works with it), so
the failure is specific to Alter and Cancel.

**This inverts the plan's ruling that Cancel is the compensation primitive.** Cancel does not
work; Delete does. The "Alter-by-GUID with Cancel+Create fallback saga" has no working leg on
this SKU.

**Strong hypothesis, untested:** the Edit Log SKU refuses XML-driven voucher alteration by
design, since its purpose is an immutable audit trail and an XML import has no authenticated
user to attribute a change to. **Qualifying Alter/Cancel on licensed standard TallyPrime is a
Phase 4 gate.**

### 3.1a A voucher can only be altered inside the company's current period

**VERIFIED via the Tally UI, 2026-07-30.** Attempting to edit a voucher dated `1-Mar-26`
raised:

```
Date cannot be below the current period (1-Apr-26)
```

This is a **third, independent date restriction**, distinct from the Education entry rule
(day 1/2/31) and the boundary rule (§2.7):

| Restriction | Applies to |
| --- | --- |
| Day-of-month must be 1, 2 or 31 | Voucher *entry* date, Education mode |
| Boundary must be day 1, 2 or 31 | `SVFROMDATE` / `SVTODATE` on reads |
| **Voucher date must be within the current period** | **Voucher *alteration*, UI and probably XML** |

**Likely bearing on §3.1's Alter failure.** Every XML alter attempt targeted vouchers dated
`20260201` or earlier, all *below* the current period start of `1-Apr-26`. The alter failures
may therefore be a period violation rather than a SKU block — Tally's XML path simply does not
report the reason the UI does.

**Retest required before concluding Alter is unavailable:** attempt `ACTION="Alter"` on a
voucher dated **inside** the current period. If it succeeds, §3.1's matrix changes materially
and the "Edit Log blocks alteration" hypothesis is wrong.

### 3.2 Determining success — I6

There are **four** distinct import response shapes:

| Shape | Meaning |
| --- | --- |
| `<RESPONSE>` with counters | Normal — apply the four-part rule |
| `<RESPONSE>` with counters **and** `LINEERROR` | Rejection |
| `<ENVELOPE>` with `HEADER/STATUS` | Wrapped variant — `STATUS=0` fails regardless of counters |
| `<RESPONSE>Unknown Request, cannot be processed</RESPONSE>` | Malformed request — **no counters at all** |

**Success requires all four conditions.** Weaker checks have each been observed passing a
failure:

- `ERRORS=0` alone → a rejected voucher returned `ERRORS=0, EXCEPTIONS=1`
- `ERRORS=0 && EXCEPTIONS=0` → a failed Cancel returned `CREATED=1, CANCELLED=0`
- counters present → a malformed request returns none; defaulting them to zero reads as a
  benign no-op

`EXCEPTIONS` can be non-zero with **no** `LINEERROR`, so treat it as failure on its own.
`LINEERROR` text is **untrustworthy for cause attribution** — an out-of-range date produced
"Voucher date is missing" when the date was present.

### 3.3 Voucher numbering decides your idempotency story

| Numbering method | Your `<VOUCHERNUMBER>` | A failed Alter |
| --- | --- | --- |
| Automatic | **discarded** — Tally assigns its own | **silently duplicates** |
| **Manual + `PREVENTDUPLICATES=Yes`** | **preserved verbatim** | **cleanly rejected** |

Under automatic numbering a client-supplied key is thrown away while the create still reports
`CREATED=1, ERRORS=0`. Any dedupe built on it is silently ineffective.

> **Rule: any voucher type Bridge writes to should use Manual numbering with
> `PREVENTDUPLICATES=Yes`.** It converts a duplicated client voucher into a clean rejection.

Creating such a type over XML works — `<VOUCHERTYPE ACTION="Create">` with
`<NUMBERINGMETHOD>Manual</NUMBERINGMETHOD>` and `<PREVENTDUPLICATES>Yes</PREVENTDUPLICATES>`.

### 3.3a `REMOTEID` IS the idempotency key — **supersedes §3.4's conclusion**

**VERIFIED 2026-07-30.** A voucher imported with a **client-supplied** `REMOTEID`, then
re-imported byte-identically:

```
import #1  REMOTEID="BRIDGE-IDEMPOTENCY-TEST-001"  ->  CREATED=1  ALTERED=0
import #2  byte-identical                          ->  CREATED=0  ALTERED=1
vouchers in Tally afterwards                       ->  1
```

**Re-importing the same payload with the same `REMOTEID` upserts. It does not duplicate.**

This is the mechanism the UDF experiments (§3.4a) were looking for and missed. The earlier
"no natural idempotency" finding (§3.4) used vouchers with **no** client `REMOTEID`, where
Tally assigns its own — that was the uncontrolled variable.

**Four properties, all verified:**

| Property | Behaviour |
| --- | --- |
| `ACTION="Create"` + existing `REMOTEID` | **Upsert** — `ALTERED=1`, no duplicate |
| `ACTION="Alter"` + `REMOTEID` | **Creates a duplicate** — inverted from intuition; use `Create` |
| Client `REMOTEID` readable afterwards | **No.** Tally overwrites the attribute with its own value (`bb8ad19e-…-00018c44`) |
| Correction path | Re-import a corrected file with the same `REMOTEID`s and the earlier rows are **overwritten** |

**Consequences.**

*Positive:* this gives real idempotency and a real correction path without a TDL plugin, without
narration hacks, and without an outbox. For a generate-a-file-the-human-imports design it means
re-running the same file is safe, and fixing a mistake is a re-import.

*Negative:* because the client key is **not readable back**, you cannot audit which client
identifier produced which voucher, and you cannot verify from a read that your key was honoured.
Any proof-of-post claim must account for that — Tally's dedupe is trustworthy but opaque.

**Untested:** whether `REMOTEID` dedupe holds across company boundaries, across a Tally restart,
or when the payload differs from the original (partial update semantics). Also untested on
licensed or standard TallyPrime.

### 3.3b Master-name matching: case- and separator-insensitive, otherwise exact

**VERIFIED 2026-07-30**, against a ledger named `BRIDGE-PROBE-LEDGER-A` and one named
`ZZ Ram & Sons Pvt Ltd`:

| Supplied name | Result |
| --- | --- |
| exact | **matched** |
| lowercase | **matched** |
| trailing space | **matched** |
| `BRIDGE PROBE LEDGER A` (hyphens → spaces) | **matched** |
| `ZZ Ram AND Sons Pvt Ltd` (`AND` for `&`) | **rejected** |
| `ZZ Ram & Sons` (missing suffix word) | **rejected** |
| `ZZ Ram & Son Pvt Ltd` (singular for plural) | **rejected** |
| entirely different name | **rejected** |

So Tally normalises **case and separators** and is otherwise **exact on letters**.

**A missing ledger rejects the voucher and does NOT auto-create the master.** Verified: ledger
count unchanged at 87 across every test, with `LINEERROR: Ledger 'X' does not exist`.

**Implication for any narration→ledger matcher:** Tally will not help with abbreviation
(`TRAD` vs `TRADERS`), symbol expansion (`&` vs `AND`), pluralisation (`VENTURE` vs `VENTURES`)
or misspelling. Those must be solved before the file is generated. Rejection is a **safety
feature** — treat auto-creating unmatched masters as a decision requiring per-name human
approval, never a fallback.

### 3.3c Vouchers can be created with real bill references — verified shape

**VERIFIED 2026-07-30.** `BILLALLOCATIONS.LIST` nests **inside** the party's
`ALLLEDGERENTRIES.LIST`:

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

Returned `CREATED=1`, and readback confirmed `NAME=BRIDGE-INV-0001`, `BILLTYPE=New Ref`. Use
`New Ref` to open a bill and `Agst Ref` to settle one. Omitting `BILLALLOCATIONS.LIST` entirely
yields `On Account`, which carries no bill identity.

### 3.4 There is no natural voucher idempotency — **narrowed by §3.3a**

Re-sending an identical payload with the same voucher number created a **second voucher**.
Combined with §3.3, the fingerprint + embedded-key dedupe is the only thing standing between
a crash-retry and a duplicated client voucher.

### 3.4a Undefined UDF fields are silently discarded — **the plan's primary idempotency key does not work as written**

**VERIFIED.** Four voucher creates were sent carrying a `BridgeTxnID` in four different
forms — `UDF:` list form, `UDF:` scalar form, and a plain non-namespaced element, plus a
no-UDF control. All four returned `CREATED=1, ERRORS=0, EXCEPTIONS=0`.

Read back with a **wildcard** fetch (so nothing was name-gated), located by verified
`$AlterID` filter at AlterIDs 101584–101587:

| Voucher | Key value present? | UDF-ish tags present? |
| --- | --- | --- |
| control (no UDF) | — | none |
| `UDF:` list form | **no** | **none** |
| `UDF:` scalar form | **no** | **none** |
| plain element | **no** | **none** |

**Tally accepts an undefined UDF, reports complete success, and throws the value away.**

This is the same silent-key-discard failure as automatic voucher numbering (§3.3), and it
means **all three candidate mechanisms for attaching a client-generated idempotency key have
now failed silently:**

| Mechanism | Status |
| --- | --- |
| Voucher number | Discarded under automatic numbering (§3.3) |
| UDF field | **Discarded without a prior TDL definition** |
| Natural dedupe | Does not exist (§3.4) |

The plan specifies the key as "a UDF (BridgeTxnID, defined via inline TDL per request)". This
result shows the **TDL definition is mandatory, not conventional** — and that omitting it
fails invisibly rather than erroring.

**The only carrier proven to round-trip today is `NARRATION`** (§6.2 — byte-exact across all
Unicode and punctuation cases). The plan already names a narration-suffix fallback; on current
evidence that fallback is the *primary* option until a UDF-with-inline-TDL request is
demonstrated end to end.

**Caveat:** narration is user-editable, so it can never be trusted alone. The
`(date, amount, ledger-set, voucher-type)` fingerprint remains mandatory secondary dedupe
regardless of which carrier wins.

**Eight forms tested — all discarded.** The reserved-index hypothesis (UDF numbers 1–29 are
reserved for Default TDL) was tested and disproven:

| Form | Result |
| --- | --- |
| `UDF:` list, `INDEX="1"` (reserved range) | discarded |
| `UDF:` list, `INDEX="30"` | discarded |
| `UDF:` list, `INDEX="500"` | discarded |
| `UDF:` list, `INDEX="900"` | discarded |
| `UDF:` scalar form | discarded |
| plain non-namespaced element | discarded |
| **`INDEX="500"` + inline `<SYSTEM TYPE="UDF">` declaration in `REQUESTDESC`** | **discarded** |
| control (no UDF) | n/a |

Every one returned `CREATED=1, ERRORS=0, EXCEPTIONS=0`; all four index variants were located
by verified `$AlterID` filter (101588–101591) and read back with a wildcard fetch. No key
value and no UDF tag survived in any case.

**This is not proof of impossibility** — the inline TDL declaration may need different
placement (`BODY > DESC > TDL` rather than `REQUESTDESC > TDL`) or different syntax. But it is
a thorough negative across every documented shape found.

> **The strategic point the plan missed:** properly defining a UDF requires **deploying a TDL
> file into the client's Tally installation**. The plan's Killed list explicitly rules out
> "TDL plugin with in-Tally UI" and per-machine TDL installs. So the UDF-based idempotency key
> was never viable *within the plan's own constraints* — the mechanism depended on a capability
> the same document had already discarded.

**Therefore, for Phase 4 as currently scoped:** `NARRATION` is the only proven carrier for a
client-generated key, and the `(date, amount, ledger-set, voucher-type)` fingerprint is not a
secondary safeguard but a **co-primary** mechanism, because narration is user-editable and can
be destroyed between write and readback.

### 3.5 Identity after write

`LASTMID` is **0** on successful master creates — unusable. Read masters back by normalised
name. `LASTVCHID` is populated for vouchers and usable, subject to a foreign-writer
cross-check. It also accepts non-numeric text without error when parsed back, so validate it.

### 3.6 Master re-create is a silent Alter

Re-sending an identical ledger `ACTION="Create"` returned `CREATED=0, ALTERED=1` — the
existing master was **overwritten** with the retry payload. Pre-read before creating, and
persist `CREATED` and `ALTERED` as distinct outbox outcomes.

### 3.7 Company pinning is asymmetric — I2

| `SVCURRENTCOMPANY` | Read | Write |
| --- | --- | --- |
| Omitted | silently uses loaded company | **silently writes to loaded company** |
| Non-existent | 0 rows, `STATUS=1` | fails closed with a clear error |

Omission is the dangerous case for both. Matching is case-insensitive and whitespace-trimmed,
so name-based identity is looser than exact-match code assumes — hence I3.

### 3.8 Bulk import is for test data only

One request may carry many objects and counters aggregate, but **counters are unattributable
at N>1**: a 100-object request returning `CREATED=99` gives no way to identify the failure.
Production writes stay batch-of-one (I10).

Measured throughput, for onboarding estimates: ~21/s on a near-empty book falling to ~10/s at
~90K vouchers. **A 100K initial load takes ~2.5 hours.** First sync for a large client is an
overnight job and must be designed as resumable with visible progress.

---

## 4. Change detection

### 4.1 The cheap probe is mandatory, not an optimisation

Company-level `ALTVCHID` and `ALTMSTID` are monotonic high-water marks that move in step with
writes. Because any filtered scan costs ~22 s on a 101K book regardless of matches (§2.3),
this probe is the only way to avoid paying that to learn nothing changed.

### 4.2 Server-side `AlterID` filtering works

`$AlterID > checkpoint` returns only changed objects — verified semantically, not merely by
row count. This makes incremental sync cheap and contradicts the published workaround.

### 4.3 Deletions remain the gap

A deleted object has no `AlterID` to exceed the checkpoint. Deletion detection still requires
either a complete scan with absence reasoning — subject to I5 and §2.7 — or the Edit Log
audit containers (§7).

A **cancelled** voucher *is* detected: it appears as a new object with a higher `AlterID`.

---

## 4.4 The Edit Log report exists and captured our edit — but user attribution needs Tally security

**VERIFIED via the Tally UI, 2026-07-30.** The `Edit Log Summary - Vouchers` report
(`Display More Reports → Edit Log`) **did record the UI edit** the voucher collection's audit
containers showed as empty.

What the report shows for our edited voucher:

| Field | Value |
| --- | --- |
| Date / Particulars | `1-Apr-26` `BRIDGE-PROBE-LEDGER-A` |
| Vch Type / No. | `Journal` / `1` |
| **Versions** | **1** — Tally versions each voucher |
| Drill-down | **Version 2 · Activity `Altered` · `30-Jul-26 14:31`** |
| **Username** | **`Unknown (Security not enabled)`** |

Also visible: a `(Deleted)` row for a deleted voucher, and report filters for
`Version Period`, `All Versions`, `Voucher Type` and `Exception Reports`.

**Three consequences:**

1. **Change history exists and is richer than AlterID diffing** — it carries version numbers,
   activity type (Altered / Deleted), and timestamps, and it surfaces deletions directly rather
   than requiring absence reasoning. If readable over XML this is strictly better than §4.2/§4.3.
2. **User attribution requires Tally security to be enabled.** Without configured Tally users
   the log records `Unknown (Security not enabled)`, and no change-reason prompt appears on
   save. **This is a material constraint on the Drift Sentinel pitch** — "who changed it" is
   only answerable if the client has Tally users set up, and many small businesses run Tally
   with no security at all. The honest claim is *what* changed and *when*, with *who* available
   only where security is configured.
3. **The safe way to reach this data is the real report name**, not a guessed collection type
   (§5.3a). The report is titled `Edit Log Summary`. Any attempt should use `<TYPE>Data</TYPE>`
   with the actual report `<ID>`, one request at a time, with an operator watching the screen.

**Also confirmed from `F11` Company Features:** `Enable Bill-wise entry: Yes` — bill
allocations are active on the test company, which is what the outstandings view depends on.

**Also confirmed:** standard TallyPrime can open Edit Log data via a migration step, and the
migration dialog states *"The Edit Log feature will remain enabled for the Company."* Migration
offers a pre-migration backup — take it, since migration is not trivially reversible.

## 5. Operational safety

### 5.1 Two distinct causes of an unresponsive gateway

| Cause | Recovers without a human? |
| --- | --- |
| Long-running request still executing | **Yes** — measured at 523 s and 113 s |
| **Modal error dialog on the Tally desktop** | **No** — blocks until someone clicks OK |

**A liveness probe cannot distinguish them from the network side.** Both present as a dead
gateway. Any watchdog must escalate to the operator rather than wait indefinitely.

### 5.2 A client timeout does not cancel server-side work — I7

After a client abandoned a 600 s read, the gateway stayed unresponsive for a further **523 s**
before recovering by itself. Total occupancy ~19 minutes, nearly nine of them after the client
had disconnected. The gateway serialises, so one abandoned expensive read blocks everything.

**A user-facing "cancel sync" cannot stop Tally.** It can only stop consuming. The
accountant's Tally remains busy either way — and during that window their own Tally is
unusable to them.

**This is the failure that generates support tickets.** Segment sizing (§2.5) exists primarily
to bound it.

### 5.3 A malformed request can freeze Tally indefinitely

Failed requests can raise a modal dialog that blocks the gateway until dismissed. On an
unattended or hosted deployment there is nobody to click. Prefer request shapes that fail with
a `LINEERROR` in the response over shapes that fail inside Tally's UI.

Even *rejected* requests can occupy Tally well beyond the HTTP response.

---

### 5.3b A self-referential `$$NumItems` hangs the gateway — I1 restated

**OBSERVED 2026-07-30, cause not isolated.** A collection whose own `<COMPUTE>` counted itself:

```xml
<COLLECTION NAME="BridgeVoucherOutstandingsV1" ISMODIFY="No">
  <TYPE>Voucher</TYPE>
  <COMPUTE>BRIDGESOURCECOUNT:$$NumItems:BridgeVoucherOutstandingsV1</COMPUTE>
  ...
</COLLECTION>
```

produced `curl: (52) Empty reply from server` and left the gateway unresponsive. A minimal
`Company` collection sent immediately before had succeeded, so the instance was healthy.

**Note the signature is new:** *empty reply* means TCP connected and the server closed without
sending anything — distinct from the connection-refused and indefinite-hang signatures seen
elsewhere. Treat it as its own failure class.

**Why this matters more than the individual defect.** The identifier name here —
`BridgeVoucherOutstandingsV1` — contains **no spaces**. It fully complied with I1 as originally
written ("no `$$` function may reference an identifier containing spaces") and still took the
instance down. The original crashing profile used `$$NumItems:BRIDGE Voucher Collection V1`,
which had *both* faults; only the spaces were recorded, and the self-reference was missed.

**I1 is therefore restated:** no `$$` function may reference an identifier containing spaces,
**and no collection may reference itself inside a `$$` function.** A collection counting its own
membership is re-entrant.

**Not isolated:** the failing request also carried two other `<COMPUTE>` fields and two-level
`BILLALLOCATIONS` sub-paths. The self-reference is the prime suspect by direct analogy with the
original crash, not a proven cause. If the cause matters, isolate one `<COMPUTE>` at a time —
but the safer course is simply not to compute counts inside a collection at all: count rows
client-side after parsing, which is free and cannot hang anything.

**This is the strongest available argument for AGENTS.md P2.** A rule that can be fully complied
with while still causing the failure it exists to prevent is a badly-written rule. A type that
rejects a self-referential collection definition cannot be complied with incorrectly.

### 5.3a An unknown collection `TYPE` hangs the instance — **do not explore by guessing**

**VERIFIED 2026-07-30.** A collection request naming a non-existent object type
(`<TYPE>EditLog</TYPE>`) produced **no HTTP response at all** and raised a modal dialog:

```
Internal Error.  Contact Tally Solutions.
Incorrect Object Type!
```

Tally **knows** exactly what is wrong and says so on screen. It tells the gateway nothing —
the same asymmetry as the `$$` crash (§I1) and the current-period edit block (§3.1a).

**Consequence for method:** Tally's object model **cannot be discovered by trying type names.**
Each wrong guess costs a hung gateway and an operator interruption. Object and report names
must come from documentation or from Tally's own UI, never from experimentation.

**This rule was violated during the investigation** — six unknown types were sent in a loop
after the first had already hung for 300 s, taking the instance down. That is exactly the
"never auto-retry a request that caused a hang" rule in §5.4. Recorded because the failure was
procedural, not technical.

### 5.4 Can the dialog be handled, or Tally restarted, programmatically? — **recommendation: no**

Researched because §5.1 and §5.3 leave the gateway blocked on a human. Three options exist;
two should be rejected.

**Option A — dismiss the dialog via Windows UI automation. Reject.**
It requires driving another application's UI, which is far outside Bridge's loopback-only,
local-data architecture. Worse, **dismissing an unknown dialog means confirming an unknown
action** — Tally's dialogs include destructive confirmations, and the blocked-gateway state
gives no way to read which dialog is showing. Fragile across versions and locales, and a
large trust and permissions expansion for a product whose pitch is restraint.

**Option B — kill and restart `tally.exe`. Reject as a default.**
Tally may hold the accountant's unsaved work; killing it risks losing their data. It is the
user's primary application, and a sync tool terminating it is not a defensible behaviour for a
product selling trustworthiness. Note `tally-database-loader` documents needing periodic
restarts to clear staleness — but as a **human** action, not an automated one.

**Option C — avoid, detect, bound, escalate. Adopt.**

1. **Avoid.** Prefer request shapes that fail in-band with a `LINEERROR` over shapes that fail
   inside Tally's UI. §8's trap index identifies which shapes are dangerous — that list is the
   primary defence.
2. **Bound.** Segment sizing (§2.5) limits how long any single request can occupy the
   instance. This is the main reason to segment by duration rather than by bytes.
3. **Detect and escalate.** Surface a distinct state — *"Tally is not responding; check for a
   dialog on the Tally window"* — with a human remediation. Do not present it as a Bridge
   error; the fix is on the Tally desktop.
4. **Never auto-retry a request that previously caused a hang.** That converts one freeze into
   a loop.

**Relevant Tally facilities, for the parked hosted topology only.** TallyPrime supports
command-line parameters including `/NOGUI` (hides the interface), `/ACTION:<name>` (run an
action then exit), `/LOAD:<company>`, `/NOINILOAD` and `/NODEF`. These describe a **batch-job**
model — start, act, exit — not a long-running server.

> **Warning, untested:** `/NOGUI` hides the window. It is **not** established that it
> suppresses modal dialogs. A hidden GUI with a blocking modal would be *worse* than the
> current situation — invisible and unclickable. **Test this before any headless design
> depends on it.**

**TallyPrime Server** is a separate paid product that runs as a Windows service without a
logged-in user. Its documentation does not mention the XML gateway or port 9000, so it is not
established as a solution for headless XML integration. Worth investigating only if the hosted
topology is revived.

## 6. Parsing

### 6.1 Tally emits XML that strict parsers reject — I9

Responses contain `&#4;` — a reference to U+0004, not a legal XML character — sourced from
Tally's own metadata fields (e.g. `OBJECTUPDATEACTION` returns `&#4; Resave`). Tally attaches
such fields regardless of the requested `FETCH`, so an ordinary ledger read is affected.

Verified with a strict parser: `Currency` and `Ledger` collection responses **fail to parse**;
`Company` parses. UTF-8 decoding succeeds in all cases — **UTF-8 validity and XML validity are
different properties and only the former holds.**

Bridge's strict fail-closed parser must gain tolerant handling of invalid character references
before the read path works against any real company.

### 6.2 Unicode round-trips cleanly for data Bridge writes

Devanagari, Gujarati, Tamil, Bengali, `₹`, curly quotes, em-dash, ellipsis, accented Latin and
ampersand all round-tripped **byte-exact**. Encoding hardening is a smaller job than the plan
assumed.

**But pre-existing Tally-held data can be lossy** — the installer-set base currency symbol
exports as `?`. Fidelity guarantees apply to what Bridge writes, not to what Tally already
holds.

### 6.3 Unescape attribute values

`&` is returned XML-escaped inside attributes (`NAME="Ram &amp; Sons"`). Compare unescaped —
a naive string match reports a false mismatch.

### 6.4 Balances must be computed, not read

A `Ledger` collection's `ClosingBalance` is a **lifetime figure** that ignores the requested
window entirely — four windows, including one with no date variables at all, returned the same
value derived from transactions outside three of them.

Compute period balances locally from window-filtered vouchers. An empty `TYPE="Amount"` is not
zero; fail closed or quarantine.

---

## 7. Do not build on these — unverified

| Assumption | Status |
| --- | --- |
| Licensed Tally accepts arbitrary period boundaries | **Untested.** The day-1/2/31 rule (verified for BOTH `SVFROMDATE` and `SVTODATE`, §2.7) may be Education-only. Ruling 4 therefore makes it an explicit compatibility profile rather than a universal constructor rule. |

> **Resolved offline by owner ruling 4.** `DateBoundaryProfile::EducationRestricted` retains
> the verified `01`/`02`/`31` rule. Licensed, unknown, absent or inconsistent product-mode
> evidence selects `ModeAgnostic`, which accepts ordinary calendar boundaries and relies on
> **I12** to fail closed if returned vouchers fall outside the requested span. The 31-day
> `NarrowDateWindow` cap remains universal because it is a resource bound, not a compatibility
> claim. Licensed behavior itself remains untested; only Bridge's unsafe pre-contact rejection
> has been removed.
| Voucher Alter/Cancel work on standard TallyPrime | **Untested.** May be Edit Log SKU behaviour |
| `AUDITENTRIES.LIST` populates on a UI edit | **Untested.** Would replace AlterID diffing on Edit Log |
| A two-level `FETCH` syntax exists for GST rates | **Not found.** Wildcard is the only known route |
| Behaviour with third-party TDL installed | **Untested.** Client machines will have it |
| Behaviour on Tally.ERP 9 | **Untested** |
| Company creation over XML | **Partial** — `CURRENCYNAME`, `SVCURRENTPATH`, `STARTINGFROM`/`BOOKSFROM` all accepted; **only the currency formal-name element remains unknown** (ref §9.10a) |

---

## 8. Trap index

| Trap | Presents as | Rule |
| --- | --- | --- |
| `$$` function with a spaced identifier | Tally process terminates | I1 |
| `SVCURRENTCOMPANY` omitted | Wrong company, `CREATED=1` | I2 |
| Company name mistyped | 0 rows, `STATUS=1` | I3, I5 |
| Response truncated | Partial rows, `STATUS=1` | I4 |
| Rejected `SVTODATE` (day ≠ 1/2/31) | Too many rows, `STATUS=1` | I5, §2.7 |
| Rejected `SVFROMDATE` (day ≠ 1/2/31) | Too few rows, `STATUS=1` | I5, §2.7 |
| `ERRORS=0` on a rejected write | Looks posted | I6 |
| Failed Cancel/Alter on a voucher | `CREATED=1` — a duplicate | I6, §3.1 |
| Auto voucher numbering | Idempotency key silently discarded | §3.3 |
| Master re-create | Existing master silently overwritten | §3.6 |
| Malformed XML | Counter-less response | I6, I8 |
| Unescaped `&` in a stock group name | Whole request rejected | I8 |
| `&#4;` in Tally's output | Strict parser rejects a valid read | I9 |
| Abandoned request | Gateway blocked for minutes | I7 |
| Modal dialog | Gateway blocked until a human clicks | §5.1 |
| `ClosingBalance` read as a period figure | Wrong balance, presented as correct | §6.4 |
| `ACTION="Alter"` + `REMOTEID` | Creates a duplicate — use `Create` | §3.3a |
| Master name differing by more than case/separators | Voucher rejected, master NOT auto-created | §3.3b |
| Omitting `BILLALLOCATIONS.LIST` | Allocation becomes `On Account` with no bill identity | §3.3c |
| Self-referential `$$NumItems` in a collection | Gateway hangs, empty reply | §5.3b |
| `<COMPUTE>` used for a per-request constant | Per-row work; request exceeds deadline | §2.3a |
| `<COMPANY Action="Create">` without `SVCURRENTCOMPANY` | **Renames the loaded company**; `CREATED=0, ALTERED=1, ERRORS=0` | I2, §9.10d |
| Reading "the company" as row 1 of a `Company` collection | Returns the wrong company; `SVCURRENTCOMPANY` does not filter it | I3, corpus §7.1 |
| Expecting `ALTVCHID` from the single-object company export | Absent there; only the collection returns it | corpus §7.2 |
| Identifying the *instance* by company GUID | Backup/restore copies share a GUID across ports | corpus §7.3 |
| Validating corpus ordering by counting inversions | Condemns a good corpus; measure `AlterID` **locality** | corpus §4 |
| Narrowing the outstandings wildcard to cut cost | 11–30× faster, `BILLTYPE` silently empty | §2.4a |
| Date-segmenting a book with a dense single day | No segment size can fit; use `AlterID` ranges | §2.4a, §10.1 |
| `ORIGINALNAME` as a direct `<COMPANY>` child | Modal dialog; gateway blocked until a human clicks OK | §9.10b |
| Inferring "recovered on its own" from network polling | Attributes an operator's click to elapsed time | §9.10b |
| Company data path sought as a `COMPANY` field | Not a field; it is the `SVCURRENTPATH` static variable | §9.10a |

---

## 9. Changelog

| Date | Change |
| --- | --- |
| 2026-07-30 | Created from the 2026-07-29/30 live-probe programme against TallyPrime Edit Log 7.0 EDU. |
