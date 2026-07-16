# Bridge implementation roadmap

## Completed repository baseline

- Independent React, Rust, and Tauri project structure
- Bridge package, crate, binary, and UI identity
- Contributor, security, review, issue, and pull-request guidance
- Managed Git and rectification playbooks

## Current hardening

1. Keep frontend and Rust checks green.
2. Remove developer-machine paths and personal/customer data from tracked
   content and publication history.
3. Make build assets and runtime discovery repository-relative or
   application-data-relative.
4. Validate development and packaging on native Windows and macOS hosts.
5. Add smoke and regression coverage for Tally, DSC, documents, sync, and local
   persistence.
6. Record platform-specific vendor dependencies without committing proprietary
   libraries, private keys, PINs, or certificate dumps.

## Tally Truth Layer

The active Tally roadmap is the
[research and Codex execution plan](./tally/TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md).
Its public support claims are constrained by the
[evidence matrix](./tally/support-matrix.md). Current implementation work is
ordered as follows:

Capability Passport profile v2 now exposes and stores provenance-bearing
feature evidence rather than relying on a broad health or display-only status.
It records only the response encoding and company facts observed by the safe
XML company probe. Endpoint evidence is narrowly named responder reachability;
selected reads, practical response limits, all writes, and
uninspected optional-transport configuration remain explicitly `Unknown`.
Endpoint probing is observation-only. Encrypted persistence occurs only after
the operator selects one current GUID-bearing company and explicitly saves a
fresh, opaque, single-use full-probe-bound scope; an owner-bound RAII lease
prevents stale or concurrent saves from consuming another review and releases
its exact review after cancellation, task abort, panic unwinding, early return,
or local atomic-storage failure so the same fresh review can be retried. The
lease keeps its endpoint session alive, reserved sessions cannot be evicted,
and a stale lease cannot alter a newer review. Other
discovered company identities are not pinned as a side effect of discovery.
An already-reserved save blocks a competing probe before it sends Tally
traffic. Post-commit review-cache cleanup cannot convert a durable save into a
reported failure; the UI instead shows an explicit restart warning.
The Passport, capability items,
endpoint observation, and selected company pin commit atomically with the
original probe time. Duplicate normalized GUIDs, unsafe identity fields, stale
UI completions, and replayed review commitments fail closed. GUID matching is
ASCII case-insensitive across selection and persistence, while the spelling
actually observed from Tally is retained.

Selected-read qualification now adds exact-scope evidence without promoting the
broad read claims. After one fresh reviewed GUID-bearing company is selected,
the operator may run `bridge.tally.ledgers/1` and a maximum-31-day
`bridge.tally.vouchers/3` window. The voucher response must echo the exact
window; both responses must match the reviewed company context, strict XML
structure, record counts, stable populated-row identities, and canonical field
constraints. Empty execution is explicitly identity-not-applicable and never a
completeness claim. Ledger failure skips the voucher request. Cancellation
restores the original review, endpoint read admission is mutually exclusive
with qualification reservation, and replacement reviews retain the original
freshness origin. Explicit save recomputes the scope commitment and atomically
stores immutable migration-v7 evidence tied by composite scope/snapshot
authority. Migration v8 consumes the full review commitment in the same
transaction and makes an exact replay idempotent, closing the cancellation
window after commit but before acknowledgement without duplicating authority.
Raw rows are discarded, decoded-response fingerprints remain local encrypted
evidence, all writes remain disabled, and public compatibility stays `Unknown`
pending consented synthetic live qualification.

1. Keep the protocol parser and shared endpoint runtime fail closed.
2. Complete native validation of the SQLCipher mirror and OS-backed key.
3. Extend canonical capability-pack models and atomic, resumable snapshots;
   Core Accounting schema v3 now retains debit/credit polarity plus optional-
   voucher state, an experimental per-ledger balance cross-view, bracketed full
   reread evidence, and a fresh end-profile comparison. Cross-view semantics
   remain capability-gated pending live Education-profile validation. Core
   canonicalization now rejects invalid request windows and any voucher outside
   the exact requested date range before snapshot or capability-canary state.
   Snapshot state v4 keeps immutable root windows while persisting deterministic
   calendar-midpoint children before dispatch; only a typed voucher response
   limit may split. The capability canary is one exact first-day window,
   one-day overflow and leaf exhaustion fail closed, and exact observation
   replays recover lost acknowledgements without an ambiguity gap. Ordinary app
   reads now decode and hash HTTP chunks without retaining the complete encoded
   body beside the decoded XML. A one-pass XML record sink and chunked staging
   transaction are still required to remove the remaining full decoded string
   and per-record durable-state rewrite.
4. Establish incremental checkpoints only from verified full snapshots plus an immutable exact-scope coverage and source-high-watermark receipt.
5. Connect the versioned AXAL destination contract without guessing endpoints.
6. The read-side operator console now provides stable-company selection,
   offline persisted profiles, separate verified-baseline/latest-attempt
   states, phase/window progress, explicit cancellation, company/pack-scoped
   recovery history, Gap Maps, proof-ledger views, safe retry guidance, and a
   paged metadata-only local mirror explorer. Proof summaries are
   hash-revalidated and a reviewed, allow-list-only redacted support export is
   available. Write preview, mappings, and conflict resolution remain disabled
   until their controlled-write roadmap slices. Separate custom ledger-balance
   corroboration and source-stability evidence are implemented, while live
   report-profile validation, documented cross-request atomicity, and the
   remaining applicability/header-total gates are still required before
   `Verified`.
7. JSONEX now has a portable, exact-scope semantic qualification contract and
   a separately feature-gated parser for the documented TallyPrime 7.0+ Ledger
   and Voucher collection-envelope shapes. A second disabled feature builds
   deterministic bytes for only the exact documented Ledger and unbounded
   `TSPLVoucherColl` example requests; every result is explicitly ineligible
   for dispatch. Parser outputs remain unbound because the documented responses
   do not prove company, query, range, completeness, or Core-schema identity.
   XML remains the sole runtime transport; no JSONEX HTTP dispatch, canonical
   adapter, runtime selection, mirror, checkpoint, proof, or write path exists.
   Matching bracketed samples remain
   shadow evidence, mismatches return an unenforced scope-local quarantine
   recommendation, and XML bracket mismatch or missing completeness/provenance
   evidence is inconclusive. Runtime enablement remains gated on exact
   TallyPrime 7.0+ release/mode, Education-profile, encoding, nested-shape,
   range-filter, repeated parity, and measured operational-benefit evidence.
8. Keep controlled writes disabled. The portable ledger-only qualification
   contract now derives exact preflight/import/readback evidence and legacy
   caller-attested durable rows cannot be promoted. Runtime dispatch remains
   blocked until the durable store persists opaque derived commitments and a
   synthetic Education company proves the exact import/readback profile.
9. India Tax now has a non-default parser for one exact Bridge-owned raw
   observation envelope. Its outputs remain explicitly unbound, retain only
   identity candidates and exact source lexemes, and cannot enter the
   canonical pack, capability registry, mirror, checkpoint, proof, runtime, or
   UI. Request construction and extraction remain blocked until exact TDL
   method evaluation, multi-registration and override behaviour, count scope,
   Core-compatible identity binding, Education-mode behaviour, and a
   release-specific report tie-out are observed with synthetic data.
10. PR13 now has a portable, fixed-cardinality observability contract with
    closed dimensions, coherent fixed-memory histograms, saturation evidence,
    bucketed privacy-reduced previews, and no exporter dependency. PR15 wires
    its first read-runtime caller and local preview command; phase/reconciliation
    metrics, persistence, general support bundles, and measured platform budgets
    remain follow-on work.
11. PR13B1 now has bounded deterministic voucher generators and a versioned
    parser-only qualification receipt. Inputs are generated before measurement;
    each sample uses a fresh worker and must match exact bytes, hashes, record
    counts, entry counts, and derived output. The receipt structurally disclaims
    live Tally, support, capability, accounting, runtime-cap, and performance
    budget authority. Windowed 50k/500k, HTTP fault/cap cases, native DB/resume,
    UI responsiveness, and reviewed baselines remain follow-on work.
12. PR13B2 binds the app to the portable bounded HTTP transport, preserves
    distinct loopback socket identities, rejects proxy/redirect/non-identity
    content encoding paths, and characterizes Content-Length, chunked,
    close-delimited, truncation, cap, timeout, and UTF-16 behavior against the
    loopback simulator. Export/import envelope ambiguity, cancellation pacing,
    and half-open probe stampedes are rectified. A bounded 50,000-ledger master
    generator and 10,000-ledger cross-encoding parser test exist. This is
    synthetic evidence only; process-isolated runtime, native database/UI, and
    live Education qualification remain pending.
13. PR14 now has a closed-schema compatibility receipt and executable release
    gate. Eleven exact Windows/Tally matrix cells cover current release/mode/transport/
    platform/company/locale/data dimensions and all remain `unknown`. A
    positive claim requires a clean exact source surface, fresh live receipt,
    reviewed Ed25519 attestation, and non-revoked key. The standalone controller
    now requires a reviewed fixture, single-use five-minute network consent
    bound to the full run/source/build evidence, sealed production read
    templates, context/range checks, an exact receipt preview, and receipt-and-
    output-bound repository-confined atomic no-overwrite save. Unknown About/
    profile values and a false no-customer-data attestation fail before consent
    and before network access. A legitimate Windows Education observation
    remains pending; macOS live/simulator evidence is also pending.
14. PR15 adds a portable read-only endpoint runtime with per-endpoint
    serialization, queue deadlines, cancellation, spacing, typed bounded retry,
    circuit admission, and schema-v2 observations. The simulator can serve a
    bounded deterministic response sequence, including a tested 500-to-200
    retry. Native source wiring and the Windows workspace checks now pass with
    the documented SQLCipher Perl/libclang prerequisites; macOS verification
    remains delegated to the configured CI host. Support-bundle UX,
    fuzz/property tests, live XML evidence, and measured performance budgets
    remain pending.
15. PR16 adds the portable Party Outstanding Confidence Receipt foundation:
    Bills & Payments schema v2, an exact unbound observation parser, and a
    fail-closed exact-arithmetic reconciliation engine. Company/party/date/
    direction/profile/scope reuse is rejected, On Account remains aggregate,
    and unknown, incomplete, unobserved, or unauthoritative empty evidence
    cannot become zero. Request construction,
    runtime/mirror/UI wiring, and live Education semantics remain pending, so
    the pack stays unknown and unsupported.
16. PR17 adds a dormant, feature-gated native Ledger Outstandings request probe
    with exact request/scope commitments and a synthetic Education fixture
    protocol. It has no production dispatch, parser authority, mirror, UI, or
    support claim.
17. PR18 adds an isolated qualification-only runner with distinct expiring
    preflight/dispatch consents, exact 13-POST bracketed execution, bounded
    byte-for-byte response comparison, separate UI-after/save gates, and a
    dedicated no-authority receipt. Candidate application/HTTP/transport
    failures are modeled with trailing identity brackets and no retry; missing
    bodies make repeatability not established. Receipt save is exact-output-
    bound and consumes a repository-confined target. UI evidence uses typed,
    bounded, deny-unknown rows plus scenario-exact settlement presence. Checked-in local profiles
    remain invalid examples and no live POST or Bills support claim has been
    made.
18. Keep selected-read support scope-bound: never copy the selected feature
    states without their scope and full review commitments, and do not promote
    broad read or completeness claims from this evidence.
19. Record Windows and macOS installer evidence before a supported release
    claim.

## Managed repository controls

1. Require pull requests and the required checks on `master`.
2. Configure the area, severity, and type labels defined in
   [managed Git guidance](./bootstrap/managed-git.md).
3. Enable private vulnerability reporting.
4. Require [review-checklist.md](../review-checklist.md) links in pull requests.
5. Use [rectification guidelines](./rectify-guidelines.md) for regressions.

## Open-source operating model

1. Publish versioned release notes and a support matrix.
2. Define the Node, pnpm, Rust toolchain, and Rust minimum-supported-version
   policy.
3. Run formatting, lint, test, build, and platform packaging checks in CI.
4. Configure organization-controlled Windows signing and macOS notarization
   credentials using protected release environments. The process and rollback
   gates are documented in [release-process.md](./release-process.md).
5. Review external governance conventions before adopting them and record
   exact mappings, exceptions, and ownership in public project documentation.

## Completion criteria

- A fresh clone builds without paths outside the repository except standard
  toolchain and application-data locations.
- Native Windows and macOS development and package builds have current evidence.
- Tally, DSC, document, sync, and persistence workflows have regression checks.
- No public artifact contains secrets, personal/customer data, or contributor
  machine paths.
- Governance and rectification controls are enforced in the managed repository.
- Production downloads are signed/notarized and carry checksums and provenance;
  unsigned CI smoke bundles are not represented as releases.
