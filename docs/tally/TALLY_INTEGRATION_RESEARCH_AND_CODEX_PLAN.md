# Bridge × Tally: Trustworthy Integration Research and Codex Execution Plan

**Repository:** `lamemustafa/bridge`  
**Research date:** 2026-07-14  
**Target:** Bridge 0.2.x, React 19 + TypeScript, Rust/Tauri 2, SQLite  
**Primary Tally target:** TallyPrime  
**Compatibility target:** Tally.ERP 9 only where the existing XML path continues to work safely  
**Testing constraint:** The available real Tally installation is an Education edition; no license restriction may be bypassed or worked around.

---

## 0. Executive directive

Bridge should not compete by claiming the longest checklist of Tally features.

It should become the **most inspectable, honest, recoverable, and accounting-safe Tally connector**:

1. It never reports “synced” merely because an HTTP request returned 200 or because rows appeared.
2. It always identifies the exact company context used for a request.
3. It records what was requested, what Tally returned, what was accepted, what was rejected, and what remains unknown.
4. It treats reads, retries, imports, deletions, and reconciliation as separate correctness problems.
5. It defaults to data minimisation and read-only behaviour.
6. It makes failure understandable to an accountant, not only to a developer.
7. It can be tested thoroughly without a live or licensed Tally instance through a faithful local simulator.
8. It enables alternate transports only after they prove semantic parity with the compatibility baseline and demonstrate a measured operational benefit.
9. It never promises rollback, real-time behaviour, completeness, or accuracy without evidence.

The memorable product layer should be called the **Tally Truth Layer**. Its user-facing pieces are:

- **Capability Passport** — what this Tally instance can actually do.
- **Proof of Sync** — evidence that a run is complete, partial, stale, unsupported, or failed.
- **Gap Map** — fields and records requested versus obtained versus intentionally omitted.
- **Safe Import Preview** — the exact write set and expected effect before anything is posted.
- **Truth States** — `Verified`, `Partial`, `Stale`, `Unsupported`, `Failed`, never a vague green check.

---

## 1. How the research was divided

This plan was produced through six independent review tracks. They should also be represented during implementation and review.

| Review track | Persona | Primary question |
|---|---|---|
| Protocol | Tally/TDL integration engineer | What does Tally actually guarantee at HTTP, XML, JSONEX, ODBC, and import-response levels? |
| Reliability | Distributed-systems/SRE engineer | What happens under concurrency, timeout, partial response, retry, interruption, and restart? |
| Accounting | Chartered-accountant/data-integrity reviewer | What evidence is required before totals, GST data, balances, or writebacks can be trusted? |
| Security | Local-agent and privacy engineer | Which data leaves Tally, which data is retained, and how can logs/support bundles avoid leaking books? |
| Product | Finance-operations UX designer | How does an accountant understand setup, freshness, gaps, conflicts, and recovery? |
| Open source | Rust/Tauri maintainer and test architect | How can contributors reproduce behaviour without proprietary data or a live Tally installation? |

A change is not “done” merely because the implementation track approves it. Protocol, reliability, accounting, security, product, and open-source acceptance criteria all apply.

---

## 2. Evidence baseline

### 2.1 What Tally officially supports

Official Tally documentation establishes the following useful baseline:

- TallyPrime supports external integration through TDL, external APIs/HTTP/DLL methods, XML, JSON, and ODBC.
- XML over HTTP is built in. Requests and responses use an `ENVELOPE` with `HEADER` and `BODY`.
- Tally export-envelope application success is expressed as `STATUS=1` and failure as `STATUS=0`; other or missing values are protocol errors. HTTP success alone is insufficient.
- At least one company must be loaded for integration, and the HTTP server is commonly exposed on port 9000.
- Tally can respond using UTF-8, UTF-16, or ASCII.
- TallyPrime 7.0 and later support native JSON. Tally recommends JSONEX for new integrations because it produces valid and predictable arrays and keys; Bridge must not assume that it is faster or semantically equivalent to XML without measurements and parity evidence.
- Tally’s JSON integration documentation explicitly treats `svCurrentCompany` as mandatory; omitting it can cause import/export against whichever company happens to be active.
- Third-party applications can read ledgers, vouchers, stock items, cost centres, reports, and other collections through HTTP/XML or ODBC.
- Tally can accept masters and vouchers. Import responses expose counters such as created, altered, and errors; invalid or duplicate entries are reflected in error results.
- TDL can add in-product UI, reports, validation, alerts, role-based execution, and internal workflow logic.
- TallyPrime 7.1 was released on 2026-05-20. It should be in the live validation matrix, but the integration must not assume every user is on 7.1.
- Tally.ERP 9 is no longer in active development. New capabilities should target TallyPrime, with ERP 9 treated as a compatibility profile rather than a parity target.

### 2.2 Education edition policy

Official public documentation reviewed for this plan does not provide a complete, integration-specific capability table for Education mode. Current Tally guidance does document the material voucher-date restriction: Education mode permits voucher entry on the 1st, 2nd, and 31st of a month. The 31st is literal and must not be generalised to the last calendar day of shorter months.

Therefore Bridge must use an **observed capability policy**:

- Never infer a write or date-range capability from folklore.
- Record whether a capability is `Documented`, `Observed`, `Inferred`, or `Unknown`.
- Do not bypass or simulate a licensed state.
- Default to read-only probes.
- Run write tests only against a clearly synthetic company, only on a documented permitted date (1st, 2nd, or 31st), and only after a non-destructive capability check succeeds.
- Include a disallowed ordinary date as a negative test; do not alter system time or otherwise work around Education-mode limits.
- Mark test results as applying to the exact Tally product/release/mode observed.
- Preserve the distinction between “not supported”, “not permitted in this mode”, “not configured”, and “not yet tested”.

### 2.3 What adjacent products teach us

These product pages are useful as workflow benchmarks, not as proof of accounting correctness:

| Product | Published integration pattern | What Bridge should borrow | How Bridge should improve it |
|---|---|---|---|
| RazorpayX | Very low-friction Tally connection, two-way payment sync, scheduled and manual sync, automatic reconciliation, vendor-payment and bank-statement flow | Fast setup, explicit manual refresh, scheduled sync, closed-loop payment state | Publish the exact sync checkpoint, source records, reconciliation evidence, exceptions, and last verified state |
| Volopay | Automated exchange, data mapping, monitoring/alerts, custom workflows, categorisation rules, transaction attachments | Mapping UX, proactive alerts, configurable rules, operator dashboard | Every suggestion must carry confidence and reasons; every alert must include remediation and whether retry is safe |
| Vyapar TaxOne | Multi-format source ingestion, bank-statement-to-voucher flow, ledger suggestions, review-before-bulk-push, GST reconciliation and checks | Review/verify gate, bulk import preview, ledger mapping assistance, reconciliation workspace | Separate deterministic rules from probabilistic suggestions; never silently create a ledger or voucher; provide an exact mutation diff |
| Sage Expense Management | Chart-of-accounts/vendor/project import, receipt-to-transaction matching, approved-item posting, two-way accounting sync | Master import, canonical coding model, evidence attachment, approval-to-post lifecycle | Make source identity and idempotency explicit; retain an immutable audit record of the approved payload and Tally response |
| Happay / EnKash | Multi-source capture, policy checks, approval flows, audit-ready records, reconciliation, ERP connectivity | Exception queues, review workflows, policy controls, audit trail | Keep the Tally connector focused: validation and accounting evidence belong in the sync contract, not in broad “AI automation” claims |
| `labs-infinitum/tally-sdk-rs` | Typed reads for masters/vouchers/reports, master creation, detailed import counters, retries, live integration tests | Broader typed surface, import-result modelling, report access, live fixtures | Retain Bridge’s async and security model; add app-level status parsing, global serialization, simulator tests, and proof of completeness |

### 2.4 Product conclusion

The recurring market promises are:

- easy setup;
- two-way or scheduled sync;
- field mapping and auto-categorisation;
- alerts and operator visibility;
- reconciliation;
- approval or review before posting;
- audit-ready records.

Bridge should implement those only where they serve the Tally workflow. Its unique advantage should be **truthful evidence**:

> “Here is the company, capability profile, range, count, identity set, checksum, reconciliation result, skipped data, and precise reason this run is Verified or Partial.”

---

## 3. Current Bridge audit

> **Implementation note (2026-07-14):** This section records the baseline that
> motivated the plan. Several P0/P1 items have since been implemented in the
> working tree. The adjacent [support matrix](./support-matrix.md) is the
> authoritative, evidence-scoped implementation status; roadmap text is never
> a support claim.

### 3.1 What is already good

The current codebase has a respectable security and maintainability baseline:

- Tally connections are loopback-only.
- Redirects are disabled.
- Request timeouts and response-size caps exist.
- XML values are escaped before insertion into TDL.
- Parsing uses a streaming-style XML event reader rather than regexes.
- Current fetches intentionally minimise fields.
- There are synthetic parser tests.
- The repository has cross-platform CI, formatting, Clippy with warnings denied, Rust tests, bundle smoke tests, dependency auditing, license inventory, and governance guidance.
- The latest merged security PR reports 36 Rust tests passing plus native Windows/macOS checks and bundle smoke.

These controls should be preserved.

### 3.2 Immediate correctness defect: serialization is not global

`TallyClient` owns a `SerialTallyQueue`, but every Tauri command creates a fresh `TallyClient`.

That means the mutex only serialises requests made through one temporary client. Two concurrent Tauri commands can each own a separate mutex and post to the same Tally endpoint simultaneously.

**Required fix:** manage a shared `TallyRuntime` in Tauri state. It must reuse one endpoint session and one queue per canonical endpoint.

### 3.3 HTTP success is currently mistaken for application success

The current `post_xml` path calls `error_for_status()` and returns the body. It does not first classify:

- Tally export-envelope `STATUS = 0` or `1`, treating any other or missing value as a protocol error;
- failure descriptions;
- `LINEERROR`;
- import counters;
- malformed success envelopes;
- unexpected content such as an HTML error page returned with status 200.

**Required fix:** introduce a typed application-response parser and make every command return a typed failure category.

### 3.4 Company context is not universally proven

Ledger and voucher requests include `SVCURRENTCOMPANY`, which is good. Company enumeration naturally cannot be scoped the same way. However, Bridge does not yet verify that the returned objects belong to the intended company, nor does it pin a stable company identity.

Official Tally documentation warns that omitting the current-company variable can exchange data with the wrong active company.

**Required fix:** every company-scoped operation must carry explicit company context and, where possible, verify a stable company identity in the response or through a paired probe.

### 3.5 Models are too shallow for honest accounting work

Current vouchers include only:

- date;
- type;
- number;
- party name;
- an optional identifier.

Current ledger data is similarly minimal. The local schema does not yet store vouchers, voucher lines, inventory lines, tax lines, bill allocations, or sync-run manifests.

This data is enough for a browser-like preview, but not enough to claim:

- complete voucher sync;
- balanced accounting entries;
- GST return preparation;
- inventory reconciliation;
- reliable writeback;
- deletion detection;
- conflict resolution.

**Required fix:** create canonical, exact models before advertising higher-level outcomes.

### 3.6 Sync and conflict modules are placeholders

The current sync engine is a small plan structure; conflict resolution is only an enum. The existing SQLite schema contains useful beginnings—outbox, sync log, alter-ID cache, conflict queue, companies, and ledgers—but the operational protocol is not implemented.

### 3.7 The UI reports activity, not truth

The UI exposes direct fetch buttons and shows row counts, with lists capped to 100 visible records. It does not show:

- whether the result is complete;
- what date range was actually honoured;
- the last durable checkpoint;
- whether Tally returned an application error;
- whether records were skipped or malformed;
- reconciliation status;
- age/freshness;
- unsupported fields;
- retry state;
- company mismatch risk.

The replacement should present a run lifecycle and Truth State rather than “N loaded”.

### 3.8 Further protocol hardening gaps

The implementation plan must address:

- backend date validation, not only frontend validation;
- UTF-16/BOM and encoding handling;
- cancellation and deadlines;
- safe retry classification;
- jittered backoff;
- per-request IDs;
- bounded diagnostic capture;
- content-type and payload-shape checking;
- adaptive request windows for large companies;
- streaming or staged parsing instead of retaining a full 32 MiB body;
- deterministic canonicalisation and hashing;
- write idempotency;
- periodic deletion sweeps.

---

## 4. Product principles and non-negotiable invariants

### 4.1 Truth before convenience

A run may be:

- `Verified` — all requested scopes completed, validation passed, checkpoint committed, and reconciliation passed.
- `Partial` — useful data exists, but one or more declared scopes, windows, validations, or records did not complete.
- `Stale` — the last verified state is older than the configured freshness target.
- `Unsupported` — the capability is unavailable for the detected product/release/mode.
- `Failed` — no new durable state was committed.

There is no generic `Success` state.

### 4.2 Company pinning

For every company-scoped request:

- the company parameter is required and non-empty;
- the request explicitly sets the current company;
- the selected company is represented internally by a stable identity where available;
- a response that indicates a different company blocks commit;
- a rename updates a label, not the record identity.

### 4.3 Atomic checkpoints

A checkpoint advances only after:

1. transport completes;
2. application response is accepted;
3. parsing completes;
4. validation completes;
5. staging writes complete;
6. reconciliation completes or is explicitly recorded as unavailable;
7. the snapshot is committed atomically.

Cancellation, crash, parse error, or mismatch must leave the previous verified checkpoint intact.

### 4.4 Exact money

Never use floating-point values for accounting amounts.

Use a canonical decimal representation containing:

- original raw value;
- normalised sign;
- integer coefficient;
- scale;
- optional currency;
- parsing/rounding status.

Any lossy conversion must be explicit and must prevent `Verified` status.

### 4.5 Read retry and write retry are different

- Safe, idempotent reads may retry selected transient failures.
- Validation errors, company mismatches, malformed payloads, and application failures do not auto-retry.
- Writes do not auto-retry until an idempotency strategy is proven for that operation.
- A timed-out write is `OutcomeUnknown`, not `Failed`, until re-read or import-log evidence resolves it.

### 4.6 Data minimisation

Fetch profiles are feature-specific:

- connection/capability;
- company identity;
- master summary;
- voucher accounting;
- GST;
- inventory;
- reconciliation;
- write verification.

Do not fetch narrations, addresses, tax identifiers, item details, or transaction lines unless a declared feature requires them. Diagnostic logs never contain raw payloads by default.

### 4.7 No fictional rollback

Do not call an operation “rollback” unless Bridge has implemented and verified a compensating Tally operation. The safe default is:

- stop;
- preserve evidence;
- mark outcome;
- guide the operator;
- allow a reviewed compensating action.

---

## 5. Target architecture

### 5.1 TallyRuntime

Add a Tauri-managed singleton:

```text
TallyRuntime
└── endpoint_sessions: HashMap<EndpointKey, Arc<TallySession>>

TallySession
├── canonical endpoint
├── shared reqwest client
├── one request gate / queue
├── capability profile cache
├── health and circuit state
├── request sequence
└── cancellation registry
```

`EndpointKey` must be canonicalised after current loopback validation, so `localhost`, `127.0.0.1`, and equivalent loopback inputs do not accidentally create competing queues for the same endpoint.

The command layer accepts `tauri::State<TallyRuntime>` rather than constructing a new client.

### 5.2 Transport abstraction

```rust
#[async_trait]
trait TallyTransport {
    async fn probe(&self, ctx: RequestContext) -> Result<TransportProbe, TallyError>;
    async fn execute(&self, request: TallyRequest, ctx: RequestContext)
        -> Result<TallyRawResponse, TallyError>;
}
```

Target-state implementations; only the XML path exists in the runtime today:

1. `XmlHttpTransport` — compatibility baseline.
2. `JsonExHttpTransport` — TallyPrime 7.0+ version-gated path after capability negotiation and XML parity validation.
3. `OdbcDiagnosticTransport` — optional, read-only count/report verification; never a silent fallback for writes.
4. `SimulatedTallyTransport` or local fake HTTP server — deterministic tests.

Avoid forcing the whole codebase to know XML or JSON shapes. Both transports normalise into the same canonical domain types.

### 5.3 Request context

Every request carries:

- request ID;
- endpoint session ID;
- operation kind;
- company identity/name;
- deadline;
- cancellation token;
- maximum response bytes;
- retry policy;
- data profile;
- expected response shape;
- safe-to-retry flag;
- redaction policy.

### 5.4 Typed error model

Minimum categories:

```text
Configuration
EndpointRejected
ConnectionRefused
TimeoutBeforeSend
TimeoutAfterSend
ResponseTooLarge
UnsupportedEncoding
UnexpectedContent
MalformedPayload
TallyApplicationError
TallyLineError
CompanyMismatch
CapabilityUnsupported
ValidationError
ReconciliationMismatch
Cancelled
OutcomeUnknown
Database
InvariantViolation
```

Each error contains:

- stable machine code;
- safe operator message;
- optional technical detail;
- retry classification;
- whether local state changed;
- whether Tally state may have changed;
- remediation steps;
- request/run ID.

### 5.5 Capability Passport

Illustrative future-state shape, not current Bridge capability evidence:

```json
{
  "observedAt": "...",
  "endpoint": "127.0.0.1:9000",
  "product": "TallyPrime",
  "release": "7.1",
  "mode": {
    "value": "Education",
    "confidence": "Observed"
  },
  "loadedCompanies": [
    {
      "name": "Synthetic Bridge Test",
      "stableId": "...",
      "identityConfidence": "Observed"
    }
  ],
  "transports": {
    "xml": { "read": "Verified", "write": "Unknown" },
    "jsonex": { "read": "Verified", "write": "Unknown" },
    "odbc": { "read": "NotConfigured" }
  },
  "features": {
    "companyRead": "Verified",
    "ledgerRead": "Verified",
    "voucherRead": "Partial",
    "voucherWrite": "Unsupported",
    "gstFields": "Unknown"
  },
  "warnings": []
}
```

Every field records provenance: `Documented`, `Observed`, `Inferred`, or `Unknown`.

### 5.6 Canonical domain model

Do not use names as primary identities.

Suggested identifiers:

```text
TallyObjectIdentity
- company_id
- object_type
- guid?
- remote_id?
- master_id?
- alter_id?
- fallback_fingerprint?
- identity_confidence
```

A fallback fingerprint is not equivalent to a stable Tally ID. It must be labelled and may not support automatic rename/deletion handling.

Core entities:

- company;
- group;
- ledger;
- voucher type;
- voucher;
- voucher ledger entry;
- inventory entry;
- batch allocation;
- bill allocation;
- cost-centre allocation;
- tax line;
- stock item;
- unit;
- godown;
- currency;
- sync run;
- sync scope;
- sync window;
- checkpoint;
- tombstone;
- mapping rule;
- import job;
- import item;
- import result;
- conflict;
- audit event.

### 5.7 Proposed SQLite evolution

Use versioned SQLx migrations, not a monolithic initial schema edit.

Add at least:

```text
tally_endpoints
tally_capability_profiles
tally_companies
tally_groups
tally_ledgers
tally_stock_items
tally_voucher_types
tally_vouchers
tally_voucher_entries
tally_inventory_entries
tally_bill_allocations
tally_cost_centre_allocations
tally_tax_lines
tally_sync_runs
tally_sync_scopes
tally_sync_windows
tally_sync_errors
tally_checkpoints
tally_tombstones
tally_reconciliation_results
tally_mapping_rules
tally_import_jobs
tally_import_items
tally_import_results
```

Important columns:

- stable internal UUID;
- source IDs and their confidence;
- company ID;
- raw source hash;
- canonical hash;
- observed alter ID;
- first/last seen run;
- deleted/tombstoned status;
- validation status;
- source transport;
- source release;
- schema version.

### 5.8 Proof of Sync manifest

Each run produces an immutable manifest:

```json
{
  "runId": "...",
  "state": "Verified",
  "company": {
    "requestedName": "...",
    "verifiedIdentity": "..."
  },
  "capabilityProfileId": "...",
  "startedAt": "...",
  "finishedAt": "...",
  "transport": "XML",
  "scopes": [
    {
      "name": "vouchers",
      "requestedRange": ["2026-04-01", "2026-06-30"],
      "windows": 13,
      "receivedRecords": 4216,
      "acceptedRecords": 4216,
      "rejectedRecords": 0,
      "canonicalHash": "...",
      "checkpointBefore": "...",
      "checkpointAfter": "..."
    }
  ],
  "reconciliation": {
    "recordCounts": "Passed",
    "entryBalance": "Passed",
    "reportTieOut": "Unavailable"
  },
  "gaps": [],
  "warnings": []
}
```

The manifest must not contain raw voucher data or sensitive values unless the user explicitly creates an encrypted support bundle.

### 5.9 Gap Map

For every scope, show:

- requested fields;
- fetched fields;
- intentionally omitted fields and why;
- unavailable fields;
- parse failures;
- unsupported object types;
- skipped records;
- lossy normalisations;
- reconciliation limitations.

This prevents a minimal export from being misrepresented as a full accounting mirror.

---

## 6. Ordered pull-request roadmap

Each PR must follow `AGENTS.md`, the repository review checklist, rollback/migration notes, and the Tally/security impact sections.

### PR 00 — Tally trust contract and ADRs

**Branch:** `chore/tally-truth-contract`  
**Risk:** low  
**Purpose:** Freeze the correctness contract before implementation.

#### Deliverables

- `docs/tally/README.md`
- `docs/tally/support-matrix.md`
- `docs/tally/privacy-model.md`
- `docs/adr/00x-tally-transport-negotiation.md`
- `docs/adr/00x-tally-company-identity.md`
- `docs/adr/00x-tally-sync-truth-states.md`
- `docs/adr/00x-tally-write-safety.md`
- Update `docs/step-by-step-roadmap.md`.

#### Decisions to record

- XML is the compatibility baseline.
- JSONEX is capability-negotiated for TallyPrime 7.0+.
- ODBC is diagnostic/read-only until separately justified.
- External integration remains the default.
- A companion TDL package is optional and may only be introduced after a documented gap cannot be solved safely from Bridge.
- Tally.ERP 9 is compatibility-only.
- Education mode is observed, not assumed.
- No remote plaintext Tally connection is added in this roadmap.

#### Acceptance

- No runtime behaviour changes.
- All terminology and Truth States are defined.
- `pnpm run build`, Cargo formatting, tests, Clippy, license checks remain green.

---

### PR 01 — Shared Tally runtime and real global serialization

**Branch:** `fix/tally-shared-runtime`  
**Risk:** high correctness / low product surface  
**Purpose:** Ensure there is one queue and one HTTP client per endpoint.

#### Code changes

- Add `src-tauri/src/tally/runtime.rs`.
- Add `TallyRuntime`, `EndpointKey`, and `TallySession`.
- Register runtime through `tauri::Builder::manage`.
- Change Tally commands to use `tauri::State`.
- Preserve current frontend command signatures where practical.
- Canonicalise equivalent loopback endpoint inputs.
- Remove the artificial 500 ms sleep unless measurements prove it is needed; make spacing configurable and observed in capability/session state.
- Make cancellation release the request gate.

#### Tests

- Two concurrent commands to the same endpoint never exceed one in-flight request.
- Commands to distinct endpoint keys can proceed independently.
- Equivalent loopback aliases share a session.
- A cancelled request does not deadlock the next request.
- Client/session reuse is observable in tests without exposing internals to the UI.
- A panic/error path releases the gate.

#### Acceptance

- Current connection/company/ledger/voucher commands still work.
- No concurrent POSTs per canonical endpoint.
- Existing security restrictions remain unchanged.

#### Implemented reservation-liveness rectification (2026-07-16)

Reviewed setup and selected-read qualification now hold an opaque owner-bound
reservation lease across every network and database await. The lease keeps the
endpoint session alive, is excluded from capacity eviction, and releases only
its exact review on cancellation, task abort, panic unwinding, early return, or
drop. Consume and replacement are idempotent; replacement preserves the
original freshness origin, and a stale lease cannot clear or consume a newer
review. Ordinary read admission remains atomically exclusive with the lease.
Native regressions cover drop, task abort, an aborted pending HTTP
qualification, active-request cleanup, stale-owner isolation, single-use
consume, freshness-preserving replacement, read exclusion, and capacity
eviction. Qualification dispatch accepts the opaque lease itself and verifies
its originating runtime instance, so knowing a public review ID or constructing
a second runtime cannot borrow another task's authority. Migration v8 also
consumes the full review commitment beside the saved Passport/company in the
same SQLite transaction. Exact replay after a lost commit acknowledgement
returns the original references; changed payload reuse fails closed, and the
consumption authority is immutable.

---

### PR 02 — Protocol envelope, encoding, validation, and typed errors

**Branch:** `fix/tally-protocol-contract`  
**Risk:** high  
**Purpose:** Stop treating HTTP 200 and parseable rows as proof of Tally success.

#### Code changes

- Add `TallyError`.
- Add request IDs and `RequestContext`.
- Validate company and date values in Rust.
- Parse XML response header `STATUS`.
- Parse failure descriptions and `LINEERROR`.
- Parse import counters into a typed result even before write UI exists.
- Detect unexpected HTML/plain text and malformed envelopes.
- Detect BOM/declared encoding and support documented UTF-8/UTF-16 responses.
- Preserve strict response byte limits.
- Classify safe read retries versus permanent errors.
- Do not auto-retry writes.
- Ensure every company-scoped request explicitly sets the current company.
- Add a post-response company verification hook.

#### Tests

- HTTP 200 + `STATUS=0` is a failure.
- HTTP 200 + line error is a typed failure.
- HTTP 500 is a transport failure.
- HTML with HTTP 200 is `UnexpectedContent`.
- UTF-8 and UTF-16 fixtures produce equal canonical values.
- malformed/truncated XML does not advance state.
- invalid dates/company names fail before network access.
- sensitive payload fragments are absent from `Display`/serialised errors.

#### Acceptance

- Frontend receives stable error codes and remediation.
- No command reports success solely from HTTP status.
- No write response can be accepted without import-result parsing.

---

### PR 03 — Deterministic Tally simulator and fixture corpus

**Branch:** `chore/tally-simulator`  
**Risk:** low runtime / high leverage  
**Purpose:** Make integration reliability reproducible for every contributor.

#### Deliverables

- A test-only local HTTP server or feature-gated simulator binary.
- Synthetic fixture corpus with no customer data.
- Fixture generation documentation.
- A scenario DSL or clear scenario enum.

#### Required scenarios

- `/status` for TallyPrime, ERP 9, unknown product.
- normal XML export.
- application failure in HTTP 200.
- line errors.
- empty collection.
- duplicate records.
- wrong-company response.
- UTF-16 response.
- malformed XML.
- truncated response.
- response over the byte cap.
- slow headers.
- slow body.
- connection reset before body.
- connection reset after request may have been processed.
- inconsistent date-filter behaviour.
- import created/altered/ignored/error counters.
- duplicate import.
- partial import.
- delayed import response with unknown outcome.
- JSONEX valid arrays.
- Canonical semantic-projection fixtures that explicitly exclude transport
  provenance; the existing synthetic JSON reference is not an official Tally
  JSONEX envelope and does not prove XML/JSONEX parity.
- unsupported capability.

#### Acceptance

- Tests run without Tally installed.
- Simulator is bound to loopback and disabled from production builds unless explicitly feature-gated.
- Fixtures are synthetic and reviewed for data leakage.

#### Implemented date-filter rectification (2026-07-16)

- The simulator now has an explicit synthetic inconsistent-date-filter
  scenario: a declared July 2026 request window with a returned June voucher.
- Core canonicalization validates the requested calendar window and every
  voucher date, rejecting malformed, reversed, before-window, and after-window
  values before any canonical window or capability canary can exist.
- Reconciliation keeps its independent out-of-range evidence check as defense
  in depth. This portable behavior does not prove that a live Tally release
  honors date filters; that support claim still requires exact live evidence.

---

### PR 04 — Capability Passport and safe setup wizard

**Branch:** `feat/tally-capability-passport`  
**Risk:** medium  
**Purpose:** Replace “reachable” with an evidence-based compatibility profile.

#### Probe categories

- product and release;
- endpoint responder reachability;
- loaded/active companies;
- stable company identity availability;
- XML read;
- JSONEX read;
- ODBC configuration as an optional diagnostic;
- encoding behaviour;
- practical response limit;
- selected master/voucher reads;
- write state: `Unknown` until explicitly tested;
- mode/license confidence, without claiming undocumented details.

#### UI

A guided setup flow:

1. Detect Tally on the configured local port.
2. Explain how to enable the HTTP server if unavailable.
3. Enumerate loaded companies.
4. Require explicit company selection.
5. Run safe read probes.
6. Present Capability Passport.
7. Show warnings and unsupported capabilities.
8. Save configuration only after the user sees the exact scope.

#### Acceptance

- “Reachable” is not equivalent to “compatible”.
- Every capability has provenance/confidence.
- Education mode never triggers write probes automatically.
- Raw server text is not used as the primary UI.

#### Implemented truth-state rectification

Capability Passport profile v2 now records and persists explicit feature
evidence for endpoint responder reachability, loaded-company presence, stable company identity,
the exact decoded response encoding, practical response-limit qualification,
company/ledger/voucher reads, and write capability. The connection probe remains
read-only: ledger and voucher reads, practical limits, and writes stay
`Unknown` until a separately scoped probe establishes them. ODBC and the TDL
companion also remain `Unknown` with `configuration_not_observed`; Bridge no
longer reports either as observed `NotConfigured` without inspecting its
configuration. Legacy profile-v1 JSON remains readable with an empty feature
map and cannot acquire invented evidence during deserialization.

The setup flow now separates observation from persistence. `Probe and discover`
updates only in-memory review state. Saving requires a second explicit action
within five minutes, an opaque review-ID-bound single-use commitment over the
canonical endpoint, original observation time, connection result, complete ordered
company result, and Passport, plus
one GUID-bearing company from that same cached probe. Only that selected
company pin and the reviewed Passport are stored in one local database
transaction using the original observation timestamp; changing endpoint or
company scope requires another review and save. The cache uses conditional
reservation: a stale or concurrent review cannot consume the current probe,
and an atomic local-store failure releases the same fresh review for retry.
An existing reservation rejects a concurrent probe before another live request
is sent. If in-memory token cleanup fails after the database commit, the save
still returns its truthful durable success plus a restart warning; cleanup
cannot disguise a committed setup as a failed save.
Duplicate normalized company GUIDs and invalid identity fields cannot claim
stable identity or be saved. Company GUID matching is ASCII case-insensitive
across review selection and database identity resolution, while persistence
retains the exact spelling observed from Tally instead of accepting
caller-controlled casing.
Late save results cannot restore live state after UI invalidation. No Tally
setup action performs a write to Tally.

Selected-read qualification now uses two exact scope-bound profiles:
`bridge.tally.ledgers/1` and `bridge.tally.vouchers/3`. The latter requires the
response to echo the exact requested maximum-31-day window, including on an
empty result. Both require strict wrapper structure, company GUID and normalized
name agreement, exact record counts, case-folded GUID collision detection, and
canonical identity/text/amount/date/AlterID/hash validation. Empty execution is
recorded as identity-not-applicable, never as proof that the source is empty or
complete. Ledger failure skips voucher qualification; cancellation restores the
parent review. The endpoint reservation is atomically exclusive with ordinary
read admission, and a replacement review retains the original probe freshness
origin rather than extending it.

Migration v7 stores the exact company/profile/window scope and two immutable
observations in the same transaction as the Passport and company pin. The
repository recomputes the full scope commitment, including hashes, decoded
encoding, outcome and verification states, and a composite foreign key prevents
an observation from borrowing another snapshot's capability item. Raw rows are
discarded. Decoded-response fingerprints are local encrypted pseudonymous
evidence, not wire hashes, and are excluded from UI serialization and public
support exports. Broad ledger/voucher features, completeness, accounting
correctness, writes, and public release support remain `Unknown`. See
[ADR 0015](../adr/0015-tally-selected-read-qualification-authority.md).

---

### PR 05 — Canonical identities, exact amounts, and versioned migrations

**Branch:** `feat/tally-canonical-model`  
**Risk:** high data model  
**Purpose:** Build a trustworthy local mirror foundation.

#### Code changes

- Add exact decimal parser.
- Add stable/candidate identity model.
- Add voucher, entry, inventory, tax, bill, and allocation types.
- Add versioned SQLx migrations.
- Replace name-as-primary-key semantics.
- Add raw and canonical hashes.
- Store source transport/release and identity confidence.
- Add repository APIs and transaction boundaries.
- Preserve migration rollback notes.

#### Property tests

- decimal parse/format round trips;
- sign conventions;
- arbitrary whitespace and valid Tally amount forms;
- stable canonical ordering;
- hash reproducibility;
- Unicode names;
- rename does not create a second entity when stable ID is available;
- fallback identity is never upgraded silently without an audit event.

#### Acceptance

- No floating-point accounting values.
- A company/ledger rename does not depend on mutable display name where a stable identity exists.
- Existing schema users are migrated safely or the migration is explicitly gated for pre-production data.

---

### PR 06 — Atomic, resumable full snapshot pipeline

**Branch:** `feat/tally-snapshot-sync`  
**Risk:** high  
**Purpose:** Move from button fetches to a durable sync protocol.

#### Pipeline

```text
Prepare
→ capability/profile check
→ company identity check
→ plan scopes/windows
→ extract
→ normalise
→ validate
→ stage
→ reconcile
→ commit
→ emit Proof of Sync
```

#### Design

- Adaptive date windows for voucher reads.
- Adaptive collection windows or filters where supported.
- Stage data in run-scoped tables/records.
- Avoid one 32 MiB in-memory string for large exports.
- Persist window completion for resume.
- Keep old verified snapshot active until commit.
- Deterministic ordering and hashing.
- Explicit Gap Map.
- Preserve source counts and parse counts.
- Use a cancellation token.
- Track progress by phase and window, not fake percentages.

#### Acceptance

- A crash/cancel leaves the previous snapshot intact.
- Resume does not duplicate records.
- Re-running unchanged data gives identical canonical hashes.
- A missing window produces `Partial`, not `Verified`.
- A response date outside the requested range is detected and handled according to the declared policy.

#### Implemented bounded-streaming and adaptive-window slice (2026-07-16)

Bridge now has a parallel decoded-only HTTP response path for ordinary native
Tally reads. It incrementally detects UTF-8, UTF-8 BOM, UTF-16LE, or UTF-16BE,
enforces encoded and decoded limits while chunks arrive, and computes both
commitments without retaining a complete encoded body. The exact-wire API is
unchanged for live qualification. Protocol regressions cover every byte split,
one-byte chunks, surrogate pairs, malformed tails, exact/plus-one caps, and
digest equivalence. The current XML parsers still consume one complete decoded
string, so this is not yet a full event-streamed record pipeline.

Snapshot state v4 binds a closed adaptive policy and immutable one-day canary.
Only the typed voucher-window response-limit outcome can create deterministic
calendar-midpoint children under an immutable root. The row-hashed split graph
is generation-CAS persisted before child dispatch; resume never replans or
refetches a split parent. Split parents cannot carry evidence, graph drift or
overlap fails closed, and reconciliation uses leaf IDs only. A one-day overflow
or leaf-limit exhaustion becomes an explicit terminal Gap without advancing the
previous checkpoint. Exact lost-ack observation replay is idempotent; a changed
replay is a conflict. Tests cover recursive/leap-date splitting, graph tamper,
post-split crash/resume, one-day overflow, leaf limits, and checkpoint
preservation. Per-record keys still live in the bounded durable JSON row and are
saved after each record; a relational/chunk cursor plus one-pass parser sink is
the next PR06 resilience/performance slice.

---

### PR 07 — Incremental sync and deletion awareness

**Branch:** `feat/tally-incremental-sync`  
**Risk:** high  
**Purpose:** Reduce work without sacrificing completeness.

#### Rules

- Use Tally change/alter identifiers only after the capability is verified for the exact object type and release.
- Checkpoints are scoped by company, object type, transport, schema version, and query profile.
- Maintain an overlap window where needed.
- Deduplicate by stable identity and canonical content.
- Periodically run a full identity sweep.
- Records absent from an incremental feed are not automatically deleted.
- A tombstone is created only after a deletion rule is proven.
- If an identifier regresses/resets, invalidate the incremental checkpoint and require a full snapshot.

#### Acceptance

- Incremental output equals a subsequent full snapshot for the same final Tally state.
- Edits, renames, and deletions are covered by tests.
- Restart/resume is idempotent.
- Unsupported incremental semantics fall back to full snapshot with an honest warning.

---

### PR 08 — Accounting reconciliation and Proof of Sync

**Branch:** `feat/tally-proof-of-sync`  
**Risk:** high accounting  
**Purpose:** Make completeness and accounting validity visible.

#### Reconciliation layers

1. **Protocol**
   - all windows responded;
   - all payloads passed Tally status checks;
   - no unclassified parse errors.

2. **Identity/count**
   - response counts versus accepted counts;
   - duplicate IDs;
   - missing required identities;
   - per-date/per-type counts.

3. **Accounting**
   - voucher ledger entries balance according to documented sign conventions;
   - voucher header/entry totals agree where applicable;
   - tax components sum to declared totals where the profile supports it.

4. **Report tie-out**
   - documented native report totals, or explicitly capability-probed custom cross-view totals, compared to the canonical mirror;
   - clearly label report configuration and unavailable or non-comparable scopes.

5. **Freshness**
   - last verified time;
   - age target;
   - current Tally capability/profile drift.

#### Acceptance

- A reconciliation mismatch prevents `Verified`.
- Results include safe drill-down identifiers but not leaked book contents.
- User can export a redacted Proof of Sync.
- “No mismatch detected” is not described as “accurate” when the comparison scope is incomplete.

---

### PR 09 — Operator-grade Tally UX

**Branch:** `feat/tally-operator-console`  
**Risk:** medium  
**Purpose:** Replace raw fetch controls with a reliable operational workflow.

#### Screens

- Setup / Capability Passport.
- Company profile.
- Sync runs.
- Proof of Sync.
- Gap Map.
- Errors and remediation.
- Local mirror explorer with proper pagination/virtualisation.
- Mapping rules.
- Import preview.
- Conflict queue.
- Diagnostics/support bundle.

#### UX rules

- Always display the selected company and stable identity confidence.
- Show last verified state separately from the latest failed/partial attempt.
- Use phase labels rather than one global `busy` flag.
- Make cancellation explicit.
- Show retry only when safe.
- Never show raw XML by default.
- Copyable request/run IDs.
- Every “Fix” action explains what it changes.
- Maintain keyboard and screen-reader accessibility.

#### Acceptance

- Row caps are explicit and do not imply dataset completeness.
- Error messages distinguish configuration, Tally application, parsing, reconciliation, and permission/mode problems.
- An accountant can determine: “What is current? What is missing? What should I do?”

#### Current implementation boundary (2026-07-15)

The read-side operator workflow is implemented: setup and capability evidence,
stable company selection, backend-canonical endpoint attribution, bounded and
truncation-labelled offline persisted profiles, verified baseline versus latest
attempt, scoped run recovery, phase/window progress, cancellation, Proof of
Sync, Gap Map, remediation guidance, copyable local correlation IDs with a
selectable fallback, and a paged metadata-only mirror explorer. Raw source
diagnostics are collapsed, require an explicit sensitive-data reveal, clear on
scope/view/close changes, reject obsolete in-flight results, and are explicitly
labelled as display-capped diagnostics rather than Proof of Sync. Direct
endpoint/runtime/source-diagnostic command errors use stable safe envelopes
with category, retry, remediation, and state-change semantics instead of
exposing raw transport errors; snapshot commands use reviewed safe messages and
codes.

Mapping rules, import preview, conflict resolution, and write-facing actions
remain intentionally unavailable. They depend on PR 11 and PR 12 safety,
approval, outbox, exact-import-result, and recovery contracts. Native Tauri/SQLx
execution and Windows/macOS assistive-technology evidence also remain pending;
a successful frontend build or static-browser inspection is not represented as
that evidence.

---

### PR 10 — JSONEX negotiated path with semantic shadowing

**Branch:** `feat/tally-jsonex-transport`  
**Risk:** medium/high  
**Purpose:** Add a version-gated, read-only JSONEX candidate path and the
evidence needed to qualify an exact scope against XML. No comparator result
enables JSONEX by itself.

#### Current implementation boundary

The repository contains the v1 qualification comparator plus an optional,
portable parser for the exact Ledger and Voucher collection-envelope shapes in
Tally's current native-JSON examples. The parser preserves documented typed
wrappers and absent-versus-empty values, validates application status before
success containers, binds the expected UTF-8 or UTF-16LE response contract,
rejects duplicate keys recursively, and enforces byte/record/structure limits.
Its outputs are explicitly `Unbound`: the documented responses do not echo a
stable company identity, requested range, Bridge query profile, completeness
counts, or Core schema. The Bridge application does not enable the parser
feature. A second disabled feature builds deterministic bytes for only the
exact documented Ledger and unbounded `TSPLVoucherColl` example payloads. It
pins fixed export headers, DOCX key/value spellings, the selected company name,
Bridge's existing 255-byte company safety cap, request byte limits, and
UTF-8/UTF-16 BOM profiles, but every output is marked
ineligible for dispatch, company verification, or date-range claims. The
application has no JSONEX HTTP dispatch or canonical adapter.

The BOM modes deliberately combine the plain DOCX logical profiles with the
separately documented charset/BOM rules. They are not claimed byte-identical to
Tally's multilingual downloads, which currently use different
`svExportFormat`/fetch spellings. Each alternative requires its own versioned
profile and evidence before any live use.

The comparator still operates only on caller-supplied, already-canonicalized
XML-before, JSONEX-candidate, and XML-after Core Accounting windows. It
validates chronological bracketing, exact current-schema scope/count
fingerprints, five-object count coverage/cardinality, record-evidence coverage,
and canonical reference integrity. Neither layer proves that a window came
from the stated transport or company, establishes source atomicity, persists
or enforces a quarantine recommendation, selects JSONEX, changes
mirror/checkpoint/proof state, or supports writes. `Matched` remains
`Shadowing`.

#### Rules

- XML/HTTP remains the reference and fallback transport.
- Release 7.0+ is necessary but not sufficient. A JSONEX candidate is eligible
  for live shadowing only after a separate Capability Passport probe records
  the exact endpoint, release, mode, company identity, encoding, and versioned
  request/query profile, and proves application status, expected response
  container, company pinning, Unicode decoding, and actual range behavior.
- Run chronologically bracketed XML-before, JSONEX-candidate, and XML-after
  read-only observations on synthetic or otherwise stable data. Tally documents
  no cross-request snapshot isolation, so a reference-bracket mismatch is
  `Inconclusive`; a matched bracket is evidence only of equal observed
  semantics, not proof of source atomicity.
- Normalise both into the same canonical model, then compare a deterministic
  semantic projection. Raw wire hashes and transport-derived nested-entry IDs
  remain provenance and are not semantic equality keys.
- Comparator v1 covers sorted group, ledger, voucher-type, voucher, and
  flattened ledger-entry semantics with exact-decimal scale normalization.
  A `Matched` result requires the exact five Core count scopes/cardinalities and
  one-to-one record evidence in all three windows. Gap Map state and nested
  source ordering are not part of v1.
- A candidate mismatch inside a matched XML reference bracket returns a quarantine recommendation
  for that exact scope. A separate durable policy must persist and enforce it.
- Preference requires a separate promotion policy backed by repeated live
  matches and measured operational benefit. Recorded timing and response bytes
  are observations, not proof of benefit by themselves.
- Writes remain disabled; no read or comparator result authorizes write
  fallback or dispatch.

#### Implemented qualification boundary

- `bridge-tally-core` now emits a raw-value-free, hash-scoped local Core Accounting parity
  observation with reference/candidate timing and payload-size observations.
- Semantic hashes are local correlation evidence and must not be copied into a
  public support export without a separate privacy review.
- The slice emits either a continue-shadowing or recommend-quarantine decision.
  Neither is persisted or enforced. Even a matching observation cannot
  qualify, select, prefer, or persist JSONEX data.
- The runtime still has no enabled JSONEX response parser or request builder,
  HTTP dispatch, live probe, canonical adapter, mirror staging, checkpoint,
  proof, fallback, or write path. The optional protocol parser returns only
  unbound documented-envelope evidence. The optional deterministic builder
  exposes only two fixed official-example profiles, including an explicitly
  unbounded voucher profile, and returns no dispatch authority. Neither feature
  is enabled by the production application.
- The old Bridge-shaped JSON simulator fixture is explicitly labelled a
  synthetic semantic reference. It is not an official Tally JSONEX envelope
  and supplies no capability or parity evidence.

#### Acceptance

- Deterministic comparator tests cover scale normalization, exclusion of
  transport-derived provenance, bracketed candidate mismatch, inconclusive reference-bracket mismatch/missing evidence, receipt
  redaction, and invalid scope/metric rejection.
- Sanitized official-derived parser fixtures now cover typed wrappers, nested
  and repeated values, omitted-versus-empty fields, application failure status,
  wrong containers, multilingual/UTF-16 behavior, duplicate-key rejection, and
  resource limits. Bounded date filtering remains unproven because the current
  official examples contain no date-range request/response contract; it still
  requires an approved read-only live observation before runtime work.
- Deterministic request tests pin the exact documented `fetch_List`, `jsonex`,
  `TSPLVoucherColl`, `Type`, and `Native Method` spellings; mandatory company
  variable; fixed export headers; BOM/charset coupling; byte limits; injection
  resistance; and non-dispatchable/unbounded evidence flags. Official spelling
  alternatives require a new versioned profile rather than silent aliases.
- Live validation must record the exact endpoint, product/release/mode/company
  and request profile and demonstrate that the requested range is honored.
- A live match cannot leave `Shadowing` until repeated evidence and a separate
  promotion policy exist; UI selection, fallback, and enforced quarantine
  claims remain pending until that policy is wired.

---

### India Tax raw-observation authority slice

Research against Tally's current GST setup, Release 3.0 schema, party-ledger,
override, and report documentation also showed that the existing
`IndiaTaxBatch` cannot faithfully represent multiple company registrations,
effective party-registration histories, voucher overrides,
uncertain/excluded populations, or tax-row provenance and valuation basis.
Direct extraction into that schema would manufacture authority.

The implemented first India Tax slice is therefore a non-default, portable
parser for one exact Bridge-owned observation envelope. It emits only
explicitly `Unbound` registrations and voucher-tax observations, preserves
identity candidates and exact Bridge-envelope decimal/text lexemes, derives response-internal
counts and claimed company/window metadata from the same bounded response, and
uses domain-separated fragment and response hashes. Its errors and Debug
surfaces do not expose company identity, GSTINs, voucher identities, monetary
values, or raw XML.

There is no request builder, TDL profile, HTTP dispatch, runtime feature,
canonical adapter, capability promotion, mirror/checkpoint/proof integration,
or GST filing/reconciliation claim. Eight initial synthetic adversarial tests
cover the authority firewall, zero-row semantics, counts/order/nesting,
case-variant and unknown attributes, owner/voucher identity ambiguity,
duplicate observations, invalid numeric lexemes, bounds/truncation, and
privacy sentinels. Live read-only source qualification and a richer canonical
schema remain prerequisites for extraction.

---

### PR 11 — Safe write sandbox and exact import results

**Branch:** `feat/tally-safe-import-sandbox`  
**Risk:** very high  
**Purpose:** Introduce write capability without endangering real books.

#### Preconditions

- Explicit user opt-in.
- Synthetic company selected.
- Capability Passport does not mark write as unsupported.
- Exact payload preview.
- Validation passes.
- Idempotency strategy exists.
- Backup guidance is shown where applicable.
- Small batch limit.
- No automatic retry.

#### Import lifecycle

```text
Draft
→ Validate
→ Preview exact diff
→ User approves
→ Post
→ Parse Tally counters and line errors
→ Re-read affected identities
→ Compare intended versus observed
→ Verified / Partial / OutcomeUnknown / Failed
```

#### Acceptance

- `CREATED`, `ALTERED`, `IGNORED`, `ERRORS`, exceptions, and line errors are preserved. Evidence separately records whether `EXCEPTIONS` was source-reported; for Tally's documented direct profile, an absent field is retained as profile-defaulted zero and is never labelled an observed zero.
- A timeout after send is `OutcomeUnknown`.
- Re-read verification is required for `Verified`.
- Duplicate submission tests prove idempotency or block auto-retry.
- No production-company write test is part of ordinary CI.

#### Current rectification status (2026-07-15)

- A portable, ledger-only qualification crate replaces the earlier
  caller-attested in-memory verification sandbox.
- Import wire, canonical intended state, raw import response, canonical
  readback, requested identities, and observed identity coverage have distinct
  domain-separated commitments.
- Import counters and ordered redacted `LINEERROR` digests are derived from the
  same bounded parser result. Duplicate status/counters/import containers,
  counters outside the exact response profile, and malformed line-error text
  fail with one stable redacted error. Caller-provided verification hashes,
  item counts, identity-presence booleans, and version strings are no longer
  accepted.
- Exact preflight and post-write projections bind company GUID, query profile,
  RemoteID, operation, name, parent, GSTIN, and opening balance. Create/alter
  only; no-op alters are rejected.
- Every prepared payload remains non-dispatchable. Thirteen portable write
  qualification tests plus six protocol-evidence tests
  cover exact applied/not-applied, ambiguous response resolution, dirty
  counters, stale/changed state, company/profile/identity mismatch, duplicate
  identity, limits, XML escaping, digest separation, and line-error redaction.
- Import wire bytes are not exposed by the public API. Exact preview
  commitments must be approved after explicit opt-in, observed capability,
  synthetic-company confirmation, backup acknowledgement, and before an
  idempotency reservation or exact preflight. The readback context must echo a
  domain-separated digest of every requested RemoteID, including identities
  expected to be absent. A qualification-only one-attempt transition then
  binds receipt and readback evaluation; a missing receipt remains
  `OutcomeUnknown` even if a later readback matches before or after state.
- Legacy durable caller-attested success/recovery promotion is blocked. Native
  store migration to persist opaque derived verdicts, live synthetic-company
  validation, HTTP dispatch, UI enablement, vouchers, deletes, and production
  writes remain explicitly out of scope.

---

### PR 12 — Mapping, conflict, outbox, and recovery

**Branch:** `feat/tally-write-orchestration`  
**Risk:** very high  
**Purpose:** Build controlled two-way workflows only after reads are trustworthy.

#### Features

- deterministic field mapping;
- mapping suggestions with confidence/reasons;
- operator approval;
- mapping versions;
- outbox with idempotency keys;
- per-item import state;
- conflict detection using source identity and observed versions;
- conflict policies per object/field;
- manual resolution;
- audit events;
- re-read verification;
- recovery from `OutcomeUnknown`.

#### Non-goals

- Generic autonomous AI bookkeeping.
- Silent ledger creation.
- Silent conflict resolution.
- Blanket “Tally wins” or “cloud wins” defaults.
- Fake rollback.

#### Acceptance

- Every mutation is traceable to source, mapping version, approver, payload hash, request ID, Tally result, and verification read.
- Retrying a completed job cannot duplicate a voucher.
- Uncertain outcomes remain visible until resolved.

---

### PR 13 — Observability, support bundle, and performance budgets

**Branch:** `feat/tally-observability`  
**Risk:** medium  
**Purpose:** Make reliability measurable without exposing books.

#### Telemetry

- phase durations;
- bytes received;
- records parsed/accepted/rejected;
- retry counts;
- error categories;
- queue wait;
- Tally response latency;
- peak staging memory where measurable;
- reconciliation duration;
- checkpoint age.

#### Redaction

- hash or omit company names in generic diagnostics;
- mask GSTIN/PAN-like values;
- omit narration and raw entries;
- never log raw request/response by default;
- support bundle is user-initiated, previewable, bounded, and redacted;
- no credentials or machine paths.

#### Performance testing

Use measured baselines rather than marketing targets. Define budgets after collecting representative synthetic sizes:

- small: 1k vouchers;
- medium: 50k vouchers;
- large: 500k vouchers;
- deeply nested voucher;
- many masters;
- slow Tally endpoint;
- maximum permitted response.

Track:

- wall time;
- peak memory;
- database size;
- resume time;
- UI responsiveness;
- hash/reconciliation cost.

#### Acceptance

- No unbounded cardinality in logs/metrics.
- No raw accounting values in default logs.
- Performance regression threshold is documented from measured baselines.
- A slow run remains cancellable and the UI remains responsive.

#### Implemented PR13A boundary

The first observability slice is a portable, local-only aggregation contract.
It accepts only closed request/queue/response enums, monotonic durations, and
response-byte counts. There are no strings, arbitrary attributes, IDs,
endpoints, timestamps, payloads, paths, book fields, event history, runtime
wiring, persistence, exporter, or automatic upload. Coherent fixed-memory
schema-v2 snapshots aggregate 1,592 versioned bucket-frequency cells; counters saturate
visibly. One terminal attempt observation prevents a single call from claiming
both queue failure and response success, while duplicate calls and collection
completeness remain explicitly unauthenticated/unestablished. The user-facing
preview has eight fixed taxonomy rows, coarse count buckets, an unstamped
collector-instance scope, a 64 KiB cap, and a domain-separated checksum. It is
a lossy custom summary, not an OTel Histogram, and explicitly establishes no
authenticity, capability, exact-rate/percentile, or performance-support claim.

Nine portable tests cover exact inclusive/overflow latency/byte cells, explicit
unavailable-byte and circuit-rejection cells, 100,000
observations without cardinality/size growth, sensitive-field exclusion,
snapshots taken during concurrent writes with matched response histograms,
deterministic idle checksums, saturation, the longest textual bucket preview
cap, schema-v2 golden taxonomy/bounds/bytes, and terminal
queue-versus-response attempt separation.
This proves aggregation/privacy/cardinality semantics. PR15 now supplies the
first read-runtime caller, but phase, reconciliation, checkpoint-age, memory,
database, resume, UI, benchmark, and budget evidence remain unavailable.

#### Implemented PR13B1 boundary

The first performance-evidence slice is a portable repository-synthetic parser
qualification harness. A bounded generator streams deterministic UTF-8 voucher
windows to temporary files before measurement. Closed scenarios cover a CI
smoke, 1k vouchers, windowed 50k and 500k corpora, and deep-voucher
characterization using 256 repository-synthetic wrappers and 256 ledger
entries. The wrapper shape is not claimed to be native Tally XML or a parser
depth limit. The generator rejects a window above the existing 32 MiB
encoded-body ceiling before touching the destination and records exact emitted
records, ledger entries, bytes, per-window hashes, and a manifest hash.

Every timing sample runs a fresh child process and covers file open/read,
bounded-body retention, payload hashing, protocol decode, voucher parsing, and
semantic-output hashing. The worker rechecks each input length and
domain-separated digest, requires exact per-window parsed counts, and matches an
independently generator-derived semantic projection before the controller
accepts the sample. `Instant` supplies monotonic, non-steady elapsed time.
Windows use `GetProcessMemoryInfo.PeakWorkingSetSize`; Unix/macOS use explicitly
labelled `getrusage` maximum resident size normalized from KiB to bytes. These
are process-lifetime maxima, their delta is not an allocation measure, and
platform methods are non-comparable. Unavailable memory is a reason code, never
zero. Receipts retain all accepted samples, omit p95 below 20, use nearest-rank
p95 at 20 or more, bind the compiler/target/actual Cargo
profile/executable/Cargo.lock digest and generator inputs, cap JSON and
validated JSON input at 256 KiB, and carry a domain-separated checksum.
CI-supplied commit evidence is embedded at build time; local absence remains
explicit.
The qualification-only body reader also enforces an inclusive 32 MiB encoded
body ceiling, rejects a declared over-limit body before reading, and reads at
most one detection byte beyond the ceiling for unknown lengths. This component
is not shared with the production HTTP path, so the receipt keeps runtime cap
binding false.

This evidence authority is parser-only. Its validator permanently rejects any
claim that live Tally was observed or that support, a Tally capability,
accounting correctness, the production runtime cap, or a performance budget was
established. The 50k/500k cases are windowed corpora and do not establish a
single-response capacity or source snapshot completeness. CI therefore gates
only deterministic correctness and privacy-reduced receipt integrity; budgets
remain blocked until repeated comparable release-profile baselines exist.

HTTP/server isolation and framing, UTF-16 scale,
many-master generation, cancellation, native database/reconcile/resume cost, UI
responsiveness, stable-runner baselines, and a live compatibility receipt remain
follow-on work.

#### Implemented PR13B2 transport-hardening boundary

The native Tally client now delegates its status and XML requests to the
portable `bridge-tally-transport` crate. This removes the earlier production
versus qualification response-reader split. The app-bound transport preserves
actual normalized loopback origin identity (`localhost` aliases only to
`127.0.0.1`; other `127/8` addresses and `::1` remain distinct), disables
proxies and redirects, applies a bounded whole-request deadline, caps outbound
XML and encoded response bodies, rejects non-identity content encoding, and
returns closed privacy-safe errors. Endpoint client/cache evidence can no
longer be silently reused across distinct local socket addresses.

The loopback simulator now has closed Content-Length, close-delimited, chunked,
and mismatched-length framing plus closed content-encoding modes. Portable
end-to-end tests cover exact cap/cap+1, streamed overflow, truncation, slow
headers, redirects/non-2xx, unsupported content encoding, and UTF-8/BOM and
BOM-qualified UTF-16LE/BE. A deterministic master generator preflights up to
50,000 ledger masters against the production 32 MiB encoded-body ceiling; a
10,000-ledger corpus traverses the production decoder and strict ledger parser;
an independently derived digest proves every parsed identity and accounting
field matches the generator expectation and remains identical across encodings.

Protocol hardening rejects duplicate/misplaced export header status fields and
DTD/processing constructs. Import result parsing accepts Tally's documented
direct `ENVELOPE/BODY/DATA` counters as a distinct profile and rejects mixing
with Tally's documented wrapped `IMPORTRESULT` profile. The later portable
runtime suite preserves post-cancellation endpoint spacing, reserves a single
half-open circuit probe, rejects stale queued admission after a circuit opens,
and prevents application/validation failures from resetting prior transport
failures.

This remains repository-synthetic evidence. The HTTP simulator is still an
in-process loopback server, large masters have not yet traversed the full
Tauri/runtime path, and native runtime tests cannot execute locally because
`libclang.dll` is unavailable. Process isolation, phase evidence, native
database/reconcile/resume/UI cost, comparable performance baselines, and a
read-only live Education compatibility receipt remain follow-on work. The
existing PR13B1 receipt is not retroactively granted runtime-cap authority.

---

### PR 14 — Compatibility matrix, live validation harness, and release gates

**Branch:** `chore/tally-compatibility-matrix`  
**Risk:** medium  
**Purpose:** Turn “supported” into reproducible evidence.

#### Matrix dimensions

- TallyPrime 7.1;
- TallyPrime 7.0;
- one supported older TallyPrime XML-only profile;
- Tally.ERP 9 compatibility profile where available;
- Education mode;
- licensed mode when legitimately available;
- XML;
- JSONEX;
- ODBC disabled/enabled;
- no company loaded;
- one company loaded;
- multiple companies loaded;
- different locale/character sets;
- large dataset;
- Bridge Windows build;
- Bridge macOS simulator/UI build;
- live Tally validation only on legitimate hosts/configurations.

#### Evidence

- exact Tally release;
- mode confidence;
- endpoint settings;
- synthetic data seed;
- operations attempted;
- observed results;
- unsupported outcomes;
- hashes/manifests;
- Bridge commit;
- redacted logs.

#### Release gate

A release may not broaden its support claim unless the matrix has current evidence. Missing evidence must be stated in release notes.

#### Implemented PR14 claim-control boundary (2026-07-15)

- `bridge-tally-compatibility` now owns a separate, closed, privacy-reduced
  live-read receipt schema. It does not broaden the parser-only qualification
  receipt and cannot itself emit a support claim.
- Eleven exact machine-readable Windows/Tally cells cover the initial release, mode,
  transport, ODBC, company-state, locale/encoding, data-tier, and platform
  dimensions. Every cell remains `unknown`; no live support evidence has been
  manufactured from synthetic tests.
- CI and the release process execute an exact-scope gate. Positive cells require
  a fresh clean-source receipt, matching compatibility-surface digest, and an
  unexpired Ed25519 maintainer-review attestation from a non-revoked key.
- The app probe no longer treats the undocumented `/status` heuristic as an
  authoritative product identifier or prerequisite for the documented XML POST
  path.
- The standalone live controller now reuses sealed production read templates,
  requires a reviewed synthetic fixture and two run/receipt-bound interactive
  confirmations, stops on marker/GUID/context/range ambiguity, does not persist
  or output raw source data (bounded responses still exist transiently in
  memory), and saves atomically without overwrite. The controller has no
  callable generic XML API; its sealed adapter internally delegates the fixed
  profiles to the generic production HTTP POST transport. It has no dependency
  path to TDL, imports, Tauri, persistence, sync, or writes. A legitimate live
  Education receipt is still pending.

#### PR14 safety rectification (2026-07-16)

- The default live controller now rejects unknown product, exact release, mode,
  ODBC, or locale evidence and a false no-customer-data attestation before
  issuing consent or allowing a network request.
- Network consent is a consumed opaque type with a fresh cryptographic nonce,
  a five-minute expiry, and a commitment to the full configuration/endpoint,
  fixture, commit/dirty state, executable, Cargo.lock, and compatibility
  surface. The surface is revalidated immediately before transport dispatch.
- Config and receipt paths are confined to the canonical repository-local
  ignored `.bridge-live` root. Receipt saving consumes a repository-issued
  target, revalidates it, accepts JSON only, binds the typed save phrase to both
  receipt and exact output, and never overwrites.
- Regression tests prove false-attestation zero-POST behavior, unknown-profile
  rejection, wrong/cross-run/expired consent rejection, output-path binding,
  repository-confined public save, and atomic no-overwrite behavior. No live
  POST or support claim is introduced by this rectification.

---

### PR 15 — Portable read runtime, typed retry, and runtime telemetry

**Branch:** `feat/tally-portable-read-runtime`  
**Risk:** medium  
**Purpose:** Make endpoint execution deterministic and observable before adding
another product pack.

#### Implemented PR15 boundary (2026-07-15)

- `bridge-tally-runtime` is a Tauri/SQLCipher-independent read control plane.
  It owns normalized per-endpoint serialization, bounded queue deadlines,
  post-attempt spacing, cancellation, a threshold/cooldown/one-probe circuit,
  deterministic bounded jitter, and a maximum endpoint-session count.
- Its public operation enum is read-only. Retry is allowed only for connection
  failure, deadline, request failure, HTTP 5xx, and rate limiting, with at most
  five attempts. HTTP 4xx, size/decode/application/parse/validation failures,
  and company mismatch never retry.
- The native Tally runtime is wired in source to this controller for status,
  capability, company, master, voucher, and validated report reads. The older
  client-local queue is no longer on the request path. Request IDs, explicit
  cancellation, capability cache, and operator snapshots remain native-facing.
- Schema-v2 observations record one terminal outcome per attempted read,
  including queue deadline/cancellation, response class, circuit rejection,
  and an explicit `unavailable` body-byte measurement. A Tauri command returns
  the bounded privacy-reduced preview plus its checksum; nothing is uploaded or
  persisted automatically.
- The loopback simulator now supports a fail-closed sequence of 1–64 response
  plans. A real portable HTTP transport/runtime test proves an exact synthetic
  HTTP 500 then HTTP 200 sequence causes one retry and two processed requests.

Nine observability tests, eight runtime tests, and fifteen simulator integration
tests pass, and focused Clippy passes with warnings denied. These are
repository-synthetic results. The native crate still stops in
`libsqlite3-sys` before Bridge code can be checked because `libclang.dll` is
unavailable; the attempted LLVM installation was cancelled and was not
retried. No live XML POST, legitimate Education receipt, Windows/macOS native
runtime result, support-bundle save/review UI, performance budget, or support
claim is established.

Production transport still retains each bounded response as bytes and decoded
text before parsing. Single-response status/export reads now report their
encoded byte count after a complete body, including when later application
validation or parsing fails. Compound capability probes, pre-response failures,
and partial transport reads remain explicitly unavailable. Streaming/staged production parsing,
partial-failure byte plumbing, property/fuzz testing, and live profile evidence
remain P1/P2 follow-on work.

#### Implemented PR16 boundary — Party Outstanding Confidence Receipt v1

PR16 establishes a portable, read-only foundation for bill allocations and
party outstandings. The version-2 canonical model preserves all four known
reference kinds plus unclassified source text, voucher and ledger-opening
origins, exact signed decimals, optional/evidence-qualified due dates, currency
basis, bill-wise state, source counts, coverage, query profile, and a fetch
bracket. Supplied derivation helpers exclude mutable amounts and dates by using
parent or exact report scope plus ordinal, and reconciliation rejects the
schema's legacy mutable-reference identity basis. Outstanding identity remains
ordinal/profile dependent and is not qualified for mirror authority until row
ordering stability is observed.

A non-default parser accepts only one exact Bridge-owned raw-observation
envelope. It is deliberately `Unbound`: its company, party, date, direction,
profile, counts, and values are response-internal claims and cannot authorize a
capability or canonical import. Parsing is bounded, UTF-8/UTF-16 aware,
fail-closed on grammar/count/duplicate/active-XML violations, and uses redacted
errors plus domain-separated response and fragment commitments.

The confidence engine requires caller-owned equality for company identity,
party, as-of date, receivable/payable direction, query profile, and exact scope
fingerprint before comparison. Opaque engine authority must separately prove
the allocation profile, outstanding profile, signed-amount semantics,
direction/sign and due-date/ageing semantics, On Account aggregation,
settled-row omission semantics, and the meaning of an empty complete scope.
There is no public constructor that can promote this
authority; a future live-qualified adapter path requires separate review. Exact arithmetic
then categorizes matches, partial settlement, On Account aggregate matches,
mismatches, disabled bill-wise tracking, incomplete coverage, incomparable
currency, profile unobserved, source change, or unavailable evidence. Unknown,
incomplete, unobserved, or unauthoritative empty evidence never proves zero.
Only an independently authorized complete and stable empty scope may match;
no such live authority exists today. The engine never invents a bill link for
On Account amounts.

Generic typed Bills canonicalization and the shared reconciliation result shape
exist, but this slice has no parser-to-canonical adapter, request builder, TDL
export, native extraction/runtime dispatch, Bills-qualified mirror/checkpoint/
proof authority, UI, reminder, payment, or write surface. It therefore remains
`Unknown, not supported`. A live Education-mode
fixture must use literal calendar day `01`, `02`, or `31`, and a qualifying
receipt must be captured before any source-semantics authority or support claim
can be promoted. Education mode has no TSS and excludes connected services, so
this fixture cannot qualify email, WhatsApp, payment-request, remote, or other
connected flows; local XML/TDL behavior remains release-specific.

Tally's Sample XML page documents report names and import-side `BILLTYPE`
values. It does not establish Bridge's raw observation envelope or a universal
read/export response schema.

#### Implemented PR17 boundary — native Ledger Outstandings observation probe

PR17 adds only a dormant, feature-gated request probe for Tally's documented
native `Ledger Outstandings` report. It fixes `Export`/`Data`, the report ID,
validated company, party-ledger and report To-date inputs, XML export, and exploded
detail, and binds the exact template, rendered request, and scope using
domain-separated SHA-256 commitments. Diagnostics redact all dynamic values.
The probe is outside the production `ReadOnlyProfile` enum and native runtime;
it has no transport method, response parser, retry, canonical adapter, mirror,
checkpoint, proof, capability, UI, or authority constructor.

This is request-shape evidence only. Official documentation does not establish
the release-stable response grammar, sign convention, due-date derivation, row
ordering, completeness, or empty-scope meaning. The companion runbook defines
a disposable INR Education fixture covering ledger opening, New Ref, partial
Agst Ref, On Account, Advance, and explicit due date, plus three unchanged
reads and negative cases. No live POST was performed: the running executable's
file metadata reports TallyPrime Edit Log 1.1.7.0, while the About screen,
loaded synthetic-company state, and independent party identity could not be
safely observed. Current TallyPrime documentation must not be generalized to
that unverified local release.

#### Implemented PR18 boundary — qualification-only typed runner

PR18 adds that runner as a required-feature standalone binary, without adding
it to the native Bridge graph. Its transport accepts only the sealed
CandidateV0 type, revalidates the frozen template and rendered request hash at
dispatch, enforces loopback/no-proxy/no-redirect, fixes a 20-second timeout,
caps the request at 64 KiB and both encoded and decoded response data at 1 MiB,
and performs no retry.

The runner rejects unknown About values, non-Education mode, configured
TDLs/add-ons, customer-data attestation gaps, placeholder identity commitments,
unreviewed registration, paths outside tracked fixture or ignored local roots,
stale/incomplete UI evidence, and any surface drift before network access. Its
preflight and dispatch consents are distinct consumed types, use fresh
cryptographic nonces, expire after five minutes, and bind the build, lockfile,
surface, fixture, scenario, endpoint, identity registration, UI-before, and
Candidate commitments. A successful run is exactly 2 preflight POSTs followed
by B0/Candidate1/B1/Candidate2/B2/Candidate3/B3 (11 POSTs), for 13 total.

Every identity bracket re-establishes the unique company GUID and party stable
source identity. A byte-repeatable positive observation requires all three
Candidate responses to have HTTP success and strict Tally `STATUS=1`; their
exact encoded bodies are compared byte-for-byte in bounded memory and then
discarded. The runner retains no raw response and
requires a separate UI-after and receipt-save confirmation. Its dedicated
receipt is structurally incompatible with `LiveCompatibilityReceipt` and fixes
authenticity, scope, grammar, accounting semantics, completeness, atomicity,
performance, runtime, mirror, and support authority to false.

The qualification runner now also preserves negative observation evidence.
Candidate application failure/unrecognized status, HTTP rejection, and
transport failure become typed attempt facts; each completes its trailing
identity bracket and the remaining fixed sequence only while company and party
identity remain valid and unchanged. This is not retry: the request budget and
order remain fixed. Missing bodies make repeatability `NotEstablished`, raw
bytes are still discarded, and the receipt retains only bounded hashes/status/
reason facts. Identity failure or drift still stops before a subsequent scoped
Candidate request. Native receipt saving now consumes a repository-issued,
exact-path-bound JSON target and recomputes the save challenge internally.
UI evidence rows are now closed typed objects with exactly the reviewed ten
fields, contiguous indices, bounded control-free text, and a 256-row cap. The
settled-reference observation is a closed enum enforced against the selected
scenario, so arbitrary JSON or invented status prose cannot authorize a run.

No live POST was performed in PR18. The checked-in profile and UI files are
deliberately invalid examples, and this workstation still lacks a separately
reviewed identity registration plus complete About/UI evidence. The next gate
is therefore a visible, synthetic-only local registration and UI evidence pass,
followed by one consented observation. Only after exact release evidence exists
may a response grammar be frozen. Parser binding, semantic authority, canonical
adaptation, production dispatch, and support promotion each remain separate
future reviews. Bills stays `Unknown, not supported`.

Official research inputs:

- <https://help.tallysolutions.com/sample-xml/>
- <https://help.tallysolutions.com/manage-receivables-outstanding-tally/>
- <https://help.tallysolutions.com/tally-prime/analysis-verification/outstandings-tally/>
- <https://help.tallysolutions.com/knowledgebase/manage-outstanding-payables-tally/>
- <https://help.tallysolutions.com/objects-and-collections/>
- <https://help.tallysolutions.com/developer-reference/tally-definition-language/appendix/>
- <https://help.tallysolutions.com/tally-prime/accounting/currency-in-tallyprime-faq/>
- <https://help.tallysolutions.com/licensing-best-practices/>

---

### Decision gate — Optional Bridge Companion TDL

Do **not** make a TDL add-on a prerequisite now.

Consider an optional open-source companion only when one or more of these remain unsolved externally:

- stable change feed or company identity;
- efficient server-side filtering;
- in-Tally validation required at the point of entry;
- role-aware Bridge actions inside Tally;
- an operator-visible Bridge status panel;
- reliable webhook-like notification;
- import preconditions that must execute inside Tally.

Before adoption, write an ADR covering:

- installation/update/removal;
- Tally release compatibility;
- signatures/provenance;
- permissions;
- performance;
- failure isolation;
- support burden;
- fallback when absent;
- Education-mode behaviour;
- whether the feature remains useful without it.

---

## 7. Test strategy

### 7.1 Test pyramid

1. **Pure unit tests**
   - builders;
   - escaping;
   - encodings;
   - parsers;
   - error classification;
   - decimals;
   - identity;
   - canonical hashing;
   - state machine.

2. **Property/fuzz tests**
   - XML/JSON escaping;
   - nested/repeated fields;
   - malformed input;
   - arbitrary Unicode;
   - decimal forms;
   - duplicate ordering;
   - parser never panics;
   - redaction never exposes seeded secrets.

3. **Simulator integration tests**
   - all protocol/failure scenarios;
   - concurrency;
   - retry;
   - cancellation;
   - resume;
   - ambiguous write.

4. **Database integration tests**
   - migrations;
   - atomic snapshot;
   - rollback;
   - checkpoint;
   - tombstones;
   - idempotent upsert;
   - outbox recovery.

5. **Frontend tests**
   - Truth State rendering;
   - setup wizard;
   - partial/failure remediation;
   - cancellation;
   - accessibility;
   - no misleading counts.

6. **Live Tally contract tests**
   - explicit, opt-in, synthetic company;
   - exact release/mode recorded;
   - no customer data;
   - destructive tests disabled by default;
   - artifacts redacted.

### 7.2 Critical failure invariants

- Concurrent same-endpoint operations: maximum in-flight requests = 1.
- Wrong company: no commit.
- HTTP 200 application error: no commit.
- Parse error: no checkpoint advance.
- Cancellation: no checkpoint advance.
- Retry after read timeout: no duplicates.
- Write timeout after send: `OutcomeUnknown`.
- Re-run unchanged snapshot: same canonical hash.
- Incremental + final full snapshot: equal canonical state.
- Reconciliation mismatch: not `Verified`.
- Logs/support output: seeded sensitive values absent.

### 7.3 Education edition live test sequence

Run only after the simulator and P0 protocol work are complete.

1. Record Tally product/release and visible mode.
2. Use a synthetic company; do not use real books.
3. Confirm HTTP server configuration.
4. Optionally record `/status` as a non-authoritative diagnostic, then run the
   documented XML POST company enumeration as the integration check.
5. Select the synthetic company explicitly.
6. Run one minimal ledger read.
7. Run one minimal voucher-range read.
8. Test an empty range and a small populated range.
9. Verify actual range filtering client-side.
10. Probe JSONEX only if release indicates 7.0+.
11. Do not write unless the Capability Passport and operator explicitly allow the synthetic write test.
12. For a write:
    - use a documented permitted Education-mode voucher date: the 1st, 2nd, or 31st, never a generic "last day" substitution;
    - include an ordinary disallowed date as a negative test without changing system time or bypassing licensing;
    - one uniquely tagged synthetic ledger first;
    - parse all counters;
    - re-read it;
    - delete/compensate only if a separately reviewed operation exists.
13. Record every capability as observed for that exact release/mode.
14. Remove or archive the synthetic company according to the operator’s normal Tally process.

No workaround may be added to evade Education-mode limits.

---

## 8. Security and privacy threat model

### Assets

- company identity;
- ledgers and parties;
- tax identifiers;
- vouchers and amounts;
- narrations;
- inventory;
- credentials for downstream AXAL;
- import decisions;
- audit evidence.

### Threats

- posting to the wrong company;
- leaking books through logs or GitHub fixtures;
- remote plaintext exposure;
- malicious local service impersonating Tally on port 9000;
- oversized/malformed payload denial of service;
- XML/JSON parser abuse;
- duplicate write after timeout/retry;
- stale or poisoned mapping;
- unsafe support bundle;
- SQL migration loss;
- UI misrepresentation of partial data.

### Required controls

- retain loopback-only Tally policy;
- capability/application-level handshake;
- explicit company pinning;
- response caps and deadlines;
- strict parsing;
- shared serial queue;
- data minimisation;
- redacted structured logs;
- immutable audit events;
- no raw payload by default;
- bounded previewable support export;
- typed ambiguous outcomes;
- idempotency and re-read verification;
- migration backups/rollback notes;
- synthetic fixtures only.

A local process can impersonate port 9000. Bridge should therefore describe the endpoint as **local and capability-verified**, not cryptographically authenticated, unless a future Tally-supported authentication mechanism is implemented for this interface.

---

## 9. Performance and resilience design

### Do

- reuse HTTP clients and endpoint sessions;
- serialise per endpoint;
- stream/stage large responses;
- use adaptive date windows;
- hash incrementally;
- commit in database transactions;
- persist resumable run/window state;
- keep UI work off the main thread;
- use bounded queues;
- make cancellation cooperative;
- retry only classified transient reads;
- apply jitter;
- trip a small circuit after repeated connection failures;
- expose next safe action.

### Do not

- fetch all native methods;
- store complete raw payloads;
- build one enormous in-memory DOM;
- retry malformed or validation errors;
- retry writes blindly;
- advance a checkpoint before reconciliation;
- equate row count with completeness;
- use names as immutable IDs;
- use floating-point amounts;
- claim “real time” without measured end-to-end latency and a defined trigger model.

---

## 10. Codex operating rules

Codex must follow these rules for every PR:

1. Read `AGENTS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `review-checklist.md`, and Tally docs before editing.
2. Keep each PR focused on one roadmap stage.
3. Preserve loopback-only Tally networking.
4. Use synthetic values only.
5. Never add sample company exports, raw responses, GSTINs, PANs, narrations, certificates, credentials, local usernames, or absolute machine paths.
6. Add tests before or with behavioural changes.
7. Do not weaken response caps, CSP, endpoint validation, or redaction.
8. Do not add a dependency until license inventory and security impact are addressed.
9. Do not modify unrelated DSC/document/AXAL behaviour except minimal compile wiring.
10. Any schema change uses a versioned migration and contains rollback/compatibility notes.
11. Every Tally request is explicit about company context when company-scoped.
12. Every response is checked at transport and Tally application levels.
13. A failed/partial/cancelled run cannot advance the verified checkpoint.
14. Writes remain disabled until the dedicated safe-write stage.
15. No unsupported Tally claim is added to README or UI.
16. Run:
    - `corepack pnpm install --frozen-lockfile`
    - `corepack pnpm run license:all`
    - `corepack pnpm run build`
    - `corepack pnpm run cargo:fmt`
    - `corepack pnpm run cargo:check`
    - `corepack pnpm run cargo:test`
    - `corepack pnpm run cargo:clippy`
    - relevant security audits
17. PR body includes:
    - functional summary;
    - tests;
    - migration/sync impact;
    - rollback;
    - Tally/security impact;
    - supported/missing platform evidence;
    - linked review-checklist line.

---

## 11. Copy-paste master prompt for Codex

```text
Repository: lamemustafa/bridge
Base branch: master

Mission:
Implement the next item in docs/tally/TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md.
Bridge must become a truthful, resilient, accounting-safe Tally connector, not a
feature-count demo.

Before editing:
1. Read AGENTS.md, CONTRIBUTING.md, SECURITY.md, review-checklist.md,
   docs/step-by-step-roadmap.md, README.md, and all files under
   src-tauri/src/tally.
2. Inspect the current tests and CI commands.
3. State the exact invariant this PR will establish.
4. Keep the PR limited to the named roadmap item.

Non-negotiable rules:
- Preserve loopback-only Tally connectivity and redirect blocking.
- Use only synthetic test data.
- Do not log or commit raw accounting payloads, customer data, GSTIN/PAN values,
  narrations, credentials, local usernames, or machine-specific paths.
- Do not treat HTTP 200 as Tally success.
- Require explicit current-company context for every company-scoped request.
- Never use floating point for amounts.
- Never advance a verified checkpoint on partial, failed, cancelled, or
  unreconciled runs.
- Do not automatically retry writes.
- Do not add write capability before the safe-write roadmap stage.
- Do not change DSC, document, or AXAL behaviour except minimal compile wiring.
- Every behavioural change needs a regression test.
- Every DB change needs a versioned migration and rollback/compatibility notes.
- Keep frontend messages honest: Verified, Partial, Stale, Unsupported, Failed.

Required validation:
corepack pnpm install --frozen-lockfile
corepack pnpm run license:all
corepack pnpm run build
corepack pnpm run cargo:fmt
corepack pnpm run cargo:check
corepack pnpm run cargo:test
corepack pnpm run cargo:clippy
corepack pnpm run security:audit:frontend
cargo audit --file src-tauri/Cargo.lock

PR output:
- concise implementation summary;
- files changed;
- tests added;
- exact commands/results;
- migration and rollback impact;
- Tally/security/privacy impact;
- remaining uncertainty;
- one linked review-checklist item.
```

---

## 12. First implementation prompt for Codex: PR 01

```text
Repository: lamemustafa/bridge
Base: master
Create branch: fix/tally-shared-runtime

Goal:
Fix the current Tally serialization scope. Today every Tauri command constructs a
new TallyClient, and every TallyClient constructs its own SerialTallyQueue. This
does not prevent concurrent commands from posting to the same Tally endpoint.

Implement:
1. Add a Tauri-managed TallyRuntime.
2. The runtime owns/reuses one TallySession per canonical loopback endpoint.
3. A TallySession owns:
   - the reqwest Client;
   - one request mutex/queue;
   - endpoint configuration;
   - any request spacing setting.
4. Register TallyRuntime with tauri::Builder::manage.
5. Change check_tally_connection, fetch_tally_companies,
   fetch_tally_ledgers, and fetch_tally_vouchers to use the managed runtime.
6. Preserve current renderer command payloads and response shapes.
7. Preserve loopback validation, no redirects, timeouts, and response byte caps.
8. Ensure localhost/127.0.0.1 aliases cannot create competing sessions for the
   same endpoint.
9. Ensure cancellation/error releases the gate.
10. Do not implement retries, JSONEX, schema changes, or new UI in this PR.

Required tests:
- concurrent same-endpoint operations observe max_in_flight == 1;
- operations on distinct endpoint keys are not forced through one global gate;
- localhost and 127.0.0.1 resolve to the same session key;
- a failed request releases the gate;
- a cancelled request releases the gate;
- existing endpoint-rejection tests remain green.

Design constraints:
- Keep production code independent of a real Tally installation.
- Use a synthetic loopback test server.
- Avoid sleeping 500 ms in unit tests. Make spacing injectable or zero in tests.
- Do not expose internal Arc/Mutex types through command responses.
- Do not add raw request/response logging.

Validation:
Run every command listed in the master prompt. Update documentation only where
needed to explain the runtime invariant. Include rollback and Tally/security
impact in the PR body.
```

---

## 13. Issue-ready backlog labels

Apply the repository’s existing area/severity conventions.

| Roadmap item | Suggested labels |
|---|---|
| PR 00 | `area:tally`, `type:chore`, `severity:p3` |
| PR 01 | `area:tally`, `type:rectify`, `severity:p1` |
| PR 02 | `area:tally`, `type:bug`, `severity:p1` |
| PR 03 | `area:tally`, `type:chore`, `severity:p2` |
| PR 04 | `area:tally`, `type:feature`, `severity:p2` |
| PR 05 | `area:tally`, `type:feature`, `severity:p2` |
| PR 06 | `area:tally`, `type:feature`, `severity:p1` |
| PR 07 | `area:tally`, `type:feature`, `severity:p1` |
| PR 08 | `area:tally`, `type:feature`, `severity:p1` |
| PR 09 | `area:tally`, `type:feature`, `severity:p2` |
| PR 10 | `area:tally`, `type:feature`, `severity:p2` |
| PR 11 | `area:tally`, `type:feature`, `severity:p1` |
| PR 12 | `area:tally`, `type:feature`, `severity:p1` |
| PR 13 | `area:tally`, `type:feature`, `severity:p2` |
| PR 14 | `area:tally`, `type:chore`, `severity:p2` |

Use the exact label names present in the repository; adjust this table rather than creating duplicates. Apply exactly one `area:*` label. Suspected vulnerabilities and sensitive-data exposures do not use this public backlog: follow `SECURITY.md` privately until coordinated disclosure is safe.

---

## 14. Definition of “top-notch”

The Tally integration is top-notch when all of the following are true:

- A fresh contributor can reproduce protocol failures without Tally.
- A user can connect with a guided, low-friction setup.
- Bridge identifies the selected company explicitly.
- Same-endpoint requests cannot race.
- HTTP and Tally application errors are distinct.
- UTF-8/UTF-16 and malformed-response cases are tested.
- Large snapshots are bounded, resumable, and atomic.
- Incremental sync is deletion-aware and periodically reconciled.
- Exact money and stable identities are used.
- Every run has a Proof of Sync and Gap Map.
- The UI distinguishes verified state from the latest attempt.
- JSONEX is enabled only where parity and a measured operational benefit have been proven; no intrinsic speed claim is made.
- Writes require preview, approval, import counters, idempotency, and re-read.
- An ambiguous write remains visible rather than being retried blindly.
- Logs and support bundles do not leak books.
- Education-mode capability is observed and honestly labelled.
- Support claims map to current compatibility evidence.
- Failures include safe, specific recovery instructions.
- No marketing adjective substitutes for a measured invariant.

---

## 15. Explicit non-goals for the first half of the roadmap

Until PR 08 is complete, do not prioritise:

- autonomous bookkeeping;
- document OCR;
- generic GST return filing claims;
- broad remote/LAN plaintext connectivity;
- production writeback;
- a mandatory TDL package;
- multi-ERP abstraction;
- cloud scheduling;
- “real-time” marketing;
- complex AI ledger mapping;
- large UI redesign unrelated to Truth States.

The first milestone is **trustworthy read sync**, not two-way feature breadth.

---

## 16. Research sources

Official Tally sources:

- Integrate with TallyPrime: https://help.tallysolutions.com/integrate-with-tallyprime/
- XML Integration: https://help.tallysolutions.com/xml-integration/
- Pre-requisites for Integrations: https://help.tallysolutions.com/pre-requisites-for-integrations/
- JSON Integration / JSONEX: https://help.tallysolutions.com/tally-prime-integration-using-json-1/
- Integration Initiated From TPAs: https://help.tallysolutions.com/integration-initiated-from-tpas/
- ODBC Integrations: https://help.tallysolutions.com/odbc-integrations/
- Integration Methods and Technologies: https://help.tallysolutions.com/integration-methods-and-technologies/
- TallyPrime 7.1 release notes: https://help.tallysolutions.com/release-notes-tallyprime-7-1/
- Licensing FAQ: https://help.tallysolutions.com/tally-prime/installation-and-licensing/licensing-faq-errors-tally/
- Licensing best practices, including Education-mode dates: https://help.tallysolutions.com/licensing-best-practices/

Product workflow benchmarks:

- RazorpayX Accounting Payouts: https://razorpay.com/x/accounting-payouts/
- Volopay ERP Integration: https://www.volopay.com/integration/
- Volopay Accounting Automation: https://www.volopay.com/accounting-automation/
- Vyapar TaxOne Data Automation: https://taxone.vyapar.com/data-entry-automation-feature
- Vyapar TaxOne GST Reconciliation: https://taxone.vyapar.com/gst-reconciliation-feature
- Sage Expense Management: https://www.fylehq.com/
- Happay: https://happay.com/
- EnKash: https://www.enkash.com/
- Open-source Rust Tally SDK: https://github.com/labs-infinitum/tally-sdk-rs

Repository evidence:

- https://github.com/lamemustafa/bridge
- `README.md`
- `AGENTS.md`
- `package.json`
- `.github/workflows/ci.yml`
- `src-tauri/Cargo.toml`
- `src-tauri/src/tally/*`
- `src-tauri/src/commands.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/db/schema.rs`
- `src-tauri/src/sync/*`
- `src/main.tsx`
