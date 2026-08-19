# Tally XML gateway — protocol reference

**Purpose.** The single source of truth for how Tally's XML gateway actually behaves, as
observed against a live instance. Everything here is either **VERIFIED** against a real
Tally or explicitly marked otherwise. Plan documents state intent; this document states
observed behaviour. Where they disagree, this document wins — or one of them is stale and
should be fixed.

**How to use.** Read the confidence marker before relying on any statement. Add findings
with their evidence and date. Never add an unverified claim without marking it.

| Marker | Meaning |
| --- | --- |
| **VERIFIED** | Observed directly against a live Tally. Request and response captured. |
| **PARTIAL** | Observed, but the rule behind it is not fully established. |
| **UNVERIFIED** | Believed, assumed, or documented elsewhere — never tested here. |

---

## 0. Observation environment

The primary baseline for VERIFIED entries was established on **2026-07-29**
against the environment below. Later dated entries state their own synthetic
environment and evidence boundary.

| | |
| --- | --- |
| Product | TallyPrime **Edit Log** (EL) — a separate SKU from TallyPrime |
| Release | 7.0 |
| Licence mode | **Educational** |
| Host | Windows 10, x86_64 |
| Gateway | port 9000, "acts as Both", ODBC enabled |
| TDLs configured | **None** — clean baseline, no third-party TDL |
| Company | `Aarav Trading Company Demo` — synthetic, ~150 vouchers over two FYs |
| Reached from | macOS dev machine via SSH loopback forward to `127.0.0.1:9000` |

**Nothing here is established for licensed mode, for standard TallyPrime, for Tally.ERP 9,
or for a company carrying custom TDL.** Several findings below are known or suspected to be
Education-mode-specific and are marked as such. Raw captures live in `.bridge-live/`
(gitignored).

---

## 1. Transport

**VERIFIED**

- `GET /status` → `<RESPONSE>TallyPrime Server is Running</RESPONSE>`. Use as a liveness
  probe; it is cheap (milliseconds) and safe.
- Requests are `POST` to `/`. The request charset controls the response charset;
  Bridge's read path uses `Content-Type: text/xml; charset=utf-16` and a
  BOM-prefixed UTF-16LE body. See §1.2.
- **The gateway serialises requests.** A single long-running request blocks every other
  caller, including `/status`. An unresponsive gateway does not imply a hung or crashed
  Tally — it may simply be busy behind another request.
- A request that terminates the Tally process produces no HTTP response at all: the client
  sees a connect-level failure or an indefinite hang, not an error document.

### 1.1 Encoding — **Tally emits XML that strict parsers reject. This is a P0 for the read path.**

**VERIFIED 2026-07-30.** Two separate questions were conflated in an earlier revision of this
section; they have different answers.

#### (a) Tally's output is NOT valid XML — confirmed

Responses contain **`&#4;`**, a character reference to U+0004, which is **not a legal XML
character**. Validated with a strict parser (Python `ElementTree`):

| Response | Strict XML parse |
| --- | --- |
| `Currency` collection | **FAILS** — "reference to invalid character number", line 59 |
| `Ledger` collection (`FETCH Name,Parent`) | **FAILS** — same, line 458 |
| `Company` collection | parses |

The source is Tally's own metadata — e.g. `OBJECTUPDATEACTION` returns the literal value
`&#4; Resave`. Tally attaches such fields regardless of the requested `FETCH` list, so an
ordinary ledger read is affected. **UTF-8 decoding succeeds** in all cases; UTF-8 validity
and XML validity are different properties and only the former holds.

> **Consequence: Bridge's strict, fail-closed XML parser will reject ordinary Tally
> responses.** This is not hypothetical — it is reproduced on a minimal `Ledger` read. The
> read path requires tolerant handling of invalid character references before it can work
> against any real company. This must be a Phase 2 task.

#### (b) Data that Bridge writes round-trips byte-exact — also confirmed

Nine ledgers created over XML with hostile names were read back **byte-identical**:

| Case | Result | | Case | Result |
| --- | --- | --- | --- | --- |
| Devanagari `श्री गणेश ट्रेडर्स` | exact | | Rupee sign `₹` | exact |
| Gujarati `શ્રી કૃષ્ણ એન્ટરપ્રાઇઝ` | exact | | Curly quotes `“ ”` | exact |
| Tamil `முருகன் டிரேடர்ஸ்` | exact | | Em-dash / ellipsis | exact |
| Bengali `রায় এন্ড সন্স` | exact | | Accented `Café Naïve` | exact |
| Ampersand `Ram & Sons` | exact (returned `&amp;`) | | | |

#### (c) But pre-existing Tally-held data can be lossy

The company's base currency symbol — set through the installer/UI, not by us — exports as
`ORIGINALNAME = '?'`. The `₹` is gone. So the third-party report that "`₹` is lost on the
wire" **does reproduce**, but only for values Tally already held, not for values Bridge
writes.

**The distinction that matters:** *round-trip fidelity for Bridge-written data is excellent;
fidelity for pre-existing Tally-held data is not guaranteed, and Tally's serialiser emits
invalid XML regardless.*

#### Method note — how the earlier wrong conclusion happened

An earlier revision recorded "clean; the claim does not reproduce". That test checked
**UTF-8 decodability** and searched ledger *name* fields only. It never validated the
document as XML, and never looked at metadata fields. Both failures were present in the very
responses it examined. **Testing the property you control is not the same as testing the
property that matters.**

**One real parser requirement:** `&` is returned XML-escaped inside attribute values
(`NAME="ZZ Ram &amp; Sons Pvt Ltd"`). Attribute values must be unescaped before comparison.

### 1.2 Request charset controls response charset

**VERIFIED 2026-08-19.** Matched requests against a synthetic validation book
established that Tally mirrors the XML request charset. An ASCII/UTF-8 request
returned literal `?` substitutions for a Devanagari ledger name; a UTF-16LE
request with `Content-Type: text/xml; charset=utf-16` returned the exact name.
The UTF-16 request body carried an `FF FE` BOM. The response declared
`text/xml; charset=utf-16` but carried no BOM (first bytes `3c 00 45 00`).

The byte cost was exactly 2x for matched responses: 7,114 → 14,228 bytes for a
ledger collection and 1,170 → 2,340 bytes for Bills Receivable. Decoded
character counts and offsets were identical; the bills pair differed at 16
positions, each `?` in the UTF-8 response versus the intended Devanagari
character in the UTF-16 response.

`GET /status` also mirrored only the request `Content-Type` header despite
having no request body (51 → 102 response bytes). A UTF-16 body without the
UTF-16 charset header returned `Unknown Request, cannot be processed`.

**Consequence.** Every XML POST read declares UTF-16. The bodyless, fixed-ASCII
`/status` liveness probe deliberately remains plain `text/xml` and expects the
measured UTF-8 response, avoiding a false outage on builds that do not mirror a
charset header on a bodyless GET. Response decoding requires both the caller's
expected encoding and the response charset; either a contradictory declaration
or a contradictory BOM fails closed. BOM-less UTF-16LE is an explicit observed
encoding, not an inference from NUL placement. Tolerant repair of illegal
numeric character references remains required after decoding (§1.1).

## 2. Two request families — and why it matters

Tally accepts two shapes for reading. They behave very differently.

### 2.1 Collection export — `<TYPE>Collection</TYPE>` — **RECOMMENDED**

**VERIFIED.** Returns a wrapped, status-bearing response:

```
ENVELOPE
 └ HEADER (VERSION, STATUS)
 └ BODY
    └ DESC → CMPINFO
    └ DATA → COLLECTION → <OBJECT> …
```

`STATUS=1` indicates application-level success. This is the only read family that reports
`STATUS`, so it is the only one compatible with a "HTTP 200 is never success" rule.

### 2.2 Report export — `<TYPE>Data</TYPE>` with a custom `REPORT`/`FORM`/`PART` — **AVOID**

> **Qualified by §12a.1.** This section is about a **custom** report definition. Tally's own
> **built-in reports addressed by name** — e.g. `<ID>Bills Receivable</ID>` — behave quite
> differently and, on the observed TallyPrime Edit Log 7.0 EDU profile with no third-party TDL,
> were usable and cheap when subjected to §12a.1's response validation and identity brackets.
> Other releases and configurations remain unverified. Do not read "avoid `<TYPE>Data</TYPE>"
> as a blanket rule.

**VERIFIED.** Returns a bare envelope with **no `HEADER` and no `STATUS`**:

```
ENVELOPE
 └ <tag derived from the LINE's XMLTAG>
    └ <tags derived from FIELD NAMEs, uppercased, spaces stripped>
```

Three problems, all observed:

1. **It can crash Tally** — see §6.1.
2. **It renders nothing without display geometry** — see §6.2.
3. **It carries no `STATUS`**, so a status-enforcing parser must special-case it.

A response shaped `ENVELOPE → COMPANYINFO → COMPANYNAMEFIELD` is not a Tally quirk; it is
simply what a custom report emits when its fields are named `Company Name Field` etc.

---

## 3. The working request template

**VERIFIED** — this exact structure returns correct, bounded data.

```xml
<ENVELOPE>
 <HEADER>
  <VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>
  <TYPE>Collection</TYPE><ID>BridgeVouchers</ID>
 </HEADER>
 <BODY><DESC>
  <STATICVARIABLES>
   <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
   <SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY>
   <SVFROMDATE TYPE="Date">{yyyymmdd}</SVFROMDATE>
   <SVTODATE TYPE="Date">{yyyymmdd}</SVTODATE>
  </STATICVARIABLES>
  <TDL><TDLMESSAGE>
   <SYSTEM TYPE="Formulae" NAME="BridgeWindow">$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate</SYSTEM>
   <COLLECTION NAME="BridgeVouchers" ISMODIFY="No">
    <TYPE>Voucher</TYPE>
    <FETCH>{curated field list}</FETCH>
    <FILTERS>BridgeWindow</FILTERS>
   </COLLECTION>
  </TDLMESSAGE></TDL>
 </DESC></BODY>
</ENVELOPE>
```

`<ID>` and `COLLECTION NAME` must match. `<` must be escaped as `&lt;` inside the formula.

---

## 4. Object types

**VERIFIED** — all readable via collection export, all returning `STATUS=1`, all sub-40 ms
on the demo company.

| `<TYPE>` | Rows on demo company | Notes |
| --- | --- | --- |
| `Company` | 1 | Also exposes `STARTINGFROM`, `ENDINGAT`, `BOOKSFROM`, `LASTVOUCHERDATE`, `ALTVCHID`, `ALTMSTID` |
| `Group` | 28 | |
| `Ledger` | 27 | See §7 before trusting any balance field |
| `VoucherType` | 24 | |
| `Voucher` | 150 (whole book) | See §5 for date scoping |

**UNVERIFIED:** stock items, godowns, cost centres, currencies, units, budgets — never probed.

---

## 5. Date and period semantics — the most dangerous area

### 5.1 `SVFROMDATE` / `SVTODATE` do not filter collection membership

**VERIFIED.** They select which *period* Tally loads, not which rows match. A collection with
no date variables returns only the current display period. Two different narrow windows can
return byte-identical result sets.

### 5.2 A `<FILTERS>` predicate does bound correctly

**VERIFIED.** With `<SYSTEM TYPE="Formulae">` comparing `$Date` against `##SVFromDate` /
`##SVToDate`, the response contains only in-window rows. Measured: unfiltered returned 2
vouchers; the same collection with a wide filter returned 150 across two financial years;
a two-day filter returned exactly 6.

### 5.3 Rejected period boundaries widen silently — **THE KEY TRAP**

**VERIFIED, Education mode.** `SVTODATE` is honoured only when its day-of-month is **1, 2 or
31**. Any other day is **silently ignored** and the period expands to the entire book.

Twenty-three data points, no exceptions:

| Day-of-month | Behaviour |
| --- | --- |
| 1, 2, 31 | honoured — data bounded to request |
| 15, 28, 29, 30 | rejected — period widens to whole book |

This mirrors the Education-mode restriction on voucher entry dates. `20250430` fails while
`20250531` succeeds because April has no 31st.

**The failure is invisible from the response.** When the period is silently widened:

- **without** a `<FILTERS>` clause you receive far **too many** rows;
- **with** a `<FILTERS>` clause you receive **zero** rows, because `##SVToDate` does not
  resolve and the predicate excludes everything.

Both return `STATUS=1`. Neither reports an error. A zero-row response is indistinguishable
from a genuinely empty period without corroboration.

**Required discipline, regardless of licence mode:**

1. Compare the returned date span against the requested span on every read. If the span
   exceeds the request, the period was not honoured.
2. Never treat a zero-row window as empty without re-querying a strictly wider window.
3. Never emit a deletion tombstone from an uncorroborated empty result.

**UNVERIFIED:** whether licensed Tally accepts arbitrary boundary dates. Probably, but nobody
has tested it. Do not build on the assumption.

**PARTIAL:** one observation does not fit the model — a request with `SVFROMDATE` =
`SVTODATE` = a valid day-1 date returned 75 rows spanning a different year. The `SVTODATE`
rule is well-supported but is not a complete model of period resolution.

### 5.4 Company period fields

**VERIFIED.** `BOOKSFROM` and `STARTINGFROM` give the true data extent. The "current period"
shown on Tally's About screen is a **display** period and does not bound what a filtered
collection can reach.

---

## 6. Crashes and rendering traps

### 6.1 `$$` functions and identifiers containing spaces

**VERIFIED — this terminates the Tally process.**

> **RULE: no `$$` function may reference an identifier containing spaces.**

Tally's formula parser stops at the first space and fails, and the failure is not handled
gracefully. Spaces are tolerated elsewhere — `<REPEAT>` and `<FILTERS>` accept spaced names —
so the rule is specific to `$$` function arguments. Name collections without spaces anyway.

Full diagnostics are retained in `.bridge-live/` and deliberately not reproduced here.

### 6.2 Custom reports need display geometry

**PARTIAL.** A custom report with no form/part display attributes returns an empty
`<ENVELOPE></ENVELOPE>` — no error, no rows. Adding `<TITLE>` on the report, `<HEIGHT>` and
`<WIDTH>` on the form, and `<SCROLLED>`/`<COMMONBORDERS>` on the part restored full output.

**Which attribute is individually necessary was not isolated** — several were changed at
once. `<PLAINXML>Yes</PLAINXML>` was tested and is *not* the cause.

Largely moot if you use collection exports (§2.1).

---

## 7. Balances

**VERIFIED.** A `Ledger` collection's `ClosingBalance` is a **lifetime figure**. It ignores
the requested window entirely — four different windows, including one with no date variables
at all, returned the identical value, derived from transactions outside three of them.

**Do not read `ClosingBalance` as a period balance.** Compute period balances locally from
window-filtered vouchers.

**PARTIAL.** At certain `SVTODATE` values the field renders as **empty** rather than a
number. An empty `TYPE="Amount"` is not zero — coercing it silently produces a wrong balance.
Fail closed or quarantine. Cause not established.

---

## 8. `FETCH` semantics

**VERIFIED.**

- Dotted ledger-entry sub-paths resolve **one level deep**: `ALLLEDGERENTRIES.LEDGERNAME`,
  `ALLLEDGERENTRIES.AMOUNT`, `ALLLEDGERENTRIES.ISDEEMEDPOSITIVE` all work. The similarly
  curated `BILLALLOCATIONS.NAME/.BILLTYPE/.AMOUNT` shape is **not trustworthy for
  outstandings**: it returned empty names and misreported real `New Ref` / `Agst Ref`
  allocations as `On Account`. Guide §2.4a records the A/B proof and the one allowed
  wildcard exception.
- **Two levels do not.** `ALLLEDGERENTRIES.RATEDETAILS.GSTRATE` returns zero elements, as do
  `ALLLEDGERENTRIES.RATEDETAILS.*` and `ALLLEDGERENTRIES.RATEDETAILS`. The data exists —
  the same window under `ALLLEDGERENTRIES.*` yields 56 `GSTRATE` elements.
  `GSTRATE` sits at `VOUCHER > ALLLEDGERENTRIES.LIST > RATEDETAILS.LIST > GSTRATE`.
- **Wildcard cost:** `ALLLEDGERENTRIES.*` measured **19,658 B/voucher** versus **3,142 B**
  curated — **6.3×**. The wildcard pulls audit-entry lists, interest collections and other
  sub-lists.
- **Bill-allocation polarity is contextual, not an amount-sign invariant.** The retained named
  `New Ref` / `Agst Ref` wildcard capture has negative amounts under
  `ISDEEMEDPOSITIVE=No` and positive amounts under `Yes`; the live `On Account` journal probes
  have negative amounts under `Yes` and positive amounts under `No`. Outstandings therefore
  uses the exact bill-allocation amount sign and does not validate it against ledger polarity.
- **Bounded outstandings parity is verified on both Education SKUs.** The identical
  `20260401..20260401` wildcard request completed as a byte-stable paired read on port 9000
  (Edit Log 7.0 EDU: 94,464 encoded bytes, 118/106 ms) and port 9001 (standard 7.1 EDU:
  99,813 encoded bytes, 110/102 ms). Both parsed exactly 7 vouchers and 12 `On Account`
  allocations and computed 600 receivable / 600 payable. The wire serialization differs by
  SKU, but the Unit A canonical result does not.

**Practical consequence:** GST rate data is currently reachable only via the wildcard. Most
voucher types (payment, receipt, contra, journal) carry no GST lines, so the wildcard cost
can be confined to sales and purchase types.

**Fields confirmed available on a voucher:** `GUID`, `MASTERID`, `ALTERID`, `REMOTEID`,
`VCHKEY`, `DATE`, `VOUCHERTYPENAME`, `VOUCHERNUMBER`, `NARRATION`, `PARTYLEDGERNAME`,
`PARTYGSTIN`, `ISCANCELLED`, `ISDELETED`, `ISDELETEDVCHRETAINED`, `ASORIGINAL`,
`ISDEEMEDPOSITIVE`, `ALLLEDGERENTRIES.LIST`, `BILLALLOCATIONS.LIST`,
`ALLINVENTORYENTRIES.LIST`, `RATEDETAILS.LIST`, `AUDITENTRIES.LIST`, `OLDAUDITENTRYIDS.LIST`.

---

## 9. Writes (import)

**VERIFIED.** Writes succeed on **Education mode** — the restriction is on the *voucher date*
(1st, 2nd, 31st), not on the calendar day of entry.

### 9.1 Response shape

A bare `<RESPONSE>` root with **no `ENVELOPE`, no `HEADER`, no `STATUS`**:

```xml
<RESPONSE>
 <CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED>
 <LASTVCHID>295</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED>
 <IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS>
</RESPONSE>
```

A `STATUS=1` rule cannot apply to imports — there is no `STATUS` to check.

### 9.1a A malformed request returns a counter-less response — **fourth response shape**

**VERIFIED.** A request whose XML is not well-formed returns:

```xml
<RESPONSE>Unknown Request, cannot be processed</RESPONSE>
```

No counters. No `STATUS`. No `LINEERROR`. A parser that reaches for `CREATED` or `ERRORS`
will find nothing — and code that defaults missing counters to zero would read this as
"nothing created, no errors", i.e. a benign no-op, when in fact the request never executed.

**Treat a counter-less import response as a hard failure**, distinct from both success and
from a counted rejection.

### 9.1b XML escaping is mandatory — a default Tally group breaks naive builders

**VERIFIED.** The above failure was caused by `<PARENT>Duties & Taxes</PARENT>`. An
unescaped `&` makes the document malformed and Tally rejects the whole request.

**`Duties & Taxes` is a stock Tally group present in every company**, so this is not an edge
case — any request builder that does not escape `&`, `<` and `>` in names, narrations, party
names or group names will fail on ordinary Indian book data. Ledger names containing `&`
("Ram & Sons", "Bharat Iron & Steel") are extremely common.

Escape on the way out; the failure is total and gives no hint of which field caused it.

### 9.2 `ERRORS=0` does not mean success — **TRAP**

**VERIFIED.** A rejected voucher returned `CREATED=0, ERRORS=0, EXCEPTIONS=1` plus a
`LINEERROR`. Any success test of the form `ERRORS == 0` reports a rejected write as posted.

> **Success requires all four: the intended counter incremented, `ERRORS=0`,
> `EXCEPTIONS=0`, and no `LINEERROR`.**

`LINEERROR` text is **untrustworthy for cause attribution** — an out-of-range date produced
"Voucher date is missing" when the date was present.

### 9.3 No natural idempotency for vouchers

**VERIFIED.** Re-sending an identical voucher payload with the same `VOUCHERNUMBER` created a
**second voucher**. Tally does not dedupe. A crash-retry duplicates client data unless the
integrator prevents it.

### 9.4 Master re-create is a silent Alter

**VERIFIED.** Re-sending an identical ledger `ACTION="Create"` returned `CREATED=0,
ALTERED=1` with no error — the existing master was **overwritten** with the retry payload.
"No duplicate was made" is not the same as "my create succeeded." Pre-read before creating,
and persist `CREATED` and `ALTERED` as distinct outcomes.

### 9.5 Identity after write

**VERIFIED.** `LASTMID` is **0** on successful master creates — unusable for master identity;
read masters back by normalised name. `LASTVCHID` is populated for vouchers and usable,
subject to a foreign-writer cross-check. `LASTVCHID` also accepts non-numeric text without
error when parsed back, so validate it.

---

### 9.8 Voucher numbering method changes everything — **use Manual**

**VERIFIED.** The voucher type's numbering method silently determines both whether your
voucher number survives and how a failed Alter behaves.

| Numbering method | Your `<VOUCHERNUMBER>` | Failed Alter behaviour |
| --- | --- | --- |
| **Automatic** (`Auto Retain`) | **Discarded.** Tally assigns its own — we sent `BRIDGE-PROBE-VCH-001` and Tally stored `1` | **Silently creates a duplicate** (`CREATED=1`) |
| **Manual** + `PREVENTDUPLICATES=Yes` | **Preserved.** `BRIDGE-MAN-0001` stored verbatim | **Cleanly rejected** (`CREATED=0, ALTERED=0, EXCEPTIONS=1`) |

Two consequences, both significant:

1. **Voucher-number-based idempotency only works with Manual numbering.** Under automatic
   numbering the client-supplied number is thrown away, so any dedupe key built on it is
   silently ineffective. This was not obvious — the create returned `CREATED=1, ERRORS=0`
   and looked entirely successful.
2. **Manual numbering converts a dangerous failure into a safe one.** The same failed Alter
   duplicates a client's voucher under automatic numbering and is rejected under manual.

> **RULE: any voucher type Bridge writes to should use Manual numbering with
> `PREVENTDUPLICATES=Yes`.** This is a safety property, not a preference.

Creating such a type over XML works: `<VOUCHERTYPE ACTION="Create">` with
`<NUMBERINGMETHOD>Manual</NUMBERINGMETHOD>` and `<PREVENTDUPLICATES>Yes</PREVENTDUPLICATES>`
returned `CREATED=1`.

Note also that `EXCEPTIONS=1` arrived with **no `LINEERROR`** — a parser must treat a
non-zero `EXCEPTIONS` as failure on its own, without waiting for an error string.

### 9.9 Bulk import throughput

**VERIFIED.** One import request may carry many `<VOUCHER>` elements; the counters aggregate.

| Objects per request | Elapsed | Rate |
| --- | --- | --- |
| 1 | 0.53 s | 2/s |
| 5 | 0.57 s | 9/s |
| 25 | 0.83 s | 30/s |
| 100 | 2.08 s | 48/s |

Per-request overhead dominates at small batch sizes. Extrapolating, 10,000 vouchers is
roughly 3–4 minutes and 100,000 roughly 35 minutes — bulk test-corpus generation is
practical.

**This is for test-data generation only.** Bridge's production write path remains
batch-of-one, because import counters are unattributable at N>1: a request carrying 100
objects that returns `CREATED=99` gives no way to identify which one failed.

### 9.7 Operation support matrix — **vouchers cannot be modified**

**VERIFIED.** Every cell tested directly; "verified" below means the resulting state was read
back and confirmed, not merely that a counter moved.

| Object | Create | Alter | Cancel | Delete |
| --- | --- | --- | --- | --- |
| **Master** (`Ledger`) | ✓ `CREATED=1` | ✓ `ALTERED=1` — keyed by **name**; parent change confirmed by readback | n/a | ✓ `DELETED=1` — keyed by name; absence confirmed by readback |
| **Voucher** | ✓ `CREATED=1` | ✗ **`CREATED=1`** — a duplicate is made, target untouched | ✗ **`CREATED=1`** — a new cancelled voucher is made, target untouched | ✓ `DELETED=1` — keyed by `REMOTEID` |

**Voucher Alter was tested with four different keys** — `MASTERID` element, `GUID` element,
`REMOTEID` element, and `REMOTEID` attribute combined with `MASTERID`. All four returned
`CREATED=1, ALTERED=0`. Each produced a duplicate voucher.

`REMOTEID` *is* a valid match key — Delete succeeds with it. So the failure is specific to
the Alter and Cancel operations, not to identification.

**Tally can report a failed match correctly** when it chooses to: deleting a non-existent
master returned `ERRORS=1` with `LINEERROR="Item does not exist!"`. Voucher Alter and Cancel
do not take that path; they create.

**Consequences for the plan's write design:**

1. **§3.1.2's ruling that Cancel is the compensation primitive is inverted by the evidence.**
   Cancel does not work; Delete does.
2. **The "Alter-by-GUID with Cancel+Create fallback saga" has no working leg** on this
   instance. Neither Alter nor Cancel functions.
3. The only working voucher-correction path here is **Delete + Create** — precisely what the
   plan sought to avoid, because it destroys the original rather than superseding it.
4. Masters behave as the plan assumes: name-keyed, alterable, deletable.

**UNVERIFIED and now critical:** whether this is Education-mode behaviour, Edit Log SKU
behaviour, or general. A SKU whose entire purpose is an immutable audit trail plausibly
refuses XML-driven voucher alteration by design. **Qualifying Alter/Cancel on licensed
standard TallyPrime is now a Phase 4 gate, not a nicety.**

**Every one of these failures is caught by the §9.2 rule** and by nothing weaker: the
intended counter (`ALTERED`, `CANCELLED`) never increments, while `ERRORS` and `EXCEPTIONS`
stay at zero. That rule has now caught three distinct silent-failure modes.

### 9.6 `ACTION="Cancel"` by `REMOTEID` creates a new voucher — it does not cancel — **TRAP**

**VERIFIED.** A Cancel request naming an existing voucher's `REMOTEID` returned
`CREATED=1, CANCELLED=0, ERRORS=0, EXCEPTIONS=0` and **created a new voucher** carrying
`ISCANCELLED=Yes`. The targeted voucher was untouched — same `AlterID`, still
`ISCANCELLED=No`.

Two consequences:

1. **The correct identifier or request shape for Cancel is not yet established.** The plan
   designates Cancel as the write-path compensation primitive; until this is solved, Cancel
   cannot be relied on for that role. A compensation that silently *creates* data is worse
   than no compensation.
2. **This validates the §9.2 success rule.** A check of `ERRORS==0 && EXCEPTIONS==0` would
   report success. Only the "intended counter incremented" clause catches it, because
   `CANCELLED` stayed at 0 while `CREATED` moved.

**UNVERIFIED:** whether `ACTION="Cancel"` works when keyed by `GUID`, `MASTERID`, or with a
fuller voucher body. Not yet tested.

**Lead found 2026-07-30.** Tally's official Sample XML page
(`help.tallysolutions.com/sample-xml/`) publishes canonical samples for **"Voucher
Alteration"** and **"Voucher Cancellation"** among its 20 request examples. Our failing
attempts were hand-built. **Before concluding Alter/Cancel is broken on this SKU, retry using
the official sample shapes verbatim** — the §9.5/§9.6 failures may be a request-shape defect
on our side rather than an Edit Log restriction. This is the cheapest next step on the write
path and it should be taken before the Phase 4 licensed-Tally gate.

## 1.2 A modal error dialog in Tally's UI blocks the gateway until a human clicks OK — **P0 operationally**

**VERIFIED 2026-07-30.** A failed company-creation import raised a modal dialog on the Tally
desktop:

```
Internal Error.  Contact Tally Solutions.
Unable to create company in path ''!
```

**While that dialog was open the XML gateway served nothing** — not `/status`, not a trivial
collection read. It required an operator to dismiss it.

This is a **distinct second cause** of gateway unresponsiveness, and it behaves differently
from the first:

| Cause | Recovers without a human? |
| --- | --- |
| Long-running request still executing (§11b.2) | **Yes** — measured at 523 s and 113 s |
| **Modal error dialog** | **No** — blocks indefinitely until dismissed |

**Consequences:**

1. **A single malformed request can freeze a CA's Tally indefinitely.** Not for the duration
   of the work — until someone notices a dialog and clicks OK.
2. **On an unattended or hosted deployment this is a permanent hang.** There is nobody to
   click. This materially affects the parked Tally-on-cloud topology and any headless-agent
   design.
3. **A liveness probe cannot distinguish the two causes.** Both present as a dead gateway.
   Bridge cannot tell "busy, will recover" from "waiting on a human" from the network side,
   so any watchdog must escalate to the operator rather than wait indefinitely.
4. This retroactively explains earlier unresponsive periods that were attributed solely to
   long-running work.

**Design rule: treat any request that can raise a Tally dialog as capable of taking the
instance down until a human intervenes.** Prefer request shapes that fail with a `LINEERROR`
in the response over shapes that fail inside Tally's UI.

## 9.11 Company pinning — reads and writes behave differently — **TRAP**

**VERIFIED.** Same `SVCURRENTCOMPANY` values, opposite safety properties.

| `SVCURRENTCOMPANY` | Read (`Ledger` collection) | Write (`LEDGER ACTION="Create"`) |
| --- | --- | --- |
| Correct name | 86 rows, `STATUS=1` | `CREATED=1` |
| **Omitted entirely** | **86 rows** — silently uses the loaded company | **`CREATED=1` — silently writes to the loaded company** |
| **Non-existent name** | **0 rows, `STATUS=1`, no error** | **fails closed** — `"Could not set 'SVCurrentCompany' to …"` |
| Empty value | 0 rows, `STATUS=1` | — |
| Different case | 86 rows — matching is case-insensitive | — |
| Trailing space | 86 rows — matching is whitespace-trimmed | — |

Three findings:

1. **Omission is the dangerous case, for both reads and writes.** With no
   `SVCURRENTCOMPANY`, Tally silently uses whatever company is loaded — confirming the
   ecosystem's documented worst failure mode. A write lands in the wrong company with
   `CREATED=1` and no indication anything was wrong. **`SVCURRENTCOMPANY` must be present on
   every company-scoped request, without exception.**
2. **A mistyped company name is safe on write and dangerous on read.** Writes fail closed
   with a clear error. Reads return **zero rows with `STATUS=1`** — indistinguishable from a
   genuinely empty company. Under §3.1.7's absence-implies-deletion rule, a company-name
   typo would present as "every voucher was deleted".
3. **Name matching is case-insensitive and whitespace-trimmed.** Convenient, but it means
   name-based company identity is looser than exact-match code would assume.

**Required:** Bridge must verify company identity *in the response* — via the company GUID —
rather than trusting that the request was honoured. Tally reports success either way.

### 9.11a How to read the company GUID — **VERIFIED, and this closes §9.11's requirement**

**VERIFIED 2026-07-30.** A single-object export returns the full company definition,
including `<GUID>`, in **0.1 s / 41 KB / 565 distinct tags**:

```xml
<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>
<TYPE>Object</TYPE><SUBTYPE>Company</SUBTYPE>
<ID TYPE="Name">Aarav Trading Company Demo</ID></HEADER>
<BODY><DESC><STATICVARIABLES>
<SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
<SVCURRENTCOMPANY>Aarav Trading Company Demo</SVCURRENTCOMPANY>
</STATICVARIABLES><FETCHLIST><FETCH>*</FETCH></FETCHLIST></DESC></BODY></ENVELOPE>
```

Observed: `<GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>`, plus `COMPANYNUMBER`,
`BASICCOMPANYFORMALNAME`, `STARTINGFROM`, `BOOKSFROM`, and object counts
(`NUMGROUPS 28`, `NUMLEDGERS 87`).

Note the **company GUID is the prefix of every master's GUID** in that company — the
`Currency` master's GUID is `bb8ad19e-…-6215e-0000001d`. That gives Bridge a cheap
cross-check that a returned master actually belongs to the intended company, which is
exactly the defence §9.11 calls for.

`TYPE=Object` is a documented shape (Tally's own samples use it for a single ledger) and it
was confirmed safe against a known-good `SUBTYPE=Ledger` control before being pointed at
`Company`. **This is the recommended company-identity probe.**

## 9.10 Company creation over XML — **PARTIAL: symbol element found, formal-name element not**

**PARTIAL.** Company creation is *attempted* by Tally — it validates and returns specific
field errors rather than refusing the operation — but no complete request was achieved.

**Method that worked: follow the error chain.** Each accepted field advances the error to the
next missing one, which is the only reliable way to reverse-engineer an undocumented import
schema.

| Element tried for base currency symbol | Result |
| --- | --- |
| `BASECURRENCYSYMBOL`, `CURRENCYSYMBOL`, `BASECURRENCY`, `CMPBASECURRENCY`, `ORIGINALNAME`, `BASICCURRENCYSYMBOL`, `SYMBOL`, nested `<CURRENCY>` block | *"The Base Currency Symbol is required!"* — all rejected |
| **`CURRENCYNAME`** | ***"The Formal Name for Currency is required!"*** — **accepted; advanced to next validation** |

With `<CURRENCYNAME>` present, six candidates for the formal name were all rejected with the
same message: `CURRENCYFORMALNAME`, `FORMALNAME`, `MAILINGNAME`, `EXPANDEDSYMBOL`,
`CURRENCYMAILINGNAME`, and a combination with `DECIMALPLACES`/`DECIMALSYMBOL`/`ISSUFFIX`/
`HASSPACE`.

### 9.10a Second pass 2026-07-30 — path and financial year solved, currency formal name still open

**Two of the three unknowns are now VERIFIED**, and the failure mode moved from
*modal dialog* to *in-band `LINEERROR`*, which makes further probing far safer.

| Unknown | Status | Element |
| --- | --- | --- |
| Data path (`Unable to create company in path ''`) | **SOLVED** | **`<SVCURRENTPATH>` — a `STATICVARIABLES` entry, not a `COMPANY` child** |
| Financial year | **SOLVED** | **`<STARTINGFROM TYPE="Date">` + `<BOOKSFROM TYPE="Date">`**, `yyyymmdd` |
| Currency formal name | **still open** | 19 candidates and 4 structural shapes rejected |

**Why the path was never going to be guessed as a field.** It is not a property of the
company at all — it is a *static variable*, the same slot as `SVCURRENTCOMPANY`. Tally
documents `SVCurrentPath` as "a directory path which has data". Supplying it changed the
error from the path complaint to the financial-year complaint, which is the proof.

Verified working prefix (everything up to the currency formal name):

```xml
<STATICVARIABLES>
  <SVCURRENTPATH>C:\Users\Public\TallyPrimeEditLog\data</SVCURRENTPATH>
</STATICVARIABLES>
...
<COMPANY NAME="Bridge Billwise Lab" ACTION="Create">
  <NAME>Bridge Billwise Lab</NAME>
  <BASICCOMPANYFORMALNAME>Bridge Billwise Lab</BASICCOMPANYFORMALNAME>
  <CURRENCYNAME>Rs.</CURRENCYNAME>
  <STARTINGFROM TYPE="Date">20240401</STARTINGFROM>
  <BOOKSFROM TYPE="Date">20240401</BOOKSFROM>
  <COUNTRYNAME>India</COUNTRYNAME>
  <ISBILLWISEON>Yes</ISBILLWISEON>
</COMPANY>
```

**Why the formal name cannot be found by export — a negative result worth keeping.**
`TALLYREQUEST=Export / TYPE=Object / SUBTYPE=Company` with `<FETCH>*</FETCH>` returns a
**565-tag** company definition. It contains **no currency symbol, no currency formal name,
and no data path** — confirmed both by tag-name scan (`PATH`/`CURR`/`FORMAL`/`SYMBOL`) and by
*value* scan for `INR`, `paise`, `Rs.`, `₹`. The only currency-ish tags are the CMPINFO
counter `<CURRENCY>8</CURRENCY>` and `<NUMCURRENCIES>1</NUMCURRENCIES>`.

**Conclusion: the base-currency fields are form-local to the Company Creation screen, not
object properties.** Export-then-reimport — normally the reliable way to recover an
undocumented Tally import schema — cannot work here, because Tally never emits them.

Candidates rejected in this pass (all returned *"The Formal Name for Currency is required!"*
with the gateway healthy):

| Shape | Candidates |
| --- | --- |
| Flat, batch 1 | `BASICCURRENCYFORMALNAME`, `CURRENCYEXPANDEDSYMBOL`, `CURRENCYORIGINALNAME`, `BASECURRENCYFORMALNAME`, `FORMALNAMEFORCURRENCY` |
| Flat, batch 2 | `BASECURRENCYNAME`, `BASICCURRENCYNAME`, `CURRENCYFULLNAME`, `FORMALCURRENCYNAME`, `CURRENCYMAILNAME`, `CMPCURRENCYFORMALNAME`, `EXPANDEDCURRENCYSYMBOL`, `CURRENCYDESC` |
| Nested sub-object | `<BASECURRENCY>`, `<CURRENCY.LIST>`, `<BASECURRENCY.LIST>` each carrying `ORIGINALNAME`/`MAILINGNAME`/`EXPANDEDSYMBOL` |

**Batching is valid here because unknown elements are silently ignored** — a batch that still
reports the same "required" error disproves every name in it at once.

### 9.10b `ORIGINALNAME` at `COMPANY` level hangs the gateway — **TRAP**

**VERIFIED 2026-07-30.** A flat combination of
`MAILINGNAME` + `EXPANDEDSYMBOL` + `DECIMALSYMBOL` + **`ORIGINALNAME`** inside `<COMPANY>`
produced **no response at all** — the request timed out at 120 s and `/status` stayed empty
thereafter. The three *nested* shapes immediately before it all returned clean in-band errors
with the gateway alive, so the difference is the flat shape, and `ORIGINALNAME` is the element
the earlier pass had also singled out as rejected-for-symbol.

**Do not send `ORIGINALNAME` as a direct `COMPANY` child.**

#### It raised the §1.2 modal dialog and blocked for 44 minutes until a human clicked OK

**VERIFIED by operator screenshot.** The Tally window showed, for the whole outage:

```
Internal Error.  Contact Tally Solutions.
Unable to create company in path ''!
```

**Timeline:** gateway dead from **22:31**; dialog still open on screen at **23:13**; gateway
responsive at **23:14:45**, coinciding with the operator dismissing it. **~44 minutes blocked,
ended by a human, not by recovery.** No partial company was created — a company list
afterwards returned only `Aarav Trading Company Demo`.

> **Correction.** This entry first recorded the outage as unattended self-recovery and used it
> to revise §11b.2's busy-regime maximum upward to 44 minutes. **That was wrong** — the
> gateway was polled from the network side only, where a dialog-blocked Tally and a busy Tally
> are indistinguishable (§1.2 point 3), and recovery was attributed to time when it was
> actually caused by the operator. §11b.2's measured maximum stands at **523 s**. A watchdog
> must not be built on the retracted number.

**The method error worth keeping:** network-side polling cannot establish *why* a gateway came
back. Attributing recovery to elapsed time requires knowing nobody touched the machine. Where
that cannot be known, the observation is unusable as a timing measurement — ask the operator
what was on screen.

#### Why this shape reached the path code at all

Every other probe in this pass carried the same `SVCURRENTPATH` and failed in-band on the
currency formal name. Only the shape containing **`ORIGINALNAME`** produced the empty-path
internal error. So `ORIGINALNAME` as a `COMPANY` child diverts Tally into a code path that
**never consults `SVCURRENTPATH`**, hits an unset path, and raises an unhandled internal error
in the UI instead of returning a `LINEERROR`.

This retroactively explains the original §1.2 dialog from the earlier session — those attempts
predate the `SVCURRENTPATH` finding, so any of them could reach the same unset-path state.

**Method note.** The probe script health-checked after every shape and stopped itself on the
first dead gateway rather than sending the remaining candidates. That is the required pattern
for this whole area — a loop of company-creation attempts has previously taken the operator's
machine down.

### 9.10c Routes ruled out by documentation review

- **`TALLYREQUEST=Execute`** exists and is real, but its only documented form is
  `TYPE=TDLAction` / `ID=Sync`. Tally's own reference states *"As of now only Sync action is
  introduced."* It is **not** a company-creation route.
- **Tally's official Sample XML page** lists 20 request samples — groups, ledgers, stock
  items, UOMs, godowns, vouchers, and reports. **There is no company-creation sample**, and no
  currency or path element appears anywhere on it.
- **The two mature open-source clients** (`TallyConnector`, `Tally.Py`) cover "all masters and
  vouchers"; neither exposes company creation. `ChangeCompany` selects an existing company.
- The official page for *"Unable to create company in path"* attributes it to folder
  permissions or corruption. **That diagnosis does not apply here** — our path was literally
  empty, i.e. never supplied.

### 9.10d The legacy `IMPORTDATA` envelope is accepted — and renames the loaded company — **P0 TRAP**

**VERIFIED 2026-07-31.** Code search (not web search) found a working third-party
implementation using an envelope **structurally different from ours**: `TALLYREQUEST` is the
single token **`Import Data`**, there is no `<TYPE>`/`<ID>`, and the body is the legacy
`IMPORTDATA / REQUESTDESC / REPORTNAME / REQUESTDATA` form rather than `DESC / DATA`.

```xml
<ENVELOPE><HEADER><TALLYREQUEST>Import Data</TALLYREQUEST></HEADER>
<BODY><IMPORTDATA><REQUESTDESC><REPORTNAME>All Masters</REPORTNAME>
<STATICVARIABLES><SVCURRENTPATH>…</SVCURRENTPATH></STATICVARIABLES></REQUESTDESC>
<REQUESTDATA><TALLYMESSAGE xmlns:UDF="TallyUDF">
<COMPANY Action="Create">
  <NAME>Bridge Billwise Lab</NAME><MAILINGNAME>…</MAILINGNAME>
  <STARTINGFROM>20240401</STARTINGFROM><BOOKSFROM>20240401</BOOKSFROM>
  <BASECURRENCYSYMBOL>Rs.</BASECURRENCYSYMBOL><FORMALNAME>Indian Rupees</FORMALNAME>
  <ISBILLWISEON>Yes</ISBILLWISEON>
</COMPANY></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>
```

**Result: `CREATED=0`, `ALTERED=1`, `ERRORS=0`, `EXCEPTIONS=0`, no `LINEERROR`.**

**No company was created. The currently loaded company was RENAMED.** `Aarav Trading Company
Demo` became `Bridge Billwise Lab` — same `GUID bb8ad19e-…-87fec0c6215e`, same
`COMPANYNUMBER 100000`, same 87 ledgers and 28 groups. A field-level diff against a
pre-incident capture showed **`NAME` was the only substantive change**; the other six diffs
were request-scoped `CMPINFO` counters. Reversing the same request with the original name
restored it.

**Three lessons, in order of importance:**

1. **`Action="Create"` on a `COMPANY` object does not mean create.** With a company loaded and
   no `SVCURRENTCOMPANY`, Tally binds the `COMPANY` object to the **loaded** company and
   alters it. This is §9.11's omission hazard in its most destructive form yet: a *creation*
   request silently mutating an unrelated production company's identity.
2. **§9.2/I6 is what caught it.** `ERRORS=0` and `EXCEPTIONS=0` and no `LINEERROR` — a check of
   errors alone would have reported success. Only "the **intended** counter incremented"
   caught it, because `CREATED` stayed 0 while `ALTERED` moved. This is the **fourth**
   distinct silent-failure mode that rule has caught.
3. **I2 applies to company creation too**, which is counter-intuitive: the one request type
   that has no existing company to pin is exactly the one that will hijack whichever company
   happens to be open.

**Leading hypothesis for why creation never succeeds:** Tally's import binds a `COMPANY`
object to the loaded company. Creation may require **no company loaded at all**. That is
testable — the operator closes all companies, then one request is sent — but it cannot be done
remotely and disrupts whoever is using the instance. **Untested.**

**Where a future attempt should start:** the hypothesis above, not more element-name guessing.
`BASECURRENCYSYMBOL` + `FORMALNAME` as a *pair* in the legacy envelope raised no currency error
at all, so the currency schema may already be solved and simply masked by the alter behaviour.

**Value judgement — unchanged, and now better founded.** Company creation is a one-time human
setup step; Bridge's users already have companies. This is worth completing only if
unattended provisioning becomes a requirement. It is **not** on Bridge's critical path, and
the remaining unknown is the one field that no export can reveal.

## 10. Change detection

**VERIFIED.** Company-level `ALTVCHID` and `ALTMSTID` are monotonic high-water marks and move
in step with writes — two master creates advanced `ALTMSTID` by 2; one voucher create
advanced `ALTVCHID` by 1. Per-object `ALTERID` is exposed on vouchers and masters.

**UNVERIFIED but high value:** `AUDITENTRIES.LIST`, `OLDAUDITENTRIES.LIST` and
`ACCOUNTAUDITENTRIES.LIST` containers appear in voucher exports on the Edit Log SKU. They
were empty on un-edited demo data. If editing a voucher populates them with before/after
values, the Edit Log SKU exposes change history directly and AlterID diffing becomes
unnecessary on that SKU. **Not tested.**

### 10.1 Server-side `AlterID` filtering works — **contradicts published community guidance**

**VERIFIED.** A `<FILTERS>` predicate of the form `$AlterID > N` filters correctly at the
server. Measured on the demo company:

| Predicate | Rows |
| --- | --- |
| `$AlterID > 0` | 150 (whole book) |
| `$AlterID > 100` | 51 |
| `$AlterID > 200` | 3 |
| `$AlterID > 500` | 0 |

Semantically verified, not merely monotonic: `$AlterID > 200` returned exactly three
vouchers with `AlterID` 440, 441 and 442.

This matters because the published community position is the opposite. A TallyForum thread
on conditional export states *"Tally Prime does not respond to any filtering criteria other
than date duration"*, and the accepted workaround is to download the entire book on every
sync and diff `AlterID` client-side — described there as "network intensive". That workaround
is unnecessary, at least on this release.

**Consequence for incremental sync:** the design becomes cheap. Probe company-level
`ALTVCHID`/`ALTMSTID`; if moved, fetch only `$AlterID > checkpoint`. Three rows instead of
one hundred and fifty.

**Known gap — deletions.** A deleted object has no `AlterID` to exceed the checkpoint, so an
AlterID-filtered scan cannot see it. Deletion detection still requires either a complete
scan with absence reasoning (subject to §5.3's corroboration rule) or, on the Edit Log SKU,
the audit-entry containers above. **A cancelled voucher IS detected** — the cancel in §9.6
produced a new object at `AlterID` 443, visible to a `> 440` filter.

**UNVERIFIED:** whether other fields filter server-side (`$IsCancelled`, `$VoucherTypeName`,
`$PartyLedgerName`). Only `$Date` and `$AlterID` have been tested. Given the forum's claim
was wrong about `AlterID`, it is probably wrong more broadly — worth testing before accepting
any "Tally can't filter that" advice.

---

## 11b. Scale findings at 101,287 vouchers — the two that matter most

**VERIFIED 2026-07-29** on a 101,287-voucher production-shaped corpus.

| Request | Time | Bytes | Rows | B/row |
| --- | --- | --- | --- | --- |
| Minimal fetch, whole book | 108 s | 123.4 MB | 101,287 | 1,218 |
| **Curated, whole book** | **600 s (client gave up)** | **282.7 MB** | **78,320 of 101,287** | 3,609 |
| Curated, one month | 7.3 s | 17.7 MB | 4,891 | 3,612 |
| Curated, `$AlterID` filtered | 24.1 s | 5.7 MB | 1,583 | 3,615 |
| Wildcard, one month | 69.2 s | 105.3 MB | 4,891 | 21,532 |

### 11b.1 A truncated read is indistinguishable from a complete one — **P0**

The whole-book curated read returned **78,320 of 101,287 rows and still carried
`STATUS=1`**. `STATUS` sits in the `HEADER` at the *start* of the document, so it is emitted
long before Tally knows whether the response will complete. There is **no trailer, no row
count, and no completeness marker anywhere in the response.**

A consumer that checks `STATUS=1` and parses what arrived will silently conclude it has the
whole book while missing 22,967 vouchers. Under §3.1.7's absence-implies-deletion rule that
is mass false tombstoning.

**Required:** completeness must be established by the *client*, from something other than the
response's own claims — expected row count from a prior probe, byte-length agreement, or an
explicit second read. `STATUS=1` proves the request started, not that it finished.

**Tally never enforced a size cap** — it streamed 282 MB without complaint. The 32 MiB limit
is Bridge's own client-side rule. Tally will not refuse an oversized request on your behalf.

### 11b.2 A client timeout does not cancel server-side work — **P0 operationally**

After the client abandoned the 600 s read, **the gateway stayed completely unresponsive for a
further 523 s**, then recovered by itself with no restart. Total server-side occupancy for
that one request: roughly **19 minutes**, of which nearly nine minutes came *after* the
client had disconnected.

Throughout that window every other request failed — including `/status`. Because the gateway
serialises (§1), one abandoned expensive read blocks the entire instance.

**Consequences:**

1. **Abandoning a request frees your socket, not Tally.** There is no observed way to cancel
   server-side work once issued.
2. **A user-facing "cancel sync" cannot stop Tally.** Phase 3's "cancellable mid-segment"
   requirement is achievable only as "stop consuming" — the accountant's Tally remains busy
   regardless.
3. **The blast radius is the accountant's own session.** For ~19 minutes that Tally was
   unusable to the person sitting in front of it. This is the failure that generates support
   tickets and destroys trust in a sync product.

**Design rule: never issue a read you cannot afford to wait out.** Segment size must be
chosen by expected *duration*, not only by byte size — a segment that fits under 32 MiB but
takes ten minutes is still unacceptable. The one-month curated read (4,891 rows, 17.7 MB,
7.3 s) is the right order of magnitude; **~5,000 vouchers per request** is a defensible
default, well under the ~9,300 the byte cap alone would allow.

### 11b.3 Filter cost is fixed, not proportional to results

`$AlterID > 190000` matched **zero** rows and still took 22.09 s. `$IsCancelled` matched
**one** row in 22.61 s. Tally evaluates the predicate across the whole collection, so every
filtered query at 101K costs ~22 s regardless of how little it returns.

Server-side filtering saves bandwidth and parsing, **not scan time**. This makes the cheap
company-level `ALTVCHID`/`ALTMSTID` probe (§10) essential rather than an optimisation — it is
the only way to avoid paying 22 s to discover that nothing changed.

### 11b.4 Bytes-per-row is stable and predictable

1,218 minimal · 3,609–3,615 curated (identical across whole-book, one-month and filtered
reads) · 21,532 wildcard. Segment sizing can be derived from a single sample read with
confidence.

## 11a. Scale measurements — 11,287-voucher corpus

**VERIFIED 2026-07-29** on a generated production-shaped corpus: 25 customers and 15
suppliers with valid-format GSTINs across six state codes, Sales/Purchase/Payment/Receipt
mix, 9%+9% CGST/SGST splits, invoice-referencing narrations, spread across every
Education-legal date from 2024-04 to 2026-03.

### Read performance

| Request | Elapsed | Bytes | Rows | B/row |
| --- | --- | --- | --- | --- |
| Minimal fetch (`DATE,ALTERID`), whole book | 5.44 s | 13.7 MB | 11,287 | 1,214 |
| **Curated fetch, whole book** | 15.88 s | **40.6 MB** | 11,287 | **3,598** |
| Curated fetch, one-month window | 0.72 s | 1.94 MB | 538 | 3,605 |
| Curated + `$AlterID` filter (no matches) | 1.99 s | 1.5 KB | 0 | — |
| Wildcard fetch, one-month window | 2.94 s | 11.6 MB | 538 | 21,532 |

**Key results:**

1. **The 32 MiB cap binds at ~9,300 vouchers** with a curated fetch. A whole-book read of
   11,287 vouchers produced **40.6 MB — already over the cap**. Segmentation is not an
   optimisation, it is a correctness requirement at any realistic client size.
2. **Bytes-per-row is highly stable** — 3,598 whole-book versus 3,605 for a single month.
   Segment sizing can be predicted reliably from a sample read.
3. **Wildcard costs 6.0×** curated (21,532 vs 3,598 B/row), confirming the earlier 6.3×
   measurement on a smaller sample.
4. **Server-side filtering is not free.** A filter matching zero rows still took 1.99 s —
   Tally evaluates the predicate across the whole collection. Filtering saves bandwidth and
   parsing, not scan time.
5. Curated read throughput is roughly **710 rows/second**.

### Write performance and degradation

10,000 vouchers imported in 624 s, zero errors, batches of 250. **Throughput degraded
monotonically as the book grew:**

| Progress | Rate |
| --- | --- |
| 250 | 21/s |
| 2,750 | 21/s |
| 5,250 | 19/s |
| 7,750 | 17/s |
| 10,000 | 16/s |

Roughly a 25% drop across the first 10K rows. **Bulk import cost is superlinear in existing
book size**, which matters for any initial-load or migration story: a 100K-voucher onboarding
will not take ten times a 10K load.

## 11. Measurements

| Quantity | Value | Basis |
| --- | --- | --- |
| Curated voucher payload | **3,142 B/voucher** | 6 vouchers, curated FETCH |
| Wildcard voucher payload | **19,658 B/voucher** | same window, `ALLLEDGERENTRIES.*` |
| Response cap | 32 MiB general; 40 MiB only for the closed wildcard outstandings request | Bridge-side limits; guide §2.5b |
| Segmentation ceiling | **Profile-specific; derived at runtime** | ~10,600 applies only to the older curated shape, not wildcard outstandings |
| Independent corroboration | 10,000 vouchers/batch | `tally-database-loader` caps here to avoid hangs |
| Whole demo book, wildcard | 2.94 MB / 150 vouchers | wide filter |
| Typical collection latency | 30–90 ms | demo company |

Segment size must be derived from **observed** per-voucher cost at runtime — inventory lines
and long narrations run heavier than this demo data.

---

## 12. Verification method — how to test this gateway

Learned the hard way; every one of these produced a wrong conclusion at least once.

1. **Write responses to a file, then inspect the file.** Never pipe `curl` through
   `head`/`tail` — SIGPIPE truncates the capture mid-element and the truncated file looks
   like a complete short response.
2. **Never count rows with a bare substring grep.** `<VOUCHER` also matches `<VOUCHERNUMBER>`
   and `<VOUCHERTYPENAME>`; `<GROUP` matches the `CMPINFO` header. Match the opening tag with
   its trailing space, or parse.
3. **Diff whole files, not fragments.** A narrow `grep -A1` window missed a 13-byte
   difference that reversed a conclusion.
4. **Check the returned span, not just the row count.** A plausible row count can come from
   entirely the wrong period.
5. **Repeat before believing anything anomalous.** Determinism distinguishes a protocol rule
   from a transient.
6. **Change one variable at a time.** A multi-variable change established that a combination
   works while leaving the actual cause unknown.

---

### 12.7 Empty-date witness profile qualification

**VERIFIED 2026-08-02** under owner-supervised dispatch against the reference
corpus. `VoucherEmptyPartitionWitnessV1` was healthy before and after every
request and produced no dialog or hang.

| Purpose | Window | Observation |
| --- | --- | --- |
| Non-empty control | `20240401..20240501` | 20 vouchers dated `20240401..20240402` |
| Empty primary | `20240502..20240601` | no rows |
| Shifted cover A | `20240501..20240531` | no rows |
| Shifted cover B | `20240531..20240601` | no rows |

The non-empty control proves the profile returns rows when rows exist; the two
date-shifted covers corroborate that the primary partition is genuinely empty.
This qualifies the runtime to use the profile only as the nearest non-empty
control and the bounded shifted cover for a positive-high-water empty primary
partition. It does not qualify a wider date window, change the universal
31-day cap, establish a compatibility claim, or establish a sizing rule.

**Parser trap.** An empty witness response also contains `CMPINFO` with bare
`<VOUCHER>0</VOUCHER>`. Counting `VOUCHER` over the whole document sees one
element and reverses the verdict. This was observed three times in the
qualification session. Count only `VOUCHER` start elements inside `<DATA>` and
deserialize through `BODY.DATA.COLLECTION`; the implementation does both.

---

## 12a. Bill-wise semantics, the native reports, and volume — live measurement 2026-08-02

**VERIFIED 2026-08-02** against TallyPrime Edit Log 7.0 EDU on port 9000, using
`Bridge Ageing Lab` — a company created for this session, every shape placed deliberately —
and `Aarav Trading Company Demo` for scale. All data synthetic.

Several entries here **correct or qualify earlier sections**; each says which.

### 12a.1 Built-in named reports work — this qualifies §2.2's blanket AVOID

§2.2 tests a **custom** `REPORT`/`FORM`/`PART` and concludes `<TYPE>Data</TYPE>` should be
avoided. That conclusion does not extend to Tally's **own built-in reports addressed by name**:

```xml
<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>
<TYPE>Data</TYPE><ID>Bills Receivable</ID></HEADER>
<BODY><DESC><STATICVARIABLES>
  <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
  <SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY>
  <SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE>
</STATICVARIABLES></DESC></BODY></ENVELOPE>
```

Accepted on the first attempt. `Bills Payable` works identically. Per-bill rows:

```
<BILLFIXED><BILLDATE/><BILLREF/><BILLPARTY/></BILLFIXED> <BILLCL/> <BILLDUE/> <BILLOVERDUE/>
```

Four properties that matter more than the speed:

1. **It was fast on the observed book.** 101,603 vouchers returned 22 rows in **0.10 s /
   5 KB**. This single measurement does not establish an asymptotic bound: voucher count,
   open-bill count, cache state, and release were not varied independently. A voucher scan over
   the same book was much more expensive, but capacity and deadline planning must treat that as
   book- and release-specific evidence.
2. **`STATUS` is emitted only on failure.** A successful response contains **no `STATUS` tag
   at all**, so you cannot assert `STATUS=1` as §2.1 allows.
   **But absence of `STATUS` does not prove success.** An unrecognised report `ID` returns a
   bare `<RESPONSE>Unknown Request, cannot be processed</RESPONSE>`, which also carries no
   `STATUS` — a parser treating "no `STATUS`" as success would read it as a report with zero
   bills and silently publish no exposure. **Require the expected report structure, and reject
   `RESPONSE` or `LINEERROR` independently of `STATUS`.**
3. **It fails closed on a company that is not loaded**, with
   `LINEERROR: Could not set 'SVCurrentCompany' to '<name>'` — see 12a.6.
4. **It carries no GUID anywhere**, so the response cannot be identity-bound the way a
   voucher or ledger collection can. Bracket every candidate native read with GUID-verified
   extent probes before and after it, and reject drift. Bracketing detects some changes; it does
   not create an atomic source cut.

An unrecognised report `ID` returns a clean `<RESPONSE>Unknown Request, cannot be
processed</RESPONSE>` — **not** the modal-dialog hang of §1.2. The hazards are different
failure paths and should not be conflated.

**They scope by balance SIGN, not by party group.** On the demo book, each report included
parties from both Sundry *Creditors* and Sundry *Debtors*. That is correct accounting — a
supplier advance is a debit balance — but a screen labelled "receivables" that renders this
list is showing every debit-balance bill, including advances paid out. The measurement records
rows, not a complete distinct-ledger census.

### 12a.2 Which date each allocation kind ages from

Measured against Tally's own *Ledger Outstandings* screen and both native reports, not
inferred from the data model.

| Kind | Ages from | Note |
| --- | --- | --- |
| `New Ref` | `BILLDATE` | which on an opening equals the voucher date |
| `Agst Ref` re-opening a settled bill | **the original bill's `BILLDATE`** | not the voucher that reused it |
| `Advance` | `BILLDATE` | always its own voucher date; see 12a.4 |
| `On Account` | **not aged at all** | shown at the *report* date, blank overdue |

The second row was contested in the codebase. Constructed explicitly — a bill opened at
3,000, settled to exactly zero, then reused by a later `Agst Ref` for 1,500 — Tally reports:

```
1-Jun-26  ZBR  3,000.00 Dr opening  1,500.00 Cr pending  Due on 1-Jun-26  Overdue 60
```

Aged from **1-Jun-26**, the original bill, though the reusing voucher is dated 1-Jul-26.
Confirmed three ways: on screen, in `Bills Receivable`, and in `Bills Payable`.

The mechanism is 12a.4: an `Agst Ref` **carries** the original bill's `BILLDATE`, so ageing
from `BILLDATE` reaches the true opening without special handling.

### 12a.3 Tally offers two ageing methods, and the anchor differs

`F6: Ageing Method` offers *Ageing by Bill Date* and *Ageing by Due Date*, and the choice
changes the buckets. On a bill dated 1-May-26 with a 30-day credit period, due 31-May-26:

| as-of | method | days | bucket |
| --- | --- | --- | --- |
| 2-Jul-26 | by Due Date | 32 | 30 to 60 |
| 2-Jul-26 | by Bill Date | 62 | 60 to 90 |

Both are correct. Neither is "the" answer, so **any tool computing ageing must state which
basis it used.**

`BILLCREDITPERIOD` tags appear in the ordinary wildcard voucher fetch — 19 occurrences in a
single partition capture of a book that sets none. That establishes serialization of the empty
field only, not the usable value or format of a configured credit period. Capture and read back
a nonzero period before claiming that due-date ageing needs no request-shape change.

**`BILLOVERDUE` in the native report is not usable as an ageing oracle.** Its as-of is
Tally's own period date, not the caller's, and it does **not** follow the `F6` setting:
toggling the UI to *Ageing by Bill Date* left the exported `BILLOVERDUE` unchanged at the
due-date value. Recompute ageing from `BILLDATE`/`BILLDUE` against an explicit as-of on
every path.

### 12a.4 The import path rewrites what you send — extends §9

§9.2 already records that `ERRORS=0` does not mean success. Eight further rewrites, each
measured by writing a distinguishable value and reading it back. **Every one reported
`CREATED=1, ERRORS=0, EXCEPTIONS=0`.**

| # | Behaviour |
| --- | --- |
| 1 | `BILLDATE` on a bill-**opening** allocation is overwritten with the voucher date; a differing supplied value is discarded |
| 2 | `BILLDATE` on an `Agst Ref` is **inherited** from the settled bill — so it legitimately differs from the voucher date |
| 3 | The allocation **kind** is rewritten when the reference name already exists: `Advance` naming an existing ref is stored as `Agst Ref` |
| 4 | `On Account` names are **stripped** — it is structurally unnamed |
| 5 | An allocation on a ledger with `ISBILLWISEON=No` is **silently discarded**; the entry stores with no allocations. Treat `No` as a mandatory fail-closed preflight for any bill-wise write |
| 6 | `VOUCHERNUMBER` is **overridden** with Tally's own per-type sequence — **under automatic numbering only.** §9.8 establishes that Manual + `PREVENTDUPLICATES=Yes` preserves it verbatim. The company measured here used the default, so this observation does not generalise; the numbering method decides it |
| 7 | On the automatically numbered voucher type measured here, `ACTION="Alter"` carrying the target `GUID` returns `CREATED:1, ALTERED:0` — it creates a duplicate rather than editing. This is not a numbering-method-independent result; see §9.8 |
| 8 | The company-level `Enable Bill-wise entry` gate does **not** apply to XML import — see 12a.5 |

Consequence for §9 generally: **a write is only verified by reading it back.** Counters
describe the operation, not the result.

One error message is actively misleading. A voucher dated on a day Education forbids returns:

```
CREATED=0  ERRORS=0  EXCEPTIONS=1
LINEERROR: Voucher date is missing for: 'Sales' voucher BAD-1.
```

The date was present and well-formed — it was illegal for the mode. Note `ERRORS=0`; the
failure lives in `EXCEPTIONS`.

### 12a.5 Configuration is not a diagnostic, in either direction

`F11 → Enable Bill-wise entry` set to **No** on a company holding bill-wise data, confirmed
on screen, changed **nothing** observable over XML: per-ledger `ISBILLWISEON` stayed `Yes`,
existing allocations survived, both native reports returned identical rows, and a **new**
allocation imported afterwards was still accepted and retained.

So the company flag gates Tally's own voucher-entry screen, not the API or the data. It is
also **not fetchable over XML**, so a client could not read it even if it meant something.

And the per-ledger flag is not a sufficient **positive** diagnostic. `ISBILLWISEON=Yes` does
not prove that a ledger has bill references, but `No` remains a mandatory fail-closed signal
before a bill-wise XML write. On the 101,603-voucher book:

```
party ledgers (Sundry Debtors / Creditors) : 60
  ISBILLWISEON = Yes : 60
  ISBILLWISEON = No  :  0
  absolute closing balance                 : 208,232,027.79
```

Every ledger correctly configured — yet only **7 of 60** carry any bill reference at all.

### 12a.6 The unallocated remainder, and how to see it

Because a voucher can post to a bill-wise ledger without allocating to a reference, a book
can be fully configured and almost entirely unallocated. On the demo book the residual is
**≈ ₹2.83 crore, about 98.4% of debtor balance** — and it is **invisible to both native
reports**, which return no error.

The residual is recoverable:

```
On Account = ledger CLOSINGBALANCE − Σ BILLCL
```

Verified to the paisa on every bill-carrying party — **at a current as-of only.** This is a
cross-request candidate calculation, not an atomic source cut: a voucher can change between
the report and ledger reads. Bracket the reads with unchanged content/high-water evidence and
retain the non-atomic qualification; no cross-request combination becomes `Verified` solely
because its arithmetic agrees.

**This identity does not hold for a historical as-of.** §7 establishes that a `Ledger`
collection's `CLOSINGBALANCE` ignores the requested window and reflects lifetime activity,
while `BILLCL` comes from the report period. Subtracting them across different time bases
would misclassify later activity as `On Account` and silently overstate the residual. For any
as-of other than now, derive the ledger balance at the same as-of before subtracting. The ledger read costs **0.08 s / 36 KB**
for 88 ledgers. A candidate current-as-of view needs at least both sign-scoped native reports
plus the ledger read, and separate identity bracketing; it must not be described as full
atomic coverage.

**Any outstandings figure that omits this is silently incomplete.** Depending on the residual's
sign, it can understate receivables, overstate them, or hide a payable position. On the observed
debtor-heavy book a native-report-only answer would present **1.6%** of the position as complete.

### 12a.7 The `Company` collection ignores `SVCURRENTCOMPANY` — qualifies §9.11

§9.11's table records a *non-existent* company name returning 0 rows on a `Ledger`
collection. A `Company` collection behaves differently: it **enumerates every loaded
company regardless of `SVCURRENTCOMPANY`**.

So requesting an existing-but-**unloaded** company returns the loaded company's rows, with
`STATUS=1`, in under 100 ms. Reading "row 1" binds to the wrong book. This is the default
state after a Tally restart, which reopens only the last-active company.

The native report path does **not** share this — it fails closed with
`Could not set 'SVCurrentCompany'`.

**No working XML company-load path was observed.** `<TALLYREQUEST>Load Company</TALLYREQUEST>`
returns `STATUS=0` with empty `DATA`; an `SVLOADCOMPANY` variable returns `STATUS=1` with
`<COMPANY>0</COMPANY>`. Those two attempts establish only that these shapes do not load a
company. Until a working supported path is observed, a client must detect the condition and
instruct the operator to open the company on the server by hand.

### 12a.8 Payload scales with voucher count; measure request time separately

The wildcard voucher fetch averaged **~21.7 KB per voucher on this corpus**, stable across a
360× range.

**Treat that as a mean, not an upper bound.** Per-voucher cost varies with content — §11 already
records this — so a book with nested inventory lines or long narrations will exceed it. A segment
projected under a transport cap using this constant can still breach it after dispatch. Sizing
must use **observed** bytes for the profile in hand, with headroom, and split adaptively when a
projection proves wrong.

| AlterID range | vouchers | payload | time | bytes/voucher |
| --- | --- | --- | --- | --- |
| 0..500 | 9 | 0.17 MiB | 1.30 s | 19,482 |
| 0..2000 | 78 | 1.59 MiB | 1.77 s | 21,390 |
| 0..8000 | 369 | 7.63 MiB | 6.72 s | 21,685 |
| 0..20000 | 948 | 19.63 MiB | 9.92 s | 21,713 |
| 2-day window, unbounded | 3,264 | 67.67 MiB | 43.67 s | 21,740 |

**A date window is not a volume bound.** A 2-day window on a dense book returned 67.67 MiB
in 43.67 s; a 31-day window returned 101.5 MiB in 81.3 s. Both exceed every stated response
limit, and both are ordinary requests.

**A pre-flight count reduced response payload on the observed windows.** A minimal `FETCH`
over those windows returned the observed voucher count with about **1/17th** the wildcard
response payload. Its elapsed-time reduction was measured separately and is not a general
cost estimate:

| window | minimal `FETCH` | wildcard | payload / elapsed ratio |
| --- | --- | --- | --- |
| 2-day | 3.98 MB / 2.61 s | 67.67 MiB / 43.67 s | 17× / 17× |
| 31-day | 5.96 MB / 5.74 s | 101.5 MiB / 81.3 s | 17× / 14× |

Incidentally, `FETCH ALTERID` and `FETCH GUID, ALTERID, DATE` return **byte-identical**
responses — Tally emits a fixed minimal envelope regardless of which narrow fields are
named. The wildcard is a **17.8× amplifier** on that baseline.

The count response can itself exceed a response limit on a dense book, or be incomplete while
appearing successful. Bound and completeness-check the count probe before using it; otherwise
recursively partition that probe too. Even then, observed bytes per voucher are a planning
estimate, not a bound. A pre-flight projection does not replace the closed streaming transport
boundary: it must fail closed when encoded response bytes exceed the cap. Planning stays upstream
to avoid dispatching likely-oversize requests; the transport cap remains the final safeguard.

**Elapsed time does not follow the same model, and subdivision does not reduce it.** The cost
has a large fixed component: Tally scans the whole collection regardless of how many rows match.
The 9-row request above took **1.30 s**, not the ~117 ms a purely linear model predicts, and
§11b.3 records a **zero-row** filter taking ~22 s on the large book. So time is closer to
`fixed_scan(book) + linear(rows)`, and **every subdivision pays the fixed cost again** — halving
a partition can increase total wall-clock even as it reduces peak payload.

Deadline planning must therefore model a book-dependent fixed scan cost separately from
serialization, and subdivision must be justified by payload, not by time.

> A separate stability hazard observed during this work is recorded privately rather than
> here, per the project's handling of crash triggers.

### 12a.9 A ledger GUID survived an observed UI rename — coverage must compare GUID to name

`LedgerOpeningCoverageV1` was captured immediately before and after renaming one ledger
through the Tally UI. The response contained the same six ledger GUIDs on both sides, while
exactly one GUID's `Name` changed. Therefore a count-only comparison, and a comparison of
GUID membership alone, are both blind to this master-data change.

Bridge must compare the full GUID-to-name map and return a partial result when it changes
during a scan. This catches the observed rename without treating a stable GUID as evidence
that the ledger master itself stayed unchanged.

This establishes only a GUI rename on the observed TallyPrime Edit Log EDU profile. XML
rename behaviour, other releases, and other configurations remain unverified.

---

## 13. Open questions

| Question | Why it matters |
| --- | --- |
| Does licensed Tally accept arbitrary period boundaries? | Decides whether §5.3 is a lab quirk or a product constraint. **Blocks intermediate window sizing** — Education permits only days 01/02/31, so 5–20 day windows cannot be tested at all |
| Does editing a voucher populate `AUDITENTRIES.LIST`? | Could replace AlterID diffing entirely on the Edit Log SKU |
| Is there a syntax for two-level `FETCH`? | Decides the 6.3× GST payload penalty |
| Why does `ClosingBalance` render empty at some dates? | Blocks trusting any balance field |
| Which report attribute makes rendering work? | Only matters if custom reports are retained |
| Behaviour on standard TallyPrime and Tally.ERP 9 | Every finding here is single-SKU |
| Behaviour with third-party TDL installed | The demo instance has none; client machines will. Could alter the report surface §12a.1 depends on |
| Is the crash observed on 2026-08-02 volume-driven or UI-driven? | It did not reproduce on an identical repeat; a UI keypress during generation is at least as likely. Recorded privately |
| What is the correct key for `ACTION="Alter"` on a voucher? | On automatically numbered types observed, `GUID`, `REMOTEID`, `MASTERID`, and the `REMOTEID`/`MASTERID` combination have each created duplicates (§9.7, §12a.4). Manual + `PREVENTDUPLICATES=Yes` instead rejects the failed Alter (§9.8); other request shapes and licensed-SKU behavior remain untested |
| Does the `On Account` residual identity hold on a bill-dominated book? | Verified on a book that is 98% unallocated; the opposite composition is the case where a false zero would look like success |

---

## 14. Changelog

| Date | Change |
| --- | --- |
| 2026-07-29 | Created. All VERIFIED entries established this day against TallyPrime Edit Log 7.0 EDU. |
| 2026-07-30 | Recorded the outstandings-only wildcard exception, curated bill-type corruption, and the contextual polarity finding. |
| 2026-08-02 | Added §12a from a live measurement session: built-in named reports (qualifying §2.2), per-kind ageing semantics, the two ageing methods, eight import rewrites (extending §9), configuration as a non-diagnostic, the unallocated remainder and its recovery, the `Company` collection ignoring `SVCURRENTCOMPANY` (qualifying §9.11), and a linear volume model with a cheap pre-flight count. |
