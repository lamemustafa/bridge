# Bridge × Tally — Prompt Playbook

Companion to `docs/tally/IMPROVEMENT_PLAN_2026H2.md`. Copy-paste prompts for driving AI codegen agents (Codex/Claude) through each roadmap phase: implementation, review/rectification cycles, change-preservation gates, and the overall orchestrator.

**How to use:** every prompt below assumes the GLOBAL RULES block (§1) is pasted beneath it. Each phase runs the same cycle:

```
IMPLEMENT → ADVERSARIAL REVIEW → RECTIFY → (repeat review/rectify until no P0/P1 findings) → PRESERVATION GATE → PR
```

The ORCHESTRATOR (§7) drives phase selection, the cycle, and phase-gate advancement.

---

## 1. GLOBAL RULES block (paste into every prompt)

```text
GLOBAL RULES — Bridge × Tally (2026-07 plan revision)

Repository: lamemustafa/bridge. Base branch: master.

Read before editing: AGENTS.md, CONTRIBUTING.md, SECURITY.md,
review-checklist.md, docs/tally/README.md, docs/tally/privacy-model.md,
docs/tally/TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md,
docs/tally/IMPROVEMENT_PLAN_2026H2.md (the current authority where the two plans
conflict), and the crates under src-tauri/crates/ relevant to your phase.

SUPERSEDED old rules (do NOT follow these from the older plan doc):
- "Writes remain disabled until the dedicated safe-write stage" → writes now
  compile into shipped builds, gated by the per-company runtime write
  allowlist (default off) plus per-batch review/approval. The canary/
  attestation/dual-compile-flag machinery is deleted, not extended.
- "Data minimisation: do not fetch narrations, addresses, tax identifiers"
  → reversed. Full-fidelity reads into the encrypted local mirror are the
  product. The privacy stance is now "full-fidelity, local, encrypted".
- MAX_LEDGER_WRITE_BATCH = 10 → batch size is exactly 1 object per import
  request, always.

STILL NON-NEGOTIABLE (unchanged):
- Loopback-only Tally connectivity; redirects blocked; size/time caps.
- HTTP 200 is never Tally success; require application STATUS=1 parsing.
- SVCURRENTCOMPANY (company pinning) mandatory on every company-scoped
  request; fail closed on mismatch.
- Exact decimals only; never floating point for amounts.
- A failed/partial/cancelled run never advances a verified checkpoint.
- "Posted" is never claimed from counters alone; only from readback.
- No automatic retry of writes without an idempotency probe first.
- Deletion tombstones only from complete, verified scans.
- Only synthetic test data. Never commit/log raw books data, GSTINs, PANs,
  narrations, credentials, usernames, or machine paths.
- Nothing is marked `Verified` in the compatibility matrix from the
  Education edition or the simulator. Simulator = regression only.
- Truth States vocabulary in UI: Verified / Partial / Stale / Unsupported /
  Failed (rendered as 3 visual tiers). No bare green checkmarks.
- Every behavioural change ships with a regression test. Every DB change
  ships as a versioned migration with rollback notes.
- Do not touch DSC/document/AXAL behaviour except minimal compile wiring.
- One roadmap phase per PR series; no scope tourism.

RULE OF RULES: no safety mechanism without a demonstrated failure mode it
prevents; no capability claim without a receipt.

Required validation before any PR:
  corepack pnpm install --frozen-lockfile
  corepack pnpm run license:all
  corepack pnpm run build
  corepack pnpm run cargo:fmt
  corepack pnpm run cargo:check
  corepack pnpm run cargo:test
  corepack pnpm run cargo:clippy
  corepack pnpm run security:audit:frontend
  cargo audit --file src-tauri/Cargo.lock

PR body must include: invariant established, functional summary, tests
added, exact commands/results, migration+rollback impact, Tally/security/
privacy impact, remaining uncertainty, linked review-checklist item.
```

---

## 2. PHASE 1 — Unseal & Simplify (weeks 1–3)

### 2.1 Implementation prompt

```text
PHASE 1: UNSEAL & SIMPLIFY
Branch series: feat/tally-unseal-*

Mission: delete the sealed-canary ceremony; make the write path a normal,
runtime-gated capability; generalize import-evidence parsing. This phase
removes code and adds one gate. Net LOC should be strongly negative.

Implement, in separate reviewable PRs:
1. DELETE: all FIXTURE_CANARY_* constants and flows; the
   fixture-canary-dispatch-seam and fixture-canary-runtime-dispatch
   features; canary_preflight.rs, canary_dispatch_admission.rs,
   canary_runtime_dispatch_coordinator.rs; the operator-attestation
   enrollment commands and their UI; the sealed one-shot dispatch envelope.
   Migrate (do not silently drop) any evidence-table rows: mark legacy
   enrollment rows as archived in a versioned migration.
2. SIMPLIFY: collapse the eight digest newtypes in bridge-tally-write to
   two (payload digest, response digest) stored per outbox row. Keep
   domain-separated hashing.
3. ADD the sole surviving gate: a per-company runtime write allowlist,
   default OFF, persisted in the mirror DB keyed by company GUID, exposed
   as one Tauri command pair (enable/disable, with the company name echoed
   back for confirmation). No writes of any kind may dispatch for a
   company not on the allowlist. This gate exists because it prevents a
   demonstrated failure mode: a dev/test build pointed at real books.
4. GENERALIZE: import-evidence parsing (STATUS/counters/LINEERROR) and
   readback parsing in bridge-tally-protocol from ledger-only to a typed
   surface usable for vouchers, stock items, cost centres. Pure parsing;
   no dispatch in this phase.
5. UI: remove "write capability: Unknown" dead-ends; replace with Passport
   states fed from the (still empty, but now honest) receipts store:
   "Writes not yet qualified on this installation".
6. EVIDENCE UNBLOCK: implement the profile-specific `Unsupported` evidence
   signature in bridge-tally-compatibility (docs/tally/compatibility/
   README.md records that no such signature exists, so live receipts
   cannot promote any Unsupported claim today). Honest negative claims
   are half the Truth Layer; the matrix must be able to record them
   before Phase 3/4 qualification runs begin.

Do NOT in this phase: build the outbox, dispatch any write, extend read
profiles, or add UI beyond the allowlist toggle and removed dead-ends.

Tests:
- allowlist default-off blocks dispatch at the lowest layer (not just UI);
- allowlist is company-GUID-keyed (a renamed company stays gated correctly);
- generalized evidence parser: golden tests for CREATED/ALTERED/DELETED/
  CANCELLED/IGNORED/ERRORS combinations, duplicate containers, malformed
  counters, LINEERROR redaction — for all four object kinds;
- migration test: legacy canary/enrollment rows survive as archived rows;
- grep-tests (see preservation gate) proving canary symbols are gone.

Exit criterion: shipped build compiles with the write crates enabled,
gated only by the runtime allowlist; canary code is gone; the
`Unsupported` evidence signature exists with a passing gate run; net diff
is code-negative.
```

### 2.2 Adversarial review prompt

```text
ROLE: adversarial reviewer for PHASE 1 (Unseal & Simplify).
You are reviewing actual diffs, not descriptions. Check out the branch and
read the code. Report findings as: [P0|P1|P2] file:line — claim — concrete
failure scenario. Then verify each of your own findings against the code
and mark CONFIRMED or WITHDRAWN. No style nits.

Hunt specifically for:
1. Incomplete deletion: any surviving FIXTURE_CANARY_* symbol, feature
   flag, attestation table write path, or docs/tally text still promising
   the canary flow (docs/tally/compatibility/synthetic-write-canary-
   fixture.md must be removed or rewritten).
2. Evidence-signature review: the new `Unsupported` signature must be
   profile-specific (a generic STATUS=0 must remain `Failed`, never
   `Unsupported` — see compatibility README); check signing-key rotation
   and revocation handling for the new signature kind.
3. Gate bypass: any code path that reaches the transport's import/POST
   surface without consulting the per-company allowlist — including test
   helpers, the qualification harness, and future-facing dead code.
   The allowlist check must live at or below the single-writer boundary,
   not in the UI or command layer alone.
4. Gate identity confusion: allowlist keyed by company NAME anywhere
   (rename → gate slips), or a missing-GUID company silently passing.
5. Evidence-parser regressions: the generalized parser accepting a
   response whose counters and object kind disagree; profile-defaulted
   zeros being reported as observed zeros.
6. Migration safety: canary/enrollment rows dropped without archive;
   migration not reversible; schema version not bumped.
7. Dishonest UI: any surface still implying write capability exists or is
   "Unknown" in the old sense; Passport text claiming qualification that
   has no receipt.
8. The deletion overreaching: serialization/circuit-breaker runtime,
   company pinning, STATUS=1 enforcement, ExactDecimal, checkpoint
   atomicity must be untouched (see preservation checklist).
```

### 2.3 Rectification prompt

```text
ROLE: rectifier for PHASE 1. Input: the CONFIRMED findings from the
adversarial review. For each finding, in severity order:
1. Restate the finding and the invariant it violates.
2. Fix it minimally; do not refactor beyond the fix.
3. Add a regression test that fails before the fix and passes after.
4. If a finding is actually invalid, prove it with a test or a code trace
   and mark it REJECTED with evidence — never silently skip it.
Re-run the full validation suite. Output: per-finding status
(FIXED/REJECTED+evidence), tests added, commands run with results.
Then hand back for another adversarial review pass. The cycle repeats
until a review pass yields zero CONFIRMED P0/P1 findings.
```

### 2.4 Change-preservation gate prompt

```text
ROLE: preservation gate for PHASE 1. You are the last check before PR.
Verify each item with a command, test run, or code citation — not by
reading PR descriptions. Output PASS/FAIL per item with evidence. Any
FAIL blocks the PR.

Must still hold after this phase:
1. Loopback-only transport: endpoint validation tests green; no new
   non-loopback host acceptance. (cargo test in bridge-tally-transport)
2. Single-writer serialization + circuit breaker untouched:
   bridge-tally-runtime tests green; no API changes.
3. Company pinning: every company-scoped request still requires
   SVCURRENTCOMPANY; fail-closed tests green.
4. STATUS=1 enforcement and response caps unchanged in
   bridge-tally-protocol / bridge-tally-transport.
5. Read pipeline unchanged: snapshot → canonical → reconcile → proof
   round-trip tests green; checkpoint atomicity tests green.
6. ExactDecimal untouched; no f32/f64 introduced anywhere near amounts
   (grep for "f64" in changed files; justify every hit).
7. Compatibility-matrix schema and Ed25519 receipt signing intact
   (bridge-tally-compatibility tests green); matrix claims file unchanged
   except where canary claims were removed.
8. Mirror encryption: SQLCipher init path unchanged; no plaintext DB.
9. No DSC/document/AXAL behavioural diff (git diff scoped check).
10. Absence-of-ceremony proof: scope the grep to implementation only, because
    the planning docs legitimately discuss the removal. Run
    `grep -ri "FIXTURE_CANARY\|dispatch-seam\|attestation" src-tauri src
    docs/tally/compatibility docs/adr` (i.e. code + evidence/ADR docs,
    NOT the roadmap/playbook/plan `.md` files) and expect only
    archived-migration and changelog references.
```

---

## 3. PHASE 2 — Full-Fidelity Reads (weeks 3–8)

### 3.1 Implementation prompt

```text
PHASE 2: FULL-FIDELITY READS
Branch series: feat/tally-full-fidelity-*

Mission: the mirror must hold everything a CA needs to review, reconcile,
and later verify writes: narration, party GSTIN/address, bill allocations,
GST tax lines, inventory lines, cost-centre allocations. You cannot verify
what you cannot read back — this phase gates every write phase.

Implement:
1. Extend the voucher read profile with a NEW version. NOTE: `bridge.tally.
   vouchers/2` (BRIDGE Voucher Export V2) AND `bridge.tally.vouchers/3`
   (BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA, the scope-bound selected-read
   profile, pinned by a CHECK constraint in migration 0007) are both taken.
   Allocate the next free version — `bridge.tally.vouchers/4` (BRIDGE Voucher
   Export V4) — and migrate any stored profile identity as needed; do not
   mutate V2 or V3. FETCH NARRATION, party ledger name, PARTYGSTIN, address
   list, BILLALLOCATIONS.LIST, ALLINVENTORYENTRIES.LIST (+ batch/godown),
   LEDGERENTRIES.LIST with GST rate/classification fields,
   COSTCENTREALLOCATIONS.LIST.
2. Extend ledger/master profiles similarly (contact, GSTIN, addresses,
   opening-bill allocations).
3. Wire the existing feature-gated IndiaTax and Bills/Outstandings parser
   packs into the runtime read path as normal (non-default-off) profiles.
4. Canonical model: extend fail-closed canonicalization for the new KNOWN
   fields; add a QUARANTINE lane for unknown TDL/UDF fields — a quarantined
   voucher lands in the mirror flagged with the unknown field names as
   evidence, never fails the whole snapshot, and surfaces in the Gap Map
   fix-it list.
5. Encoding/normalization hardening: UTF-8/UTF-16LE/BOM fixtures;
   non-English (Devanagari, Gujarati, Tamil) company/ledger/narration
   fixtures in the simulator corpus; NFC normalization + case-insensitive
   collation for name keys (Tally name uniqueness is effectively
   case-insensitive).
6. Migration: versioned mirror schema evolution for the new fields
   (voucher lines, bill allocations, inventory lines, tax lines) with
   rollback notes.
7. Rewrite docs/tally/privacy-model.md and README claims in the SAME PR
   that un-minimizes reads: the stance is now "full-fidelity, local,
   SQLCipher-encrypted; nothing leaves the machine".

Tests:
- golden round-trip: simulator company → snapshot → canonical → re-export
  diff clean for every wired voucher type, including bill/inventory/tax
  lines and non-English names;
- quarantine: a voucher with an unknown UDF is mirrored+flagged while the
  snapshot completes and the proof records Partial-with-reason;
- encoding: UTF-16LE + BOM + numeric character references parse; a
  malformed-entity fixture fails closed with a typed error;
- migration up/down tests.

Exit criterion: clean round-trip diff on the Education instance across all
wired voucher types; quarantine lane demonstrated on a custom-UDF fixture.
```

### 3.2 Adversarial review prompt

```text
ROLE: adversarial reviewer for PHASE 2 (Full-Fidelity Reads). Same output
contract as Phase 1 review (confirmed findings only, file:line, failure
scenario).

Hunt specifically for:
1. Silent field loss: a FETCH list naming a field the parser then drops;
   canonical model fields that never reach the mirror schema; NULL-vs-
   empty-string conflation (absent narration must be absent, not "").
2. Quarantine overreach/underreach: known fields routed to quarantine
   (hides bugs) or unknown fields still failing whole snapshots; quarantine
   evidence leaking raw values into logs (field NAMES are evidence; field
   VALUES in logs are a privacy finding).
3. Amount fidelity: any new tax/inventory line parsed through anything but
   ExactDecimal; sign conventions (IsDeemedPositive) mishandled on new
   line types; Dr/Cr balance invariant not re-checked with lines present.
4. Identity/normalization traps: NFC normalization applied on read but not
   on the keys used for diffing (same ledger counted twice); case-collation
   asymmetry between mirror and reconciliation.
5. Bounded-resource regressions: new list explosions (AllInventoryEntries
   on huge vouchers) versus the 32 MiB response cap — is there a paging or
   windowing story? Does a capped response get honestly labeled Partial?
6. Profile versioning: V2 or V3 mutated instead of the new V4 added;
   compatibility surface hashes not regenerated; old checkpoints silently
   reinterpreted as V4 data without a forced re-baseline.
7. Privacy-doc dishonesty: code un-minimizes but privacy-model.md/README
   still claim minimization (or vice versa).
```

### 3.3 Rectification prompt

Use the Phase 1 rectification prompt verbatim, with "PHASE 2" substituted. (The rectification contract is identical for every phase: fix confirmed findings in severity order, regression test per fix, REJECT only with evidence, re-run validation, loop until a review pass is clean of P0/P1.)

### 3.4 Change-preservation gate prompt

```text
ROLE: preservation gate for PHASE 2. Verify with commands/tests/citations;
PASS/FAIL per item; any FAIL blocks.

Must still hold:
1. All Phase 1 preservation items (re-run that checklist first).
2. Checkpoint atomicity with the new schema: kill-mid-snapshot test leaves
   the previous verified checkpoint readable and consistent.
3. Old-profile compatibility: a mirror created pre-migration opens, and
   the app forces an honest full re-baseline rather than mixing older
   (V2/V3) and new (V4) voucher data under one Verified label.
4. Data minimization REMOVAL is complete and consistent: no residual code
   path silently strips narration/GSTIN (grep the old omission tests —
   they must be inverted, not deleted-and-forgotten).
5. Proof-of-Sync and Gap Map still compute; reconciliation totals still
   tie out on the extended model (run the reconciliation test suite).
6. Response caps and streaming limits unchanged; no unbounded buffering
   added for the bigger payloads.
7. Diagnostics/logs still value-free: run the log-redaction test suite;
   grep new code for narration/GSTIN in log macros.
```

---

## 4. PHASE 3 — Drift Sentinel v1 + Sync Beacon (weeks 8–12)

### 4.1 Implementation prompt

```text
PHASE 3: DRIFT SENTINEL v1 + SYNC BEACON
Branch series: feat/tally-drift-sentinel-*

Mission: the acquisition wedge. A CA checkpoints a company ("I signed off
on these books"), and Bridge thereafter reports every voucher/master
created, edited, deleted, or back-dated in Tally since that checkpoint,
with before/after diffs. Plus the Sync Beacon: dual-timestamp freshness
that never shows green from cache.

Implement:
1. Incremental scan engine wired to the EXISTING bridge-tally-incremental
   crate (do not rewrite it):
   a. Cheap probe first: company-level ALTMSTID/ALTVCHID high-water marks;
      if unchanged vs checkpoint, skip the scan. Availability of these
      fields is a compatibility claim per Tally version — record it.
   b. Index scan: per object type, minimal inline-TDL projection of
      GUID, MASTERID, ALTERID (+ DATE, VOUCHERTYPENAME for vouchers),
      SEGMENTED per FY/month with per-segment checkpoints, scheduled
      off-hours by default, visible progress, cancellable. Never issue a
      single unbounded full-books export.
   c. Diff rules: new GUID → created (fetch full); higher AlterID →
      edited (fetch full, diff vs mirror); absent from a COMPLETE VERIFIED
      scan → deleted (tombstone); lower AlterID → backup-restore detected
      → calm re-baseline flow, not a tamper alarm.
2. Sign-off checkpoints: operator-created, named, timestamped marks bound
   to (company GUID, scan receipt, mirror content hash). Multiple named
   checkpoints per company.
3. Drift report: per checkpoint, the list of changed/new/deleted/
   back-dated objects with field-level before/after diffs (from mirror
   history), each row carrying its evidence (AlterID pair, scan receipt).
   Exportable as PDF/JSON ("what changed since sign-off" pack).
4. Sync Beacon (UI): persistent pill — last-verified timestamp, latest-
   attempt timestamp+outcome, next scheduled check. A failed attempt NEVER
   erases last-verified; staleness self-degrades (configurable thresholds).
   Clicking opens the sync drawer: 24h attempt timeline (every failure
   visible), per-domain freshness table, "Run connection check" self-test.
5. Truth-state rendering compression: Verified(+time) / Attention(+reason
   +one fix action) / Broken(+remediation) — five states preserved in
   tooltips/evidence.

Do NOT: write anything to Tally; build multi-company dashboards; alert
via any external channel (in-app only in v1).

Tests (simulator + Edu instance):
- the tamper-catch scenario end-to-end: edit one, back-date one, delete
  one in the fixture; drift report lists exactly those three with diffs;
- back-dated voucher outside any recent window is caught (date-unbounded
  index scan or segment coverage proof);
- truncated/failed scan produces ZERO tombstones and an honest Partial;
- AlterID regression triggers re-baseline UX state, not tamper alarm;
- beacon state machine: failure preserves last-verified; staleness
  transitions at thresholds; no state renders a bare green.

Exit criterion: demo proof points 1 and 2 (tamper catch, completeness
under fire) pass live; drift pack exports.
```

### 4.2 Adversarial review prompt

```text
ROLE: adversarial reviewer for PHASE 3 (Drift Sentinel + Beacon).
Confirmed findings only, file:line, concrete failure scenario. This phase
carries the product's credibility: a single false negative (missed change)
in a demo kills the wedge. Severity-rank accordingly.

Hunt specifically for:
1. False negatives: segment boundary off-by-one (voucher dated on the FY
   boundary scanned by neither segment); back-dated voucher into an
  already-verified month escaping because only recent segments re-scan;
   master edits that don't bump the probed high-water mark; voucher-type
   filter blind spots (all 24 types covered by the scan?).
2. False tombstones: scan marked complete despite truncation/cap hit;
   per-segment completeness conflated with whole-scan completeness;
   company with books-from date later than scan start treated as deletion.
3. False tamper alarms: backup-restore (AlterID regression) path actually
   reachable and calm; Tally company rename mid-scan; checkpoint bound to
   name not GUID anywhere.
4. Beacon dishonesty: any code path where a render shows Verified without
   a timestamp, or where an in-flight attempt hides a previous failure, or
   where "next check" lies when the scheduler is off/backgrounded.
5. Performance landmines: scan concurrency vs a CA actively typing in
   Tally (request spacing honored? cancellable mid-segment?); memory on
   500k-row index scans (streaming, not Vec-everything?).
6. Evidence gaps: drift rows without scan-receipt linkage; diffs computed
   against a mirror state that wasn't the checkpoint's state (history
   versioning correct?).
7. Privacy: drift PDF/JSON pack leaking full narrations/GSTINs beyond what
   the operator explicitly exported; log redaction on diff paths.
```

### 4.3 Rectification prompt

Phase 1 rectification contract, substituting "PHASE 3". Additional rule for this phase:

```text
For any finding in category 1 or 2 (false negatives / false tombstones),
the regression test must be an end-to-end simulator scenario, not a unit
test of the diff function alone — the demo-killing bugs live between the
layers.
```

### 4.4 Change-preservation gate prompt

```text
ROLE: preservation gate for PHASE 3. PASS/FAIL with evidence.

Must still hold:
1. Phase 1 + Phase 2 preservation checklists (re-run).
2. Read-only guarantee intact: this phase dispatches zero imports; grep
   changed code for the import/dispatch surface; allowlist untouched.
3. Snapshot pipeline unaffected: full-snapshot tests green; incremental
   scans and full snapshots cannot interleave into a corrupted checkpoint
   (concurrency test).
4. bridge-tally-incremental public contract: existing tests green
   unmodified (wiring, not rewriting).
5. Request-spacing/circuit-breaker behaviour unchanged under scan load;
   no scan path bypasses the single-endpoint queue.
6. Truth States: the five-state model still exists internally; compression
   is render-only (evidence views show all five).
7. Beacon adds no new network egress; no telemetry/exporter added.
```

---

## 5. PHASE 4 — Write Core & Voucher Writes (months 3–6)

### 5.1 Implementation prompt

```text
PHASE 4: WRITE CORE, THEN VOUCHER WRITES
Branch series: feat/tally-write-core-*, then feat/tally-voucher-writes-*
Precondition: licensed TallyPrime lab VM exists (month-2 rental). Edu is
regression-only from here on.

Mission: reliable, evidenced writes. Masters first (name-keyed idempotency
is simpler), then vouchers (payment/receipt/journal/contra).

Implement — write core (masters):
1. Outbox state machine in the mirror DB:
   PENDING → DISPATCHING → {CONFIRMED | CONFIRMED_WITH_DIVERGENCE | REJECTED | OUTCOME_UNKNOWN}
   OUTCOME_UNKNOWN → probe → {CONFIRMED | CONFIRMED_WITH_DIVERGENCE | PENDING | MANUAL}
   `CONFIRMED_WITH_DIVERGENCE` is the terminal state when readback (step 4)
   proves the write landed but Tally normalized/dropped a field vs intent;
   it is a distinct persisted state, never collapsed into `CONFIRMED`, and it
   surfaces in the Gap Map. Aggregations that report "posted & clean" must
   count only `CONFIRMED`; divergence is "posted, review". No auto-retry from
   this state (the write succeeded).
   Row durably committed (fsync) BEFORE dispatch, carrying: intent digest,
   canonical payload, idempotency key, company GUID, operation kind,
   pre-image AlterID (alters).
2. Batch size exactly 1 object per import request. Delete/replace
   MAX_LEDGER_WRITE_BATCH.
3. Single-writer actor owns the import surface; reads gated during
   dispatch→readback windows; queue depth visible.
4. Readback verification: after counters accept, re-export the object
   (masters by normalized name; vouchers by LASTVCHID) and
   ALWAYS cross-check the fetched object against the idempotency key and
   the (date, amount, ledger-set, voucher-type) fingerprint before
   promoting to CONFIRMED — LASTVCHID can be clobbered by a foreign
   writer between import and readback. Mismatch → key-search fallback →
   else OUTCOME_UNKNOWN. Persist the BridgeID ↔ GUID/MasterID binding.
   Field-diff readback vs intent; divergence → CONFIRMED_WITH_DIVERGENCE,
   surfaced in the Gap Map, never silent.
5. OutcomeUnknown recovery: on restart, DISPATCHING rows → probe by key +
   fingerprint; found+matching → CONFIRMED; absent → PENDING (safe
   re-dispatch); alter with foreign AlterID bump → MANUAL. Bounded retries
   (3, backoff) then MANUAL with evidence.

Implement — voucher writes (after masters CONFIRMED-path is soak-tested):
6. Voucher Create for payment/receipt/journal/contra with full lines,
   bill allocations, narration. Idempotency: client UUID in a UDF
   (BridgeTxnID, defined via inline TDL per request) with narration-suffix
   fallback — WHICH of the two is authoritative is a per-version
   compatibility claim qualified on the licensed lab. The fingerprint
   check is mandatory secondary dedupe regardless (narration is user-
   editable; never trust the embedded key alone on re-dispatch).
7. Cancel qualified as the compensation primitive (ACTION=Cancel by
   REMOTEID/GUID). Alter-by-GUID qualified per version; where flaky, the
   fallback is a Cancel+Create saga bound in one outbox transaction with
   crash recovery between legs. Delete only with mirror-side reference
   pre-check; always readback-verified by absence + next-scan absence.
8. Every CONFIRMED write emits a signed receipt row (payload digest,
   response digest, readback digest, operator, approver, timestamps,
   company GUID) — the Proof-of-Post substrate.
9. Qualification runs on the licensed lab populate the compatibility
   matrix per (product, release, operation); Edu results are labeled
   education-mode and never Verified.

Tests (the non-negotiable five, plus unit coverage):
- crash mid-dispatch → restart → recovery resolves to exactly-once (probe
  finds the voucher → CONFIRMED; or absent → re-dispatch), proven by final
  Tally state in the simulator AND on the licensed lab. Use an OS-agnostic
  crashpoint: a test-only injected panic/abort at the point between "outbox
  row committed" and "response parsed" is the primary mechanism (runs on the
  Windows matrix targets). Where an external process kill is used, it must be
  cross-platform — `taskkill /F /PID` on Windows, `kill -9` on POSIX — and the
  licensed-lab evidence must record the Windows result specifically, since the
  compatibility matrix targets Windows;
- duplicate re-dispatch with edited narration (key destroyed) is still
  caught by the fingerprint check;
- foreign writer interleaves between import and readback → LASTVCHID
  cross-check catches it (no false CONFIRM);
- alter with concurrent foreign edit → MANUAL, never blind retry;
- company not on allowlist / company mismatch → blocked below the
  command layer.

Exit criteria: ledger create/alter Verified on licensed lab with
kill-test; voucher create/cancel Verified for the four types; matrix rows
signed; zero unexplained/duplicated/missing vouchers across a 500-voucher
soak.
```

### 5.2 Adversarial review prompt

```text
ROLE: adversarial reviewer for PHASE 4 (writes). This is the highest-risk
phase in the product's life: a single duplicated or vanished voucher at a
design partner ends adoption. Confirmed findings only; assume hostile
conditions (power cuts, foreign writers, flaky Tally versions).

Hunt specifically for:
1. Exactly-once holes: any path where a row can dispatch twice without
   passing BOTH the embedded-key probe and the fingerprint check; fsync
   ordering (row durable before HTTP leaves?); crash between outbox commit
   and dispatch vs between dispatch and response — are both distinguishable
   on recovery?
2. Readback lies: promoting CONFIRMED from counters alone anywhere;
   LASTVCHID used without cross-check; readback racing the next queued
   write (actor gating actually enforced?); CONFIRMED_WITH_DIVERGENCE
   downgraded to CONFIRMED in any aggregation/UI.
3. Saga integrity: Cancel+Create fallback — crash after Cancel, before
   Create: is the books-state honestly represented and the recovery path
   tested? Can the saga half-apply invisibly?
4. Idempotency-key fragility: UDF definition rejected by a Tally version
   → does the write proceed keyless (finding!) or fail closed pending
   qualification? Narration suffix colliding with user content? Key
   surviving voucher Alter by a foreign writer?
5. Gate integrity: allowlist checked at actor level for EVERY operation
   kind incl. Cancel/Delete/saga legs; approval evidence bound to the
   exact payload digest (approve-then-mutate impossible)?
6. Matrix honesty: any Verified row sourced from Edu/simulator; receipts
   signed over the right tuple (product, release, mode, operation).
7. Blast-radius: reference pre-check for master delete racing a foreign
   voucher creation (pre-check stale) — is the LINEERROR path handled as
   REJECTED-with-evidence, not retry?
```

### 5.3 Rectification prompt

Phase 1 rectification contract, substituting "PHASE 4". Additional rules:

```text
- Any finding in categories 1–3 (exactly-once, readback, saga) must be
  fixed with BOTH a simulator end-to-end test and, where the behavior is
  version-dependent, a licensed-lab qualification run recorded as a
  matrix receipt.
- No finding in this phase may be resolved by weakening a check (e.g.,
  dropping the fingerprint verify to make a test pass). If a check is
  wrong, replace it with a stronger one and say why.
```

### 5.4 Change-preservation gate prompt

```text
ROLE: preservation gate for PHASE 4. PASS/FAIL with evidence.

Must still hold:
1. Phases 1–3 preservation checklists (re-run; especially: Drift Sentinel
   scans and the write actor share the endpoint queue without starvation).
2. Read paths cannot dispatch writes: type-level proof that read profiles
   cannot reach the import surface (the ReadOnlyProfile boundary in
   bridge-tally-read-transport is intact).
3. Allowlist default remains OFF; no migration flips existing companies on.
4. No automatic write retry beyond the bounded OutcomeUnknown probe path;
   REJECTED (semantic) errors never auto-retry.
5. Checkpoint/proof semantics: a write updates the mirror only through
   readback-confirmed state, never by assuming intent; snapshots and
   incremental scans reconcile Bridge-originated writes without double
   counting.
6. Receipts/evidence remain value-redacted (digests, not payloads) in any
   exportable surface; raw XML only behind the explicit sensitive-data
   reveal.
7. Simulator still covers the whole grammar (import counters, LINEERROR,
   LASTVCHID) — regression suite runs without the licensed lab present
   (CI must not depend on live Tally).
```

---

## 6. PHASE 5 — The Thin Product Loop (months 6–8)

### 6.1 Implementation prompt

```text
PHASE 5: THIN PRODUCT LOOP (Import → Review → Post → Proof)
Branch series: feat/tally-review-post-*
Precondition: Phase 4 exit criteria met.

Mission: the first thing a CA firm USES daily. Excel/CSV in, verified
vouchers in Tally out, evidence pack in the client file. Single company
at a time. Keyboard-first.

Implement:
1. Import (S2): Excel/CSV/paste ingestion → column-mapping step with
   auto-detected chips over first 5 rows; guesses visually distinct and
   confirmed once; mapping templates saved per client+source and
   auto-applied ("Using saved template ✎"). Import NEVER posts: it creates
   a Review batch. No PDF/OCR in this phase.
2. Review grid (S3): rows = Date | Party/Narration | Amount Dr/Cr |
   Proposed Ledger | Voucher Type | Flags | Status. Filter pills as
   counters (All/Ready/Needs mapping/Duplicates/Errors). Ledger
   suggestions with confidence WORDS (Matched/Suggested/No match), each
   with inspectable rationale ("mapped 14× previously for narrations
   containing…"). History seeding: on first connect, mine the mirror's
   12 months of posted vouchers into narration→ledger candidate rules.
   Rule promotion after 2 identical corrections (rules visible/editable).
   Duplicate flags vs batch + mirror (fingerprint), side-by-side compare,
   Skip default / Post-anyway recorded. Per-row errors in accountant
   language; batch cannot advance with Errors > 0; partial submission
   normal. Bulk edit: selection + set-ledger/type/date, "Accept all
   Matched" (Suggested requires explicit second toggle).
   Inline "+ Create ledger under <group>" spawns a master draft ordered
   before dependent vouchers in the same batch.
3. Post queue (S4): visible stepper Draft → Validated → Previewed →
   Approved → Posting → Posted → Verified. Revalidation at queue time
   against fresh mirror. Preview renders accountant-readable vouchers +
   batch header (counts, net Dr = net Cr) + View XML disclosure. Approval
   rules per company: none / any-other-user / named checker; approver +
   timestamp + comment recorded and bound to the payload digest.
   Posting via the Phase 4 actor (serialized, per-row live ticks,
   interruptible between objects). Per-row "Posted — verifying…" →
   "Verified in Tally, HH:MM" only after readback. Failed rows →
   remediation list with one primary fix action each (error-translation
   catalog: gateway off / wrong company / ledger missing / duplicate /
   period locked / divergence), never failing the whole batch.
4. Proof-of-Post export: per-batch PDF/JSON — what was posted, who
   approved, Tally response, readback verification, timestamps. Positioned
   as supplementary workpaper evidence (never "MCA audit trail").
5. Onboarding: guided connect (gateway how-to with illustrated 3-step),
   company pin, allowlist enable with typed company-name confirmation,
   Passport self-test ("Run connection check").

Tests:
- E2E simulator: 142-row CSV → review → approve → post → all rows
  Verified; kill mid-batch at row 61 → resume → zero duplicates;
- mapping template round-trip; history-seeded suggestions deterministic;
- duplicate side-by-side correctness (fingerprint collision + distinct);
- error catalog: every raw condition maps to exactly one card with one
  primary action; no raw XML/STATUS text in headlines;
- accessibility: full keyboard path through import→review→post.

Exit criterion (definition of done, verbatim): one article at one real
firm posts one client's weekly register for four consecutive weeks with
zero unexplained, duplicated, or missing vouchers, and the partner
exports one Proof-of-Post pack.
```

### 6.2 Adversarial review prompt

```text
ROLE: adversarial reviewer for PHASE 5 (product loop). Review as three
people: a hostile CA partner, a careless article, and an engineer.
Confirmed findings only.

Hunt specifically for:
1. Paths from file to Tally that skip review or approval (drag-drop
   shortcuts, retry flows, master drafts riding along unapproved).
2. Approval binding: batch mutated after approval (row edited, mapping
   changed) still posting under the old approval; approval digest checked
   at dispatch time, not queue time?
3. Suggestion honesty: confidence words backed by real rule provenance;
   "Matched" ever produced by a single occurrence; rationale strings
   fabricated rather than derived; history mining leaking cross-client
   rules (client A's narration rules suggesting client B's ledgers).
4. Duplicate UX traps: fingerprint near-misses (same day, same amount,
   different party) flagged or not; "Post anyway" decisions not recorded
   in the audit feed.
5. Stepper truthfulness: any state advancing on optimistic UI; "Verified"
   rendered from anything but a readback receipt; remediation retry
   re-posting instead of probing first.
6. Error catalog gaps: unmapped LINEERROR falling through to raw text in
   the headline; wrong-company card actionable-but-wrong (points at
   switch-workspace when Tally-side switch is needed).
7. i18n/format: Indian digit grouping, Dr/Cr never signed, date-format
   ambiguity in CSV import (DD/MM vs MM/DD) — a silent transposition is a
   P0 (wrong-date vouchers posted).
8. Proof pack: includes rows that were remediated-then-posted? excludes
   excluded rows explicitly? tamper-evident enough for its claim (and no
   stronger claim than it can carry)?
```

### 6.3 Rectification prompt

Phase 1 rectification contract, substituting "PHASE 5". Additional rule:

```text
Findings in categories 1, 2, 5 and the date-transposition case are
release-blocking regardless of severity label: fix with E2E tests. UX
polish findings (3, 6 wording, 8 formatting) may batch into a follow-up
PR only if they cannot cause a wrong posting.
```

### 6.4 Change-preservation gate prompt

```text
ROLE: preservation gate for PHASE 5. PASS/FAIL with evidence.

Must still hold:
1. Phases 1–4 preservation checklists (re-run).
2. The write actor remains the ONLY dispatch path; UI holds no transport
   handles; grep for invoke() surfaces that reach import besides the
   outbox commands.
3. Drift Sentinel + Beacon unaffected by product-loop load (scan + batch
   post concurrency test); Beacon never shows Verified during an
   in-flight batch's unverified rows.
4. Evidence views (Passport, Gap Map, receipts) still reachable — demoted
   to the Evidence section, not deleted.
5. Proof-of-Post claims audited: no "MCA/statutory audit trail" language
   anywhere in UI/docs/marketing strings.
6. Mapping rules and history mining are strictly per-company scoped
   (test: two companies, disjoint suggestions).
7. Onboarding never auto-enables the write allowlist; typed confirmation
   required; fresh install is read-only end to end.
```

---

## 7. ORCHESTRATOR prompts

### 7.1 Master orchestrator (session-level)

```text
ROLE: Orchestrator for the Bridge × Tally roadmap.
Authority documents, in precedence order:
1. docs/tally/IMPROVEMENT_PLAN_2026H2.md (current plan)
2. docs/tally/PROMPT_PLAYBOOK.md (this file)
3. docs/tally/TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md (legacy;
   superseded where they conflict — see GLOBAL RULES)

Loop, until stopped:
1. ORIENT: read the plan's roadmap table, the repo's open PRs/branches,
   and the compatibility matrix. Determine the current phase = the lowest-
   numbered phase whose exit criterion is not yet met with evidence.
   Never skip a phase gate because later work "seems parallelizable" —
   the only sanctioned parallelism is: Phase 3 UI work may overlap
   Phase 2 backend once Phase 2's schema is merged.
2. DECOMPOSE: split the phase's implementation prompt into PR-sized units
   (one invariant per PR, reviewable in under ~600 diff lines where
   possible; deletions exempt).
3. EXECUTE the cycle per PR unit:
   a. Run the phase IMPLEMENTATION prompt (scoped to the unit) + GLOBAL
      RULES.
   b. Run the phase ADVERSARIAL REVIEW prompt on the diff. Reviews must
      be performed by a fresh context that did not write the code.
   c. If CONFIRMED P0/P1 findings exist: run the RECTIFICATION prompt;
      return to (b). Hard limit: 4 review/rectify cycles per unit — if
      findings persist after 4, STOP and escalate to the founder with the
      unresolved findings; do not merge, do not descope silently.
   d. Run the phase PRESERVATION GATE prompt. Any FAIL → rectify → back
      to (b), because preservation fixes are changes too.
   e. Open the PR with the required body. CI must be green.
4. PHASE GATE (see 7.2) before declaring a phase done.
5. RECORD: after each merged PR, append one line to docs/tally/
   EXECUTION_LOG.md: date, PR, invariant established, evidence link
   (test name / matrix receipt). This log is the orientation input for
   step 1 of the next iteration.

Standing rules for you, the orchestrator:
- You never write code and review it in the same context.
- You never mark a phase exit criterion met without naming the artifact
  that proves it (test run, matrix receipt, demo recording note).
- Scope creep from ANY prompt (including reviewer suggestions) is parked
  in a BACKLOG.md list, not implemented.
- If two prompts in this playbook conflict, the phase's preservation gate
  wins, then GLOBAL RULES, then the implementation prompt.
- If reality contradicts the plan (a Tally behavior, a crate assumption),
  STOP the unit, write a one-paragraph deviation note with evidence, get
  founder sign-off, then amend the plan file BEFORE coding around it.
```

### 7.2 Phase-gate advancement prompt

```text
ROLE: Phase-gate auditor. Input: a claim that phase N is complete.
You are hostile to the claim. Output: ADVANCE or BLOCKED with reasons.

Procedure:
1. Quote the phase's exit criterion verbatim from the plan.
2. For each clause, demand the artifact: test output, signed matrix
   receipt, migration applied, demo scenario transcript, log entry.
   Re-run the decisive tests yourself; do not trust pasted output.
3. Run ALL preservation gates from phases 1..N (cumulative, not just N).
4. Check the negative space: list everything the phase prompt said
   "Do NOT" — verify none of it leaked in (grep, diff scan).
5. Check honesty surfaces: README, docs/tally, UI strings — no claim
   exceeds current evidence (no "Verified" language for Edu-only results,
   no write claims beyond qualified operations, no marketing adjectives
   without a receipt).
6. Verdict:
   - ADVANCE: every clause evidenced, preservation cumulative-green,
     honesty surfaces clean. Name the next phase and its first PR unit.
   - BLOCKED: numbered list of missing artifacts/failures, each with the
     smallest action that would unblock it.
```

### 7.3 Cycle-controller prompt (per PR unit, if running semi-automated)

```text
ROLE: Cycle controller for one PR unit of phase N.
State machine you enforce: IMPLEMENT → REVIEW → [RECTIFY → REVIEW]* →
PRESERVE → PR. Max 4 REVIEW iterations, then ESCALATE.

Your job each turn:
1. Name the current state and the prompt to run (from the playbook).
2. Verify the previous state actually completed: implementation = tests
   listed in the prompt exist and pass; review = findings are CONFIRMED/
   WITHDRAWN with file:line; rectification = per-finding FIXED/REJECTED
   with evidence; preservation = PASS/FAIL table complete.
3. Refuse transitions on missing evidence ("review says LGTM with no
   findings and no citations" → rerun review with the phase's hunt list).
4. Keep a running unit ledger: findings raised/fixed/rejected, cycles
   used, scope parked to backlog.
5. On ESCALATE or completion, emit the unit summary for EXECUTION_LOG.md.
```

### 7.4 Deviation / plan-amendment prompt

```text
ROLE: Deviation recorder. Trigger: implementation or review discovered
that reality contradicts the plan (Tally version behavior, crate
assumption, timeline, market fact).

Produce, in one message:
1. The plan clause contradicted (quote + file).
2. The evidence (test output, licensed-lab observation, source link).
3. Impact set: which phase prompts / preservation items / marketing
   claims are affected.
4. Two options with costs: amend plan vs work around; recommend one.
5. On founder approval: the exact edit to docs/tally/IMPROVEMENT_PLAN_2026H2.md
   and to the affected playbook prompts, in the same commit, with a
   dated "Deviation" note. Never let code and plan diverge silently.
```

---

## 8. Quick index

| Phase | Implement | Review | Rectify | Preserve |
|---|---|---|---|---|
| 1 Unseal & Simplify | §2.1 | §2.2 | §2.3 (canonical) | §2.4 |
| 2 Full-Fidelity Reads | §3.1 | §3.2 | §2.3 pattern | §3.4 |
| 3 Drift Sentinel + Beacon | §4.1 | §4.2 | §4.3 | §4.4 |
| 4 Write Core + Vouchers | §5.1 | §5.2 | §5.3 | §5.4 |
| 5 Thin Product Loop | §6.1 | §6.2 | §6.3 | §6.4 |
| Orchestrator | — | 7.2 gate | 7.4 deviation | 7.1 loop / 7.3 cycle |

Later-phase work (Alter drafts, sales/purchase GST splits, bank-statement variants, multi-client worklist, master hygiene, GSTR-2B bulk resolution) reuses this template: write the implementation prompt from the plan's LATER table, clone the nearest phase's review hunt-list and preservation gate, and always run the cumulative preservation stack.
