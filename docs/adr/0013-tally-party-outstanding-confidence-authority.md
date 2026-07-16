# ADR 0013: Tally party outstanding confidence authority

Status: accepted for the portable PR16 foundation; live extraction and support
authority remain unavailable.

## Context

Tally exposes bill allocations and receivable/payable reports, but a parsed
row alone does not prove that the selected company, party, date, direction, or
report scope was queried. Empty output also cannot distinguish a true zero from
disabled bill-wise tracking, incomplete coverage, drift, an unsupported
currency shape, or an incompatible export profile. On Account amounts have no
truthful bill-level link.

## Decision

Bridge uses a versioned Bills & Payments canonical model and a separate exact
raw-observation parser. Parser output is explicitly unbound and cannot enter the
runtime or promote compatibility. Before a confidence decision, the caller
must match company identity, party ledger, as-of date, direction, query profile,
and scope fingerprint exactly.

Authority for allocation shape, outstanding shape, signed amounts, On Account
aggregation, settled-row omission, due-date interpretation, direction/sign,
and complete-empty meaning is opaque, bound to the exact expected scope, and
defaults to false. No public constructor can promote it. Source values or
echoed request metadata cannot set it; a future live-qualified adapter
constructor requires separate review. Reconciliation uses
signed decimal arithmetic without floating-point conversion. Ledger-opening
allocations are retained, On Account is aggregate-only, and due dates carry
explicit evidence. Missing, drifted, disabled, partial, foreign-currency, or
unobserved states fail closed with categorical outcomes; no numeric confidence
score is used.

## Consequences

- A legitimate live receipt for the exact export profile is required before
  source-semantics authority can be enabled.
- Outstanding IDs remain ordinal/profile dependent and are ineligible for
  mirror authority until the export's row ordering stability is observed.
- The Bills & Payments pack remains `Unknown, not supported` while its request,
  parser adapter, native extraction/runtime, Bills-authoritative mirror/proof,
  and UI surfaces are absent. Generic typed canonicalization and shared
  reconciliation result plumbing do not supply that authority.
- Education-mode live fixtures must use literal calendar day `01`, `02`, or
  `31`. Education mode has no TSS and excludes connected services, so these
  fixtures cannot qualify email, WhatsApp, payment-request, remote, or other
  connected flows.
- Future UI and support artifacts must replace raw source IDs with bounded,
  proof-local aliases.
- Payment automation and all writes require a separate decision and evidence
  path.
