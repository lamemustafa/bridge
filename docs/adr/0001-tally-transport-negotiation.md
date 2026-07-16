# ADR 0001: Negotiate Tally transports from observed evidence

Status: accepted.

## Decision

XML/HTTP is the compatibility baseline. JSONEX, a TDL companion, and ODBC are
separate transports and remain unavailable until an exact product, release,
mode, and query profile proves them. Preference order is selected from current
evidence, not a hard-coded version guess. XML remains the fallback when it can
represent the requested pack truthfully.

Every transport uses the same canonical pack and proof contracts. Parity must
be demonstrated before a faster transport can replace an already verified
path. Transport success, application success, parsing, reconciliation, and
delivery are distinct phases.

Production HTTP remains loopback-only with redirects disabled. This protects
the network boundary but does not authenticate the local Tally process.

## Consequences

New transports may improve throughput without changing accounting meaning.
Release strings or HTTP 200 cannot enable a capability. Unsupported and unknown
profiles remain visible rather than silently falling back to incomplete data.
