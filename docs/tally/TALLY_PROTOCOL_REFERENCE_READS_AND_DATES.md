# Tally XML gateway protocol reference — reads and date semantics

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

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
31**. Any other day is silently ignored and the period expands to the entire book. The rule is
literally the set `{1, 2, 31}`, not “month end”: both `20260630` and `20260228` were refused.
Under Education, observed requests for days 15 and 22 were silently aged to the book end.

Twenty-three Education-mode data points, no exceptions:

| Day-of-month | Behaviour |
| --- | --- |
| 1, 2, 31 | honoured — data bounded to request |
| 15, 28, 29, 30 | rejected — period widens to whole book |

**VERIFIED, limited licensed observation (2026-08-22).** On one machine, one TallyPrime
Silver licence tier, one Tally build, and one Bridge Ageing Lab book, arbitrary `SVTODATE`
days were honoured:

| Requested day | Returned span as-of | Result |
| --- | --- | --- |
| 07 | 2026-08-07 | honoured |
| 15 | 2026-06-15 | honoured |
| 22 | 2026-07-22 | honoured |
| 23 | 2026-08-23 | honoured |

This settles #115 item 1: licensed arbitrary-boundary support is no longer unresolved for
that observed TallyPrime Silver profile. It does **not** establish the same result for another
machine, Tally build, licence tier, company, or configuration.

**The failure is invisible from the response.** When the period is silently widened:

- **without** a `<FILTERS>` clause you receive far **too many** rows;
- **with** a `<FILTERS>` predicate bound to `##SVFromDate` / `##SVToDate`, you receive
  **zero** rows, because the refused date variable does not resolve and the predicate excludes
  everything. This observation does not describe a predicate using literal date bounds.

Both return `STATUS=1`. Neither reports an error. A zero-row response is indistinguishable
from a genuinely empty period without corroboration.

**VERIFIED, limited literal-predicate counter-observation (2026-08-21).** Retained request
and response bytes from TallyPrime 7.1 Education, synthetic `WR2 Unicode Lab`, distinguish
that variable-dependent failure from a literal-date predicate. Both requests sent
`SVFROMDATE=20260701` and the refused `SVTODATE=20260830`:

| Predicate bounds | Actual voucher rows | Returned voucher dates | Response bytes | Response SHA-256 |
| --- | --- | --- | --- | --- |
| `$Date >= ##SVFromDate AND $Date <= ##SVToDate` | 0 | none | 3,022 | `49d1cf0c7cf56220fbaa7e2f583a99835f6af74dda7ad807ad54a34277bbff7e` |
| `$Date >= $$Date:"20260701" AND $Date <= $$Date:"20260831"` | 3 | all `20260801` | 9,974 | `544dd8facaffa54263ded46db4f18a75cb940450ca49aacce7f0014b22703b98` |

Both responses carried `STATUS=1`. Rows and dates were counted from complete XML elements,
not substring matches. The literal predicate does not depend on the refused date variable;
this is direct counter-evidence to treating every filtered voucher collection as the zero-row
failure above. Ordinary agent voucher reads and import readback use literal bounds and validate
returned row dates. This differs from a native opening/balance report with no returned span,
whose monetary period still requires current-mode admission.

**Measurement limits:** one Education instance and one synthetic company. The literal request
used `DATE,VOUCHERNUMBER`; the variable request used the broader accounting projection. Its
literal upper bound was `20260831`, one day after the refused static-variable upper bound;
the three observed vouchers fall inside both windows. This was not an otherwise-identical
full-projection comparison, does not qualify every arbitrary-date window or mode transition,
and does not establish that repeating an empty response proves absence. A regression test
protects the literal predicates in both agent renderers; it is not additional live evidence.

**Import verification admission policy:** persisting any `not_found` verdict
requires a fresh observed TallyPrime product and recognised mode before and
after the voucher reads. Release and licence tier are returned facts, not an
independent refusal. An unqualified product or mode must leave the prior proof
and batch status unchanged. Positive historical readback remains available.
Education Journal construction retains its day-1/day-2/day-31 date refusal;
verification uses literal voucher bounds and returned-row validation. This is
not a claim that Gold or another licensed release has completed live import
qualification, nor that Education was observed to reject a literal day-15
predicate.

**Bridge native-outstandings policy.** The paired ledger snapshot is the only exact money
discriminator available after a zero-row Bills response: a book with no named bills and no
party-ledger residual has no balance whose as-of can be misattributed, but any non-zero
`CLOSINGBALANCE - sum(BILLCL)` residual means the requested date is unconfirmed. Bridge must
withhold that report with `native_outstandings_as_of_unconfirmed_without_bill_references`; it
must not call the period honoured merely because no named bill supplied a counter.
Likewise, no bill rows, empty `BILLOVERDUE` counters, and zero-only counters identify no
effective date: a substituted period can produce the same absence or zeros. Bridge withholds
those reports under `native_outstandings_as_of_unconfirmed_without_effective_date_evidence`;
this is a fail-closed policy boundary, not a claim that a particular Tally release always
serializes future-due counters as zero or empty.

**Required discipline, regardless of licence mode:**

1. Compare the returned date span against the requested span on every read. If the span
   exceeds the request, the period was not honoured.
2. Never treat a zero-row window as empty without re-querying a strictly wider window.
3. Never emit a deletion tombstone from an uncorroborated empty result.

**PARTIAL:** one observation does not fit the model — a request with `SVFROMDATE` =
`SVTODATE` = a valid day-1 date returned 75 rows spanning a different year. The `SVTODATE`
rule is well-supported but is not a complete model of period resolution.

### 5.4 Company period fields

**VERIFIED.** `BOOKSFROM` and `STARTINGFROM` give the true data extent. The "current period"
shown on Tally's About screen is a **display** period and does not bound what a filtered
collection can reach.

### 5.5 `OPENINGBALANCE` follows `SVFROMDATE`, not just the ledger master

**VERIFIED — live 2026-08-21, Aarav.** In a native `List of Ledgers` export,
`OPENINGBALANCE` is the opening of the loaded `SVFROMDATE` period, inclusive of the ledger
master's own opening balance. It is not permanently the master opening, and it is not a
date-less/current-display value.

Two ledgers discriminate the candidate meanings:

| Ledger | Master opening | FY24 movement | `SVFROMDATE=20250401` opening | Check |
| --- | ---: | ---: | ---: | --- |
| HDFC Current | 350000.00 | -598044.20 | -248044.20 | `350000.00 + -598044.20 = -248044.20` |
| Petty | 25000.00 | -33960.00 | -8960.00 | `25000.00 + -33960.00 = -8960.00` |

At `FROM=BOOKSFROM`, only 3 of 88 Aarav ledgers were non-zero; the same date-less request had
59 of 88 non-zero. That separates the book-start/master-opening result from the currently loaded
display period. Bridge therefore pins the native master export to `BOOKSFROM..LASTVOUCHERDATE`
and treats its period as an accounting input, not a cosmetic request variable.

**CORRECTION — live 2026-09-06, licensed synthetic lab.** Period opening is
account-dependent; the two cash/bank observations above do not establish a
running-balance rule for every ledger. Three Sales vouchers on 2026-08-01
contributed `306.06`, but that Sales ledger returned native opening `0.00` for
both 2026-08-02 and 2026-09-01. A debtor carried `-102.02` into September, and
Cash carried a 2026-09-01 posting of `-12.50` into 2026-09-02. Paired native
responses and MCP results agreed. These observations corroborate the earlier
account-nature distinction: observed balance-sheet ledgers carry prior balances;
observed nominal ledgers open at zero for the selected period.

For a period movement report, use the observed native opening at the requested
`from`, then apply only direct voucher entries inside the literal window. Do not
add pre-window nominal activity to that opening. The resulting calculated
closing is a period movement figure, not the balance-sheet `CLOSINGBALANCE`
method. This is bounded evidence for the recorded ledgers and host, not universal
report parity. Empty opening values remain unknown, never zero.

**Compatibility boundary.** Education mode can silently refuse a non-01/02/31 `BOOKSFROM` and
load its display period instead. The master response does not carry a returned date span, so this
path cannot apply the voucher reader's I12 span comparison. Before dispatch, Bridge instead uses
a freshly observed endpoint `DateBoundaryProfile`: verified Education evidence rejects unsupported
boundaries; observed licensed mode permits ordinary calendar dates under the limited evidence
above. Native opening, compliance-ledger, and native-outstandings reads require a recognized
current mode and a matching closing mode observation. A missing or stale cached probe cannot
admit these date-dependent balances. Unknown mode is refused. Do not replace this with a global
day-of-month rule or imply that one licensed observation qualifies every installation.

### 5.6 Native Trial Balance fields

**VERIFIED within captured synthetic scope — 2026-08-21 and 2026-09-08.**
For ledger-wise native Trial Balance, request `TBALOPENING`, `DEBITTOTALS`,
`CREDITTOTALS` and `TBALCLOSING` from `List of Ledgers` with both period
boundaries admitted under the observed mode. `CLOSINGBALANCE` is not a substitute:
the captured Profit & Loss row for 1 June–31 July 2026 has native Trial Balance
closing `7000.00`, while the balance-sheet method yields `11027.00`.

The captured native response retains empty amount elements. They are distinct
from an explicit numeric `0.00`; absent required fields are invalid. Signed
amounts use negative debit and positive credit. Opening columns need not net to
zero: the captured opening corpus has an opening net of `-49833.50`. Report that
difference separately; do not manufacture a balancing ledger. Numeric column
sums must disclose any empty observations excluded from arithmetic.

The export includes dormant ledger masters that Tally's rendered report may
omit. Stable paired bytes and bracketed company/mode/book-extent observations
are bounded source evidence, not an atomic snapshot or a completeness proof.
The committed `native/trial_balance_*` fixtures retain complete decoded responses
and provenance. Earlier Education observations established the opening and
closing Trial Balance fields, but did not establish the gross debit and credit
fields used by this four-column request. This runtime therefore admits the
complete native report only on observed licensed TallyPrime. The 8 September
runtime checks exercised six-, eight- and eleven-ledger synthetic companies;
they do not qualify Education mode, other currencies, account types or
production books.

#### Parent follow-up query contract

The retained-capture parent query is an exact row selector, not a qualified
financial group balance. It compares the parsed `PARENT` observation as-is:
returned text is case-preserving and case-sensitive, and a returned empty
string is distinct from `NotObserved` (a `PARENT` element omitted by that
response). It does not infer `Primary`, normalize a hierarchy, or turn either
state into the other.

The captured native fixtures include Tally's invalid numeric control reference
form `&#4; Primary`; XML sanitization preserves it as a replacement-marker
prefixed returned value. That marker remains an exact selectable value and is
not renamed to `Primary`. The null and empty parent cases in the report-level
tests are synthetic mutations that prove this distinction; they are not claims
that those cases were observed in the captured native Trial Balance responses.
The query only summarizes rows already present in the retained capture and
reports numeric totals with their empty-amount counts.

**Local discovery contract.** Parent discovery also uses only the opaque handle
of that retained capture. It returns at most 100 distinct observations, exact
row counts for those observations, and an overflow indicator. The backend
retains only a bounded set of candidates while scanning borrowed capture rows;
it does not build a second index of every distinct parent in the webview.
Discovery runs away from the UI thread and never acquires Tally data.

An empty search preserves source order without creating per-row search strings.
A nonempty search ranks literal raw/display-label matches first, then exact
matches using Unicode lowercase conversion, then lowercase substring matches.
Source order breaks ties. This folding is for discovery only; the subsequent row selector
still receives the original exact observation. Missing and empty fields use
explicit field-state labels, while returned group strings use a `Group:` prefix
in both the selector and report table. Large-capture resource tests are derived
synthetic stress cases, not additional live-Tally or completeness evidence.

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

**CORRECTION — 2026-08-24, licensed TallyPrime Silver 7.1.** The earlier claim that a
`Ledger` collection's `ClosingBalance` was a lifetime figure that ignored the requested window
was wrong. Against `BRIDGE CORPUS DENSE` (29,930 vouchers, 2025-04-01 through 2026-03-31),
`SVTODATE=20250630` returned `-2715644.20` and `SVTODATE=20260331` returned
`-11815459.60`; 122 of 123 ledgers differed, with only empty `Cash` unchanged. The result
held for both `List of Ledgers` with `ISMODIFY="Yes"` (the production shape) and plain
`<TYPE>Ledger</TYPE>` with `ISMODIFY="No"`, so `ISMODIFY` is not the discriminator.

`ClosingBalance` is therefore **as-of scoped** and must be read with the same admitted
`SVFROMDATE`/`SVTODATE` window as the bill report it is reconciled against. The prior four
windows all enclosed every transaction in the observed book; identical balances were the
expected result of that accidental test shape, not evidence that the window was ignored. This
is the same window-mismatch trap corrected on #177.

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
  wildcard exception. **§8.2a below measures a second, narrower loss on an instance
  where §2.4a's corruption does not reproduce, and the cheaper fetch that avoids it.**
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

### 8.1 Ledger-master field availability — **VERIFIED 2026-08-28; field-presence only**

A sequential read-only `List of Ledgers` collection probe against a loaded TallyPrime demo
book requested `NAME`, stable identifiers, `PARENT`, `OPENINGBALANCE`, and the candidate
party/master fields. The response reported success, contained 89 ledger rows and 91,746
encoded bytes, and has recorded SHA-256 `8dd316f8f5fb70c82514bcdec9f1f7e79f876286a7fbc7879633c6714adfb3d0`.
No names, identifiers, or values from the response are retained here.

- `PARTYGSTIN` was returned on 40 rows. Its presence is source evidence, not a reason to
  manufacture a value on rows where it is absent.
- An unfiltered object export showed every candidate tag on all 88 ledger objects, empty
  in this corpus: `INCOMETAXNUMBER`, `NAMEONPAN`, `LEDPINCODE`, `LEDGSTPINCODE`,
  `MSMEREGNUMBER`, `LEDUDYAMREGNUMBER`, `BANKACCHOLDERNAME`, `BANKDETAILS`, `IFSCODE`,
  `EMAIL`, `LEDGERPHONE`, `STATENAME`, and `LEDADDRESS.LIST`. A collection `FETCH` emits
  only values that are set, so absent output is not evidence that a field is unavailable.
  Bridge requests these verified tags but must not manufacture a value when the master is empty.
- This is an observation of this request/profile, not a claim that every Tally build or
  configuration lacks those fields. Product release/version was not observed in this probe.

The raw response is deliberately not committed: a wire capture carrying party identifiers
does not meet this repository's public-fixture privacy boundary. The hash, byte count, and
field-presence observation above preserve the reproducible evidence boundary without copying
client-like data into source control.

### 8.2 `List of Groups` can return a request-computed company GUID — **VERIFIED 2026-08-31; single captured profile only**

A read-only native `List of Groups` collection request against the synthetic
`Aarav Trading Company Demo` on licensed TallyPrime 7.1 requested
`NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME` and added:

```xml
<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>
```

The healthy, 27,140-byte `STATUS=1` response had 28 Group rows. All 28 carried
the selected company's computed GUID; the retained, privacy-screened fixture
hash is `bb2c20f7d9e11634f9cf1f6429f655dc31d50b60fca72c71a6ce981c47db099c`.
The request and capture were bracketed by healthy `/status` checks. The raw
fixture contains no GSTIN, PAN, contact, address, email, phone, website, or
PIN-code fields and intentionally retains Tally's invalid numeric references.

This establishes only the exact request, licensed 7.1 instance, and synthetic
company profile described here. It does **not** establish that other releases,
modes, or Group shapes emit the field; Bridge must continue to fail closed when
the response lacks or mismatches the selected company GUID.

### 8.2a Curated `BILLALLOCATIONS` drops the `On Account` type — **VERIFIED 2026-09-11; single instance**

**Scope: TallyPrime 7.1 Silver, licensed, `education_mode: false`, one company, 144 allocations.**
This does **not** reproduce §2.4a's corruption and does **not** supersede it.

Three FETCH shapes, same instance, same window, same filter:

| Fetch | `New Ref` | `Agst Ref` | `On Account` | `BILLTYPE` absent | Bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| `BILLALLOCATIONS.{NAME,BILLTYPE,AMOUNT}` (curated) | 31 | 1 | **0** | 112 | 150,512 |
| `ALLLEDGERENTRIES.BILLALLOCATIONS.*` | 31 | 1 | **6** | 106 | 168,051 |
| `ALLLEDGERENTRIES.*` (wildcard) | 31 | 1 | **6** | 106 | 1,103,107 |

Named allocations: 32 in all three. So here the curated path does **not** collapse `New Ref` or
`Agst Ref` — all 32 survive intact. What it does is silently omit `BILLTYPE` on the six
`On Account` allocations, which then arrive as **amount-only containers**.

**Why that is worse than a missing field.** An amount-only container is indistinguishable from a
ledger entry that has no allocation at all, and Tally emits such placeholders legitimately. A
reader cannot tell "unattributed money against this entry" from "nothing here" — so the correct
handling of a placeholder, ignoring it, deletes a real allocation and reports nothing. A boundary
that *rejects* untyped rows fails loudly; one that *skips* them, which is right for genuine
placeholders, turns this into silent loss. Both boundaries are defensible alone.

**`ALLLEDGERENTRIES.BILLALLOCATIONS.*` recovers what the entry wildcard recovers**, at **1.12×**
the curated payload against **7.3×**, introducing no element type the parser did not already
receive — the allocation's children are the same set either way.

**But Bridge's agent reads use `ALLLEDGERENTRIES.*` anyway, deliberately.** The narrower shape is
measured equivalent *here* and untested on the instance §2.4a describes, which is the one where
curated allocation paths misreport `New Ref`/`Agst Ref` as `On Account`. The asymmetry decides it:
if the narrow shape is wrong there, a reader silently receives incorrect bill types on compliance
data; if the wide shape costs too much, that is loud, measurable and fixable. An unverified
narrowing is not worth a payload saving when the failure mode is silently-wrong evidence.

Use the narrower shape only where the payload genuinely binds and the instance is known good.
A read that **discards** allocations should fetch neither — Bridge's `ledger_movement` profile
omits them entirely rather than paying for data its result type drops.

**What is still unknown.** Whether `BILLALLOCATIONS.*` also cures §2.4a's `New Ref`/`Agst Ref`
corruption **on the affected instance** is untested — nobody in reach has that book. Until someone
runs this three-way A/B there, §2.4a's rule stands for outstandings specifically: bill-level
outstandings needs `ALLLEDGERENTRIES.*`. This section says only that the allocation wildcard is
strictly more faithful than the curated triple and strictly cheaper than the entry wildcard.

**How the loss was nearly missed**, because the method generalises: a first A/B bucketed
allocations with no `BILLTYPE` as "(none)" on both sides and compared the buckets. The wildcard's
typed `On Account` rows sat inside the curated side's "(none)" pile, the tallies matched exactly,
and the conclusion published was "identical". A comparison whose categories can absorb the
difference cannot detect the difference — count the absent case as its own bucket.

### 8.2b `RESERVEDNAME` is a group's rename-proof identity, and `NAME` is not — **VERIFIED 2026-08-20**

**Why this matters:** any rule of the form "ledgers under Sundry Debtors are parties" or
"ledgers under Bank Accounts hold money" is written against a name a user is free to change.

Three observations on the §8.2 collection, over two synthetic companies (28 and 29 groups) on
TallyPrime 7.1:

1. **Every predefined group carries a non-empty `RESERVEDNAME`, and a user-created group
   carries an empty one.** The empty value is Tally's own positive signal that the row is
   user-created — it is not a missing field, and a reader that never requested the attribute
   at all must be kept distinct from both.
2. **A predefined group can be renamed over XML and `RESERVEDNAME` survives it.** An
   `Import Data` / `All Masters` `<GROUP ACTION="Alter">` carrying a `NAME.LIST` renamed the
   predefined `Suspense A/c` — `ALTERED=1`, group count unchanged, and the readback returned
   `<GROUP NAME="WR5 Renamed Suspense" RESERVEDNAME="Suspense A/c">`. The book was restored
   afterwards and the group set verified identical to the committed fixture. Renaming
   predefined groups is not exotic in books migrated from other software.
3. **`RESERVENAME` is a different field with the opposite meaning.** In the same readback,
   `RESERVEDNAME` held the original predefined identity and `RESERVENAME` held the *current*
   name. They differ by one letter; reaching for the wrong one silently restores the bug.

A group also exposes `PARENTSTRUCTURE` — its whole ancestry chain, separated by raw `U+0003` —
but **ledgers do not**: fetched explicitly against all 88 ledgers of one company it returned
zero occurrences. So a ledger-to-group ancestry walk climbs one `PARENT` hop at a time through
the Group collection; `PARENTSTRUCTURE` is a shortcut for the group tree only. A top-level
group's `PARENT` is the control-marked reserved root of §1.1, not the word `Primary`.

> **RULE: classify a group by `RESERVEDNAME`; treat an empty one as "user-created, keep
> climbing"; treat an absent one as no evidence at all.** §9.13's cash/bank gate is built on
> exactly this.

---

### 8.2c `REFERENCE`, `ISPOSTDATED`, `ISINVOICE`, `PARTYGSTIN` on the voucher `FETCH` — **VERIFIED presence 2026-09-18; `PARTYGSTIN` population UNVERIFIED**

**Scope: TallyPrime 7.1 Silver, licensed, `education_mode: false`, synthetic company
`BRIDGE SHAPE LAB`, twelve one-month voucher windows (2025-04 through 2026-03), 67 vouchers
total.** Requests and responses retained with request/response SHA-256 in the capture manifest.

Adding `REFERENCE,ISPOSTDATED,ISINVOICE,PARTYGSTIN` to the end of the voucher `FETCH` list (after
`ALLLEDGERENTRIES.*`) returned cleanly on every window; none of the four altered the shape of any
other field or the response's `STATUS`. Per field, counted exactly across all 67 vouchers:

- **`REFERENCE`** (`TYPE="String"`, the same shape `NARRATION` uses): populated on 9 of 67 rows,
  every one carrying the literal value `SHAPELAB-MANUAL-1`; empty
  (`<REFERENCE TYPE="String"></REFERENCE>`) on the remaining 58. Both shapes are structurally
  identical to `NARRATION`'s already-handled empty/populated cases.
- **`ISPOSTDATED`** (`TYPE="Logical"`): `No` on 66 rows, `Yes` on 1 (the 2025-06 window). This is
  live corroboration — not just the wire-request change a prior commit made — that this
  release/licence does assert the tag when it is requested; it does not establish that every
  release ever asserts it, which is exactly the case the parser's optional-field handling (empty
  or absent means "not observed", never `false`) defends against.
- **`ISINVOICE`**: unlike the other three logicals here, Tally emits this **without** a
  `TYPE="Logical"` attribute on all 67 rows — always exactly `<ISINVOICE>No</ISINVOICE>` or
  `<ISINVOICE>Yes</ISINVOICE>` (16 `Yes`, 51 `No`), never with a `TYPE` attribute at all. Bridge's
  scalar admission matches on tag name only and never inspects attributes, so the missing `TYPE`
  does not change how it is read.
- **`PARTYGSTIN`** (`TYPE="String"`): the tag is present and well-formed on all 67 rows, but
  **empty on every one** — this synthetic company's party ledgers carry no GSTIN. The capture
  proves the tag round-trips through this FETCH list; it does not establish what a populated
  value looks like on the wire. Treat presence and the empty case as **VERIFIED**, a populated
  value as **UNVERIFIED**.

No fixture in this repository was captured with this FETCH list before this date. `REFERENCE`,
`ISINVOICE` and `PARTYGSTIN` are proven at the parser layer by fault-injecting these exact
observed shapes (empty and `SHAPELAB-MANUAL-1`/`Yes`/`No`) into an existing captured voucher
fixture, following §8.2a's own convention for a shape not yet exercised in the committed corpus.

### 8.2d A ledger's `CURRENCYNAME` is the NAME of the Currency master it is kept in — **VERIFIED 2026-09-23; TallyPrime 7.1, synthetic and client-derived books**

**Scope:** TallyPrime 7.1 Silver, licensed, `education_mode: false`. `CURRENCYNAME` appended to the
`FETCH` of the production `List of Ledgers` snapshot. Two books are captured and committed:
- `BRIDGE CORPUS FOREX` (INR base plus a `$` master), as `ledgers_currency_forex_live`;
- `Bridge Billwise Lab` (one master), as `ledgers_currency_single_live`.

Three client-derived lab copies were read the same way, counts only (see "Every ledger carries the
field" below).

**The field names a master by its NAME, not its ORIGINALNAME.**
- On FOREX, the `$` ledgers carry `$` and the rupee ledgers carry `I₹`, which is the rupee master's
  NAME. Its ORIGINALNAME is `₹`.
- On Billwise every ledger carries `Rs.`, where NAME and ORIGINALNAME are both `Rs.`.
- The base master's NAME therefore differs by book: `I₹`, `₹` or `Rs.` have been seen. A comparison
  must use the NAME read from the same book, never a constant.

**The company's own `CURRENCYNAME` is the base master's ORIGINALNAME, not its NAME.** Measured on
a `Company` collection export with `CURRENCYNAME` in its `FETCH` (2026-09-22; it lists every
loaded company, so it is not committed): `₹` on FOREX, `Rs.` on Billwise. On FOREX that differs
from the `I₹` its ledgers carry. The company field identifies the base master (bridge#551); ledgers
are compared with that master's NAME.

**Every ledger carries the field.** No ledger row in any book read lacked `CURRENCYNAME` or had it
empty:
- FOREX: 10 of 10;
- Billwise: 13 of 13;
- the client-derived copies: 109 of 109 across three books, each with one master.

On each single-master book every value equals that master's NAME.

**Adding the field changes nothing else.** On FOREX the response was otherwise byte-identical to
the same request without it. The same holds for the native Trial Balance request
(`TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING` plus `CURRENCYNAME`): measured on FOREX and
Billwise, not committed.

**Why it matters.** A foreign-currency ledger's bills arrive from the Bills reports as plain
amounts, and a zero foreign balance closes as a plain `0.00` (`FOREX_LEDGER_CAPTURE_PROVENANCE.md`).
Only this field tells such a ledger from a base one.

**Not measured:**
- whether a book with a single Currency master can hold a ledger in another currency. It is inferred
  not to, because Tally assigns a ledger its currency from the masters;
- releases before 7.1.

---

### 8.2e A voucher type's class comes from Tally's class functions, not its display name — **VERIFIED 2026-09-24; TallyPrime 7.1, one synthetic book**

**Scope:** TallyPrime 7.1 Silver, licensed, `education_mode: false`. One synthetic book (bridge#625).
- Its reserved Purchase type was renamed to `PURCHASE A/C`, and its reserved Payment type to
  `PAYMENT A/C`.
- A child type `Purchase Local` was created under the renamed Purchase type.
- A user type named exactly `Purchase` was created under Attendance.
- One child type was created under each of the seven other classes, and one each under
  Reversing Journal and Memorandum.
- Three purchase vouchers were posted: two under the renamed type, one under the child.

**What is committed.** Only the class-resolving voucher read's response is committed, as
`native-vouchers-renamed-purchase-class`. It establishes, at row level, Purchase alone (three
rows). Everything else below was measured on 2026-09-24 as one request or write at a time with
the response inspected, but those captures are not committed.

**A renamed reserved type keeps its identity.** Measured, captures not committed.
- It keeps its `RESERVEDNAME` and GUID, and stays self-parented under its new `NAME`.
- A child's `PARENT` is the parent's new name.
- A voucher carries its type only by display name (`VOUCHERTYPENAME` and `@VCHTYPE`). That name
  follows a rename at once.
- A rename moves the company's `ALTMSTID` and the type's `ALTERID`, but no voucher `ALTERID` and
  not `ALTVCHID`.
- Moving a type with vouchers under another class by gateway `Alter` is refused in-band: "Cannot
  change Type of Voucher!".

**Class, per voucher row, in the same response.** Each is a COMPUTE on the voucher collection.

`$GUID:VoucherType:$VoucherTypeName` (committed):
- It returns the row's type GUID, company-prefixed like other masters.

`$ReservedName:VoucherType:$VoucherTypeName` (committed):
- It returns the type's own `RESERVEDNAME`.
- That is empty for a child type, so on its own it misses children.

`$$Is<Class>:<type name>` for Sales, Purchase, Payment, Receipt, Contra, Journal, Debit Note and
Credit Note. Row-level `Yes` is committed for Purchase; the rest was measured, not committed.
- **Reserved types.** Each function answers `Yes` on its own class's reserved type, including
  Payment under its renamed name. The other seven functions answer `No` on that type.
- **Types of no class.** Every function answers `No` on the Attendance type named `Purchase`.
- **Children.** Every child type answers `Yes` to exactly its parent's class and `No` to the other
  seven, for all eight classes. The Payment child sits under the renamed `PAYMENT A/C`.
- **Other reserved types.** On the 16 other reserved types (Attendance, Delivery Note, Job Work In
  and Out Order, Material In and Out, Memorandum, Payroll, Physical Stock, Purchase Order, Receipt
  Note, Rejections In and Out, Reversing Journal, Sales Order, Stock Journal), all eight functions
  answer `No`. So do children created under Reversing Journal and Memorandum.
- The values carry `TYPE="Logical"`.

**A type name is resolved ignoring case.** `$GUID:VoucherType:"purchase local"` and
`"Purchase Local"` return the same GUID, and `$$IsPurchase:"purchase local"` answers `Yes`. This was
measured for ASCII letters only; non-ASCII case folding is unmeasured.

**Two traps.**
- `$$Is<Class>` on a name that no type carries returns `No`, not an error. So it cannot prove a type
  exists, and the GUID COMPUTE is what does that.
- An unknown `$$` function omits its element from every row, rather than returning `No`. A reader
  must treat a missing element as a refusal.

**Not measured:**
- grandchild types (a child of a child);
- whether a voucher type copied into the book from another company keeps a foreign GUID prefix.
  Bridge refuses a row whose type GUID is not the company's (`voucher_type_unresolved`), so such a
  book would fail loudly on a type-filtered read, not answer wrongly;
- releases before 7.1.

---

### 8.3 GST duty head — the vocabulary is irregular and `TAXTYPE` qualifies it — **VERIFIED 2026-09-12; single instance**

**Scope: TallyPrime 7.1 Silver, licensed, one company, 28 ledger masters.** Captured from
`List of Ledgers` with `FETCH … TAXTYPE, GSTDUTYHEAD`, retained as
`tests/fixtures/agent/native-ledger-masters-duty-heads.utf16le.xml`.

**The measured vocabulary, verbatim on the wire:**

| `GSTDUTYHEAD` | meaning |
| --- | --- |
| `CGST` | central tax |
| `IGST` | integrated tax |
| `State Tax` | state tax — **NOT** `SGST` |
| `UT Tax` | union-territory tax |
| `Cess` | cess |

`SGST` never appears. The state head is spelled `State Tax`, which is why the set is enumerated
rather than pattern-matched, and why an unrecognised spelling is surfaced with its raw value
instead of being normalised into a neighbour.

**`TAXTYPE` qualifies the head and the two can contradict.** Four states, and all four are
distinguishable only because both fields are read:

| `TAXTYPE` | `GSTDUTYHEAD` | classification |
| --- | --- | --- |
| `GST`, or not observed | one of the five | recognised |
| `GST`, or not observed | anything else, non-empty | unrecognised, raw value retained |
| observed, non-`GST` (e.g. `Others`) | absent or empty | not a tax ledger |
| observed, non-`GST` | non-empty | **contradictory — neither is asserted** |

The last row is a response contradicting itself. Classifying head-first recognises it and never
consults `TAXTYPE`, which releases the contradiction as valid compliance data. Only an **observed**
non-`GST` tax type contradicts: an absent or empty `TAXTYPE` is not evidence that the ledger is
non-GST, and treating it as such would refuse real GST ledgers on any version that omits the field.

**Absence has two wire shapes and they mean the same thing.** This instance **omits**
`GSTDUTYHEAD` entirely for non-GST ledgers — 0 self-closing elements across 28 masters — while a
committed capture elsewhere in the repository carries `<GSTDUTYHEAD/>`. A reader that treats only
one shape as absent classifies ordinary ledgers wrongly on the other.

**Nested markup is refused, not flattened.** A scalar reader that counts depth and concatenates
child text turns `<GSTDUTYHEAD><VALUE>CGST</VALUE></GSTDUTYHEAD>` into a recognised `CGST`. Both
fields here are read with a scalar reader that rejects any child element, because an unexpected
response shape must fail at the boundary rather than become compliance data.

**The head is settable at CREATE and silently not settable at ALTER.** Measured both ways. An
`ACTION="Create"` master import carrying `TAXTYPE` and `GSTDUTYHEAD` returns `CREATED=2 ALTERED=0
ERRORS=0` and the values read back set. An `ACTION="Alter"` against an existing ledger returns
`ALTERED=1 ERRORS=0` — a success — and the field stays empty. An earlier note of ours recorded only
the second and concluded the head "cannot be set by import", which was an alter-time observation
written as an import-time rule.

**Not established:** whether these five spellings hold across Tally versions or localisations. The
capture is one instance. An unrecognised value is therefore surfaced, never guessed.

