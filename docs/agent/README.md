# Bridge MCP

For ordinary Claude Desktop installation, start with the install page when it
is deployed, or use the [fallback installation guide](./INSTALL.md).
The developer configuration below remains for supported client integrations.

`bridge_mcp` is Bridge's newline-delimited JSON-RPC 2.0 MCP server. It uses
Bridge's loopback-only Tally XML transport. Reads are enabled by default.
The MCPB extension also exposes Journal preparation and posting by default,
with separate native approval for each new attempt. Command-line installations
retain explicit environment switches.

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
The resolved data path must be valid Unicode so saved artifact paths can be
returned exactly in JSON. Invalid platform encoding is refused before state
creation with `agent_data_dir_encoding_invalid`; no lossy path alias is used.

On Unix, new data directories use mode `0700`; an existing data directory
must belong to the current user and have that mode. Otherwise startup refuses
it without changing its permissions. Select a new dedicated leaf under a shared
parent rather than using the shared directory itself. Windows directories retain
their inherited ACLs; symlink and reparse-point leaves are refused.
Local journals, locks, staging files and delivery receipts must be regular files
with a single link, owned by the current user on Unix. Admission checks the
opened file before reading it or changing bytes or permissions. Existing files
that do not meet these requirements are refused; reconcile their storage before
continuing. New transaction directories are private from creation.

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
`read_evidence`, `egress_log`, and `verify_import`; `voucher_schema` and
`validate_masters` are also available by default (eleven read/schema tools).
The MCPB extension adds Journal building and posting by default, for thirteen
total. Each call returns compact JSON with the
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

The runtime retains its paired read, verified company and book-extent checks.
Native ledger openings, basic/compliance ledger balances, and native outstandings
require a freshly observed supported product and licence mode before and after
the reads. The operation then uses that mode's date-boundary profile: Education
accepts only its observed day-1/day-2/day-31 native boundaries, while Licensed
mode uses ordinary boundaries. An unsupported Education boundary returns the
operation-specific period refusal before its report is dispatched; Bridge never
rounds it. Release and licence tier remain observed facts, not blanket monetary
read exclusions. This shared runtime gate also affects native desktop consumers,
including those with an operator-supplied currency assertion. A prior status call
or cached profile does not grant admission.

The retained Education observations in protocol sections 5.3, 5.5 and 12a remain
valid within their recorded scope. Ordinary voucher reads retain their separate
literal-date and returned-row validation contract.
A genuinely empty voucher response uses the same wider-window
corroboration as `vouchers` before zero movement can be reported. Cancelled and
optional rows establish response presence while contributing no accounting movement.

Capability profile version 4 adds the observed licence tier alongside the release.
Older serialized profiles remain readable with an unknown tier, but saved profile
reuse requires a fresh matching version-4 observation. Additive database migration
26 stores the observed tier as nullable `silver` or `gold`; historical snapshots,
including earlier version-4 rows, stay null. A tier change changes the reviewed
setup commitment. Historical commitments without a tier retain their exact bytes.

If discovery rejects company identity fields, `tally_status` reports the profile
refusal reason and partial evidence with the completed source commitments. A
valid empty collection remains distinguishable from invalid discovery.

## Voucher-file preparation and verification

The MCPB extension exposes `verify_import` by default as a read-only recovery
tool. `build_import_xml` remains behind `BRIDGE_AGENT_ENABLE_IMPORT=1` for a
command-line installation, or is enabled with Journal posting as described
below. A licensed synthetic-lab Journal file cycle and
exact-file repeat import were observed on 2026-09-06. New file generation accepts
only `Journal`, with freshly observed supported TallyPrime product and licence mode
before and after build reads. Release and licence tier are returned as observed
facts; they do not independently refuse a Journal file. `tally_status` reports
the observed release and licence tier; the optional status-page banner cannot
supply these facts. `Payment`, `Receipt`, and `Contra` are refused until each has
live import/readback evidence. Historical batch records remain readable. The response records
`live_evidence: "synthetic_lab_readback"` and links to
[the assessment](ASSESSMENT-2026-09-06.md). This does not qualify every voucher
type, host, licence mode, or manually imported file.

1. Call `voucher_schema` and produce a payload matching its schema. Transaction
   IDs are client-supplied, unique within the batch, and retained in the local import ledger.
2. Call `validate_masters` with every ledger name. Correct every `near_miss`
   with the exact live spelling; Bridge never creates masters.
3. Call `build_import_xml` with the payload. It checks exact decimal balance,
   company date extent, live masters, and local journal integrity,
   repeats the full catalogue to reject intervening changes, then writes `<data_dir>/imports/<batch_id>.xml` and records an append-only
   `agent-import-ledger.jsonl` line. `voucher_number` is optional: when absent,
   Tally applies the voucher type's own numbering configuration; when supplied,
   it is validated and sent so a Manual-type duplicate policy can reject it.
4. In Tally, with the intended company open, use **Gateway of Tally → Import →
   Vouchers** to import the file. Bridge does not dispatch this manual step.
   Alternatively, use the separately approved MCP or desktop Journal posting
   flow below instead of importing the file manually.
5. Call `verify_import` with the company GUID and batch ID. It reads the date
   window back, compares the exact signed ledger entries, reports missing or
   divergent rows and duplicates, writes `.proof.json` and `.proof.md`, and
   appends the verification status to the local import ledger.

The file path is deliberately not a direct-posting path. Masters must already
exist and match exactly. File generation requires fresh supported product/mode
observations; the checks do not make a later manual import atomic with the earlier reads.
Education-mode Journal dates must be on day 1, 2, or 31. A different requested
date is refused as `education_voucher_date_unsupported`; Bridge does not move it.
Optional narration and reference must contain 1–2,000 Unicode characters when
supplied; omit them when unused. Control characters and the reserved attribution
marker are refused, including XML entity-encoded marker spellings. Voucher
numbers contain 1–32 characters and cannot contain controls or `$`. Lengths match
JSON Schema's character semantics; the 5 MB request-frame limit remains separate.
If verification would report any `not_found`, the supported TallyPrime product and
licence mode must have been observed before and after readback. Otherwise
`verification_mode_unqualified` withholds the absence verdict and leaves the
previous proof and status intact.
Positive historical readback remains available on an unqualified profile.
A failed profile probe remains a read failure.

Safety boundary: local loopback only, bounded responses, verified company tuple
selection, append-only receipts, and separately approved Journal dispatch. Unsupported:
Tally Cloud Access, every non-loopback Tally host, and change enumeration. A
`posted_verified` result is a readback comparison of the selected date window,
not live-Tally qualification or a claim that every Tally configuration or
licence mode has been qualified.

## Approved Journal posting

The MCPB extension makes **Allow Journal posting** available by default.
Turn it off for a read-only connector; existing saved settings remain respected.
For command-line installation, set `BRIDGE_AGENT_ENABLE_WRITES=true`.
This enables `build_import_xml` and `post_import`; `verify_import` remains
available so an uncertain saved batch can be checked after posting is turned
off. `BRIDGE_AGENT_ENABLE_IMPORT=true` alone exposes the manual file workflow,
while verification remains available without either switch. Both switches
accept `true`/`false` or `1`/`0`; invalid values stop startup. No model-supplied argument can grant approval. Claude controls
its own tool-call permission prompts: Bridge cannot preselect **Always allow**
for the user. That client permission does not approve an accounting entry.

One native-approved Journal and restart reconciliation have been observed on
macOS against a synthetic Silver 7.1 instance. This remains a preview: Windows
interactive approval and Gold/Education live posting have not been established.

1. Validate the exact existing ledger names and build **one Journal** using the
   file workflow above. A Journal is a voucher; Payment, Receipt, Contra,
   sales, purchases, tax, inventory and master creation remain unavailable.
2. Call `post_import` with the original `company_guid` and `batch_id`.
3. Review the native dialog's company, endpoint, date, numbering, reference,
   narration, every debit/credit entry, and totals. Choose **Post Journal** on
   macOS or **Yes** on Windows to permit this attempt. **Cancel** or Escape
   declines on macOS; Return may leave the dialog open. Windows defaults to
   **No**. Long or directionally ambiguous previews are refused; use the
   manual file workflow instead. A desktop session is required.
4. Bridge refreshes company identity, product/mode, date admission and exact
   masters, checks the batch is absent, then records a durable dispatch intent
   before one POST through the existing serial Tally queue. It saves response
   commitments/counters and performs mandatory accounting readback. Only a
   clean create response together with matching readback confirms the first
   posting as `posted_verified`.

Cancel, client disconnect, or the two-minute approval timeout ends the pending
approval. If dispatch has already begun, cancellation cannot undo Tally's
work. A timeout, crash, malformed response or incomplete readback requires
`verify_import` on the **same original batch**. Once dispatch intent exists,
`post_import` only reconciles and never resends, including after process restart.
If another process holds import admission, the call returns `import_admission_busy`
without waiting for that process or scheduling a later post. Reconcile any
recorded attempt before requesting another action. Confirmation requires all seven
result counters to be explicitly observed: `CREATED`, `ALTERED`, `DELETED`,
`IGNORED`, `ERRORS`, `CANCELLED`, and `EXCEPTIONS`. Missing counters remain
incomplete evidence even when voucher readback matches. Older saved responses
without these presence records remain readable but cannot establish a clean
response. Keep the original history for investigation; never guess missing
presence records or resend a Journal to obtain a new receipt.
An intent may exist even if the request never reached Tally: this is deliberately
an unknown outcome, not permission to build a replacement voucher. The saved
response metadata helps distinguish clean counters from readback alone.

Posting binds the saved batch to its loopback endpoint and full company tuple.
Legacy batches without that endpoint binding remain readable/verifiable but
cannot be posted. Only a uniquely selectable loaded company is admitted.
Existing batch files and proofs retain their formats with additive optional
journal metadata; no database migration or background queue is introduced.
Disabling the switch and restarting the connector removes posting from tool
availability without deleting reconciliation evidence. Retain this connector
version for recovery: older binaries may refuse the new dispatch journal records.

This is a bounded first posting slice, not blanket host/licence qualification.
A ledger mapper is unnecessary for exact existing names: `validate_masters`
returns exact matches and bounded near matches. Resolve ambiguity with the
user rather than silently creating or choosing a ledger.

## Review and post from the Bridge app

1. Build one Journal with `build_import_xml` as above. Keep its original XML
   file and local batch history on the same computer. The desktop app and MCP
   use the same default data directory; a custom `BRIDGE_AGENT_DATA_DIR` must
   be the same for both processes.
2. In Bridge, set the same Tally host and HTTP port used to build the file.
   Open **Review Journal file** from the overview and choose the original XML.
   Selection is local: it does not contact Tally. Bridge accepts only bytes
   matching a single saved, admitted batch and its original private file.
3. Review the company, date, reference, narration, ledger entries and totals.
   Choose **Post Journal**, then review and approve the independent native
   dialog. The app uses the same validation, dispatch and readback service as
   MCP. Changing app connection settings cannot redirect an open review.
4. If an attempt is already recorded or its outcome is uncertain, use
   **Reconcile original batch**. This action only reads and cannot open an
   approval dialog or send an import. Confirmation needs both the original
   clean response and matching readback. Keep the original batch when recovery
   is inconclusive; do not rebuild it as a retry.

An XML file alone is not portable posting authorization. Files generated
elsewhere, edited files, legacy unbound batches and unsupported voucher types
are refused. This first desktop flow has no Journal editor or arbitrary XML
importer. It adds no persisted format beyond the shared posting service.

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
Status commits its status and company-discovery responses. Scoped agent reads use
a single attempt. Company discovery can retry transient failures; its commitments
describe the terminal attempt, excluding earlier attempts. Read failures retain
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

`tally_status.education_mode` is a boolean: `true` for observed Education mode,
`false` for observed Licensed mode, and `null` when mode is unobserved. Product
identity uses the observed gateway capability; an optional status-page banner
cannot override it. Clients of the earlier string-valued field must update.

Company selectors accept native hyphenated UUID spelling, case-insensitively.
Malformed selectors refuse before network reads. A malformed observed GUID cannot
construct a verified identity; discovery reports `identity_state: "invalid_guid"`
instead of `verified_tuple`.
Nonempty `BOOKSFROM` must also parse as a valid Tally date before scoped access;
discovery reports `invalid_books_from` for a malformed value.
Observed company numbers must contain 1–16 ASCII digits, using the same rule as
desktop selection. Discovery labels malformed values `invalid_company_number`;
scoped access refuses them before any company report is read.

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

Voucher `offset`, `limit`, ledger and voucher-type selectors apply after the
complete source window is read and validated. They do not page Tally's work.
The fixed profile fetches named voucher fields and three ledger-entry fields;
it does not expand `ALLLEDGERENTRIES.*`. Each source response is subject to the
transport's 32 MiB limit and 20-second deadline. A failed source read releases
no complete page or movement total. These client limits do not bound Tally's
server-side generation cost. Dense-window throughput and automatic source
partitioning are unqualified; start with a narrow date window and do not treat
a small output limit as a source-volume safeguard. Even a single day can be too
large. The connector does not automatically retry or subdivide such a failure.

Byte-limited pages retain forward progress or return `agent_response_too_large`;
they never advertise the same offset after removing every row. Outstandings
trims both collections to a shared page width because they share an input offset.
Active vouchers without observed accounting entries are refused before movement
filtering; cancelled and optional vouchers remain excluded from movement totals.
Movement corroborates the complete opening-ledger snapshot after voucher reads
and rejects unknown entry names before selecting a ledger. Caller-specified
opening dates require a freshly observed supported product/mode and a valid native
boundary before and after the read; a prior status call or cached profile does not
grant admission.

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

New files use `identity_scheme: "batch_v1"`. Each voucher's wire `REMOTEID` and
narration marker share a UUID derived from the generated batch ID and caller's
`bridge_txn_id`. The caller ID remains the local transaction label; it is not
sent directly as Tally's upsert key. Reused labels in independent batches therefore
have different wire identities. Retry the saved file: rebuilding after losing the
batch journal creates a new identity and does not deduplicate the business event.
Historical records without an identity scheme retain their original raw-label
interpretation. Unknown schemes are refused. Narration markers support readback
attribution; they are not authenticated provenance.

If file publication fails after its recovery marker is created, the partial
response retains `result.batch_id`. This does not mean a complete XML file or
import-ledger row exists. Preserve the recovery journal and any staged bytes;
reconcile them before continuing the blocked import workflow.

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
Each active voucher must also have nonempty entries whose signed amounts sum
exactly to zero before movement arithmetic or ledger selection. An unequal
projection returns `voucher_entries_unbalanced`, retaining source evidence.
This necessary check cannot detect an omitted subset that itself balances.

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

Before publishing a new import file, the builder reads the exact min/max-date
verification window with the same fixed projection used by `verify_import`.
Both native responses must agree, and all rows must pass company, accounting
and window admission, including the GUID/AlterID pair required by verification. A timeout, oversized response or invalid source prevents
file and batch publication while retaining completed source evidence.
`verification_preflight` records the observed rows, paired source bytes and
response commitment. It proves that the current window was readable; the new
import or later changes can still make subsequent verification exceed the
transport limits. It is not a future-capacity reservation.

Migration 26 adds a nullable, constrained capability-tier column without
backfilling observations or deleting historical rows. Existing immutable-snapshot
triggers remain active. Local migration tests preserve historical columns and
review hashes and exercise the previous named-column insertion shape. An older
executable has not been tested against the migrated database; older binaries
cannot reproduce new tier-bound review commitments and require a fresh review.
Keep the additive column on rollback; no downgrade SQL is required.

Older binaries cannot read the new compact status records. Preserve the data directory, import ledger, and proofs; use the
new binary for the import workflow or disable imports after a binary downgrade.
Do not truncate the ledger to force downgrade compatibility. A binary rollback
does not undo a separately imported Tally voucher. Existing files and transaction
IDs remain local recovery evidence. Downgrading to a binary predating the
financial-read profile gate restores broader monetary admission and is not a
recommended way to access an unqualified endpoint.

Journal admission streams every record and checks even unrelated compact-record
hash bindings. Builds retain no historical voucher payloads; verification retains
only its requested batch. Each record is limited to 32 MiB. Memory still grows
with distinct batch IDs and hashes, and scan time grows with total journal bytes;
there is no automatic truncation. Every nonempty record must end with a newline;
a complete JSON object at an unterminated tail is refused before a later append
can concatenate objects. Preserve the file for explicit recovery instead of
truncating history or silently repairing it. Reserved narration markers begin exactly with
`[BRIDGE:`. Unrelated text such as `[BRIDGE CLUB]` is ordinary narration, while
malformed or multiple reserved markers still refuse verification.
