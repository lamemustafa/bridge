# ADR 0005: Tally snapshot recovery authority

Status: Accepted (2026-07-15)

## Decision

An interrupted Tally snapshot may resume only from an encrypted durable row that binds an immutable
plan, run identity, generation, and state payload. One worker owns a time-bounded lease. Every state
write is generation compare-and-swap protected, and terminal rows are append-only evidence.

The immutable plan includes the company identity and display-name selector, endpoint-bound capability
snapshot, product, release, mode, transport, pack schema, query profiles, filters, and windows. Resume
re-runs the read-only canary with the stored query context and requires the exact observed capability
profile. A renamed company starts a new run rather than mutating an interrupted plan.

State v4 introduced a closed adaptive-window policy and a separate exact one-day capability
canary. The caller's root windows remain immutable. If and only if a voucher-window response reaches
the typed response-size limit, Bridge calendar-midpoint-splits that leaf, records the parent/children
and policy in the row-hashed state, and generation-CAS persists the graph before dispatching either
child. Child identities bind the parent, query profile, filter commitment, and exact range. A one-day
overflow or configured leaf-limit exhaustion fails closed without changing the previous checkpoint.
Oversized masters, timeouts, parse failures, application failures, and cancellations cannot trigger a
split. Legacy v3 states remain inspectable but are not restart-resumable under the new policy.

Current v5 state retains that exact adaptive graph but moves per-record membership into normalized,
encrypted SQLite window attempts. Durable state stores only the owner-bound attempt or completed
receipt, count, and hash commitments; it never embeds the full identity map. Both v3 and v4
nonterminal rows remain inspectable for operators but are not resumable as v5 runs because the
normalized membership authority cannot be reconstructed truthfully.

Before final reconciliation, Bridge sums the hash-bound `member_count` values in completed v5
receipts without hydrating their canonical maps. The run fails closed with
`reconciliation_record_budget_exceeded` above 100,000 aggregate records, including on restart and
for a previously staged reconciled decision. Validated record-key and digest length limits make
that count a deterministic transient-memory ceiling. Reconciliation moves, rather than clones,
the hydrated maps, and normalized SQLite membership remains the only restart authority.
`CommitPending` also persists the compact reconciled proof, never the canonical membership map.
Recovery verifies an already-written immutable receipt against that proof without hydration. If no
receipt exists, an over-budget pending decision is replaced by the same non-advancing Failed proof
as an immediate over-budget run. Legacy v5 pending rows without the compact proof can still retry
within the bound; an already-committed legacy row fails explicitly rather than inventing authority.

Record staging accepts a replay only when every stored provenance, canonical payload, exact-decimal,
AlterID, validation, and rejection fact matches. Generated observation ID and local observation time
are deliberately excluded. Thus a crash after insert but before state-key persistence resumes as an
exact idempotent acknowledgement; changed content is an explicit conflict and cannot advance a
checkpoint.

For a file-backed mirror, each run also owns a per-resume-key kernel advisory lock beside the database.
The operating system releases that lock when the worker or process exits. This lock is the live-owner
authority: a restarted process can reclaim a durable row even when the wall clock moved backwards and
left a far-future UTC expiry, while a live process cannot be displaced even when its stored expiry looks
old. File-backed save and heartbeat operations require the lock as well as exact owner/run/generation
equality. In-memory test databases have no cross-process identity and retain the bounded UTC fallback.

The worker renews its diagnostic UTC lease before and after every connector call and every 30 seconds
while a call is in flight, as well as immediately before local reconciliation and commit authority.
Cancellation is polled during connector calls and rechecked at the same post-staging boundaries. If it
is observed before `CommitPending` is durably established, Bridge commits a Cancelled proof without
advancing the checkpoint instead of allowing a stale Completed or Partial outcome.

The native app's ordinary Tally HTTP path now incrementally decodes UTF-8/BOM and UTF-16LE/BE chunks,
computes encoded and decoded hashes, and enforces both caps without retaining a second complete wire
body. Exact-wire qualification keeps its separate byte-preserving API. Current XML parsers still
require the complete decoded string; one-pass record-sink parsing and chunk-transaction staging remain
a later PR06 slice and are not claimed by this decision.

Before mirror commit, the durable `CommitPending` row stores a SHA-256 commitment to the complete
expected ledger facts: run and batch identity, capability/company/pack binding, outcome, verification,
timestamps, provenance-backed accepted, rejected, and provenance-unavailable counts, snapshot hash,
the domain-separated digest of the complete canonical record-count map, checkpoint before/after,
gaps, and warnings. Proof contract v3 and migration 0011 add this count-map commitment without
rewriting historical v1/v2 ledger rows or hashes.
After a crash, only a hash-valid immutable proof-ledger receipt for the exact run and batch matching
that commitment can make the run terminal. Recovery reads that historical receipt even if a later
verified run now owns the current checkpoint; the pending run's receipt facts remain independently
verifiable. Checkpoint advancement at the original commit still requires equality with the checkpoint
observed before extraction inside the same SQLite write transaction.

If a reconciled `CommitPending` run loses that checkpoint compare-and-swap,
Bridge replaces the stale advancing decision with a durable non-advancing
Failed proof carrying `snapshot_checkpoint_changed`. It closes the staging
batch and all open attempts while preserving the winning checkpoint. Pending
Failed and Cancelled decisions never participate in checkpoint CAS and retain
their original outcome and reason even if another run advances the live head.

If the wall clock moves backwards during a terminal path, Bridge clamps the completion timestamp to
the durable batch start and records `local_clock_moved_backwards`. The repository independently
rejects any commit whose completion precedes its batch start, so an impossible negative-duration
proof cannot enter the ledger.

Failed and cancelled proofs carry the complete accumulated safe gap set, while the proof-ledger
receipt also carries the complete warning set. Crash recovery reconstructs those same deterministic
sets from the durable state, so job status, proof output, and immutable ledger evidence cannot diverge.

## Consequences

- A crash can be retried without inventing a new plan or proof receipt.
- Concurrent workers and concurrent verified runs fail closed instead of overwriting authority.
- Legacy v3 and v4 rows without v5 normalized membership receipts are inspectable but not resumable.
- Duplicate legacy snapshot run IDs block migration with an explicit diagnostic; Bridge never chooses
  one recovery row silently.
- The row hash is corruption evidence, not a keyed authenticity claim against an authorized database
  writer. SQLCipher and local credential controls remain part of the trust boundary.
