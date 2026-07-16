# ADR 0014: Native outstandings probe has observation-only authority

## Status

Accepted for an isolated, consent-gated qualification runner. Production
dispatch, parsing, and support promotion are rejected pending live
release-specific evidence and a separate promotion decision.

## Context

Tally's current integration documentation identifies a native report named
`Ledger Outstandings`, its mandatory `LedgerName` variable, and common export
variables. Current product documentation also describes opening, pending, due,
overdue, bill-wise, and On Account views. It does not define a stable XML
response grammar, exact sign convention, row ordering, empty-scope meaning, or
all method names needed to reproduce that report in a Bridge-owned TDL export.

Bridge's existing Bills parser accepts a different Bridge-owned envelope only
as unbound evidence. Treating a native report response as that envelope, or
adapting either response directly into a canonical Bills pack, would invent
authority that has not been observed.

## Decision

Bridge may expose a non-default, portable qualification probe that renders only
the documented native `Ledger Outstandings` request surface:

- `TALLYREQUEST=Export` and `TYPE=Data`;
- report ID `Ledger Outstandings`;
- validated `SVCURRENTCOMPANY`, `LedgerName`, and `SVTODATE` inputs;
- XML export format and `EXPLODEFLAG=Yes`.

The candidate definition is compile-time separated from production read
profiles and the native runtime. It owns the exact request bytes and publishes
domain-separated template, request, and scope commitments with redacted
diagnostics. A second non-default feature may connect that sealed type to a
dedicated qualification-only transport. That transport accepts no raw XML,
uses loopback with no proxy or redirect, applies fixed 64 KiB request and 1 MiB
response caps, has a 20-second timeout, and makes exactly one attempt.

The standalone runner requires distinct consumed preflight, dispatch, UI-after,
and save confirmations. It verifies separately registered company and party
identity commitments, surrounds three candidate observations with four fresh
identity brackets, compares exact encoded response bytes only in bounded
memory, and retains no raw response. Its separate receipt records only bounded
observation facts and fixes every semantic, completeness, runtime, mirror, and
support authority flag to false.

A Candidate response with failed or unrecognized Tally application status, an
HTTP rejection, or a transport failure is a modeled attempt fact rather than
an unreceipted controller abort. The runner completes that attempt's trailing
identity bracket and the remaining fixed observation sequence with zero retry,
provided every identity read remains valid and unchanged. Any identity failure
or change still stops before another scoped Candidate request. Repeatability is
`NotEstablished` when any Candidate response body is unavailable. Save consent
is bound to both the sealed receipt and a consumed repository-issued output
target under canonical ignored `.bridge-live`; an existing file is never
replaced.

Operator UI evidence is a closed typed schema, not arbitrary JSON. It permits
at most 256 ordered rows with contiguous zero-based indices, exactly the ten
reviewed projection fields, a bounded nonempty row kind, and bounded control-
free display text. The settled `INV-001` observation is a closed
`present`/`omitted`/`unobserved` enum and must satisfy the selected scenario's
exact presence rule; invented labels cannot satisfy the gate.

The protocol candidate's local posture remains `CandidateOnlyNoTransport` and
`ProfileUnobserved`; the separate transport does not mutate or promote that
type. A successful HTTP and application response proves only that the sealed
request was answered in the attested observation bracket.
It would not prove responder authenticity, company or party identity,
completeness, stable ordering, accounting semantics, zero outstandings, source
atomicity, release support, or production suitability.

CandidateV0 is immutable request-shape evidence. Any byte or scope change must
create a new candidate version. A future live-qualified profile must be a
separate type; CandidateV0 must never be renamed or promoted in place.

Before any parser or adapter is frozen, a disposable synthetic INR company must
be exercised using the reviewed native-probe runbook. The exact Tally
product/release/mode must be verified in-product, the company and party stable
identities must be independently observed, unchanged reads must be bracketed,
and raw response behavior must be compared with the visible native report.
Unknown, empty, disabled, ambiguous, reordered, or drifted observations fail
closed.

## Consequences

This creates a reviewable request and observation boundary without overstating
compatibility. It deliberately postpones the high-risk step: interpreting
release-specific native report output. Compatibility remains `Unknown, not
supported`; the qualification receipt is structurally incompatible with the
support gate, and no live request is permitted when the loaded company may
contain customer data.
