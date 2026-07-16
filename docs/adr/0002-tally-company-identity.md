# ADR 0002: Bind all company-scoped work to durable source identity

Status: accepted.

## Decision

A display name is not a durable company identity. Bridge binds a run to source
lineage, an observed company GUID where available, and a fingerprint of stable
observations. Company-scoped requests explicitly set the current company and
the response must be checked against the intended identity before commit.
Observed GUIDs are ASCII-case-folded before both storage and fingerprinting, so
letter-casing drift cannot manufacture a second runtime identity.

Fallback fingerprints are labelled as weaker evidence. They do not authorize
automatic rename or deletion behavior. Identity drift invalidates incremental
checkpoints and requires a full, verified snapshot.

The operator console correlates a fresh probe with an encrypted persisted
profile through an opaque, versioned SHA-256 key over the canonical endpoint
and case-folded observed company GUID. This lets the live GUID-bearing record
recover the persisted mirror ID after restart without exposing the stored GUID,
guessing from a display name, or merging the same GUID across endpoints.

## Consequences

Renames do not silently create a new company when a durable ID is available.
Ambiguous identity prevents checkpoint advancement. Public diagnostics expose
generated mirror IDs and safe drift codes, not company names or book data.
