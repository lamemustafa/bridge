# ADR 0004: Keep Tally writes disabled behind a controlled sandbox

Status: accepted for the safety contract; transport dispatch is not yet
enabled.

**Amended 2026-09-21 (Deviation, 2026-09-08: #236).** Transport dispatch *is*
enabled for one path, `post_import`, which shipped without citing this ADR.
The text below is kept as written; read the **Amendment** at the end before
relying on it as a description of current behavior.

## Decision

Writes require an observed runtime capability, explicit operator opt-in, a
synthetic company during initial validation, backup guidance acknowledgement,
small batches, exact validation and preview commitments, approval evidence, a
durable idempotency reservation, parser-derived Tally counters, and a strict
company-bound read-after-write verification.

The first live write has a deliberately narrower rule. Before write capability
can honestly be recorded as observed, Bridge may prepare one sealed synthetic
ledger-canary *candidate* only after a fresh, GUID-bound company check and a
local fixture enrollment that records the operator's disposable-company and
backup acknowledgements. Candidate status is not general write authorization,
does not enable arbitrary payloads or batch writes, and never upgrades the
Capability Passport. Only a parser-derived import receipt together with the
exact, company-bound readback can create observed write evidence. Until then,
including after a timeout, support remains unknown.

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
derived commitments separately before runtime wiring is considered. Fixture
enrollment is local, explicit, and revocable; it stores only durable local
commitments and safe status. It is not proof that a company is disposable, is
not a Tally write, and is insufficient to arm a generic write path.

## Amendment — Deviation, 2026-09-08: #236

### What happened

On 2026-09-08, #236 (`e1ea3c86`) added `post_import`, which posts one saved
Journal to Tally over the XML gateway after a native operator approval. #238
(`ea6a4219`) added a desktop action that uses the same service. Neither change
cited this ADR or edited it. So from that date this ADR's status line ("transport
dispatch is not yet enabled") and its Consequences ("every prepared write
remains ineligible for dispatch") were false. This amendment records the
deviation. The owner chose to document it rather than withdraw the path. The
amendment changes no code.

The owner accepted #236's scope on 2026-09-08 as a "quiet-company" first
version: the approval tells the operator to pause other edits and imports and
to keep the company and Tally mode unchanged until Bridge finishes. Support
for concurrent external changes is open as #239.

### Scope of the dispatching path

Evidence base: master `4f050da`. Paths are relative to `src-tauri/src/`.

- **Entry points.** The MCP tool `post_import` (`agent.rs`, `tool_payload`)
  and the desktop "post a saved Journal" action
  (`agent_desktop_journal.rs`, `DesktopJournalService::post`) both call
  `post_import_checked` (`agent_import_post.rs`).
- **What it can send.** Exactly one voucher per call, of type Journal, from a
  batch Bridge built and saved earlier under the `BatchV1` identity scheme.
  Enforced in `admit_saved_journal_integrity`: one voucher, Journal only,
  amendments refused (`import_post_amendment_requires_file_import`), saved
  bytes re-hashed against the saved `sha256`. Tally numbers the voucher
  (`require_native_numbering`). The rendered review must fit a native message
  box (`admit_fresh_saved_journal`: at most 1,600 characters and 24 lines).
- **What it cannot send.** Masters, other voucher types, multi-voucher
  batches, alters and deletes. No caller-supplied XML reaches the gateway:
  the request is re-rendered from the saved batch
  (`render_native_journal_xml`, `agent_import.rs`).
- **Gates.** For the CLI, `BRIDGE_AGENT_ENABLE_WRITES=true`, otherwise
  `import_posting_disabled` (`agent.rs`) and the tool is absent from discovery
  (`agent_catalog.rs`). In the MCPB, the "Allow Journal posting" setting
  defaults to **on** (`packaging/mcpb/manifest.json`, `enable_writes`). The
  desktop service sets `writes_enabled: true` unconditionally
  (`agent_desktop_journal.rs`, `Settings::for_desktop_journal`).
- **A separate, non-shipping dispatch path.** `TallyRuntime::post_lab_import`
  (`tally/runtime.rs`) posts XML without approval or the journal. It is
  compiled only with the `lab-writes` Cargo feature, which no CI workflow
  enables (`tests/lab_writes_ci_gate.rs`). At runtime it also requires
  `BRIDGE_LAB_WRITES=1`, port 9001 and a matching target and deny-list GUID
  set. It is outside this amendment's map and is noted so that "one
  dispatching path" is not read as "one dispatch call site".

### Clause-by-clause map

"Met" means the code enforces the clause on the path above. "Partly" and
"Not met" name exactly what is missing. The map describes code; the only live
evidence is #236's own report that a desktop build posted one human-approved
synthetic Journal and reconciled it without a resend. That report was not
re-run for this amendment.

| # | ADR 0004 requirement | Status | How #236 meets it, or what is missing |
|---|---|---|---|
| 1 | Observed runtime capability | Partly | `qualified_import_profile` (`agent_import.rs`) requires freshly observed TallyPrime in Licensed or Education mode, with `ProductAndMode` Supported/Observed. This is product/mode capability, not observed *write* capability: nothing requires a prior observed write, and the Capability Passport is neither read nor upgraded. |
| 2 | Explicit operator opt-in | Partly | The CLI is opt-in (above). The MCPB exposes posting to the model by default; there the per-post native approval (row 7) is the only opt-in. The desktop action is started by the operator in Bridge's own window (pick a saved file, review it, post it: `commands.rs`, `pick_for_review` and `post_reviewed`), but has no separate enable switch. |
| 3 | Synthetic company during initial validation | Not met | Nothing restricts posting to a synthetic or enrolled company. Any company that passes identity admission is eligible. |
| 4 | Backup guidance acknowledgement | Not met | The approval preview (`admit_fresh_saved_journal`) and dialog (`tally/approved_import.rs`, `show_review`) contain no backup guidance and record no acknowledgement. |
| 5 | Small batches | Met | Exactly one Journal per call (`admit_saved_journal_integrity`), under a review-size cap. Stricter than the ADR requires. |
| 6 | Exact validation and preview commitments | Partly | Validation is exact. Before approval, the saved batch is re-hashed, and the company tuple, the date against `BOOKSFROM`, duplicate absence, and every ledger (exact match, bound by identity through `bind_selected`) are checked. After approval, inside the endpoint queue, the company identity and unique name scope, the date against the observed mode's boundary, ledger identity and duplicate absence are checked again (`post_approved_import`, `recheck_import_admission`); `BOOKSFROM` is not. The preview is rendered from the same saved batch, and `ApprovedImport` carries the exact XML that was approved. But the preview text is not committed: no digest of what the operator saw is persisted. |
| 7 | Approval evidence | Partly | Approval runs in a separate native-dialog subprocess, so the model never supplies an approval boolean (`approved_import.rs`: `confirm`, `run_confirmation`). Decline and a 120 s timeout both refuse. No approval record is written: the durable intent (row 8) implies approval but carries no approval time or preview digest. |
| 8 | Durable idempotency reservation | Met, by a different mechanism | Before the POST, `StatusRecord::dispatch_native` is appended and `sync_data`-flushed to `agent-import-ledger.jsonl`, under the admission lock, after a check that the batch was not already dispatched or changed (`post_import_checked`, the `before_dispatch` closure). A cross-process lease covers intent, the POST, the response append and readback (`dispatch_lease::acquire`). A dispatched batch is only ever reconciled, never re-sent. This is not the `tally_import_idempotency_state` or outbox tables of migration `0003_tally_safe_writes.sql`, which `post_import` does not use. |
| 9 | Parser-derived Tally counters | Met | `parse_import_outcome` (`bridge-tally-protocol`, `import_outcome.rs`). A clean response requires every counter to be reported, and exactly created 1, altered 0, deleted 0, with zero ignored, errors, cancelled, exceptions and line errors (`is_clean_success_for(1, 0, 0)`), and an application status other than failure (`import_outcome_is_clean`). |
| 10 | Strict company-bound read-after-write verification | Met | Readback is mandatory (`verify_import_after_current_dispatch`): it is bound to the verified company GUID and compares accounting entries, not the response. `posted_verified` requires both a clean response and a verified readback (`finalize_current_dispatch`). |
| 11 | First-write canary rule: one sealed synthetic ledger-canary candidate, fixture enrollment with disposable-company and backup acknowledgements, no Passport upgrade | Not met | The first dispatched write was a Journal, not a ledger canary. `post_import` does not consult the write-fixture enrollment (`commands.rs`, `enroll_tally_write_fixture`), which exists but gates nothing on this path. The one part that holds: no Passport upgrade. |
| 12 | Lifecycle, outcome-unknown after bytes may have been sent, no automatic retry, no rollback | Met, with different terminal names | draft (`build_import_xml`) → validate → preview → approve (native) → arm (lease plus durable intent) → send (`ReadRetryPolicy::SINGLE_ATTEMPT`) → parse → verify. Terminal states are `posted_verified` or `reconciliation_required`, and a transport failure after intent is `import_dispatch_outcome_unknown`. Every response carries `resent: false` and `automatic_retry: false`. No rollback is claimed or implemented. The ADR's separate `partial` and `failed` verdicts are folded into `reconciliation_required`. |
| 13 | Domain-separated commitments; opaque parser-derived evidence; line-error text reduced to digests | Partly | Line errors are kept only as ordered domain-separated digests (`import_outcome.rs`, `domain_sha256`), and counts come only from the parser, so a caller cannot assert them. But the request and response commitments (`request_sha256`, `response_sha256` in `ledger::DispatchResponse`) are plain SHA-256 over the wire bytes, not domain-separated. |
| 14 | First qualification profile: ledger-only create/alter, RemoteID-bound preflight, exact counts, a lost response stays unknown | Partly | Counts are exact (row 9). A lost response is never promoted: without a clean persisted response the verdict stays `reconciliation_required`, even if a later readback matches (`finalize_current_dispatch`, `finalize_previous_attempt_reconciliation`). But the profile is voucher create, not ledger create/alter. The preflight is not RemoteID-bound: each post uses a fresh random `REMOTEID` (so a public file's REMOTEID is never upserted), and duplicate absence is checked by the `[BRIDGE:…]` narration attribution over the batch's date window, twice, inside the queue (`require_absent_verification_result`, `recheck_import_admission`). |
| 15 | Consequences: no public byte getter or transport adapter; every prepared write ineligible for dispatch; legacy rows not promotable; fixture enrollment local, explicit, revocable | Partly | No public byte getter: `ApprovedImport::xml` is `pub(super)`. But a transport path now exists (`post_approved_import`), and prepared Journals are eligible for dispatch. Legacy journal outcomes without counter-presence evidence cannot reach clean confirmation. Fixture enrollment is local, explicit and revocable, and irrelevant to this path (row 11). |

### Follow-ups

These are listed and not fixed here. Each one is either an implementation change,
or an owner decision to amend the requirement instead:

1. **Synthetic-company and canary gating (rows 3, 11).** Either require write-fixture enrollment
   before `post_import`, or record the owner's decision that a single
   human-approved Journal on any admitted company replaces the canary rule.
2. **Backup acknowledgement (row 4)** in the approval, or an explicit decision to drop it.
3. **Default opt-in (row 2).** The MCPB default of posting-on, and the desktop
   service's unconditional `writes_enabled`.
4. **Approval evidence (rows 6, 7).** Persist a digest of the approved preview, and the approval
   time, in the dispatch intent.
5. **Domain-separated request and response commitments (row 13).**
6. **Two write contracts.** The migration `0003` outbox and idempotency tables versus the
   `agent-import-ledger.jsonl` journal: decide which one is authoritative, and retire or
   connect the other.
7. **Write capability in the Passport (row 1).** Decide whether a verified post
   should become observed write evidence, and under what rule.
8. **Concurrent external changes:** #239 (accepted limitation).
