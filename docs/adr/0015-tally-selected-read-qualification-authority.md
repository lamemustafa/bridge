# ADR 0015: Selected-read qualification is exact-scope evidence only

## Status

Accepted for the local read-only setup flow. Broad ledger or voucher support,
source completeness, accounting correctness, write authority, and public
release support remain rejected without separate evidence.

## Context

A successful company-list probe proves neither that a selected company's
ledgers can be exported nor that a bounded voucher request is honored. Treating
any parseable or empty response as broad support would be especially unsafe:
Tally can return application failures inside HTTP success, company context can
drift, empty rows do not prove source emptiness, and an Education installation
does not establish behavior for other releases or license modes.

## Decision

Bridge may qualify exactly two versioned XML profiles after an operator reviews
one fresh GUID-bearing company and chooses an inclusive voucher window of at
most 31 days:

- `bridge.tally.ledgers/1` for the selected company's ledger export; and
- `bridge.tally.vouchers/3` for the same company and an echoed exact
  `FROMDATE`/`TODATE` window.

Qualification is single-attempt and read-only. It requires successful Tally
application status, an exact reviewed wrapper skeleton, no unexpected wrapper
attributes or CDATA, matching company GUID and normalized name, exact schema and
record-count evidence, case-insensitive GUID collision checks, and records that
pass the same bounded identity, text, amount, date, AlterID, and fragment-hash
validation required by canonicalization. A populated result requires verified
record identities. A proven zero-row profile execution records identity
evidence as `not_applicable_empty`; it never becomes a completeness or source-
empty claim.

The runtime reserves the reviewed probe before either selected request through
an opaque owner-bound lease. Read admission and reservation are mutually
exclusive at the endpoint session, and each qualification dispatch proves the
reservation owner, originating runtime instance, and exact session endpoint.
The lease holds the session alive and releases only its exact review on
cancellation, task abort, panic unwinding, early return, or ordinary drop.
Explicit consume and replacement disarm it idempotently; a stale lease
cannot clear a newer review. Reserved sessions are also excluded from endpoint
capacity eviction. Cancellation therefore restores the original review rather
than manufacturing an observation. Replacement reviews inherit the original
probe freshness origin, so failed or repeated qualification cannot renew stale
setup authority.

The returned UI object contains no ledger, voucher, amount, or company identity
values beyond the already reviewed company list. Raw rows are discarded. The
local encrypted mirror may retain request and decoded-XML SHA-256 commitments,
the observed decoding label, bounded result buckets, and verification states.
The decoded-response hash is explicitly not a wire-byte hash and is classified
as a local pseudonymous fingerprint; it is excluded from UI serialization and
public support export.

On explicit save, migration v7 stores the selected scope, the two observations,
and the Capability Passport in one transaction. Migration v8 records the full
review commitment, a domain-separated canonical setup-payload commitment, and
the resulting snapshot/company authority in that same transaction. An exact
replay returns those original references without inserting a second setup; a
different payload under an already consumed review fails closed. This makes a
lost acknowledgement after SQLite commit recoverable without renewing or
duplicating reviewed authority. Consumption rows are immutable and require the
snapshot and company to share one endpoint.

The repository recomputes the scope commitment from the canonical endpoint,
case-folded persisted company GUID, observed company name, profiles, window,
observation time, outcome, encoding, hashes, and all verification states.
Observations have a composite foreign key to the same scope and capability
snapshot. Scope, observation, consumption, and capability-item rows are
immutable. Legacy case-fold company GUID collisions block migration and are
never auto-merged.

## Consequences

The Passport may report `selected_ledger_read` or
`selected_voucher_window_read` only for the committed company/profile/window.
The existing broad `ledger_read` and `voucher_read` features remain `Unknown`.
A ledger failure skips the voucher request and preserves a partial Unknown
outcome; no failure is converted to `Unsupported` without an explicit contract.
No Tally write is attempted or authorized.

Portable and native simulator evidence can validate this authority boundary,
but it cannot create a public compatibility claim. The exact Windows/Tally/
release/mode support-matrix cell remains `Unknown` until a consented synthetic
live observation and the separate compatibility attestation gate succeed.
