# ADR 0003: Represent sync truth, gaps, and freshness explicitly

Status: accepted.

## Decision

Bridge uses separate outcome (`completed`, `failed`, `cancelled`, or
`outcome_unknown`), verification (`verified`, `partial`, or `unverified`), and
freshness (`fresh`, `stale`, or `never_verified`) states. Capability states are
`supported`, `unsupported`, `unknown`, and `not_configured`.

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
