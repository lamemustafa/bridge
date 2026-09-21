# Tally XML gateway — protocol reference

<!-- protocol-reference-parts: TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md | TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md | TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md | TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md | TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md | TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md -->

> **Adding or renumbering a section?** Several branches extend this reference at once and a number is
> invisible to them until it merges. Two branches have already collided, and `1.2` is used twice
> below. `scripts/check-protocol-section-numbers.mjs` fails CI on a duplicate; see
> [`SECTION-REGISTER.md`](./SECTION-REGISTER.md) for what it does and does not guarantee.
> Editing this index or any part also stales its compatibility-surface pin; the reseal procedure is in
> [`docs/release-process.md`](../release-process.md#compatibility-surface-reseal).

**Purpose.** The reference set is the single source of truth for how Tally's XML gateway actually
behaves, as observed against a live instance. Everything in the parts is either **VERIFIED**
against a real Tally or explicitly marked otherwise. Plan documents state intent; this reference
wins where they disagree — or one of them is stale and should be fixed.

**How to use.** Read the confidence marker before relying on any statement. Add findings with their
evidence and date. Never add an unverified claim without marking it.

| Marker | Meaning |
| --- | --- |
| **VERIFIED** | Observed directly against a live Tally. Request and response captured. |
| **PARTIAL** | Observed, but the rule behind it is not fully established. |
| **UNVERIFIED** | Believed, assumed, or documented elsewhere — never tested here. |

## Reference parts

| Part | Sections |
| --- | --- |
| [Tally XML gateway protocol reference — environment and request shapes](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md) | `0. Observation environment` onward |
| [Tally XML gateway protocol reference — reads and date semantics](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md) | `5. Date and period semantics` onward |
| [Tally XML gateway protocol reference — write responses and master identity](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md) | `9. Writes (import)` onward |
| [Tally XML gateway protocol reference — voucher writes](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md) | `9.5 Identity after write` onward |
| [Tally XML gateway protocol reference — company identity and creation](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md) | `1.2 A modal error dialog` onward |
| [Tally XML gateway protocol reference — measurements and open questions](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md) | `10. Change detection` onward |

## Legacy section anchors

This canonical index retains the GitHub fragment identifiers formerly emitted by every section
heading, including nested headings. Existing links to
`TALLY_PROTOCOL_REFERENCE.md#...` therefore keep resolving after the material moved into the
parts above. The section-number gate verifies the complete anchor set against every part and the
base revision.

<a id="0-observation-environment"></a>

[0. Observation environment](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#0-observation-environment)
<a id="1-transport"></a>

[1. Transport](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#1-transport)
<a id="11-encoding--tally-emits-xml-that-strict-parsers-reject-this-is-a-p0-for-the-read-path"></a>

[1.1 Encoding — **Tally emits XML that strict parsers reject. This is a P0 for the read path.**](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#11-encoding--tally-emits-xml-that-strict-parsers-reject-this-is-a-p0-for-the-read-path)
<a id="a-tallys-output-is-not-valid-xml--confirmed"></a>

[(a) Tally's output is NOT valid XML — confirmed](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#a-tallys-output-is-not-valid-xml--confirmed)
<a id="b-data-that-bridge-writes-round-trips-byte-exact--also-confirmed"></a>

[(b) Data that Bridge writes round-trips byte-exact — also confirmed](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#b-data-that-bridge-writes-round-trips-byte-exact--also-confirmed)
<a id="c-but-pre-existing-tally-held-data-can-be-lossy"></a>

[(c) But pre-existing Tally-held data can be lossy](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#c-but-pre-existing-tally-held-data-can-be-lossy)
<a id="method-note--how-the-earlier-wrong-conclusion-happened"></a>

[Method note — how the earlier wrong conclusion happened](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#method-note--how-the-earlier-wrong-conclusion-happened)
<a id="d-the-marking-rule-bridge-applies-before-parsing"></a>

[(d) The marking rule Bridge applies before parsing](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#d-the-marking-rule-bridge-applies-before-parsing)
<a id="12-request-charset-controls-response-charset"></a>

[1.2 Request charset controls response charset](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#12-request-charset-controls-response-charset)
<a id="2-two-request-families--and-why-it-matters"></a>

[2. Two request families — and why it matters](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#2-two-request-families--and-why-it-matters)
<a id="21-collection-export--typecollectiontype--recommended"></a>

[2.1 Collection export — `<TYPE>Collection</TYPE>` — **RECOMMENDED**](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#21-collection-export--typecollectiontype--recommended)
<a id="22-report-export--typedatatype-with-a-custom-reportformpart--avoid"></a>

[2.2 Report export — `<TYPE>Data</TYPE>` with a custom `REPORT`/`FORM`/`PART` — **AVOID**](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#22-report-export--typedatatype-with-a-custom-reportformpart--avoid)
<a id="3-the-working-request-template"></a>

[3. The working request template](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#3-the-working-request-template)
<a id="31-observing-the-running-release-and-licence-tier"></a>

[3.1 Observing the running release and licence tier](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#31-observing-the-running-release-and-licence-tier)
<a id="4-object-types"></a>

[4. Object types](./TALLY_PROTOCOL_REFERENCE_ENVIRONMENT_AND_REQUESTS.md#4-object-types)
<a id="5-date-and-period-semantics--the-most-dangerous-area"></a>

[5. Date and period semantics — the most dangerous area](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#5-date-and-period-semantics--the-most-dangerous-area)
<a id="51-svfromdate--svtodate-do-not-filter-collection-membership"></a>

[5.1 `SVFROMDATE` / `SVTODATE` do not filter collection membership](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#51-svfromdate--svtodate-do-not-filter-collection-membership)
<a id="52-a-filters-predicate-does-bound-correctly"></a>

[5.2 A `<FILTERS>` predicate does bound correctly](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#52-a-filters-predicate-does-bound-correctly)
<a id="53-rejected-period-boundaries-widen-silently--the-key-trap"></a>

[5.3 Rejected period boundaries widen silently — **THE KEY TRAP**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#53-rejected-period-boundaries-widen-silently--the-key-trap)
<a id="54-company-period-fields"></a>

[5.4 Company period fields](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#54-company-period-fields)
<a id="55-openingbalance-follows-svfromdate-not-just-the-ledger-master"></a>

[5.5 `OPENINGBALANCE` follows `SVFROMDATE`, not just the ledger master](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#55-openingbalance-follows-svfromdate-not-just-the-ledger-master)
<a id="56-native-trial-balance-fields"></a>

[5.6 Native Trial Balance fields](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#56-native-trial-balance-fields)
<a id="parent-follow-up-query-contract"></a>

[Parent follow-up query contract](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#parent-follow-up-query-contract)
<a id="6-crashes-and-rendering-traps"></a>

[6. Crashes and rendering traps](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#6-crashes-and-rendering-traps)
<a id="61--functions-and-identifiers-containing-spaces"></a>

[6.1 `$$` functions and identifiers containing spaces](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#61--functions-and-identifiers-containing-spaces)
<a id="62-custom-reports-need-display-geometry"></a>

[6.2 Custom reports need display geometry](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#62-custom-reports-need-display-geometry)
<a id="7-balances"></a>

[7. Balances](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#7-balances)
<a id="8-fetch-semantics"></a>

[8. `FETCH` semantics](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#8-fetch-semantics)
<a id="81-ledger-master-field-availability--verified-2026-08-28-field-presence-only"></a>

[8.1 Ledger-master field availability — **VERIFIED 2026-08-28; field-presence only**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#81-ledger-master-field-availability--verified-2026-08-28-field-presence-only)
<a id="82-list-of-groups-can-return-a-request-computed-company-guid--verified-2026-08-31-single-captured-profile-only"></a>

[8.2 `List of Groups` can return a request-computed company GUID — **VERIFIED 2026-08-31; single captured profile only**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#82-list-of-groups-can-return-a-request-computed-company-guid--verified-2026-08-31-single-captured-profile-only)
<a id="82a-curated-billallocations-drops-the-on-account-type--verified-2026-09-11-single-instance"></a>

[8.2a Curated `BILLALLOCATIONS` drops the `On Account` type — **VERIFIED 2026-09-11; single instance**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#82a-curated-billallocations-drops-the-on-account-type--verified-2026-09-11-single-instance)
<a id="82b-reservedname-is-a-groups-rename-proof-identity-and-name-is-not--verified-2026-08-20"></a>

[8.2b `RESERVEDNAME` is a group's rename-proof identity, and `NAME` is not — **VERIFIED 2026-08-20**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#82b-reservedname-is-a-groups-rename-proof-identity-and-name-is-not--verified-2026-08-20)
<a id="82c-reference-ispostdated-isinvoice-partygstin-on-the-voucher-fetch--verified-presence-2026-09-18-partygstin-population-unverified"></a>

[8.2c `REFERENCE`, `ISPOSTDATED`, `ISINVOICE`, `PARTYGSTIN` on the voucher `FETCH` — **VERIFIED presence 2026-09-18; `PARTYGSTIN` population UNVERIFIED**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#82c-reference-ispostdated-isinvoice-partygstin-on-the-voucher-fetch--verified-presence-2026-09-18-partygstin-population-unverified)
<a id="83-gst-duty-head--the-vocabulary-is-irregular-and-taxtype-qualifies-it--verified-2026-09-12-single-instance"></a>

[8.3 GST duty head — the vocabulary is irregular and `TAXTYPE` qualifies it — **VERIFIED 2026-09-12; single instance**](./TALLY_PROTOCOL_REFERENCE_READS_AND_DATES.md#83-gst-duty-head--the-vocabulary-is-irregular-and-taxtype-qualifies-it--verified-2026-09-12-single-instance)
<a id="9-writes-import"></a>

[9. Writes (import)](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#9-writes-import)
<a id="91-response-shape"></a>

[9.1 Response shape](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#91-response-shape)
<a id="91a-a-malformed-request-returns-a-counter-less-response--fourth-response-shape"></a>

[9.1a A malformed request returns a counter-less response — **fourth response shape**](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#91a-a-malformed-request-returns-a-counter-less-response--fourth-response-shape)
<a id="91b-xml-escaping-is-mandatory--a-default-tally-group-breaks-naive-builders"></a>

[9.1b XML escaping is mandatory — a default Tally group breaks naive builders](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#91b-xml-escaping-is-mandatory--a-default-tally-group-breaks-naive-builders)
<a id="92-errors0-does-not-mean-success--trap"></a>

[9.2 `ERRORS=0` does not mean success — **TRAP**](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#92-errors0-does-not-mean-success--trap)
<a id="93-voucher-idempotency-depends-on-remoteid--this-sections-title-used-to-say-the-opposite"></a>

[9.3 Voucher idempotency depends on `REMOTEID` — **this section's title used to say the opposite**](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#93-voucher-idempotency-depends-on-remoteid--this-sections-title-used-to-say-the-opposite)
<a id="94-master-re-create-is-a-silent-alter"></a>

[9.4 Master re-create is a silent Alter](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#94-master-re-create-is-a-silent-alter)
<a id="94a-a-partial-ledger-alter-preserves-the-omitted-party-gstin"></a>

[9.4a A partial ledger `Alter` preserves the omitted Party GSTIN](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#94a-a-partial-ledger-alter-preserves-the-omitted-party-gstin)
<a id="94b-master-name-matching-case--and-separator-insensitive-otherwise-exact"></a>

[9.4b Master-name matching: case- and separator-insensitive, otherwise exact](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#94b-master-name-matching-case--and-separator-insensitive-otherwise-exact)
<a id="94d-master-name-matching-on-licensed-tallyprime-71"></a>

[9.4d Master-name matching on **licensed** TallyPrime 7.1](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#94d-master-name-matching-on-licensed-tallyprime-71)
<a id="94c-real-catalogues-carry-families-a-partial-name-cannot-separate"></a>

[9.4c Real catalogues carry families a partial name cannot separate](./TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md#94c-real-catalogues-carry-families-a-partial-name-cannot-separate)
<a id="95-identity-after-write"></a>

[9.5 Identity after write](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#95-identity-after-write)
<a id="98-voucher-numbering-method-changes-everything--use-manual"></a>

[9.8 Voucher numbering method changes everything — **use Manual**](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#98-voucher-numbering-method-changes-everything--use-manual)
<a id="913-payment-receipt-and-contra--the-bank-statement-voucher-shapes"></a>

[9.13 Payment, Receipt and Contra — the bank-statement voucher shapes](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#913-payment-receipt-and-contra--the-bank-statement-voucher-shapes)
<a id="99-bulk-import-throughput"></a>

[9.9 Bulk import throughput](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#99-bulk-import-throughput)
<a id="97-operation-support-matrix--vouchers-cannot-be-modified"></a>

[9.7 Operation support matrix — **vouchers cannot be modified**](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#97-operation-support-matrix--vouchers-cannot-be-modified)
<a id="96-actioncancel-by-remoteid-creates-a-new-voucher--it-does-not-cancel--trap"></a>

[9.6 `ACTION="Cancel"` by `REMOTEID` creates a new voucher — it does not cancel — **TRAP**](./TALLY_PROTOCOL_REFERENCE_VOUCHER_WRITES.md#96-actioncancel-by-remoteid-creates-a-new-voucher--it-does-not-cancel--trap)
<a id="12-a-modal-error-dialog-in-tallys-ui-blocks-the-gateway-until-a-human-clicks-ok--p0-operationally"></a>

[1.2 A modal error dialog in Tally's UI blocks the gateway until a human clicks OK — **P0 operationally**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#12-a-modal-error-dialog-in-tallys-ui-blocks-the-gateway-until-a-human-clicks-ok--p0-operationally)
<a id="911-company-pinning--reads-and-writes-behave-differently--trap"></a>

[9.11 Company pinning — reads and writes behave differently — **TRAP**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911-company-pinning--reads-and-writes-behave-differently--trap)
<a id="911a-how-to-read-the-company-guid--verified-and-this-closes-911s-requirement"></a>

[9.11a How to read the company GUID — **VERIFIED, and this closes §9.11's requirement**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911a-how-to-read-the-company-guid--verified-and-this-closes-911s-requirement)
<a id="911b-a-year-end-split-can-duplicate-a-company-guid--verified-composite-identity-required"></a>

[9.11b A year-end split can duplicate a company GUID — **VERIFIED; composite identity required**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911b-a-year-end-split-can-duplicate-a-company-guid--verified-composite-identity-required)
<a id="911c-extent-reads-must-retain-the-verified-company-tuple--verified-on-one-endpoint"></a>

[9.11c Extent reads must retain the verified company tuple — **VERIFIED on one endpoint**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911c-extent-reads-must-retain-the-verified-company-tuple--verified-on-one-endpoint)
<a id="911e-the-audit-reads-company-part-is-the-one-admitted-object-export--code-live-unmeasured"></a>

[9.11e The audit read's company part is the one admitted Object export — **CODE; live unmeasured**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911e-the-audit-reads-company-part-is-the-one-admitted-object-export--code-live-unmeasured)
<a id="910-company-creation-over-xml--partial-symbol-element-found-formal-name-element-not"></a>

[9.10 Company creation over XML — **PARTIAL: symbol element found, formal-name element not**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910-company-creation-over-xml--partial-symbol-element-found-formal-name-element-not)
<a id="911d-svcurrentcompany-cannot-be-trusted-as-a-write-guard--trap"></a>

[9.11d `SVCURRENTCOMPANY` cannot be trusted as a write guard — **TRAP**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#911d-svcurrentcompany-cannot-be-trusted-as-a-write-guard--trap)
<a id="910a-second-pass-2026-07-30--path-and-financial-year-solved-currency-formal-name-still-open"></a>

[9.10a Second pass 2026-07-30 — path and financial year solved, currency formal name still open](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910a-second-pass-2026-07-30--path-and-financial-year-solved-currency-formal-name-still-open)
<a id="correction--2026-08-23-base-currency-fields-are-currency-master-properties"></a>

[Correction — 2026-08-23: base-currency fields are Currency-master properties](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#correction--2026-08-23-base-currency-fields-are-currency-master-properties)
<a id="910a1-multiple-currency-master-rows-do-not-identify-the-base-currency"></a>

[9.10a.1 Multiple Currency-master rows do not identify the base currency](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910a1-multiple-currency-master-rows-do-not-identify-the-base-currency)
<a id="910b-originalname-at-company-level-hangs-the-gateway--trap"></a>

[9.10b `ORIGINALNAME` at `COMPANY` level hangs the gateway — **TRAP**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910b-originalname-at-company-level-hangs-the-gateway--trap)
<a id="it-raised-the-12-modal-dialog-and-blocked-for-44-minutes-until-a-human-clicked-ok"></a>

[It raised the §1.2 modal dialog and blocked for 44 minutes until a human clicked OK](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#it-raised-the-12-modal-dialog-and-blocked-for-44-minutes-until-a-human-clicked-ok)
<a id="why-this-shape-reached-the-path-code-at-all"></a>

[Why this shape reached the path code at all](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#why-this-shape-reached-the-path-code-at-all)
<a id="910c-routes-ruled-out-by-documentation-review"></a>

[9.10c Routes ruled out by documentation review](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910c-routes-ruled-out-by-documentation-review)
<a id="910d-the-legacy-importdata-envelope-is-accepted--and-renames-the-loaded-company--p0-trap"></a>

[9.10d The legacy `IMPORTDATA` envelope is accepted — and renames the loaded company — **P0 TRAP**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#910d-the-legacy-importdata-envelope-is-accepted--and-renames-the-loaded-company--p0-trap)
<a id="912-item-invoices--allledgerentrieslist-is-silently-discarded--trap"></a>

[9.12 Item invoices — `ALLLEDGERENTRIES.LIST` is silently DISCARDED — **TRAP**](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#912-item-invoices--allledgerentrieslist-is-silently-discarded--trap)
<a id="912a-the-shape-that-works"></a>

[9.12a The shape that works](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#912a-the-shape-that-works)
<a id="912b-actiondelete-by-remoteid--confirmed-working-on-a-live-book"></a>

[9.12b `ACTION="Delete"` by `REMOTEID` — confirmed working on a live book](./TALLY_PROTOCOL_REFERENCE_COMPANY_IDENTITY_AND_CREATION.md#912b-actiondelete-by-remoteid--confirmed-working-on-a-live-book)
<a id="10-change-detection"></a>

[10. Change detection](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#10-change-detection)
<a id="101-server-side-alterid-filtering-works--contradicts-published-community-guidance"></a>

[10.1 Server-side `AlterID` filtering works — **contradicts published community guidance**](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#101-server-side-alterid-filtering-works--contradicts-published-community-guidance)
<a id="11b-scale-findings-at-101287-vouchers--the-two-that-matter-most"></a>

[11b. Scale findings at 101,287 vouchers — the two that matter most](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11b-scale-findings-at-101287-vouchers--the-two-that-matter-most)
<a id="11b1-a-truncated-read-is-indistinguishable-from-a-complete-one--p0"></a>

[11b.1 A truncated read is indistinguishable from a complete one — **P0**](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11b1-a-truncated-read-is-indistinguishable-from-a-complete-one--p0)
<a id="11b2-a-client-timeout-does-not-cancel-server-side-work--p0-operationally"></a>

[11b.2 A client timeout does not cancel server-side work — **P0 operationally**](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11b2-a-client-timeout-does-not-cancel-server-side-work--p0-operationally)
<a id="11b3-filter-cost-is-fixed-not-proportional-to-results"></a>

[11b.3 Filter cost is fixed, not proportional to results](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11b3-filter-cost-is-fixed-not-proportional-to-results)
<a id="11b4-bytes-per-row-is-stable-and-predictable"></a>

[11b.4 Bytes-per-row is stable and predictable](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11b4-bytes-per-row-is-stable-and-predictable)
<a id="11c-a-windowed-voucher-read-is-bounded-before-it-is-sent--rule-the-measurements-below-are-verified-the-bounds-own-requests-are-unverified-live"></a>

[11c. A windowed voucher read is bounded before it is sent — **rule; the measurements below are VERIFIED, the bound's own requests are UNVERIFIED live**](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c-a-windowed-voucher-read-is-bounded-before-it-is-sent--rule-the-measurements-below-are-verified-the-bounds-own-requests-are-unverified-live)
<a id="11c1-what-was-measured"></a>

[11c.1 What was measured](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c1-what-was-measured)
<a id="11c2-why-the-existing-limits-do-not-protect-tally"></a>

[11c.2 Why the existing limits do not protect Tally](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c2-why-the-existing-limits-do-not-protect-tally)
<a id="11c3-the-rule"></a>

[11c.3 The rule](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c3-the-rule)
<a id="11c4-what-this-does-not-establish"></a>

[11c.4 What this does not establish](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c4-what-this-does-not-establish)
<a id="11c5-live-evidence-2026-09-21"></a>

[11c.5 Live evidence, 2026-09-21](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11c5-live-evidence-2026-09-21)
<a id="11a-scale-measurements--11287-voucher-corpus"></a>

[11a. Scale measurements — 11,287-voucher corpus](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11a-scale-measurements--11287-voucher-corpus)
<a id="read-performance"></a>

[Read performance](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#read-performance)
<a id="write-performance-and-degradation"></a>

[Write performance and degradation](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#write-performance-and-degradation)
<a id="11-measurements"></a>

[11. Measurements](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#11-measurements)
<a id="12-verification-method--how-to-test-this-gateway"></a>

[12. Verification method — how to test this gateway](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12-verification-method--how-to-test-this-gateway)
<a id="127-empty-date-witness-profile-qualification"></a>

[12.7 Empty-date witness profile qualification](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#127-empty-date-witness-profile-qualification)
<a id="12a-bill-wise-semantics-the-native-reports-and-volume--live-measurement-2026-08-02"></a>

[12a. Bill-wise semantics, the native reports, and volume — live measurement 2026-08-02](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a-bill-wise-semantics-the-native-reports-and-volume--live-measurement-2026-08-02)
<a id="12a1-built-in-named-reports-work--this-qualifies-22s-blanket-avoid"></a>

[12a.1 Built-in named reports work — this qualifies §2.2's blanket AVOID](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a1-built-in-named-reports-work--this-qualifies-22s-blanket-avoid)
<a id="12a2-which-date-each-allocation-kind-ages-from"></a>

[12a.2 Which date each allocation kind ages from](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a2-which-date-each-allocation-kind-ages-from)
<a id="12a3-tally-offers-two-ageing-methods-and-the-anchor-differs"></a>

[12a.3 Tally offers two ageing methods, and the anchor differs](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a3-tally-offers-two-ageing-methods-and-the-anchor-differs)
<a id="12a4-the-import-path-rewrites-what-you-send--extends-9"></a>

[12a.4 The import path rewrites what you send — extends §9](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a4-the-import-path-rewrites-what-you-send--extends-9)
<a id="12a5-configuration-is-not-a-diagnostic-in-either-direction"></a>

[12a.5 Configuration is not a diagnostic, in either direction](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a5-configuration-is-not-a-diagnostic-in-either-direction)
<a id="12a6-the-unallocated-remainder-and-how-to-see-it"></a>

[12a.6 The unallocated remainder, and how to see it](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a6-the-unallocated-remainder-and-how-to-see-it)
<a id="12a7-the-company-collection-ignores-svcurrentcompany--qualifies-911"></a>

[12a.7 The `Company` collection ignores `SVCURRENTCOMPANY` — qualifies §9.11](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a7-the-company-collection-ignores-svcurrentcompany--qualifies-911)
<a id="12a8-payload-scales-with-voucher-count-measure-request-time-separately"></a>

[12a.8 Payload scales with voucher count; measure request time separately](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a8-payload-scales-with-voucher-count-measure-request-time-separately)
<a id="12a9-a-ledger-guid-survived-an-observed-ui-rename--coverage-must-compare-guid-to-name"></a>

[12a.9 A ledger GUID survived an observed UI rename — coverage must compare GUID to name](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#12a9-a-ledger-guid-survived-an-observed-ui-rename--coverage-must-compare-guid-to-name)
<a id="13-open-questions"></a>

[13. Open questions](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#13-open-questions)
<a id="14-changelog"></a>

[14. Changelog](./TALLY_PROTOCOL_REFERENCE_MEASUREMENTS_AND_OPEN_QUESTIONS.md#14-changelog)
