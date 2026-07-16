# ADR 0012: Tally live compatibility evidence

- Status: accepted
- Date: 2026-07-15

## Context

The prose support matrix could describe missing live evidence but could not
prevent a release from broadening a claim. The existing qualification receipt
is intentionally repository-synthetic and permanently disclaims live Tally,
runtime-capability, support, accounting, and performance authority. Extending
it would erase an important trust boundary.

Tally's documented third-party integration contract is XML sent by HTTP POST.
The local responder is unauthenticated. Product and exact release are visible
to an operator in Tally's About UI; they are not established by the XML
message-format `VERSION` field. The `/status` path is a Bridge heuristic rather
than an authoritative Tally integration API.

## Decision

Bridge keeps live read observation in the separate
`bridge-tally-compatibility` crate and schema
`bridge.tally.live-read-qualification/1`. A receipt uses closed enums, rejects
unknown fields, is capped at 256 KiB, and binds:

- Bridge commit, clean/dirty state, executable, Cargo.lock, and a deterministic
  compatibility-source surface;
- exact product, release, mode, platform, architecture, loopback family,
  transport, ODBC state, company-load state, locale, encoding, and a reviewed
  fixture-owned synthetic dataset tier;
- a synthetic fixture-manifest commitment and sealed read-profile/template
  identifiers; and
- closed operation outcomes, application status, size/count buckets, and safe
  reason codes.

Receipts exclude raw XML/JSON, endpoint and port, headers, raw errors, company
names/GUIDs, tax identifiers, ledger or voucher identifiers, amounts,
narrations, usernames, and paths. They permanently state that no writes were
attempted and that responder authenticity, accounting correctness, source
completeness/atomicity, performance support, and automatic support eligibility
are not established.

The receipt checksum provides accidental-change detection only. A positive
matrix cell additionally requires an exact-scope maintainer review attestation
signed with Ed25519 by a configured non-revoked key. The release gate verifies
the signature, key validity at review and release time, attestation expiry,
receipt age, exact claim dimensions and operations, clean source state, commit,
and current compatibility-surface digest. Evidence cannot be generalized to a
different cell.

The checked-in matrix is the claim authority. Missing evidence remains
`unknown`; absence is never converted to success. `Unsupported` requires a
signed, exact-scope receipt with an observed required-profile application
failure; connection failure alone is insufficient. Parser-only receipts cannot
serve as live evidence.

## Live-controller boundary

The compatibility crate validates evidence and release claims but performs no
network requests. The separate `bridge-tally-live-read` controller requires
exact observed About/profile values, an affirmative no-customer-data
attestation, and explicit interactive consent. Its single-use network consent
uses fresh cryptographic randomness, expires after five minutes, binds the full
configuration and endpoint plus the reviewed source/build/fixture evidence,
and revalidates the compatibility surface immediately before dispatch. It uses
a canonical repository-local ignored configuration and reaches
the generic HTTP transport only through a typed adapter that accepts the sealed
`ReadOnlyProfile` enum shared with production. It accepts no arbitrary XML,
reports, TDL, imports, or generic payloads and has no dependency path to Tauri
commands, sync, persistence, or write crates. It stops before company-scoped
reads if the reviewed synthetic fixture marker or unique GUID is absent or
ambiguous, and verifies unique ledger/voucher sentinels, count bounds, company
context, and voucher dates before marking the fixture contract verified. The
operator separately attests that no customer data is loaded. It previews the
exact allow-list receipt and requires receipt-and-output-bound consent before
atomic no-overwrite JSON save through a consumed repository-issued target.

## Consequences

CI can reject stale, tampered, revoked, source-drifted, or scope-mismatched
positive claims even when no live Tally host is available. The initial matrix
therefore contains only explicit `unknown` cells. A legitimate Education-host
run remains necessary before any exact Windows cell can become `observed` or
`supported`; no license restriction may be bypassed and no write probe is
authorized by this ADR.
