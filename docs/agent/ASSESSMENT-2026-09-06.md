# Bridge MCP implementation assessment — 2026-09-06

This assessment replaces simulator-only readiness claims for PR #228. The
connector now has a working end-to-end slice on a licensed TallyPrime instance
containing only operator-created synthetic companies. It remains a local stdio
read connector with an opt-in file-generation/readback workflow. It does not
send imports to Tally. The PR must not be merged until the final candidate's
required hosted checks and review pass.

## Design decision

Retain the small stdio adapter and reuse Bridge's native protocol and runtime.
The failures were primarily duplicated external-protocol assumptions, not a
missing framework. Split protocol admission, tool catalog, financial reads,
parsing, response limits, and receipts into separate modules; move tests out of
the production files. Keep one runtime dispatch path for connector reads and
preserve company identity, paired-read, date-admission, and redaction controls.

A wholesale SDK replacement would change dependencies and transport behavior
without establishing any Tally semantics. The official Rust SDK remains a
reasonable future integration option. For this candidate, enforce the small
protocol surface explicitly and test it with the official JavaScript client.
No new production dependency is required. [Official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)

The stdio transport uses newline-delimited JSON-RPC and reserves stdout for
protocol messages. Initialization negotiates a supported protocol version;
unknown tools produce protocol errors. Structured results also have equivalent
serialized JSON text content for compatible older clients. These choices follow
the protocol's transport, lifecycle, and tool requirements.
[Transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports),
[lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle),
[tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)

## What changed and why

| Surface | Decision and resulting behavior |
| --- | --- |
| Native collection requests | Use explicit object-type elements, a defined collection matching the export ID, and measured ledger-entry fetch paths. |
| Native parser | Read actual collection objects and direct scalar fields; preserve strict row validation and exact decimal values. Retained live captures exercise counters, numeric padding, Unicode, and nested allocations. |
| Ledger movement | Use Tally's observed period opening at the requested start. Apply only in-window, non-cancelled, non-optional voucher entries. Eliminate the earlier-history scan. |
| Change enumeration | Hide and refuse `changed_since`. A client result cap is not a server-work bound, and unqualified snapshot continuation is not a reliable change feed. No bypass setting is added. |
| Import verification | Reserve explicit markers before fallback matching, consume each observed row once, and distinguish attributed postings from matching content. Equivalent decimal spellings compare equally without changing stored XML bytes. |
| Response recovery | Preserve a persisted batch ID through result caps, final framing caps, and receipt failures. The client can recover without generating another transaction. |
| Receipts and proofs | Derive released field paths from final redacted output. Serialize proof publication. Use portable receipt locking and decode only complete UTF-8 tail lines. |
| Financial summaries | Label gross exposure explicitly and keep billed/unallocated receivable and payable directions separate. |
| MCPB packaging | Validate the actual per-platform manifest, packaged executable, legal resources, and extracted archive launch. |

The Windows receipt handle now requests read access in addition to append access.
That supplies the access required by `LockFileEx` while retaining append-only
writes. [Microsoft file-lock contract](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)

MCPB staging follows the actual manifest 0.1 schema, including executable command,
entry point, and environment mappings; official CLI validation is part of bundle
CI. A ZIP file with a manifest-shaped object is insufficient evidence.
[Official schema](https://github.com/anthropics/mcpb/blob/70fe3b34cd6dff1b3bba046638edc72a6467a4fb/src/schemas/0.1.ts)

### Dated correction: period opening

The earlier implementation calculated a running opening by adding every prior
voucher to the book-start opening. This is not Tally's period presentation for
nominal accounts. Live observations showed Sales opening at zero after earlier
Sales activity, while a debtor and Cash carried their balances into the next
period. The new `balance_basis` explicitly identifies native period opening plus
direct voucher movement. Calculated closing is not the balance-sheet
`CLOSINGBALANCE` method. See [protocol reference section 5.5](../tally/TALLY_PROTOCOL_REFERENCE.md#55-openingbalance-follows-svfromdate-not-just-the-ledger-master).

No account classification is guessed from names or immediate parent groups.
Missing opening observations stay partial. This corrects the unshipped contract;
it does not establish universal report parity across every ledger type.

## Live evidence and limits

The operator authorized synthetic experimentation on the licensed endpoint.
Requests were serial, with gateway health checked before and after each tool
call. Exact request/response bytes, JSON-RPC frames, executable hashes, and local
receipts were retained. No dense-book or destructive experiment was required.
The operator import action was separate from the MCP process and used its exact
generated file. No automatic import retry was used.

| Probe | Observed result |
| --- | --- |
| Discovery | Licensed mode observed; 16 loaded company tuples returned. A duplicate GUID across split books was reported and scoped access refused. |
| Ledger catalog | Nine ledgers read; exact names, Unicode names, and a missing name distinguished. Compliance read completed. |
| Vouchers | Three existing vouchers returned for 2026-08-01; pagination returned two then one without loss. Adjacent-day emptiness was corroborated. |
| Movement | Three Sales entries totaled `306.06`; counterpart amounts were `101.01`, `102.02`, and `103.03`. Cancelled/optional state is observed explicitly. |
| Period opening | Sales opening remained `0.00`; a debtor carried `-102.02`; Cash carried `-12.50` after the test Journal. |
| Outstandings | A bill-wise lab returned billed receivable `8777` and payable `5000`. Another lab without confirming bill-reference evidence returned the runtime's explicit partial reason. |
| File build | One Journal, equal debit/credit `12.50`, persisted with a pre-import voucher mark of 3. Generated XML SHA-256: `7c02fd1b598d70157fa676e5a41ae16184034d39169793d8554e31713cd0f127`. |
| Before import | Verification reported one `not_found`, zero `posted_verified`. |
| Operator import | Tally reported created 1, altered 0, errors 0, exceptions 0. Those counters alone were not treated as verification. |
| After import | MCP readback found one attributed posting, exact signed entries, and AlterID 4. Tally replaced REMOTEID; the retained narration marker established attribution. |
| Exact-file repeat | Tally reported created 0, altered 1. Readback retained the same voucher GUID/master ID, one posting, no duplicate, and AlterID 5. |
| Controlled comparison | A temporary change to the owned test Journal was reported as divergent with masked entry names in both MCP representations. The original generated file was restored afterward. |
| 256-byte build response | A persisted second batch returned a 152-byte error containing its recovery batch ID. That second file was not imported. |

This qualifies the recorded synthetic slice only. It does not qualify every
voucher type, account nature, licence mode, release, customer dataset, or desktop
client. Direct Tally imports remain outside the connector. The compatibility
matrix remains `unknown`; these observations are not substituted for its signed
qualification process. Tally's official integration documentation describes the
XML interface, but returned bytes and readback establish the behavior above.
[Official XML integration documentation](https://help.tallysolutions.com/xml-integration/)

## Regression provenance

The protocol fixture tree contains unchanged UTF-16LE captures for an empty
native collection and a three-voucher response. A separate ledger-catalog
regression derivative changes only documented synthetic identities/names to fit
the existing simulator. Its metadata records original and transformed hashes
separately; it is not represented as unchanged live output. Fixture-integrity
policy remains enforced. Simulators test regressions after live observation.

## Verification and release gate

Use Rust 1.96, Node 24, and the pinned pnpm 11.7.0. From the repository root:

```sh
pnpm test
pnpm run build
pnpm run license:all
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --locked --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
node scripts/check-ci-workflow-consistency.mjs
node scripts/check-fixture-byte-integrity.mjs
node scripts/check-tally-live-read-boundary.mjs
node scripts/check-tally-request-builder-hazards.mjs
```

Local candidate verification: **850 Rust workspace tests**, **115 agent tests
within that workspace**, **48 tools-workspace tests**, **107 Node tests**, **6
Vitest tests**, and **2 Playwright tests** passed. Both Rust workspace Clippy
runs passed with warnings denied. Frontend build, formatting, licensing,
fixture-integrity, workflow, read-boundary, request-hazard, compatibility gate,
and matrix-Markdown checks passed. Compatibility gate: 11 unknown claims,
zero evidenced claims. These counts describe the settled local source; fresh
hosted checks are still required for its published commit.

The final macOS arm64 release binary also passed five live tools (status,
vouchers, movement, outstandings, and restored-batch verification), with party
masking selected. That same binary passed eight checks using official MCP
JavaScript SDK 1.30.0, negotiating 2025-11-25 down to 2025-06-18. Official MCPB
CLI 2.1.2 validated and packed the archive. Its extracted executable and all four
legal resources matched the staged bytes; executable mode survived extraction;
the manifest command initialized and listed ten default tools successfully.

- Release executable SHA-256: `fb41a41f9e6c5375e2cc61bd6855a2f13eb1e7babb8b42eca9b737ae1f114aee`.
- MCPB archive SHA-256: `600e1727a3cc420a81f788570464e0a1e2d2b832c72998500cb03cc85d08b227`.
- Source fingerprint (297 build-input files, unchanged through build): `6b612a8b54aeb97f7b3bafe13f4d7f214e9a7452a79c2889de089f25e56e788e`.

CI builds, validates, packs, extracts, and launches the actual MCPB on Windows
and macOS. The portable smoke checks initialization, ten default tools, the local
voucher schema, and its persisted egress receipt with bounded execution and
output. Five harness regressions and the final local archive passed this check.
Hosted results must confirm the same check for the published candidate; desktop
client installation remains separate. The local archive is unsigned and is not
a production release.

The final verification record must distinguish local tests, actual archive
launch, official client smoke, live Tally evidence, and hosted Windows/macOS
checks. None substitutes for the others. Graphify data and its refresh script
were unavailable in this checkout; structural discovery used focused source
tracing instead.

Three of 163 sealed-surface entries changed: CI packaging verification, the
native period-opening adapter, and its protocol-reference correction. The
existing pin set was audited and rehashed, sealed, and repointed using the
release-process commands. No compatibility cell was promoted.

## Migration, rollback, and security impact

See [README migration notes](README.md#protocol-and-migration-notes) for output
field changes, unavailable change enumeration, strict argument admission, and
fingerprint-only attribution. There is no database migration. Preserve local
import files and their append-only ledger across binary rollback. Rolling back
the connector does not undo an operator import into Tally.

The connector remains loopback-only and never dispatches import XML. Company
identity, response redaction, bounded input/output, local private-file handling,
and exact emitted-byte receipts remain review requirements. Public fixtures
contain synthetic data and no credentials or operational machine paths. Required
hosted Windows checks must confirm the platform-specific file-lock correction.
