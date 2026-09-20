# Tally XML gateway protocol reference — environment and request shapes

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

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

#### (d) The marking rule Bridge applies before parsing

This is a rule Bridge chose, not an observation of Tally. `bridge-tally-protocol` exposes it as
`mark_forbidden_numeric_references`. Its native collection parsers, the standard `List of
Ledgers` catalogue, the Bridge-schema group and voucher-type parser, and every agent-facing parser
in the app apply it to decoded text before an XML parser sees it, so one wire text has one
spelling in all of them.

- A decimal or hexadecimal (`x` or `X`) numeric reference to a code point XML 1.0 forbids (a C0
  control other than tab, LF and CR, a surrogate, U+FFFE, U+FFFF, or beyond U+10FFFF) becomes the
  literal text U+FFFD `#` *n* `;`, with *n* in decimal. `&#4; Primary` reads as
  `U+FFFD#4; Primary`; that prefix is the crate's `TALLY_SANITIZED_ROOT_MARKER`.
- A U+FFFD already present, literal or as a legal reference, that is directly followed by `#`,
  digits and `;` becomes U+FFFD `#65533;`. Every marker then stands for exactly one source atom, so
  the rewrite is reversible: `U+FFFD#65533;` reads back as U+FFFD and `U+FFFD#`*n*`;` as the
  reference to *n*.
- Everything else passes through unchanged: legal references, raw characters including raw C0
  controls (Tally sends raw U+0003 in `PARENTSTRUCTURE`), and a `&#` whose `;` is not within the
  twelve bytes after it. The scan continues past such a `&#`, so a later forbidden reference is
  still marked.
- The rewrite refuses nothing. A consumer of decoded Tally text refuses a raw U+0000, U+FFFE or
  U+FFFF itself; a NUL usually means the bytes were decoded with the wrong encoding.

The twelve-byte window fits `#`, ten digits and `;`. A reference padded with leading zeros beyond
that is left to the XML parser. quick-xml, the parser this crate uses, resolves a padded
reference to a C0 control character, U+FFFE or U+FFFF to the raw character instead of refusing
it; it refuses U+0000, surrogates and code points above U+10FFFF, which are not valid characters
(`quick-xml` 0.41 `escape.rs`, `parse_number`). None of the committed fixtures contains a padded reference or an unterminated `&#`.

**The reserved root.** `is_tally_reserved_root` accepts only the marked form: after trimming, the
text starts with `U+FFFD#4;` and the rest, trimmed again, is `Primary` (ASCII case ignored). A
bare `Primary` names a group a user called that, and an ancestry walk passes through it like any
other group (§8.2b). The same marker prefixes Tally's other reserved values (`&#4; Resave`,
`&#4; Not Applicable`), which are not the root.

**Parsers the rule does not cover yet.** The company-list and gateway-capability parsers,
`export_status`, the Bridge-schema ledger, voucher and period-balance report parsers, and the
import-outcome parser still unescape a forbidden reference to its raw character. None of their
outputs reaches a reserved-root test.

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

### 3.1 Observing the running release and licence tier

**VERIFIED 2026-09-06; one synthetic TallyPrime Silver 7.1 endpoint.** Adding
only `<COMPUTE>BridgeRelease : @@VersionReleaseString</COMPUTE>` to the existing
native `CompanyListV2` collection returned `BRIDGERELEASE` = `7.1` for all 16
loaded companies. The same rows returned `PRODUCTNAME` = `TallyPrime`,
`EDUMODE` = `No`, `SILVER` = `Yes`, and `GOLD` = `No`. Two independent calls
returned identical 20,412-byte UTF-16LE bodies, SHA-256
`998280fac5b6b40b23ee3879dcb35780bc439083a4d1a3a3fb9a64231adbaa50`.
The 2,524-byte request SHA-256 was
`9df2a53f085dac2636e9435462b612c1487ec6f903677815036c9f39163f7dd8`.
The unchanged captured response and metadata are retained in
`src-tauri/crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.*`.

The expression was a discovery lead from the upstream
[TallyConnector implementation](https://github.com/Accounting-Companion/TallyConnector/blob/03c7f4f53914bcbf017849c8e268154cffcfd76e/src/TallyConnector/Services/BaseTallyService.cs).
The live bytes establish this observation; the upstream code is not proof of
Bridge compatibility. No licence serial, account identifier, machine path,
report definition, or new dispatch path was needed.

**Observed-profile boundary.** Preserve an unobserved release as unknown. A
missing, empty, or disagreeing release across company rows cannot identify a
release. Silver or Gold is known only when Education is false and exactly one
tier flag is true. The observed label does not establish other releases,
customised TDL, Gold, ERP9, Edit Log, or every voucher configuration. It does
not promote any compatibility-matrix claim.

The former TallyPrime Silver 7.1 admission rule was retired on 2026-09-07. The
current runtime requires a fresh observed TallyPrime product and recognised
mode for each operation; it retains release and tier as returned facts rather
than categorical exclusions. Education uses its observed day-1/day-2/day-31
native boundary profile and refuses an unsupported operation date. Licensed
mode permits ordinary dates conditionally, while the paired reads, identity,
strict amounts, literal voucher bounds and operation-specific validation remain
mandatory. The limited Silver observation below does not qualify Gold or a
different licensed release; a Gold live monetary read is still missing.

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

**UNVERIFIED:** godowns, cost centres, currencies, units, budgets — never probed. Stock items
are unprobed *as a collection read*; for writing them inside an invoice see §9.12.

---

