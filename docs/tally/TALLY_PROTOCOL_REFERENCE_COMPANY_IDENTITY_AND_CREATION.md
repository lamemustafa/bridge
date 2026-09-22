# Tally XML gateway protocol reference — company identity and creation

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

---

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

Observed: `<GUID><SYNTHETIC-COMPANY-GUID></GUID>`, plus `COMPANYNUMBER`,
`BASICCOMPANYFORMALNAME`, `STARTINGFROM`, `BOOKSFROM`, and object counts
(`NUMGROUPS 28`, `NUMLEDGERS 87`).

Note the **company GUID is the prefix of every master's GUID** in that company — the
`Currency` master's GUID is `<SYNTHETIC-COMPANY-GUID>-0000001d`. This illustrative, redacted excerpt preserves the observed company-prefix relationship. That gives Bridge a cheap
cross-check that a returned master actually belongs to the intended company, which is
exactly the defence §9.11 calls for.

`TYPE=Object` is a documented shape (Tally's own samples use it for a single ledger) and it
was confirmed safe against a known-good `SUBTYPE=Ledger` control before being pointed at
`Company`. **This is the recommended company-identity probe.**

### 9.11b A year-end split can duplicate a company GUID — **VERIFIED; composite identity required**

**VERIFIED 2026-08-26 from captured Company-collection responses.** The pre-split capture
has 14 companies; the post-split capture has 15. The added child retained its parent's `GUID`.
Tally distinguishes the two displayed books with a name suffix, not a new GUID. Across the
post-split capture, `COMPANYNUMBER` and `NAME` each have 15 distinct values, `GUID` has 14,
and `MASTERID` is the constant `29`. Company-level `ALTVCHID` also collided at `2785`; it is
not an identity discriminator.

`COMPANYNUMBER` is the Tally data-folder name. Tally's own migration guidance says an operator
can rename it, and does not document whether a deleted number can later be reused. It must
therefore remain observed provenance within the tuple, not become a Bridge re-key or a claim of
permanent identity on its own.

**Compatibility rule.** Opening a composite-era mirror with a pre-composite Bridge binary is
blocked at startup. The older binary attempts its retired GUID-only migration and the database
rejects it transactionally; operators must restore a compatible binary rather than rolling back
against a schema that can contain same-GUID books.

**Design rule.** Pin a company only by the complete observation
`(canonical_origin, COMPANYNUMBER, GUID, NAME, BOOKSFROM)`. Never migrate an older GUID-only
pin by guessing fields from a current listing: re-observe and review the full tuple. This rule
is implemented by the composite-identity migration and `snapshot_source_pin`.

### 9.11c Extent reads must retain the verified company tuple — **VERIFIED on one endpoint**

**VERIFIED 2026-09-09 field and versioned request observations on one endpoint.**
A paired Company collection extent read returned 16 company rows, including a
same-GUID split pair. Adding only `CompanyNumber` to the existing extent fetch
returned that field on all 16 rows; removing the new field left the other response
fields unchanged. The captured response is retained in
[`native-company-book-extents-with-number.utf8.xml`](../../src-tauri/crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml),
with an adjacent provenance note. Wire numbers contained leading whitespace;
comparison uses the trimmed observed digit string and preserves leading zeros.

**Design rule.** `CompanyBookExtentV2` selects the full verified
`(NAME, GUID, COMPANYNUMBER, BOOKSFROM)` tuple. A same-GUID book with a different
display scope may coexist; a presentation-equivalent sibling, duplicate tuple,
missing or malformed number, or changed tuple refuses admission. Native clients
and snapshot connectors retain the original verified identity for every opening
and closing extent read. Neither GUID-only fallback nor a new database identity
is needed. Paired equality, health checks, and the `ALTMSTID` master-change witness
remain required independently of tuple selection.

A separate single-attempt native runtime observation then used the exact V2
renderer and production extent client. Opening and closing full-tuple extents
agreed; the independently retained V2 paired response had the same decoded bytes
as the field observation. Fresh Company collection and licensed-mode checks
passed around the reads. This establishes the V2 extent boundary on that endpoint,
not a financial report, another endpoint, a compatibility cell, or accounting
writes. V1 remains for historical corpus interpretation; production extent reads
use V2 without fallback.

### 9.11e The audit read's company part is the one admitted Object export — **CODE; live unmeasured**

**Code, 2026-09-21.** Every agent read passes `AgentReadRequest::parse`, which admits
only an `Export` of `TYPE=Collection`. The tally-read v1 read also needs a `company` part
naming exactly one company. A `Company` collection cannot give that (§12a.7), and both
audit consumers take the first `COMPANY` element that has a GUID. So the part is the
single-object export of §9.11a. It reaches dispatch through one typed constructor,
never through `parse`:

- **Construction.** `AgentReadRequest::company_object(&VerifiedCompanyIdentity)`
  renders the pinned `audit_company_object_v1` template (`ReadOnlyProfileId`).
  - The request is `Export`, `TYPE=Object`, `SUBTYPE=Company`,
    `ID TYPE="Name"` = the verified display name (validated and XML-escaped), and the same
    `SVCURRENTCOMPANY`.
  - It carries a `FETCHLIST` of exactly `GUID`, `NAME`, `BOOKSFROM` and `ISINTEGRATED`.
  - It has no TDL, filter, compute, `ORIGINALNAME` or wildcard. A structural test pins
    every element of the rendered request.
- **Everything else stays refused.** `parse` is unchanged and refuses every `TYPE=Object`
  envelope, including this one and its case, whitespace, subtype, fetch and
  Collection-plus-Object variants. So no XML from outside the constructor can use the
  shape.
- **Admission of the response** (`bridge_tally_protocol::audit_company_part`).
  - `HEADER/STATUS` is `1`, and nothing follows the envelope.
  - There is exactly one `DATA/TALLYMESSAGE` and one child `COMPANY`, and no other `COMPANY`
    element carries a `GUID` or a `BOOKSFROM`. The consumers take the first `COMPANY` with a
    GUID anywhere, and the Rust consumer its `BOOKSFROM` from the first `COMPANY` with one,
    so admission ensures both are this one. The `CMPINFO/COMPANY` object counter has neither.
  - The company carries a `NAME` attribute, and `GUID` and `BOOKSFROM` occur exactly once
    each.
  - `GUID` must equal the verified GUID and `BOOKSFROM` the verified date: a year-split
    sibling shares the GUID (§9.11b).
  - `ISINTEGRATED` may be absent or empty, in which case the stock test reports "unknown",
    but it may not repeat. A read field with a child element is refused.
  - `NAME` is recorded, not compared. On licensed Silver 7.1 (2026-09-21) a collection
    returned a display form of a stored ledger name ("Round Off" for "ROUND OFF"), so a
    name comparison could refuse the right company. The GUID and `BOOKSFROM` already bind
    the part.

**Unmeasured, and required before any use on a client book:**

- what an `ID` that does not resolve does, for example a company unloaded between the
  listing and the read: a refusal, an empty response, or the §1.2 modal;
- names with `&`, quotes or non-ASCII text in the `ID` (§9.11a measured one plain name);
- whether an explicit `FETCHLIST` narrows an Object export (only `FETCH *` was measured,
  §9.11a);
- whether two sends of the same Object export are byte-stable, since the `CMPINFO`
  counters could differ;
- the part read while another user is writing to the company. Gold differs from Silver only in
  allowing several users, so a book changing during a read is the normal case there. The read's
  company high-water bracket, not this part, is what refuses a moved book.

The first live use is one request on a synthetic company on the licensed lab, with an
operator watching the Tally screen (§1.2), stopping on the first silence.

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

### 9.11d `SVCURRENTCOMPANY` cannot be trusted as a write guard — **TRAP**

**VERIFIED 2026-09-10 (licensed TallyPrime 7.1 Gold, hand import through the UI).** A
`Vouchers` import carried an `<SVCURRENTCOMPANY>` whose value had two letters of the company
name transposed. It did not match the loaded company. The voucher was **created in the loaded
company** — `CREATED=1`, `ERRORS=0`, `EXCEPTIONS=0`, no `LINEERROR` — and confirmed present in
the day book. 147 further vouchers imported the same way.

**This does not generalise to every mismatched name, and the difference matters.** A separate
measurement (2026-08-19, TallyPrime 7.1, port 9001) sent a voucher import naming a company that
**existed but was not loaded**, and it **failed closed** with
`LINEERROR: Could not set 'SVCurrentCompany' to '<name>'`. Two distinct cases:

| the name refers to | observed | confidence |
|---|---|---|
| a company that exists but is not loaded | fails closed, names the problem | **VERIFIED** |
| a company that matches nothing | imports into the loaded company | **UNVERIFIED** — inferred |

The second row is **not** an observation, and listing it as one is what this
table did before. What was observed is that a name with two letters transposed
imported into the loaded company. That the transposition matched *nothing* is an
inference from its shape; the box's company list was never enumerated, so it may
have matched some other company, or none. Read the paragraph below before citing
this row — and do not let a reader take the row alone.

A plausible reading is that Tally refuses when it can see a company it is being asked to switch
to and cannot, and ignores a name resolving to nothing. **That is a hypothesis.** The 2026-09-10
box's company list was never enumerated, so "matches nothing" is inferred from the transposition,
not established. Settling it needs one session: enumerate the companies, then import twice —
once naming an existing-but-unloaded company, once naming a string known to match nothing.

**What is established, and it is enough to design against:**

1. **`SVCURRENTCOMPANY` is not a guard.** There is a verified case where a name that did not
   match the loaded company still posted into it. It cannot prove a write landed where it was
   aimed, and the failure is invisible — the import succeeds and looks correct.
2. **The dangerous case is the likely one.** A typo, a renamed company or a year-suffixed name
   is the ordinary operator error, and that is the shape that passed. The shape that fails
   closed is the rarer, more deliberate one.
3. **Establish identity before the write and confirm after it.** Read the company and compare
   the GUID (§9.11a), then read the posted voucher back. This is what the read path already
   does via GUID binding; the write path needs the same discipline.

Bridge's own import header carries this element, so none of this is specific to hand-built
files.

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

Verified working prefix (everything up to the currency formal name; the selected Tally data-directory value below is a schematic placeholder, not a tested literal):

```xml
<STATICVARIABLES>
  <SVCURRENTPATH><SELECTED-TALLY-DATA-DIRECTORY></SVCURRENTPATH>
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

#### Correction — 2026-08-23: base-currency fields are Currency-master properties

The negative object-export result above remains valid, but its conclusion was too broad.
`TYPE=Object` / `SUBTYPE=Company` with `<FETCH>*</FETCH>` does **not** emit the base-currency
fields: they are not properties of the `Company` object. It does not establish that Tally never
exports them.

The Currency master collection does return them. The request rendered by
`render_company_currency_request` is exactly `TYPE=Collection` with `<TYPE>Currency</TYPE>` and
fetches `NAME`, `MAILINGNAME`, and `DECIMALPLACES`. Three captures committed on this PR establish
that response shape. On the same licensed TallyPrime Silver 7.1 machine on 2026-08-23, current books
reported `I₹` (U+0049 followed by U+20B9) with `MAILINGNAME` `INR`; older books reported `Rs.`
with `MAILINGNAME` `Indian Rupees`.

The Company Creation formal name remains a property on a different master. That boundary explains
why the 19 earlier probes, sent as `Company` children, all failed; it does not justify treating
the fields as form-local or relying on a Company object export to recover them.

#### 9.10a.1 Multiple Currency-master rows do not identify the base currency

**VERIFIED 2026-08-23, with a deliberately narrow conclusion.** The production Currency
collection request returned two Currency-master rows for the synthetic `BRIDGE CORPUS FOREX`
book. The captured UTF-16LE response is committed as
`currency_multi_live.utf16le.xml` (SHA-256
`b64c0d5feb528fa02f81de576de5c766a95e1da1000975b1e2932868ae34118b`); its provenance records
the production request, a healthy gateway before and after, and the structural count.

This collection returns Currency masters, not a link from one row to the company's base-currency
setting. With more than one row, the read therefore cannot establish which currency may label
monetary figures. Bridge must fail closed rather than infer INR from any individual row. This does
**not** say that the Tally company is misconfigured or that it has more than one base currency;
only that this read cannot establish one. The parser's `currency_count == 1` admission rule and
the party/ledger-master recovery text rely on this boundary. §9.10a.2 adds the read that does
identify the base.

#### 9.10a.2 The company's `CURRENCYNAME` names its base master by `ORIGINALNAME` — **VERIFIED 2026-09-22 on three books; a rule, not a proof**

Measured on licensed TallyPrime 7.1 with plain `Collection` exports: no formula, no filter
(bridge#551).

**Company collection.** Fetching `NAME, GUID, CURRENCYNAME` lists every loaded company, each with a
`CURRENCYNAME`. For the books with several masters:
- `BRIDGE CORPUS FOREX` (INR base, `$` added) reports `₹`.
- A second INR book with a `USD` currency added also reports `₹`.
- A USD-based control book (base `$`, with `₹` added) reports `$`.

`BASECURRENCYSYMBOL`, `BASECURRENCYNAME` and `FORMALNAME` came back empty or absent.

**Currency collection.** With `ORIGINALNAME` added to the fetch:
- On the INR books, master `NAME` `I₹` has `ORIGINALNAME` `₹`, next to `$`/`$` or `UUSD`/`USD`.
- On the control book, `$`/`$` and `I₹`/`₹`.
- A single-master book reads `Rs.`/`Rs.`.

**The match is on `ORIGINALNAME`, not `NAME`.** Matching the company value against master `NAME`
finds nothing on the INR books. Ledgers carry the master `NAME` (`I₹`, `$`).

**Bridge's rule** for a book with more than one master: the base is the one master whose
`ORIGINALNAME` equals the company's `CURRENCYNAME`, character for character.
- Exactly one match whose `MAILINGNAME` is `INR` or `Indian Rupees`: the book is admitted as INR.
- Exactly one match that is not INR: refused as `company_base_currency_not_inr`. Bridge supports
  INR-based books only.
- No match, or more than one: refused as `company_base_currency_undetermined`.

A single-master book keeps the §9.10a.1 rule and sends no extra read. The `Company` read is paired
like the currency read, and a disagreement between the two reads refuses as
`company_base_currency_changed`. The read evidence covers both reads. Trial Balance, the party and
ledger masters, outstandings and the company sweep all apply this one rule.

**What is not established:**
- that `ORIGINALNAME` rather than `NAME` is the match on a book whose base master was never renamed
  (the two fields are then the same);
- that row order, `RESERVEDNAME` or `MASTERID` mean anything. They are never used.
- the rule on any release other than TallyPrime 7.1, and a base symbol changed by a later Company
  Alteration. Either would fail closed unless another master happens to hold the new value;
- how an INR book's foreign-currency ledgers and bills read on every monetary path. Such books are
  now admitted, and only a composite forex `CLOSINGBALANCE` has been captured.

These are exports. §9.10b's trap is `ORIGINALNAME` sent as a `COMPANY` child in an **import**.

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

### 9.12 Item invoices — `ALLLEDGERENTRIES.LIST` is silently DISCARDED — **TRAP**

**PARTIAL 2026-09-10 (licensed TallyPrime 7.1 Gold, hand import through Gateway of Tally >
Import > Vouchers, into a live book).** Read the marker: this document defines VERIFIED as a
captured live request *and response*, and a desktop-UI import produces no gateway response. The
payload and the read-back voucher are the evidence; the HTTP exchange is not. **The same payload
has not been replayed through the XML gateway**, so nothing below establishes what the gateway
returns — only what Tally stores. Everything measured is a stored-state observation and holds as
such.

This is the first item-invoice write recorded here; §5's inventory note still stands for reads,
and units, godowns and batches remain unprobed.

A sales invoice was sent as `ISINVOICE=Yes` / `OBJVIEW="Invoice Voucher View"` with the party and
two GST ledgers in **`ALLLEDGERENTRIES.LIST`** — correct for every accounting voucher, and the
element every other write in this document uses — plus item lines in `ALLINVENTORYENTRIES.LIST`.

Tally **created the voucher** and kept the inventory half exactly as sent: both item lines, the
quantity, the rate, and a service line carrying an amount with no quantity. It **discarded all
three ledger entries**. The stored voucher had a blank party, no CGST, no SGST, and a total of
just the item lines — a one-sided sales voucher, credited to a sales ledger with no debit
anywhere.

**In an invoice voucher the element is `LEDGERENTRIES.LIST`.** Re-sent unchanged except for that
element name, the voucher posted correctly.

This is §9.2's silent-discard family in its widest form yet: not one field, the entire accounting
half. **The UI import summary named no problem** — the discard surfaced only in Tally's own
`Import Exceptions` report, as *"Mismatch in total amount between Credit and Debit entries"*. What
the **gateway** counters report for this payload is untested; do not assume they are silent too.
Either way the §9.2 rule covers it: read the voucher back.

> **RULE (sales item invoices): `ALLLEDGERENTRIES.LIST` for an accounting voucher,
> `LEDGERENTRIES.LIST` for a sales invoice voucher (`ISINVOICE=Yes`). The wrong element is
> accepted, not refused.**

**Scope.** One sales invoice, one company, one Gold instance. The other `ISINVOICE=Yes` shapes —
purchase, debit note, credit note — are **UNVERIFIED** and this section prescribes nothing for
them. There is a reason to expect them to match (the element belongs to the invoice *view*, not to
the voucher type) and that is a hypothesis, not a default.

**Procedure for the first write of an untested invoice type.** It costs one voucher and settles the
question — but it *deliberately risks the silent one-sided write described above*, and its cleanup
depends on a `Delete` that is itself not qualified on a licensed book (§9.12b). So run it where a
bad voucher does not matter:

1. **Use a disposable synthetic company, and take a backup first** — `docs/adr/0004-tally-write-safety.md`
   requires both for an initial write, and this is exactly the case it was written for. If the
   process stops before the deletion, or the delete fails, the malformed voucher stays.

   **Naming the disposable company does not put the write in it.** §9.11d is a verified case of a
   mismatched `<SVCURRENTCOMPANY>` posting into the **loaded** company with `CREATED=1, ERRORS=0,
   EXCEPTIONS=0`. **Which** mismatches behave that way is UNVERIFIED — §9.11d's own provenance note
   calls the "matches nothing" reading a hypothesis, because that instance's company list was never
   enumerated, and a separate measurement had an existing-but-unloaded name fail closed. So do not
   reason about which *kind* of wrong name is dangerous; there is a verified silent case and no rule
   saying when it applies. A probe that deliberately sends a malformed voucher is the worst payload
   to aim at the wrong book, so do what §9.11d's third conclusion already requires and this
   procedure did not:

   - **Immediately before sending**, read the company and compare the **complete identity tuple**
     `(canonical_origin, COMPANYNUMBER, GUID, NAME, BOOKSFROM)` against the disposable company's.
     **Not the GUID alone** — §9.11b is VERIFIED that a year-end split gives the child its parent's
     GUID, so a GUID-only check approves the wrong book of a split pair, and binding the read-back
     to that same non-unique GUID cannot expose the mistake either. Not the name alone, for the
     §9.11d reason above.
   - **Bind the read-back in step 2 to that same tuple.** Reading "the current company" afterwards
     asks a question whose answer may have changed, and a voucher found in the wrong book still
     counts as found.
   - **If any field differs, stop.** Do not switch companies and retry from memory — load the
     disposable company and re-observe the whole tuple.

   **A pre-write check does not bind the write, and calling it "fresh" does not change that.**
   Between the read and the import the loaded company can change — an operator switching books, a
   scheduled task, a second client. Nothing in this procedure closes that window, and an earlier
   draft of this section presented immediacy as though it did. State it as what it is: a residual
   unsafe outcome, narrowed but not removed.

   What actually narrows it, in order of how much they are worth:

   1. **Run the probe on an instance nobody else is using**, with only the disposable company
     loaded, for the duration. Sole control of the box is the only thing here that removes the
     race rather than shrinking it.
   2. **Never run this procedure on a machine that has a customer book loaded at all.** If a
     customer company is open, there is a book for a mis-aimed write to land in. If none is, the
     worst case is a wasted probe.
   3. Keep the gap between the identity read and the send as short as possible, in the same
     session, with no operator interaction in between.

   If (1) and (2) cannot be arranged, **do not run this probe**. The question it answers is worth
   one voucher in a disposable book; it is not worth a malformed voucher in a customer's.
2. Send a single voucher, then read it back. **The read-back is what decides**: if the party and
   tax ledgers are missing and the stored total is the inventory lines alone, the element was
   discarded. `Import Exceptions` *may* also carry
   *"Mismatch in total amount between Credit and Debit entries"* — that was the message on the one
   sales invoice measured, and an absent entry is not evidence that the write succeeded.
3. **If the read-back matches the voucher you intended, you are done.** Record the element and
   stop. Do **not** delete a good voucher to try the other element: the alternate may be the
   discarded shape, and you would be trading a correct voucher for a malformed one.

   **"Party and tax ledgers are present" is not that comparison.** It proves the outer list was
   accepted and nothing more — amounts, signs, bill allocations and inventory fields can still be
   missing or rewritten, and §12a.4 lists eight rewrites that each reported clean counters. Compare
   the read-back against the **intended state** field by field, as
   `docs/adr/0004-tally-write-safety.md` requires. A subset check recorded as "this element works"
   becomes the evidence someone else builds a batch on.
4. **Only if the read-back showed the discard**, remove it with `ACTION="Delete"` by `REMOTEID`
   (§9.12b), re-send with the other element, and **read that back too** — the second attempt is
   as unproven as the first, and stopping after sending it leaves the question open and possibly
   a second bad voucher behind.
5. **Record the answer here, naming which element was tried first and which transport carried
   it** — the desktop UI import or the XML gateway. The element-order note alone is not enough to
   read the result later.

   The observation this rule comes from was a **UI import**, and this section says a few
   paragraphs above that what the *gateway* counters report for this payload is untested. A probe
   sent through the gateway therefore varies two things at once — invoice type **and** transport —
   so its result cannot be entered as a comparable answer to the one-variable question. Either
   use the same UI-import route, or record the transport and keep the conclusion **partial**,
   scoped to the path it was measured on.

   This is the same discipline §9.12b needed and for the same reason: a finding inherits the scope
   of the path that produced it, and a table row that omits the path silently widens it.

**Do not run step 2 against a customer's live book, and do not send a batch of a new invoice type
before that single voucher has been read back.**

**`Import Exceptions` accumulates across imports.** The same report also listed 25 unrelated
`Duplicate Voucher No.` entries from that book's earlier history, and the Gateway was already
flagging data exceptions before the import ran. **Its counts are not attributable to your import**
— read the dates and voucher numbers before concluding anything.

### 9.12a The shape that works

```xml
<VOUCHER VCHTYPE="…" ACTION="Create" OBJVIEW="Invoice Voucher View" REMOTEID="…">
 <DATE>…</DATE><EFFECTIVEDATE>…</EFFECTIVEDATE>
 <VOUCHERTYPENAME>…</VOUCHERTYPENAME><VOUCHERNUMBER>…</VOUCHERNUMBER>
 <PARTYLEDGERNAME>…</PARTYLEDGERNAME><BASICBASEPARTYNAME>…</BASICBASEPARTYNAME>
 <PERSISTEDVIEW>Invoice Voucher View</PERSISTEDVIEW><ISINVOICE>Yes</ISINVOICE>
 <LEDGERENTRIES.LIST>                       <!-- party: debit, negative -->
  <LEDGERNAME>…</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-118.00</AMOUNT>
  <BILLALLOCATIONS.LIST>
   <NAME>…invoice no…</NAME><BILLTYPE>New Ref</BILLTYPE><AMOUNT>-118.00</AMOUNT>
  </BILLALLOCATIONS.LIST>
 </LEDGERENTRIES.LIST>
 <LEDGERENTRIES.LIST>…CGST: credit, positive…<AMOUNT>9.00</AMOUNT></LEDGERENTRIES.LIST>
 <LEDGERENTRIES.LIST>…SGST: credit, positive…<AMOUNT>9.00</AMOUNT></LEDGERENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST>
  <STOCKITEMNAME>…</STOCKITEMNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>
  <RATE>100.00/Nos</RATE><ACTUALQTY>1 Nos</ACTUALQTY><BILLEDQTY>1 Nos</BILLEDQTY>
  <AMOUNT>100.00</AMOUNT>
  <ACCOUNTINGALLOCATIONS.LIST>                <!-- the shape that was observed -->
   <LEDGERNAME>…sales ledger…</LEDGERNAME>
   <ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>100.00</AMOUNT>
  </ACCOUNTINGALLOCATIONS.LIST>
 </ALLINVENTORYENTRIES.LIST>
</VOUCHER>
```

**Every figure above is synthetic** — one line of 100.00 at 9% + 9%, party 118.00, balancing to
zero. The measured voucher's own amounts are not reproduced anywhere in this section. The shape is
what was observed; the numbers are constructed to illustrate it.

Four further observations, each measured:

1. **Each inventory line carried its own `ACCOUNTINGALLOCATIONS.LIST`**, naming the sales ledger
   and repeating the line amount, and the voucher posted.

   **That is the observed working shape, not a proven requirement.** Both attempts in the A/B
   included the nested allocations and changed only the outer ledger-list element, so this records
   that the nested shape is *accepted* — not that every line needs one, and not that a
   voucher-level sales-ledger element cannot work. Encoding the stronger claim would make a future
   writer reject valid shapes or add structure it does not need. A one-variable probe that omits
   or moves the allocation would settle it.
2. **A service line carries an amount and no quantity** — omit `RATE`, `ACTUALQTY` and `BILLEDQTY`
   entirely and the line posts with a blank quantity, matching hand entry.
3. **The party line needs `BILLALLOCATIONS.LIST` / `New Ref`**, or the amount lands On Account and
   cannot be aged (§9.x bill-wise behaviour applies unchanged). **Preflight `ISBILLWISEON=Yes` on
   the party ledger first — it is mandatory, not advisory.** §12a.4 row 5 records that an
   allocation on a ledger with `ISBILLWISEON=No` is *silently discarded* and the entry stores with
   no allocations at all. Sending `New Ref` does not by itself prevent an unaged amount: on a
   non-bill-wise party the invoice posts and the reference is gone.

   Two boundaries on that. The discard is 12a.4's measurement on an **accounting** voucher, not on
   this invoice shape — it is the reason the preflight is mandatory, and it is not a measurement of
   invoice behaviour. And 12a.4's "every one reported `CREATED=1, ERRORS=0, EXCEPTIONS=0`" is a
   **gateway** observation about accounting vouchers; what the counters say for a non-bill-wise
   *invoice*, through either path, is **UNVERIFIED**. Preflight the ledger; do not rely on any
   counter to tell you afterwards.
4. **The stored tax matched the sum of per-line rounded tax, not tax on the total.** The two
   methods can disagree once a line's tax carries a fraction of a paisa — but not always, and the
   difference is what matters rather than the fraction. Two lines of 100.01 at 9% *agree*: each
   9.0009 rounds to 9.00 for 18.00, and 9% of the 200.02 total is 18.0018, which also rounds to
   18.00. Three lines of 100.05 at 9% *disagree*: each 9.0045 rounds to 9.00 for 27.00, while 9%
   of the 300.15 total is 27.0135, which rounds to 27.01. They part company only when the
   accumulated per-line rounding crosses a half-paisa boundary.

   **UNVERIFIED — this is a read-back, not a calculation probe**, and the policy follows from
   that. The A/B recorded here changed only the ledger-list element name. Nothing varied or
   omitted the *supplied* tax, so reading one balanced import back cannot distinguish Tally
   **calculating** tax per line from Tally simply **storing the amount it was given**.

   > **Until that is settled: send the source document's own tax.** If Tally stores what it
   > receives — the possibility this evidence cannot rule out — then recomputing tax per line
   > *replaces* the invoice's figures with different ones wherever the two methods diverge, which
   > is a worse outcome than either rounding convention.
   >
   > **When there is no source figure, this evidence does not tell you which formula to use.** The
   > A/B supplied the tax, so it establishes nothing about how a missing one should be synthesised;
   > per-line and on-total are equally unsupported here. Do not silently pick one — **fail closed
   > and ask**, or record explicitly which convention the run chose so the difference is
   > attributable later. The per-line figure is what the measured voucher ended up holding, which
   > is a reason to prefer it *if you must choose* and not a reason to believe it is right.

   The settling probe is one variable: send a tax amount differing from **both** methods and read
   back what is stored.

   **Do not derive a validation tolerance from any single figure.** The two methods are not always
   apart (two lines of 100.00 at 9% agree), and where they are apart the gap is not bounded by one
   example — each line contributes up to half a paisa, so the worst case grows with the line count.
   Fixed slack is therefore the wrong instrument whatever you validate against.

   **Validate against the preserved source figure, not against a formula.** An earlier version of
   this paragraph said to compare with the per-line sum "which is exact", and that contradicted the
   directive four lines above it: a source invoice that rounds on the total would be flagged by the
   very check meant to protect it — the check rejecting the exact figure this section requires
   sending. Neither formula is established here, so neither can be the reference.

   - **A source figure exists** (the ordinary case): that figure is what was sent, so it is what
     the read-back must equal. Exactly, not within slack — a difference means Tally recalculated,
     which is the open question and worth a stop rather than a tolerance.
   - **No source figure**: a convention was chosen, and choosing one produced **a concrete amount
     that was sent**. Compare the read-back against that amount, exactly, the same as above — this
     is step 3's field-by-field comparison and the tax field is not exempt from it. Skipping it
     lets Tally recalculate or rewrite the figure with nothing noticing, which is the one thing
     this probe exists to detect.

     What stays unverified is the amount's **accounting correctness**, not its round-trip. Those
     are different claims and an earlier draft of this section collapsed them: "we do not know
     whether this figure is right" became "there is nothing to check", which is false the moment a
     number is on the wire. Record which convention was used, and record that the stored value
     matched what was sent.

   Until the settling probe runs, "the tax is correct" is not a claim this document can support;
   "the tax is unchanged from the source" is, and it is the one worth enforcing.

   *The worked figures above are synthetic. They reproduce the arithmetic the measured invoice
   showed; the invoice's own amounts are not reproduced here.*

The whole voucher must still sum to zero across `LEDGERENTRIES` **and** `ALLINVENTORYENTRIES`.

### 9.12b `ACTION="Delete"` by `REMOTEID` — confirmed working on a live book

The one-sided voucher above was removed with `ACTION="Delete"` keyed by the **client-supplied**
`REMOTEID`, and a corrected voucher created in its place. That is the first live confirmation of
the Delete + Create path §9.7 argues for, outside the lab.

Worth noting against `IMPLEMENTATION_GUIDE.md` §3.3a, which records that Tally overwrites the
attribute with a value of its own: the client-supplied string still **worked as a delete key**
afterwards. Whatever the export shows, the value you sent remains addressable.

**`Alter` is UNVERIFIED on this profile — not ruled out.** This experiment exercised only `Delete`.
The `Alter` failures on record (§9.6, and `IMPLEMENTATION_GUIDE.md` §3.1) came from an
Education/Edit Log instance, and §3.1a says itself that every alter attempt targeted vouchers
outside the company's current period, which may be the whole explanation — it calls for an
in-period licensed retest before concluding Alter is unavailable. Nothing here promotes that result
to a licensed conclusion. Use Delete + Create because it is the path that is confirmed, not because
`Alter` is known to fail.

**Whether an operator-entered voucher can be deleted this way is UNVERIFIED.** The intuitive limit
— "no `REMOTEID` you supplied, so no key" — does not follow: `IMPLEMENTATION_GUIDE.md` §3.3a
records that Tally assigns its own `REMOTEID` to every voucher and exports it, and the native
voucher parser requires the attribute on every returned voucher. So an exported Tally-generated
`REMOTEID` is a candidate key that has simply not been tried. **Test a delete against an operator
voucher's exported ID before telling anyone that hand correction is their only option.**

