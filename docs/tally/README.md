# Tally Truth Layer

Bridge's Tally integration is designed to be inspectable and fail closed. It
must distinguish network reachability from Tally application success, observed
capability from assumption, and a completed request from a verified snapshot.

## Public contracts

- [Support matrix](./support-matrix.md) records what is implemented, what has
  been observed on a real Tally host, and what remains planned.
- [Executable compatibility matrix](./compatibility/compatibility-matrix.json)
  enumerates exact claim cells and fails closed when positive claims lack
  current reviewed evidence.
- [Privacy model](./privacy-model.md) defines what may be retained or included
  in diagnostics.
- [Research and execution plan](./TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md)
  contains the source research, product model, threat analysis, and staged
  implementation plan.

The architectural decisions are recorded in:

- [Transport negotiation](../adr/0001-tally-transport-negotiation.md)
- [Company identity](../adr/0002-tally-company-identity.md)
- [Sync truth states](../adr/0003-tally-sync-truth-states.md)
- [Write safety](../adr/0004-tally-write-safety.md)
- [Synthetic qualification authority](../adr/0010-tally-synthetic-qualification-authority.md)
- [Live compatibility evidence authority](../adr/0012-tally-live-compatibility-evidence.md)
- [Party outstanding confidence authority](../adr/0013-tally-party-outstanding-confidence-authority.md)

## Non-negotiable invariants

1. Production Tally traffic is loopback-only and redirects are rejected.
2. Company-scoped operations name and verify the intended company.
3. HTTP success is never treated as Tally application success.
4. Unknown, unsupported, and not-configured are distinct states.
5. A checkpoint advances only after durable staging and reconciliation.
6. Absence in an incremental response is not deletion.
7. Writes are disabled unless their exact runtime capability was observed, and
   ambiguous post-send outcomes are never retried automatically.
8. Logs, fixtures, screenshots, and support bundles use synthetic data and safe
   reason codes, not book data.

## Contributor verification

The deterministic simulator and canonical core are the default development
surface. Live tests are supplemental and must record the exact product,
release, operating mode, host platform, and company fixture used. Education
mode is tested as Education mode; Bridge does not bypass its restrictions.

The simulator and fixture rules live in
[`src-tauri/crates/tally-protocol-simulator`](../../src-tauri/crates/tally-protocol-simulator/README.md).
The parser-only evidence contract lives in
[`src-tauri/crates/bridge-tally-qualification`](../../src-tauri/crates/bridge-tally-qualification/README.md).
The separate live-observation DTO and release gate live in
[`src-tauri/crates/bridge-tally-compatibility`](../../src-tauri/crates/bridge-tally-compatibility/README.md).
That crate performs no network requests. The separate
[`bridge-tally-live-read`](../../src-tauri/crates/bridge-tally-live-read/README.md)
controller requires a reviewed synthetic fixture and two interactive
confirmations, and exposes only byte-identical sealed production read profiles.
See the [Education runbook](./compatibility/live-education-runbook.md). No
receipt should be manufactured by hand.

Do not advertise a capability from a descriptor or roadmap item. A capability
becomes supported only when the exact release, mode, transport, query profile,
required fields, and invariants have current evidence.
