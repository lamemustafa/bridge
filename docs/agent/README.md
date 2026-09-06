# Bridge MCP

`bridge_mcp` is Bridge's newline-delimited JSON-RPC 2.0 MCP server. It uses
Bridge's loopback-only Tally XML transport for reads. It can render a local
voucher import file, but it never sends that file—or any import request—to
Tally.

Build and run it with Rust 1.96:

```sh
rustup run 1.96.0 cargo run --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
```

Configure it with `BRIDGE_TALLY_HOST` (default `localhost`),
`BRIDGE_TALLY_PORT` (default `9000`), `BRIDGE_AGENT_DATA_DIR` (Bridge's
platform application-data directory by default), `BRIDGE_AGENT_MAX_ROWS`
(default `500`), `BRIDGE_AGENT_MAX_BYTES` (default `200000`), and
`BRIDGE_AGENT_REDACTION` (`none`, `mask_parties`, or `drop_narration`). The
host is validated by `bridge-tally-transport`; non-loopback hosts are refused.
On Unix, new data directories use mode `0700`; an existing data directory
must belong to the current user and have that mode. Otherwise startup refuses
it without changing its permissions. Select a new dedicated leaf under a shared
parent rather than using the shared directory itself. Windows directories retain
their inherited ACLs; symlink and reparse-point leaves are refused.

Claude Desktop example:

```json
{
  "mcpServers": {
    "bridge-tally": {
      "command": "/absolute/path/to/bridge_mcp",
      "env": {"BRIDGE_TALLY_HOST": "localhost", "BRIDGE_TALLY_PORT": "9000"}
    }
  }
}
```

Cursor uses the same server object in `.cursor/mcp.json`:

```json
{"mcpServers":{"bridge-tally":{"command":"/absolute/path/to/bridge_mcp"}}}
```

The read tools are `tally_status`, `list_companies`, `outstandings`,
`ledger_masters`, `ledger_movement`, `vouchers`,
`read_evidence`, and `egress_log`; `voucher_schema` and `validate_masters` are also
available by default (ten tools total). Each call returns compact JSON with the
company identity where scoped, a read timestamp, request/response commitments,
byte count, completeness reason, and truncation state. Before a tool response is written, Bridge appends a metadata-only
`response_prepared` record to `agent-egress.jsonl`, including a unique `receipt_id`.
After `write_all` and `flush` succeed, it appends a `stdio_write_completed` record
with the same ID and response hash plus `bytes_written`. This confirms the local
stdio write, not consumption by the client. A missing completion leaves delivery
unconfirmed; a completion-record failure terminates the session before another
request runs. Receipt lines never contain voucher bodies. Redaction happens before a result reaches the client.
`fields_prepared` contains sorted, unique leaf paths from the final redacted,
byte-bounded `structuredContent`, including `company`, `evidence`, and `result`.
For example, `result.open_bills[].due_date` records the prepared field, never its
value. Arrays use `[]` without indices; null fields and empty collections remain
represented. Protocol refusals with no structured payload list no prepared fields.
`rows_prepared` and `bytes_prepared` describe the planned output, including its
newline. Tool-call notifications produce only a `notification_refused` record;
no response or write-completion record is produced. Existing untyped receipts
and their former `*_returned` fields remain readable historical records, but
cannot establish completed delivery. Consumers must join new records by
`receipt_id`; preparation alone is not a completed write.
All output object keys must remain server-defined; identifiers belong in values,
including when adding new grouped reports.
`egress_log` reads only the final 256 KiB, in 64 KiB reverse-seek chunks, so a
larger receipt file still yields its bounded tail without loading the head.
`changed_since` is unavailable: it is omitted from tool discovery and direct calls
are refused as `changed_since_unqualified` before contacting Tally. Bounded change
enumeration and snapshot continuation have not been qualified. There is no operator
setting to enable this tool.

### Ledger-movement opening decision

`ledger_movement` reads the native ledger opening with `SVFROMDATE` set to the
requested `from` date, then combines it with voucher entries from the requested
window. It does not scan earlier voucher history. The ordinary ledger-master
export remains pinned to `BOOKSFROM`.

`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §5.5 records account-dependent native
period semantics: observed balance-sheet ledgers carry balances into the period,
while observed nominal ledgers open at zero. The returned `balance_basis` is
`tally_period_opening_plus_direct_voucher_movement`. Calculated closing is a
period movement result, not Tally's balance-sheet `CLOSINGBALANCE` field. Bridge
uses the returned opening without guessing account classification from ledger names
or immediate parent groups. A missing opening keeps both opening and closing
unestablished, including at book start. Qualification covers the recorded account
groups and instances; it is not a claim of universal ledger-report parity.

The runtime retains its paired read, verified company, book-extent checks, and
endpoint date-boundary admission. A rejected opening boundary is refused before
the ledger export. A genuinely empty voucher response uses the same wider-window
corroboration as `vouchers` before zero movement can be reported. Cancelled and
optional rows establish response presence while contributing no accounting movement.

## Voucher-file loop (manual Tally import only; disabled by default)

`build_import_xml` and `verify_import` are hidden unless the operator sets
`BRIDGE_AGENT_ENABLE_IMPORT=1`. A licensed synthetic-lab Journal file cycle and
exact-file repeat import were observed on 2026-09-06. New file generation accepts
only `Journal`, with freshly observed licensed TallyPrime before and after build
reads. Other products, Education and unknown modes are refused before publishing
a file. `Payment`, `Receipt`, and `Contra` are refused until each has
live import/readback evidence. Historical batch records remain readable. The response records
`live_evidence: "synthetic_lab_readback"` and links to
[the assessment](ASSESSMENT-2026-09-06.md). This does not qualify every voucher
type, host, licence mode, or manually imported file, so the feature remains opt-in.

1. Call `voucher_schema` and produce a payload matching its schema. Transaction
   IDs are client-supplied, unique, and retained in the local import ledger.
2. Call `validate_masters` with every ledger name. Correct every `near_miss`
   with the exact live spelling; Bridge never creates masters.
3. Call `build_import_xml` with the payload. It checks exact decimal balance,
   company date extent, live masters, and previously built transaction IDs,
   repeats the full catalogue to reject intervening changes, then writes `<data_dir>/imports/<batch_id>.xml` and records an append-only
   `agent-import-ledger.jsonl` line. `voucher_number` is optional: when absent,
   Tally applies the voucher type's own numbering configuration; when supplied,
   it is validated and sent so a Manual-type duplicate policy can reject it.
4. In Tally, with the intended company open, use **Gateway of Tally → Import →
   Vouchers** to import the file. Bridge does not dispatch this step.
5. Call `verify_import` with the company GUID and batch ID. It reads the date
   window back, compares the exact signed ledger entries, reports missing or
   divergent rows and duplicates, writes `.proof.json` and `.proof.md`, and
   appends the verification status to the local import ledger.

The file path is deliberately not a direct-posting path. Masters must already
exist and match exactly. File generation requires observed licensed TallyPrime;
the checks do not make a later manual import atomic with the earlier reads.
If verification would report any `not_found`, licensed TallyPrime must have been
observed before and after readback. Otherwise `verification_mode_unqualified`
withholds the negative verdict and leaves the previous proof and status intact.
Positive historical readback remains available in an observed unqualified mode.
A failed mode probe remains a read failure.

Safety boundary: local loopback only, bounded responses, verified company tuple
selection, append-only receipts, and no agent import dispatch. Unsupported:
Tally Cloud Access, every non-loopback Tally host, and change enumeration. A
`posted_verified` result is a readback comparison of the selected date window,
not live-Tally qualification or a claim that every Tally configuration or
licence mode has been qualified.

## Evidence-shaped outputs

Every tool response carries the same outer evidence envelope. This is an
illustrative response shape from the synthetic simulator test; it is not a
live-Tally compatibility claim:

```json
{
  "company": {"name":"BRIDGE SYNTHETIC BOOK","guid":"00000000-0000-4000-8000-000000000001","identity_state":"verified_tuple"},
  "read_at":"2026-09-04T00:00:00.000Z",
  "evidence":{"request_sha256":"…","response_sha256":"…","bytes":123,"state":"complete"},
  "truncated":false,
  "result":{"companies":[{"name":"BRIDGE SYNTHETIC BOOK","guid":"00000000-0000-4000-8000-000000000001"}]}
}
```

For Tally reads, request commitments hash the transmitted request body (UTF-16LE
for XML; empty for the status GET), and response commitments hash encoded response
bodies. Multiple sources combine their commitments in read order. `evidence.bytes`
counts committed response bodies, including both accepted bodies of a paired read;
it excludes auxiliary health and identity guards and is not total network traffic.
Status commits its status and company-discovery responses. Read failures retain
source observations already returned to the connector, including when parsing or
window validation fails. Runtime-internal requests that fail without returning
source evidence are not fabricated; zero retained bytes does not establish that
no HTTP request was attempted. Local-only tools and refusals without retained
source observations carry local evidence.

`outstandings` returns the runtime's paired native result. A complete read has
billed totals explicitly scoped to open bills, four overdue-age buckets, an
`unaged` bucket for future-due or unobserved ages, top parties,
open bills, and unallocated counts and directional totals; a refused runtime read instead has `state: "partial"` and its
exact `partial_reason`. `ledger_movement` returns literal-window voucher
movement with exact decimal `opening`, `debit`, `credit`, `closing`, parent,
and `vouchers_touching`. `ledger_masters` accepts `fields: "compliance"` to
return the paired party-master GSTIN/PAN/MSME/bank/IFSC/email/phone/state and
address observations; `mask_parties` redacts the ledger name before it leaves
the server. The unavailable `changed_since` implementation must not be used as
change-enumeration evidence; its retained internal response states that
deletion detection is unsupported.

## Protocol and migration notes

The server negotiates MCP `2025-06-18` or `2024-11-05`, returning a supported
version when a client proposes a newer one. Initialization must precede tool
requests. Incoming frames are limited to 5 MB. All responses obey the configured
byte cap, including control replies, the JSON-RPC wrapper, and newline. A tool
catalogue that cannot fit returns `agent_response_too_large`; the session remains
usable. Text content contains the same
serialized, redacted JSON as `structuredContent` for older clients.

Port zero and ports above 65535 are rejected at startup.
Unknown arguments, wrong selector types, and invalid enums are rejected before
any Tally request. Checkpoint numeric strings are no longer accepted at the tool
boundary. `changed_since` is unavailable; existing clients must stop calling it.

`validate_masters` accepts 1–100 nonblank names, each at most 1024 characters.
Near-miss suggestions are limited to 25 names and 8192 UTF-8 bytes per requested
name; `candidate_count` and `candidates_truncated` preserve ambiguity. Import
planning allows 1000 vouchers but at most 100 distinct ledger names per batch.
Repeated uses of a ledger do not consume additional distinct-name slots.
Voucher-type and ledger selectors share the 1024-character bound; ledger
lookup keys are computed once before scanning live names.

Byte-limited pages retain forward progress or return `agent_response_too_large`;
they never advertise the same offset after removing every row. Outstandings
trims both collections to a shared page width because they share an input offset.
Active vouchers without observed accounting entries are refused before movement
filtering; cancelled and optional vouchers remain excluded from movement totals.
Movement corroborates the complete opening-ledger snapshot after voucher reads
and rejects unknown entry names before selecting a ledger. Caller-specified
opening dates require freshly observed product and licence mode before and after
the read; a prior status call or cached licensed profile does not grant admission.

Top-party ranking uses `gross_exposure`, with billed and unallocated receivable
and payable fields kept separate. `totals.scope` is `open_bills_only`.
`unallocated.totals` contains `receivable`, `payable`, and `gross_unallocated`.
The previous ambiguous `outstanding_total` and `unallocated.amount` fields have
been removed. Gross exposure is not net money due.

A fingerprint match without a retained transaction marker is
`matching_content_observed`, with attribution unestablished; it is not counted
as `posted_verified`. Verification entry differences are structured objects;
duplicate metadata uses `fingerprint_sha256`. Each observed voucher can satisfy
at most one expected transaction. Exact numeric comparison tolerates equivalent decimal spellings
without changing the generated file or its stored hash.

If a build persists a file but its response exceeds the framing budget or its
preparation receipt fails, the recovery JSON-RPC error contains
`error.data.batch_id`. Retain it and use `verify_import` or inspect the local import
ledger; do not blindly rebuild or import another batch. If stdout itself fails,
the recovery ID may not reach the client; the generated XML and import ledger
remain available for local recovery. Proof JSON,
Markdown, and ledger status are published under one admission lock. Handled
publication failures restore the prior proof pair and ledger state. Builds create
the journal first, write and sync staged XML, then expose the importable filename. An interrupted
publication or failed rollback leaves a recovery journal and blocks further import
admission until the local files and ledger are reconciled. Preserve the journal,
its backups, and generated XML; do not delete it merely to retry. This is explicit
recovery after a partial file transaction, not a power-loss atomicity guarantee.

Prepared receipts count the bounded master-validation and loaded-company rows. Unknown tool
names are represented by `unknown` and `tool_name_sha256`; company IDs are
canonical UUIDs. Failed receipt appends restore the previous file length.
An incomplete log or failed rollback stops the session; a persisted build still
returns its recovery batch ID before termination. Reads withheld by the result
byte cap retain partial source commitments in the in-process evidence store.
Voucher selectors are applied after source-emptiness corroboration; a nonempty
source with no matching ledger can return a complete empty selection. Amounts
must parse as exact decimals, polarity flags must be `Yes` or `No`, and dates
must be valid calendar dates before ordinary voucher rows are released. Ledger
selectors require matching catalogues before and after the voucher read, unique
names, and catalogue membership for every observed entry. Movement
metadata counts all source vouchers, including non-posting rows excluded from
balances. Ordinary voucher rows expose boolean `cancelled` and `optional` fields;
non-posting amounts are not presented without that state. Movement repeats the
voucher source after its final opening snapshot and refuses changes to either
source before calculating balances. This establishes stability across repeated
observations, not an atomic Tally snapshot.

Evidence-history and egress-log reads use the smallest of the requested limit,
`BRIDGE_AGENT_MAX_ROWS`, and the 256-record ceiling. Omitted history sets
`truncated`; egress-log reads also report omissions from the bounded tail scan. Missing terminal newlines, invalid
UTF-8, or invalid JSON in retained complete rows return `egress_log_incomplete`.

Verification captures the batch journal generation before readback and checks it
under the exclusive publication lock. If another process published the same batch
in the meantime, `import_verification_conflict_retry` preserves the newer proof
and status; repeat verification to obtain a fresh observation. Unrelated batches
do not conflict, and no file lock is held during network reads.

Verification rejects malformed accounting fields before matching. Distinct,
fully attributed expected vouchers may have identical accounting contents; each
observed identity can satisfy only one expected transaction, and unexpected
duplicate postings remain blocking. A stable observed identity claiming multiple
expected transaction tags returns `import_verification_tag_ambiguous` before
matching, so transaction order cannot choose an attribution.

Verification appends a compact status record bound to the batch ID and original
file hash, rather than duplicating vouchers and narration. The reader accepts
existing full batch records and their historical updates. Output row limits do
not determine verification-source completeness; the collection read is uncapped
by that setting and its identity set is independently corroborated.

No database migration is required. Older binaries cannot read the new compact
status records. Preserve the data directory, import ledger, and proofs; use the
new binary for the import workflow or disable imports after a binary downgrade.
Do not truncate the ledger to force downgrade compatibility. A binary rollback
does not undo a separately imported Tally voucher. Existing files and transaction
IDs remain local recovery evidence.
