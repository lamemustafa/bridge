# ADR 0008: Keep India Tax observations unbound until source authority is proven

Status: accepted for the dormant parser contract; extraction and canonical
promotion are not enabled.

## Decision

Bridge may parse one exact, Bridge-defined India Tax observation envelope only
behind the non-default `india-tax-observation-parser` feature. The result is
named `Unbound`, retains source identity candidates and exact raw lexemes, and
records only response-internal counts and claimed company/window metadata.
It is never an `IndiaTaxBatch`, capability observation, source checkpoint,
mirror update, reconciliation result, filing artefact, or proof of GST
correctness.

The first contract has no request builder, TDL report, HTTP dispatch, runtime
selection, UI, or canonical adapter. This is deliberate: Tally documents the
GST storage and method surface, including multiple company registrations,
effective party-registration histories, voucher overrides, and detailed GST
fields, but that documentation does not prove an exact loopback export
contract for Bridge's target release and Education mode. A Bridge-owned
response shape can make parsing deterministic; it cannot prove that the
proposed source methods populate that shape truthfully or completely.

The parser therefore requires one exact envelope grammar, application success,
one context before rows, exact field order and cardinality, bounded decoding,
valid dates and exact-decimal lexemes, at least one identity candidate, unique
profile-local observation keys, and matching response-internal counts. It
rejects unknown, duplicated, misplaced, truncated, or oversized input without
partial output. Debug and error surfaces redact company identity, GSTINs,
voucher identities, values, and raw XML. Fragment and response hashes are
domain-separated correlation evidence only. Application success and all other
values are envelope claims; the parser cannot authenticate Tally as their
producer, and the lexemes are exact only within this Bridge-defined envelope.

## Evidence boundary

Tally's current product documentation says a company can have multiple GST
registrations and party GST details can have effective histories or be
overridden in transactions. Tally's developer documentation also lists the GST
schema/method surface and print-analysis methods. Those sources justify a
broader future observation model, but not a claim that a raw GSTIN is active,
owner-verified, or portal-validated, nor that a raw rate/value/amount tuple is
tax-correct or return-ready:

- https://help.tallysolutions.com/set-up-gst-details-in-company/
- https://help.tallysolutions.com/tally-prime/gst-master-setup/india-gst-creating-party-ledgers-for-gst-tally/
- https://help.tallysolutions.com/setting-up-gst-in-tallyprime/
- https://help.tallysolutions.com/what-are-the-changes-in-xml-tags/
- https://help.tallysolutions.com/developer-reference/schema-and-invoice-changes/gst-comprehensive-invoice/

The comprehensive-invoice analysis methods are computed print/report-context
methods, not a documented batch-extraction contract. Education mode also has
no TSS/Connected Services and restricts voucher entry to the 1st, 2nd, and
31st; the documentation does not establish unrestricted read behaviour, so
that remains unknown until observed:

- https://help.tallysolutions.com/licensing-best-practices/

## Consequences

The portable parser and adversarial corpus can mature without implying source
support. `IndiaTax` remains `Unknown, not supported`; zero rows do not prove an
empty GST population. A later request/extraction slice must independently
qualify exact TDL method evaluation, company and date binding, count scope,
multi-registration and override behaviour, stable Core-compatible identities,
Education-mode behaviour, source atomicity, and a release-specific report tie-out before any
canonical adapter or capability promotion is considered.
