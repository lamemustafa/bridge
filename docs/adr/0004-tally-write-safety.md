# ADR 0004: Keep Tally writes disabled behind a controlled sandbox

Status: accepted for the safety contract; transport dispatch is not yet
enabled.

## Decision

Writes require an observed runtime capability, explicit operator opt-in, a
synthetic company during initial validation, backup guidance acknowledgement,
small batches, exact validation and preview commitments, approval evidence, a
durable idempotency reservation, parser-derived Tally counters, and a strict
company-bound read-after-write verification.

The lifecycle is draft, validate, preview, approve, arm, send, parse, verify,
and then verified/partial/failed/outcome-unknown. A timeout or connection loss
after bytes may have been sent is outcome-unknown and is never retried
automatically. Bridge does not call an action rollback unless a compensating
Tally operation has been implemented and verified.

Wire bytes, canonical intended state, the import response, canonical readback
state, and identity coverage use distinct domain-separated commitments. Import
and readback evidence is opaque and parser-derived; callers cannot assert
counts, identity-presence booleans, or a payload hash as proof of observed Tally
state. Raw line-error text is reduced to ordered, domain-separated digests and
is never retained by the portable contract.

The first qualification profile is ledger-only, limited to create/alter, and
requires a RemoteID-bound preflight. Alter intent declares the exact before and
after state and no-op alters are rejected. A parsed receipt must match exact
create/alter counts (with zero deletes) before an exact applied verdict is
possible. A lost response remains outcome-unknown even when a later readback
matches before or after state; that observation may aid investigation but is
not promoted without import-result evidence. Automatic retry is always false.

## Consequences

The network-free contract can be reviewed and tested before dispatch is
introduced. It commits to private deterministic import bytes but exposes no
public byte getter or transport adapter, and every prepared write remains
ineligible for dispatch. Legacy durable rows created by the earlier
caller-attested recovery contract remain readable but cannot be promoted to a
success/recovery terminal state. A later migration must persist the opaque
derived commitments separately before runtime wiring is considered.
