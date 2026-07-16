# ADR 0003: Represent sync truth, gaps, and freshness explicitly

Status: accepted.

## Decision

Bridge uses separate outcome (`completed`, `failed`, `cancelled`, or
`outcome_unknown`), verification (`verified`, `partial`, or `unverified`), and
freshness (`fresh`, `stale`, or `never_verified`) states. Capability states are
`supported`, `unsupported`, `unknown`, and `not_configured`.

A structurally valid, identity-bound execution of the sealed Core Accounting
profile is recorded as observed `unknown` with the stable reason
`sealed_profile_executed`. That exact evidence may authorize a snapshot
attempt, but it never promotes absent or coincidentally populated fields to
supported. The run's reconciliation and proof remain responsible for a
Partial or Verified result. Canary rows are qualification evidence only and
are re-fetched after the durable run start before they can enter snapshot data.
Snapshot start and end probes are also lifecycle evidence, not interactive
setup reviews: they execute through an uncached runtime path and cannot replace
a review token awaiting qualification or save. Restart admission accepts only
the same exact persisted `unknown` + observed + `sealed_profile_executed`
receipt; other unknown reasons and the ordinary supported-pack convention do
not authorize a Core resume. Snapshot company discovery is likewise a fresh,
validated lifecycle read that remains available after the single-use setup
review has been consumed; it neither requires nor recreates that review cache.

A verified proof requires all planned windows, stable identities, canonical
hashes, count invariants, and pack-specific reconciliation. Warnings may
describe unavailable secondary comparisons, but a known completeness or
accounting mismatch is a gap and prevents verification. Only a verified commit
advances a checkpoint. Incremental absence never means deletion; a tombstone
requires an observed deletion rule for the exact scope.

Typed tax identifiers prove only the validation explicitly performed by their
type. A GSTIN-shaped value from Tally does not prove that the registration is
active, belongs to the intended entity, or was verified by an external portal.

## Consequences

The UI can say exactly what is known without converting partial data into a
green success state. A previous verified snapshot remains active when a later
run fails. Schema placeholders cannot produce verified empty snapshots.
