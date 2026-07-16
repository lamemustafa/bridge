# Bills native outstandings probe runbook

This runbook defines a future consent-gated observation of Tally's native
`Ledger Outstandings` report using only the disposable synthetic fixture in
[`education-native-outstandings-v0.json`](./fixtures/education-native-outstandings-v0.json).
It does not qualify a production connector or create a support claim.

CandidateV0 now has an isolated, non-default qualification runner, but the
checked-in example remains **NO-GO for live dispatch**. Never send or
copy/paste the candidate XML. A live observation is allowed only when all of
the following exist together:

- the reviewed typed runner whose dispatch method accepts only
  `SealedNativeLedgerOutstandingsProbe`;
- a separately registered and reviewed local company and party identity
  commitment;
- visible About-screen evidence for the exact product, release, Education mode,
  locale, HTTP server port, and configured TDL/add-on counts;
- complete structured UI-before and UI-after evidence for the selected scope;
- separate, run-bound preflight and dispatch consent challenges.

Build or invoke only the required-feature binary from the repository root:

```sh
cargo run --locked --manifest-path src-tauri/Cargo.toml \
  -p bridge-tally-live-read \
  --features bills-native-outstandings-probe-runner \
  --bin bridge-tally-native-outstandings-probe -- \
  run .bridge-live/native-outstandings-profile.json \
  .bridge-live/native-outstandings-observation.json \
  --consent read-only-synthetic-native-outstandings
```

The binary is absent from default builds and from the native Bridge feature
graph. Its receipt is an observation-only
`BillsNativeOutstandingsProbeReceiptV0`, not a `LiveCompatibilityReceipt`.

The shipped
[`native-outstandings-profile.example.json`](./native-outstandings-profile.example.json)
is deliberately unusable. Its identities are all-zero placeholders, its About
fields are unknown, and its attestations are false. A future runner must reject
it until a separate local registration and review replaces every placeholder.
Do not invent a GUID or another Tally identity merely to pass the gate.

## Safety gate

- Use one invocation for exactly one reviewed scenario and one ToDate:
  `20260701`, `20260702`, or `20260731`. Do not batch dates.
- Tally must be bound to literal IPv4 or IPv6 loopback on the explicitly
  reviewed port. Redirects, proxies, DNS hosts, and non-loopback endpoints are
  forbidden.
- The About screen must visibly confirm exact Tally product, release, Education
  mode, locale, HTTP server port, and zero configured TDLs/add-ons for the
  baseline observation. Executable file metadata is not About evidence.
- Exactly one loaded company must be the high-entropy synthetic fixture company.
  No customer, production, personal, or other company data may be loaded.
- Company and party identities must match nonzero commitments from a separate
  reviewed local registration. Name equality alone is insufficient, and an
  observed identifier is not evidence that its stability has been qualified.
- Request bytes must come from the sealed, non-default CandidateV0 profile.
  Caller-provided XML, TDL, report names, formulas, headers, or request counts
  are forbidden.
- No import, write, reminder, settlement, payment, email, WhatsApp, connected
  service, or fixture-creation action is permitted.
- No automatic retry or compensating request is permitted. A preflight or
  identity-bracket failure stops with fewer than 13 POSTs. A Candidate
  application, HTTP, or transport failure is recorded, followed by its required
  identity bracket and the remaining fixed Candidate/bracket observations, but
  only while identity remains valid and unchanged.
- PR18 retains no raw Tally response. Response bodies may exist only in bounded
  memory long enough to validate, hash, and compare them, and must then be
  discarded.

If About evidence, UI evidence, the company marker, either identity commitment,
or any request commitment cannot be verified, stop before the applicable
consent challenge. Do not interpret an empty or unrecognized response as zero.

## Reviewed Education fixture

The fixture uses INR, enables bill-by-bill tracking, and uses these immutable
synthetic markers:

- company:
  `BRIDGE-PR18-NATIVE-OUTSTANDINGS-COMPANY-019f605f-e6cf-77b2-ac95-31722887a911`
- party:
  `BRIDGE-PR18-NATIVE-OUTSTANDINGS-PARTY-019f605f-e6cf-77b2-ac95-31722887a911`

Education-mode voucher events use only literal calendar days 1, 2, or 31.

| Source event | Date | Synthetic detail | Expected accounting state |
|---|---:|---|---:|
| Ledger opening | fixture setup | `OPEN-001`, INR 250 Dr | opening bill INR 250 Dr |
| Sales | 2026-07-01 | `INV-001`, New Ref INR 1,000 Dr, explicit due 2026-07-31 | invoice pending INR 1,000 Dr |
| Sales | 2026-07-01 | separate On Account INR 125 Dr | On Account INR 125 Dr |
| Receipt | 2026-07-02 | INR 400 Agst Ref `INV-001` and INR 50 Advance `ADV-001` Cr | invoice pending INR 600 Dr; advance INR 50 Cr |
| Receipt | 2026-07-31 | final INR 600 Agst Ref `INV-001` | invoice settled; report presence or omission remains unobserved |

These are fixture and accounting expectations, not expected XML tags, signs,
ordering, completeness, or zero semantics. The exact Candidate profile,
template, rendered-request, and scope hashes for each allowed scenario are
sealed in the reviewed fixture manifest. A mismatch requires a new reviewed
candidate or fixture version; it must not be accepted as drift.

## Structured UI evidence

Before invoking the runner, copy the incomplete
[`native-outstandings-ui-before.example.json`](./native-outstandings-ui-before.example.json)
and
[`native-outstandings-ui-after.example.json`](./native-outstandings-ui-after.example.json)
under ignored `.bridge-live/` and complete the UI-before capture. The examples
are rejection fixtures: zero hashes,
zero timestamps, false visibility flags, empty projections, `unobserved`
settlement state, or `evidence_complete: false` must never authorize dispatch or
complete a receipt.

For the selected scenario, both observations must record:

- the exact fixture, scenario, company marker, party marker, and ToDate;
- the native `Ledger Outstandings` report in Detailed mode;
- Opening, Pending, Due, and Overdue columns visibly present;
- a screenshot SHA-256 and nonzero capture time;
- every visible row in display order using the exact ordered projection fields
  declared by the fixture manifest;
- the displayed date, reference, opening and pending amount text, due and
  overdue text, voucher details, and Dr/Cr text without normalizing them;
- an explicit operator attestation that no Tally interaction occurred inside
  the dispatch bracket.

Each projection row is a deny-unknown-fields object containing exactly the ten
declared fields. `row_index` must be contiguous from zero, `row_kind` is a
nonempty bounded label, each copied display field is bounded and control-free,
and at most 256 rows are accepted. The settled-reference value is the closed
enum `present`, `omitted`, or `unobserved`; arbitrary prose is rejected. The
July 1 and July 2 scenarios require `present`, while July 31 requires an
explicit `present` or `omitted` observation.

The runner must compare canonical structured before/after projections. A
screenshot hash only binds an artifact; before and after screenshot hashes are
not expected to be equal and do not prove semantic equality. UI evidence is
operator-observed, not machine-attested.

For the July 31 scenario,
`inv_001_settled_reference_observation` must be exactly `present` or `omitted`
in both observations. The runner must not assume omission. For July 1 and July
2 it must record `present`.

## Two-stage consent and exact POST sequence

A complete invocation that reaches every Candidate uses exactly **2 preflight
POSTs + 11 dispatch POSTs = 13 POSTs**. The dispatch consists of eight sealed identity
reads and three byte-identical CandidateV0 reads. Preflight authority cannot be
reused as dispatch authority.

### Stage 1: identity preflight consent

Before any network request, validate the reviewed fixture/profile hashes, exact
About observations, nonzero reviewed identity commitments, loopback endpoint,
no-customer-data attestation, and selected single scenario. Then require an
exact run-bound challenge equivalent to:

`PREFLIGHT NATIVE-OUTSTANDINGS <fixture-id> <scenario-id> 2POSTS <digest-prefix>`

The digest must bind the runner/build and reviewed surface, executable and lock
hashes, fixture and local-registration hashes, About observations, canonical
loopback endpoint, selected scenario, and the exact two-request budget.

Only after exact consent may the runner send, in order:

1. sealed `CompanyListV1` once;
2. sealed `LedgersV1` once, scoped to the uniquely verified company.

The company read must return exactly one loaded company whose marker and
identity match the reviewed local commitment. The ledger read must return
exactly one matching synthetic party whose independently observed source
identity matches its commitment. Any mismatch, ambiguity, invalid application
status, truncation, encoding failure, or response-limit failure ends the run.
No CandidateV0 request follows.

### Stage 2: UI bracket and dispatch consent

The runner validates the already captured UI-before observation before the
preflight challenge, then reuses its commitment when binding dispatch. After
both preflight reads pass, it seals CandidateV0 for the single scenario and
verifies its profile, template, rendered-request, and scope hashes against the fixture.
Require a new exact challenge equivalent to:

`DISPATCH NATIVE-OUTSTANDINGS <fixture-id> <scenario-id> 11POSTS <digest-prefix>`

This digest must additionally bind the two observed identity commitments, both
preflight response commitments, the UI-before commitment, the exact sealed
CandidateV0 commitments, and the immutable 11-request interleave below. It is
single-use and cannot be replayed for another date, endpoint, fixture, release,
UI capture, or build.

The runner may then execute exactly this order with zero retries and no Tally
interaction between requests:

1. identity bracket B0: sealed `CompanyListV1`, then sealed `LedgersV1`;
2. CandidateV0 attempt 1;
3. identity bracket B1: sealed `CompanyListV1`, then sealed `LedgersV1`;
4. CandidateV0 attempt 2;
5. identity bracket B2: sealed `CompanyListV1`, then sealed `LedgersV1`;
6. CandidateV0 attempt 3;
7. identity bracket B3: sealed `CompanyListV1`, then sealed `LedgersV1`;
8. capture UI-after evidence immediately after B3.

Every B0-B3 identity read must reproduce the uniquely verified company and
party context and match the same reviewed identity commitments. Each Candidate
request must be byte-identical. A complete fixed-budget run therefore has 2
preflight POSTs followed by 8 bracket identity POSTs and 3 Candidate POSTs, for
13 POSTs in one fixed order. A preflight or identity failure stops the run and
does not consume the remaining budget. A Candidate failure does not authorize
a retry: it is recorded as the already-budgeted attempt, its trailing bracket
is completed, and the fixed sequence continues only while identity is valid and
unchanged.

For each of the three Candidate responses, record only bounded metadata:
attempt ordinal, template/request/scope hashes, HTTP and strict application
status, encoding, encoded byte count, response hash when available, and a safe
reason code for non-success. Record
the corresponding bounded status and commitments for every bracket identity
read. Compare all three available Candidate response bodies byte-for-byte
while they remain in memory. Hash equality is a commitment, not a substitute
for the in-memory byte comparison. If any body is unavailable, byte
repeatability is `NotEstablished`. Drift, a bracket mismatch, a failed attempt,
or any ambiguous grammar keeps the result `ProfileUnobserved`.

After B3, capture and validate UI-after evidence. A canonical before/after
projection mismatch invalidates the whole sequence.

The final save challenge commits to both the sealed receipt and its exact JSON
output under the canonical repository-local ignored `.bridge-live` directory.
The output target is issued before dispatch, consumed on save, revalidated, and
never overwrites an existing file.

## No raw retention in PR18

PR18 has no raw-response save mode, save challenge, XML output file, diagnostic
preview, or automatic upload. Raw response bytes must never enter logs, errors,
receipts, screenshots, clipboard helpers, or `.bridge-live/`. Only hashes,
counts, statuses, encodings, timings, and the bounded operator-created UI
evidence may survive. A later request to retain raw fixture responses requires a
separate security/privacy review and new explicit authority.

## Negative observations remain blocked

Wrong-company, wrong-party, no-company, bill-wise-disabled, empty-party, and
other negative scenarios are outside this positive fixture runner. They require
separate reviewed fixture/scenario manifests and separate consent budgets. Do
not weaken the identity gate to exercise them.

## Promotion boundary

A probe result cannot enter `BillsAndPaymentsBatch`, mirror, checkpoint, proof,
capability, support matrix, or production runtime paths. Before adding a parser,
freeze the exact observed grammar for the attested release and add adversarial
replay, scope-mismatch, truncation, encoding, duplicate, drift, row-reordering,
and false-zero tests. Before adding a canonical adapter, separately qualify
sign, direction, due-date, On Account, settled-row omission, coverage, and
empty-scope semantics.

CandidateV0 request bytes and commitments are immutable. Any request change
must introduce CandidateV1; a future qualified profile must use a separate type
instead of renaming or broadening CandidateV0.
