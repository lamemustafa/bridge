# ADR 0015: Selected-read qualification is exact-scope evidence only

## Status

**Withdrawn 2026-09-17 (#474).** Accepted below for the local read-only setup
flow when written, and still an accurate record of what was decided and why.
The implementing path is gone: see **Amendment** below before relying on
anything in this ADR as current behavior.

## Context

A successful company-list probe proves neither that a selected company's
ledgers can be exported nor that a bounded voucher request is honored. Treating
any parseable or empty response as broad support would be especially unsafe:
Tally can return application failures inside HTTP success, company context can
drift, empty rows do not prove source emptiness, and an Education installation
does not establish behavior for other releases or license modes.

## Decision

Bridge may qualify exactly two versioned XML profiles after an operator reviews
one fresh GUID-bearing company and chooses an inclusive voucher window of at
most 31 days:

- `bridge.tally.ledgers/1` for the selected company's ledger export; and
- `bridge.tally.vouchers/3` for the same company and an echoed exact
  `FROMDATE`/`TODATE` window.

Qualification is single-attempt and read-only. It requires successful Tally
application status, an exact reviewed wrapper skeleton, no unexpected wrapper
attributes or CDATA, matching company GUID and normalized name, exact schema and
record-count evidence, case-insensitive GUID collision checks, and records that
pass the same bounded identity, text, amount, date, AlterID, and fragment-hash
validation required by canonicalization. A populated result requires verified
record identities. A proven zero-row profile execution records identity
evidence as `not_applicable_empty`; it never becomes a completeness or source-
empty claim.

The runtime reserves the reviewed probe before either selected request through
an opaque owner-bound lease. Read admission and reservation are mutually
exclusive at the endpoint session, and each qualification dispatch proves the
reservation owner, originating runtime instance, and exact session endpoint.
The lease holds the session alive and releases only its exact review on
cancellation, task abort, panic unwinding, early return, or ordinary drop.
Explicit consume and replacement disarm it idempotently; a stale lease
cannot clear a newer review. Reserved sessions are also excluded from endpoint
capacity eviction. Cancellation therefore restores the original review rather
than manufacturing an observation. Replacement reviews inherit the original
probe freshness origin, so failed or repeated qualification cannot renew stale
setup authority.

The returned UI object contains no ledger, voucher, amount, or company identity
values beyond the already reviewed company list. Raw rows are discarded. The
local encrypted mirror may retain request and decoded-XML SHA-256 commitments,
the observed decoding label, bounded result buckets, and verification states.
The decoded-response hash is explicitly not a wire-byte hash and is classified
as a local pseudonymous fingerprint; it is excluded from UI serialization and
public support export.

On explicit save, migration v7 stores the selected scope, the two observations,
and the Capability Passport in one transaction. Migration v8 records the full
review commitment, a domain-separated canonical setup-payload commitment, and
the resulting snapshot/company authority in that same transaction. An exact
replay returns those original references without inserting a second setup; a
different payload under an already consumed review fails closed. This makes a
lost acknowledgement after SQLite commit recoverable without renewing or
duplicating reviewed authority. Consumption rows are immutable and require the
snapshot and company to share one endpoint.

The repository recomputes the scope commitment from the canonical endpoint,
case-folded persisted company GUID, observed company name, profiles, window,
observation time, outcome, encoding, hashes, and all verification states.
Observations have a composite foreign key to the same scope and capability
snapshot. Scope, observation, consumption, and capability-item rows are
immutable. Legacy case-fold company GUID collisions block migration and are
never auto-merged.

## Consequences

The Passport may report `selected_ledger_read` or
`selected_voucher_window_read` only for the committed company/profile/window.
The existing broad `ledger_read` and `voucher_read` features remain `Unknown`.
A ledger failure skips the voucher request and preserves a partial Unknown
outcome; no failure is converted to `Unsupported` without an explicit contract.
No Tally write is attempted or authorized.

Portable and native simulator evidence can validate this authority boundary,
but it cannot create a public compatibility claim. The exact Windows/Tally/
release/mode support-matrix cell remains `Unknown` until a consented synthetic
live observation and the separate compatibility attestation gate succeed.

## Amendment (2026-09-17, #474): the implementing path is withdrawn

The command surface this ADR authorized was never wired to a caller. The
setup/readiness flow that was meant to invoke it was replaced (the "readiness
workflow change" already noted in `docs/rust-module-conventions.md`'s
`commands.rs` map), and from that point `qualify_selected_tally_reads`,
`fetch_tally_ledgers`, `fetch_standard_tally_ledger_catalog`, and
`fetch_tally_vouchers` were declared `#[tauri::command]` functions with no
entry in `generate_handler!`. `scripts/tally-setup-safety.test.mjs` pinned that
absence, and `scripts/tauri-command-registration.test.mjs` (#462) allow-listed
it, rather than either side calling the path or removing it.

Issue #474 decided **delete** over **keep, with a reason**. This PR removed the
four commands and every helper reachable only from them in
`src-tauri/src/commands.rs` — 12 helper functions and three structs
(`QualifySelectedReadsRequest`, `VoucherRequest`, and the response type
`SelectedReadQualificationResult`) — and the one direct
unit test of an otherwise-exclusive helper
(`selected_read_observation_distinguishes_empty_identity_evidence` in
`commands_tests.rs`). What this measured, precisely:

- **Compiler-verified dead code.** With the four commands' and their two
  request/result structs' visibility narrowed from `pub` to `pub(crate)` (the
  minimum narrowing that removes their automatic "reachable from the crate
  root" status; narrowing the surrounding `pub mod`s instead breaks a real
  cross-crate use in `tests/unit_a_live.rs`),
  `cargo check --locked --workspace --all-targets --all-features` from
  `src-tauri/` reported all 18 deleted items (4 commands, 2 structs, 12
  functions) as `never used` / `never constructed`, and nothing else. The
  nineteenth deleted item, `SelectedReadQualificationResult`, draws no warning
  under this method (rustc still visits a dead function's signature types); a
  tree-wide search confirms its only references were inside `commands.rs`.
  After deletion, the same command against the same tree reports zero warnings.
- **Scope.** This is `commands.rs`-only evidence. The runtime- and db-layer
  machinery this ADR describes — `CachedProbeReservation`,
  `TallyRuntime`/`TallyClient::qualify_selected_ledgers` and
  `::qualify_selected_vouchers`, `db::tally_mirror::
  selected_read_scope_commitment_sha256` and its commitment-material types —
  is **not** touched by this change, and is a separate decision.

  Coverage is **uneven**, and the difference matters:
  `qualify_selected_ledgers` has direct unit tests that call it independently
  of the deleted commands (`tally/connection_tests.rs:375`,
  `tally/runtime_tests.rs:1503`, `:1593`, `:1725`), as does the
  `db::tally_mirror` commitment material. **`qualify_selected_vouchers` has
  none** — a tree-wide search finds only its two definitions
  (`tally/connection.rs:1635`, `tally/runtime.rs:3169`) and one internal call
  at `tally/runtime.rs:3190`. Deleting `fetch_tally_vouchers` removes its last
  caller outside that pair, so it is left reachable only from `runtime.rs` and
  pinned by no test. It survived this compiler check because the check was
  scoped to `commands.rs`, not because anything exercises it. Whoever next
  decides the fate of this machinery should treat the vouchers path as
  unprotected rather than assume the ledgers path's coverage extends to it.
- **Test names.** `cargo test -p bridge --lib -- --list`, sorted, before and
  after, differs by exactly one line: the removed
  `commands::tests::selected_read_observation_distinguishes_empty_identity_evidence`.
  Every other test name is unchanged.

**What this does not claim.** It does not claim the runtime/db qualification
machinery is dead — it measurably is not, by its own tests. It does not claim
selected-read qualification was a bad design; the Context and Decision above
still describe a real, carefully-scoped authority boundary. It claims only
that the specific `#[tauri::command]` surface had no live caller and no
reintroduction plan on record, so the pinned safety test and the allow-list
that existed only to keep it unexposed were themselves dead weight.

**To reintroduce this path**, a future change would need to: restore the four
commands and their exclusive helpers in `commands.rs` (this history is in
version control), register them in `generate_handler!`, wire an actual caller
in the setup/readiness flow, restore or rewrite `commands_tests.rs` coverage
for the command layer (the runtime/db layer already has it), and record that
decision here or in a superseding ADR — not by re-adding a pinned
"deliberately unexposed" allow-list, which this PR also removed from
`scripts/tauri-command-registration.test.mjs` as no longer meaningful once
nothing is deliberately unexposed.

## Amendment (2026-09-26, #732): the runtime qualification path is deleted

The 2026-09-17 amendment left the runtime layer "a separate decision". #732 made it. The path's
ledger read (`ledgers_v1`) carried an amount FIELD without `<TYPE>Amount</TYPE>`, and a FIELD
without it returns money as a display string, sign dropped (protocol reference §6.3). Tracing who
consumed that read showed nothing live reached the path. Following AGENTS.md P4, the path is
deleted rather than fixed.

**Deleted:**
- `TallyRuntime::qualify_selected_ledgers`, `TallyRuntime::qualify_selected_vouchers` and
  `TallyRuntime::fetch_companies_for_reservation`. The last one existed, by its own doc, so a
  reservation owner could qualify its selected reads.
- `TallyClient::qualify_selected_ledgers` and `TallyClient::qualify_selected_vouchers`,
  `SelectedReadObservation`, and `SELECTED_LEDGER_QUERY_PROFILE_ID` and
  `SELECTED_VOUCHER_QUERY_PROFILE_ID`.
- What the compiler then reported as unused:
  - `post_xml_with_request_wire_sha256` and `observed_encoding_label`;
  - `validate_selected_read_identity_evidence`, `validate_selected_ledgers` and
    `verify_selected_company_name`;
  - `EducationReportFamilyRefusal` and `refuse_report_formula_in_education`;
  - `CachedProbeReservation::authorize`;
  - the `runtime_identity` fields that only `authorize` read.
- The five tests that called the deleted methods. A sixth test,
  `ordinary_read_admission_and_review_reservation_are_mutually_exclusive`, keeps its live
  assertions and drops the two that called `authorize`.

**Kept:**
- `CachedProbeReservation`, which `reserve_cached_probe_fresh` still creates for two commands.
- The `db::tally_mirror` commitment material.
- The `ledgers_v1` and voucher profiles in `bridge-tally-protocol`, which the live-read tools still
  send. `ledgers_v1`'s opening-balance FIELD now declares `<TYPE>Amount</TYPE>`. Its sealed
  template digest and the ledger canary's, which derives from it, are updated deliberately.

**How this was measured:**
- **Compiler.** `cargo check --locked --workspace --all-targets --all-features` for `src-tauri/`
  and for `tools/` builds with every deleted item gone. `cargo clippy` with `--all-features` and
  `-D warnings` reports nothing.
- **Tests.** `bridge`'s sources carry exactly five fewer `#[test]`/`#[tokio::test]` attributes
  than master (1441 to 1436): the five deleted tests.
- **Results.** `cargo test -p bridge-tally-protocol` passes 348 of 348. `cargo test -p bridge
  --lib`: 1425 passed, 0 failed, 6 ignored.
