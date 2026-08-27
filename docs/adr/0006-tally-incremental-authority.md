# ADR 0006: Incremental Tally authority remains evidence gated

Status: Accepted (2026-07-15)

## Decision

Bridge treats a Tally change identifier as reusable only within an exact scope. The scope binds source
lineage, stable company identity and fingerprint, object type, capability-profile version, product,
release, mode, transport, pack schema, query profile, filter hash, and date/overlap policy. Drift in
any field requires a full snapshot.

An incremental cursor can be established only by a verified full-snapshot proof and an immutable,
directly observed capability record proving monotonic per-object identifiers and inclusive lower-bound
behavior plus an explicit source high watermark for that same scope. The generic pack proof is
necessary but insufficient: an immutable establishment receipt must additionally bind the complete
exact scope, durable snapshot plan, source response, coverage manifest, proof hash, and canonical
watermark. Cursors are stored as canonical decimal text, not signed integers or
floating-point numbers. Any future cursor change will require generation compare-and-swap and an
immutable audit event.

The v6 establishment receipt is itself the generation-1 audit event, and the checkpoint head cannot
exist without that receipt. Both are immutable and database triggers reject updates or deletion.
Because no protocol verifier yet authenticates source-response, coverage, and bracketing-watermark
evidence, a separate database gate currently rejects every establishment-receipt insert. Hashing
caller-provided values is not treated as authority. Checkpoint establishment and advancement are not
enabled. A future reviewed migration must replace that gate only when its verifier can atomically
construct the receipt from structured observations. Any future advancement API must atomically
verify the prior generation and exact proof receipt, write the next generation, and append its audit
event before this restriction can be relaxed.

The current implementation deliberately provides policy, exact-scope persistence, and honest readiness
fallback only. It does not expose live AlterID filtering, checkpoint establishment or advancement,
tombstone activation, scheduling, or a public incremental-start command. Portable transition and
deletion-authority receipts have private fields; production callers cannot self-attest `Verified` or
`Observed` authority.

## Split-book `ALTVCHID` collision

**VERIFIED 2026-08-26** on licensed TallyPrime Silver. A year-end split of `BRIDGE PROBE B
SANDBOX` returned two genuinely distinct books from `COMPANY_EXTENT_FETCH` (`Name`, `GUID`,
`BooksFrom`, `LastVoucherDate`, `ALTVCHID`, `ALTMSTID`):

| COMPANYNUMBER | GUID | BOOKSFROM | ALTVCHID |
| --- | --- | --- | ---: |
| 100005 | `ec4454ae-5c4c-4bfa-b3b0-68182a749689` | 20250401 | 2785 |
| 100014 | `ec4454ae-5c4c-4bfa-b3b0-68182a749689` | 20260401 | 2785 |

The separate per-object `ALTERID` field differed (207 and 435); this observation is specifically
that company-level `ALTVCHID` collided. `ALTVCHID` is therefore a progress counter, not an
identity discriminator. The GUID collision itself is recorded in
`TALLY_PROTOCOL_REFERENCE.md` §9.11b.

Any future incremental scope must key the complete observed book tuple
`(canonical_origin, COMPANYNUMBER, GUID, NAME, BOOKSFROM)`, never GUID alone. In particular,
two split twins must not share a `scope_sha256` or checkpoint watermark.

This is latent, not a live defect: no runtime path writes `tally_incremental_*` checkpoint heads.
Current Core Accounting snapshots are `Partial`, so they cannot establish incremental authority;
the schema-only foundation remains behind an unreachable `Verified` establishment state.

## Reasons for remaining disabled

- Current Core Accounting snapshots are `Partial`, so they cannot establish incremental authority.
- Existing generic Core proof rows do not bind an object-specific query/filter contract, complete
  numeric AlterID coverage, or an independently observed source high watermark.
- The Education profile has not proved an explicit source high watermark, inclusive-bound semantics,
  empty-feed behavior, reset/regression behavior, cancelled-voucher coverage, or deletion rules.
- A maximum identifier in returned rows is not an acceptable substitute for an explicit source high
  watermark.
- Absence from a delta never means deletion. A tombstone requires a separately observed rule and a
  verified final-state proof.
- Verification must hash and reconcile the complete materialized state after overlay, not the delta
  payload alone.

## Consequences

Bridge may do more full reads than a speculative integration, but it cannot silently lose edits,
invent deletions, or advance a cursor from partial evidence. The legacy name-based `altmastid_cache`
helper is retired from runtime code; the old table remains only for database compatibility.
