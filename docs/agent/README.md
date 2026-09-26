# Bridge MCP

For ordinary Claude Desktop installation, start with the install page when it
is deployed, or use the [fallback installation guide](./INSTALL.md).
The developer configuration below remains for supported client integrations.

`bridge_mcp` is Bridge's newline-delimited JSON-RPC 2.0 MCP server. It uses
Bridge's loopback-only Tally XML transport. Reads are enabled by default.
The MCPB extension also exposes voucher file preparation and bank-statement
parsing by default. Voucher posting (one Journal, Payment, Receipt or Contra) is
off by default because of the two limits under *Approved voucher posting* below;
the **Allow voucher posting (Journal, Payment, Receipt, Contra)** setting adds it, with
separate native approval for each new attempt. Command-line installations
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

Before requesting financial data through an MCP client, the client may send the selected
Tally result to its AI provider, including company
identity, party or open-bill details, and amounts. An unset
`BRIDGE_AGENT_REDACTION` defaults to `none`; `mask_parties` masks party names
and `drop_narration` drops narration. Neither setting removes amounts. Set the
environment variable before launch when that better fits the workflow.

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

The ordinary default tools are `tally_status`, `list_companies`,
`voucher_schema`, `validate_masters`, `verify_import`, `outstandings`,
`ledger_masters`, `ledger_movement`, `trial_balance`, `vouchers`,
`voucher_presence`, `read_evidence`, and `egress_log`. For a command-line
installation, `BRIDGE_AGENT_ENABLE_IMPORT=true` also exposes
`build_import_xml` and `parse_bank_statement`, which prepares local
bank-statement voucher proposals. `BRIDGE_AGENT_ENABLE_WRITES=true` enables
that import workflow and exposes `post_import` and `acknowledge_post_review`. The MCPB extension always
sets `BRIDGE_AGENT_ENABLE_IMPORT=true` and maps its **Allow voucher posting
(Journal, Payment, Receipt, Contra)** setting, off by default, to `BRIDGE_AGENT_ENABLE_WRITES`.
This is a source-configuration inventory, not a claim that an installed client
uses a particular setting or that a tool is qualified for every runtime. Each
call returns compact JSON with the
company identity where scoped, a read timestamp, request/response commitments,
byte count, completeness reason, and truncation state. A refused call returns
`result.error` with `code`, which names what failed, and `message`. Where a runtime
refusal has a typed, data-free reason, the error also carries `cause`, which names why
(for example `company_base_currency_undetermined` beside `party_ledger_master_read_failed`).
A read whose two paired halves differ, because the book changed while Bridge was reading it,
carries `native_report_pair_changed`. A voucher-window part that is not admitted
(`voucher_window_part_not_admitted`) names why, and a census disagreement also carries
`counts`, the rows the part `returned` against the rows the census `counted`.
A compliance ledger read (`ledger_masters fields=compliance`) that its company's master-alteration
mark cannot bound within Bridge's response budget is refused before any ledger request (#637). The
refusal has cause `ledger_masters_too_large` and a `size` object: `master_alter_id`,
`estimated_bytes` and `budget_bytes`. The mark is an upper bound on ledgers, since every master
raises it, so a company with fewer ledgers may be refused. `fields=basic` still reads it, and a
precise count is pending (#668).
When Bridge got no response it could read, the `cause` names why and the error also carries
`endpoint`, the configured origin that was tried (#629). The causes are:
- `endpoint_invalid`: the configured endpoint failed validation. The `endpoint` field then appears only
  if a valid origin can still be formed from the configuration.
- `endpoint_unreachable`: the connection was not accepted.
- `request_failed` or `request_deadline_exceeded`: the request failed before any response, or its
  deadline passed.
- `http_status_failure`, `response_content_type_unsupported` or
  `response_content_encoding_unsupported`: the responder was rejected on its HTTP status or headers
  before any body was read.
- An `endpoint_…` runtime code: Bridge held the request back and sent nothing.

A wrong or reset port therefore reads as an endpoint problem, not as a Tally data problem. A
failed read without `endpoint` either received a response whose body then failed to read, decode,
parse or pass Bridge's checks, or hit a local limit or fault that does not involve the endpoint. A
withdrawn call is `request_cancelled`.

Like `remediation`, `cause`, `counts`, `size` and `endpoint` are omitted when `BRIDGE_AGENT_MAX_BYTES` is below
4,096, so that the code always fits. Before a tool response is written, Bridge appends a metadata-only
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
While a tool other than `post_import` runs, a `notifications/cancelled` naming it
stops the call before its next queued operation. An operation already started runs
to completion, including every request it makes (a paired read, its brackets and any
retries), because abandoning a request would not stop Tally; so a cancellation can
still be followed by the rest of that operation's requests. The call is answered with
`request_cancelled` and partial evidence, never with part of a read. Closing the
input is not a cancellation: the call in flight still completes. Requests other than
`ping` sent while a call runs are served after it, in order; a ping is answered at
once. Once eight requests are waiting, further input (including a ping or a
cancellation of the call) stays unread until the call ends. The lab write tools are
not cancellable.
`egress_log` reads only the final 256 KiB, in 64 KiB reverse-seek chunks, so a
larger receipt file still yields its bounded tail without loading the head.
`changed_since` is unavailable: it is omitted from tool discovery and direct calls
are refused as `changed_since_unqualified` before contacting Tally. Bounded change
enumeration and snapshot continuation have not been qualified. There is no operator
setting to enable this tool.

### Native Trial Balance

Use `trial_balance` with `company_guid`, `from` and `to` (YYYYMMDD or
YYYY-MM-DD) for native ledger totals without a voucher scan. For example, ask
for the selected company's Trial Balance from 1 April to 31 March. The runtime
requires freshly observed Licensed TallyPrime for this four-column report.
Education mode is refused before report dispatch until this complete request
has mode-specific live qualification. Dates before book start are refused.
The monetary scope also requires one observed INR currency master.

Each opening, debit, credit and closing value is either
`{"state":"present","value":"-7000.00"}` or `{"state":"present_empty"}`.
Negative values are debits; positive values are credits. No empty value is
converted to zero. `totals` sums numeric observations across all returned ledgers
and carries an `empty_count` for each column. A fully observed nonzero opening
net is the difference in opening balances; Bridge does not add a balancing row.

`offset` and `limit` restrict output, not the source read. Each call captures a
fresh report; compare source evidence before combining separate pages. Native
Trial Balance uses Tally's TBAL fields, including the Profit & Loss treatment;
see [protocol semantics](../tally/TALLY_PROTOCOL_REFERENCE.md#56-native-trial-balance-fields).
It may include dormant ledgers hidden by Tally's screen. Paired response,
company, mode and extent checks detect observed changes, but do not prove an
atomic snapshot or voucher-level reconciliation. Keep the company quiet during
reads. Use `ledger_movement` with narrow dates when voucher detail is needed.

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
Every voucher, movement, presence and verification read observes the mode from the
`CompanyListV2` response of its own identity bracket, at no extra request. In Education
mode a read whose `SVFROMDATE` or `SVTODATE` is not on day 1, 2 or 31 is refused as
`window_part_boundary_unsupported_in_education` before it is sent, because Education
answers a read starting on another day with an empty collection rather than an error. A
divided window is checked whole before its first part, and a read whose closing bracket
reports Education is refused the same way, since either mode may have served it. Any
`EDUMODE` value other than `No` counts as Education even when the other capability fields
do not parse; a company list with no `EDUMODE` field keeps ordinary boundaries, and
`EDUMODE = Yes` itself has not yet been captured from a live Education instance. The end
side is held to the same rule without a live measurement in these shapes, so an Education
whole-month read ending on the 30th is refused.
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
tool, and `build_import_xml` by default because it always sets
`BRIDGE_AGENT_ENABLE_IMPORT`. A command-line installation keeps
`build_import_xml` behind `BRIDGE_AGENT_ENABLE_IMPORT=1`, or enables it with
Journal posting as described below. New file generation accepts `Journal`, `Payment`, `Receipt` and `Contra`, each
with freshly observed supported TallyPrime product and licence mode before and
after the build reads. Release and licence tier are returned as observed facts;
they do not independently refuse a file. `tally_status` reports the observed
release and licence tier; the optional status-page banner cannot supply these
facts. Every other voucher type is refused.

The four rest on different observations, and each build reports its own in
`live_evidence` rather than a single blanket claim:

- `Journal` — a licensed synthetic-lab file cycle and exact-file repeat import
  observed 2026-09-06; see [the assessment](ASSESSMENT-2026-09-06.md). The controlled
  repeat does not qualify recovery after an unknown outcome.
- `Payment`, `Receipt`, `Contra` — a licensed TallyPrime 7.1 Gold bank-statement
  import observed 2026-09-10; see
  [reference §9.13](../tally/TALLY_PROTOCOL_REFERENCE.md). These three are
  admitted with two or more entries (at least one debit and one credit, no
  ledger on both sides; more than two is bridge#466 and rests on narrower
  evidence: hand-built files of that shape were imported and read back
  over the gateway ([reference §9.3](../tally/TALLY_PROTOCOL_REFERENCE_WRITE_RESPONSES_AND_MASTERS.md);
  a Contra only with a repeated ledger), and one Bridge-built three-entry Receipt was imported
  over the gateway and verified, but no multi-entry Payment or Contra has been, and none of the three, including that Receipt, through Tally's Import menu,
  so such a voucher reports `live_evidence` as `hand_built_gateway_readback` and
  its build result warns so) with no voucher number and no reference, and every leg on
  their money side must be a ledger whose live group
  ancestry reaches a reserved `Bank Accounts`, `Cash-in-Hand` or `Bank OD A/c`
  identity, while
  their counterparty side must be established as holding no money — money on
  both sides is a `Contra`, and an unresolvable group is refused too. A money group is admitted only where a captured ledger sits under
  it, so a ledger under `Bank OCC A/c`, which no capture carries, is
  refused on either side until one is captured. Bill-wise allocation is not supported: every party amount lands On
  Account, and a build that names a counterparty warns so.

Historical batch records remain readable. None of this qualifies every host,
licence mode, or manually imported file, and only an unnumbered single-voucher
`Journal` batch is eligible for native posting.

1. Call `voucher_schema` and produce a payload matching its schema. Transaction
   IDs are client-supplied, unique within the batch, and retained in the local import ledger.
2. Call `validate_masters` with every ledger name. **`build_import_xml` admits
   `exact` only**, so replace the payload name for every row that is not
   `exact`, and never invent one:
   - `identifier` — the row is bound by a decisive identifier. Copy its
     `exact_live_spelling` into the payload verbatim; the import file carries
     whatever you send byte for byte. Folded names do not bind through this
     generic catalogue, even when exactly one candidate is found. Historical
     `normalized` records remain readable, but current validation does not
     produce them; revalidate against the current catalogue before selection.
   - `near_miss` — the row is **not** bound and Bridge chose nothing. Where
     `listing` is `withheld`, `candidates` is empty: there is no listed name to
     pick. This includes `master_binding_no_discriminating_candidate` and
     `master_binding_identifier_conflict`. Obtain a more complete source name
     for an indistinguishable name family; conflicting identifiers require
     correction of the source identity or explicit operator selection against
     the observed ledger list. Identifier-conflict recovery is independent of
     `listing`: a `truncated` result may show an outside name candidate while
     omitting the whole large identifier family, so choosing only among listed
     candidates is insufficient. Inspect the complete observed catalogue and
     correct or explicitly confirm the intended source identity; a fuller name
     alone does not settle conflicting identifiers. Where candidates are listed,
     each carries its comparison rule; even a single candidate still requires a
     decision. For every reason,
     render `candidate_count_is_lower_bound` as "at least N", never an exact total.
   - `missing` — no live ledger matched. Bridge never creates masters.
3. Call `build_import_xml` with the payload. It checks exact decimal balance,
   company date extent, live masters, and local journal integrity,
   repeats the full catalogue to reject intervening changes, then writes `<data_dir>/imports/<batch_id>.xml` and records an append-only
   `agent-import-ledger.jsonl` line. On a `Journal`, `voucher_number` and
   `reference` are optional: when a number is absent Tally applies the voucher
   type's own numbering configuration, and when supplied it is validated and
   sent so a Manual-type duplicate policy can reject it. On `Payment`,
   `Receipt` and `Contra` **both fields are refused** — neither element's fate
   has been observed on those types, and the bank's own reference belongs in
   the narration, which survives. A payload carrying one is rejected before any
   live read.
4. In Tally, with the intended company open, use **Gateway of Tally → Import →
   Vouchers** to import the file. Bridge does not dispatch this manual step.
   Alternatively, use the separately approved MCP voucher posting (or, for a
   Journal, the desktop posting) flow below instead of importing the file manually.
5. Call `verify_import` with the company GUID and batch ID. It reads the date
   window back, compares the exact signed ledger entries, reports missing or
   divergent rows and duplicates, writes `.proof.json` and `.proof.md`, and
   appends the verification status to the local import ledger. It compares the
   date, voucher type and entries; it does **not** compare `EFFECTIVEDATE` or
   `PARTYLEDGERNAME`, which `Payment`, `Receipt` and `Contra` files carry — see
   the limits noted in reference §9.13.

The file path is deliberately not a direct-posting path. Masters must already
exist and match exactly. File generation requires fresh supported product/mode
observations; the checks do not make a later manual import atomic with the earlier reads.
In Education mode, voucher dates must be on day 1, 2, or 31 — for every
voucher type, not only `Journal`. A different requested date is refused as
`education_voucher_date_unsupported`; Bridge does not move it.
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

## Approved voucher posting

**Voucher posting is off by default in the MCPB extension** while two known
limits remain. The post names its company only by name, and Tally cannot bind an import to a company's GUID. Bridge confirms the company as its last request before the post, and afterwards reports which companies changed, but another loaded company renamed to, or loaded under, the exact same name in that moment would still receive the voucher
([#574](https://github.com/lamemustafa/bridge/issues/574)). And Bridge cannot
delete or roll back a voucher it has posted, so a wrong post must be corrected
by hand in Tally ([#579](https://github.com/lamemustafa/bridge/issues/579)).
The saved batch file is now checked byte for byte against the approved record
before posting ([#575](https://github.com/lamemustafa/bridge/issues/575), fixed).
**Allow voucher posting (Journal, Payment, Receipt, Contra)** turns it on for
users who accept those risks. Existing
saved settings are respected, so an installation that saved the earlier default
may still have posting on; check the setting.
For command-line installation, set `BRIDGE_AGENT_ENABLE_WRITES=true`.
This enables `build_import_xml`, `parse_bank_statement`, `post_import` and
`acknowledge_post_review`, which asks the local user, in its own native dialog, to
record that they reviewed a post whose masters check found a changed ledger. That
record changes no verification status and nothing in Tally (#239).
`verify_import` remains available so an uncertain saved batch can be checked
after posting is turned off. `BRIDGE_AGENT_ENABLE_IMPORT=true` alone exposes
the manual file workflow and bank-statement proposal preparation, while
verification remains available without either switch.

`BRIDGE_AGENT_ENABLE_BATCH_POST=true`, together with
`BRIDGE_AGENT_ENABLE_WRITES=true`, lets `post_import` post a saved batch of 2 to
50 vouchers in one import, after one approval of the batch's summary: every
ledger's debit and credit totals, the money Receipts bring in and Payments take
out, and the standing cautions. It is off by default. It is a command-line
setting only, not in the MCPB extension, until a batch post through Bridge has
been proved on a live book. A batch is `posted_verified` only when Tally created
exactly that many vouchers, the readback verifies every one, and the company's
voucher mark moved by exactly that many. Otherwise it is
`reconciliation_required` (`batch_step_unconfirmed` when only the mark
was not confirmed). Review a doubted batch's vouchers in Tally and do not rebuild it;
`acknowledge_post_review` records that review, one doubt at a time, and changes
no verdict.

All three switches accept `true`/`false` or `1`/`0`; invalid values stop startup. No model-supplied argument can grant approval. Claude controls
its own tool-call permission prompts: Bridge cannot preselect **Always allow**
for the user. That client permission does not approve an accounting entry.

One native-approved Journal and restart reconciliation have been observed on
macOS against a synthetic Silver 7.1 instance. This remains a preview: Windows
interactive approval and native posting on Gold or Education have not been
established. What has been observed on licensed 7.1 Gold is `verify_import`
returning `posted_verified` for Bridge-built Payment, Receipt and Contra files
sent over the gateway by a script rather than by this tool. That was verified
on one book, and partial on a second where larger reads failed (bridge#485); see
[reference §9.13](../tally/TALLY_PROTOCOL_REFERENCE.md).
Native posts of a Payment, a Receipt, a Contra and a three-entry Receipt have
been observed live on a synthetic Silver 7.1 company, each reading back
`posted_verified` (ADR 0004, amended 2026-09-23).

1. Validate the exact existing ledger names and build **one Journal, Payment,
   Receipt or Contra** using the file workflow above. Sales, purchases, tax,
   inventory and master creation remain unavailable. For a Payment, Receipt or
   Contra, `post_import` classifies every leg again from the ledgers' current
   parents and the group tree, before approval and again after approval inside
   the endpoint queue (before the final duplicate check and the post), and refuses with `import_bank_classification_changed` if any
   leg changed; nothing is sent. Every post, of any type, is refused with
   `import_multi_currency_unsupported` if the company defines more than one
   currency: Bridge does not post into multi-currency books yet. This is checked
   before approval and again inside the queue. A Currency read that names no
   usable master refuses with `import_base_currency_undetermined`. Inside the
   queue, a change to the company's masters from just before the catalogue
   re-read to the last read before the post refuses with `post_masters_moved`
   (`post_masters_unconfirmed` if it cannot be checked); re-run the post. This
   sees only changes that move the company's master AlterID (`ALTMSTID`):
   measured for ledger renames and creates made through the gateway. A regroup,
   an edit made in Tally's own screens, and whether posting a voucher moves it
   are not yet measured. A queue catalogue re-read that does not parse as this
   company's catalogue refuses with `post_catalogue_unreadable`, whose `cause`
   names why, and nothing is sent; a repeated or unusable ledger name refuses
   again until it is corrected in Tally. Separately, the build records each ledger's GUID, and a
   post refuses any ledger now on another GUID (renamed and replaced, or deleted
   and recreated, since the build) with `import_masters_changed_since_build`,
   naming it. The name now means a different ledger: confirm the intended one
   (it may be under a new name) with `validate_masters` before building again.
   A batch built before this record existed is refused with
   `import_batch_predates_ledger_binding`, before any Tally request; build it
   again. Any other read inside the queue that fails before the post is refused
   with `post_queue_read_failed`, with a `cause` where one is known; nothing is sent, and
   the post can be re-run. Rebuild only when `attempt_recorded` is `false`.
2. Call `post_import` with the original `company_guid` and `batch_id`.
3. Review the native dialog's company, endpoint, date, numbering, reference,
   narration, every debit/credit entry, and totals; for a bank voucher, also the
   side that must be bank or cash. Choose **Post voucher** on
   macOS or **Yes** on Windows to permit this attempt. **Cancel** or Escape
   declines on macOS; Return may leave the dialog open. Windows defaults to
   **No**. Long or directionally ambiguous previews are refused; use the
   manual file workflow instead. A desktop session is required.
4. Bridge refreshes company identity, product/mode, date admission and exact
   masters, checks the batch is absent, then records a durable dispatch intent
   before one POST through the existing serial Tally queue. It saves response
   commitments/counters and performs mandatory accounting readback. Only a
   clean create response together with matching readback confirms the first
   posting as `posted_verified`. Unless the company's master AlterID is proven
   unmoved across the POST, Bridge re-reads the ledgers and reports it in
   `masters_after_post`. If an approved ledger no longer resolves to its
   approved GUID (`posted_under_changed_masters`), or that check cannot be
   completed (`masters_after_post_unconfirmed`), the voucher is in Tally but the
   result is `reconciliation_required`: review it in Tally, correct it there if
   needed, and do not rebuild the event. Bridge records the check with the
   batch. A changed ledger stays reported on every later `verify_import`, even
   after the voucher is corrected in Tally; a check that could not finish is
   finished by the next `verify_import` that finds the voucher. If Bridge cannot
   record the check, the post is refused before anything is sent
   (`post_masters_record_unavailable`).

Keep the selected company free of other imports and ledger changes while posting,
and leave Tally's product/licence mode unchanged. Bridge serializes its own writers;
its separate checks cannot lock out Tally UI edits or other importers. Concurrent
external changes are outside this preview's validated posting workflow.

Cancel, client disconnect, or the two-minute approval timeout ends the pending
approval. If dispatch has already begun, cancellation cannot undo Tally's
work. A timeout, crash, malformed response or incomplete readback requires
`verify_import` on the **same original batch**. Once dispatch intent exists,
`post_import` only reconciles and never resends, including after process restart.
Each post sends a fresh `REMOTEID`, and one that any recorded dispatch intent
already carries is refused as `import_remote_id_reused` before any Tally request
(protocol reference §9.3: a resend can undo a person's cancel or delete).
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
Existing batch files and proofs keep their formats; new journal fields are
optional on read, and no database migration or background queue is introduced.
Disabling the switch and restarting the connector removes posting from tool
availability without deleting reconciliation evidence.
**Keep this connector version for recovery.** The journal reader refuses any
record carrying a field it does not know. So after a native post, an older
connector refuses the whole journal, including reconciliation of batches it
wrote itself. Since bridge#579, each native dispatch intent records the
REMOTEID it sent, which 0.2.0 and earlier do not know.

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
   dialog (its button reads **Post voucher**). The app uses the same validation, dispatch and readback service as
   MCP. Changing app connection settings cannot redirect an open review.
4. If an attempt is already recorded or its outcome is uncertain, use
   **Reconcile original batch**. This action only reads and cannot open an
   approval dialog or send an import. Confirmation needs both the original
   clean response and matching readback. Keep the original batch when recovery
   is inconclusive; do not rebuild it as a retry.

Bridge prevents Journal posting during a Core Accounting snapshot, including
snapshots in another updated Bridge process using the same operating-system
account and Tally port. Wait for the snapshot to finish or cancel it before
posting. If a snapshot start or resume result is uncertain, use local evidence
to restore monitoring or cancel the active run; connection settings stay locked
until that uncertainty is resolved.

A snapshot does not itself mean a Journal was attempted. Bridge can still
check the saved local history during a snapshot; an active posting process
keeps that result uncertain until its attempt can be observed safely.

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
the server. Each `ledger_masters` row's `opening_balance` is the opening at the
start of the company's books, and `opening_balance_as_of` names that date (the
admitted `BOOKSFROM` the request pins). On a book holding several years it is
not the current year's opening: for a period's opening, use `trial_balance` or
`ledger_movement` with that period's `from`.

The unavailable `changed_since` implementation must not be used as
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
name; `candidate_count`, `candidate_count_is_lower_bound` and
`candidates_truncated` preserve ambiguity and count precision. A true lower-bound
flag means "at least N" even when no candidates are listed. Import
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
have different wire identities, so rebuilding after losing the batch journal
creates a new identity and does not deduplicate the business event.

**An unknown outcome requires read-only reconciliation for every voucher type.**
Preserve the original batch and saved file, then call `verify_import`. Do not
re-import or rebuild the same business event, including a `Journal`. The
controlled repeat observation returned `CREATED=0, ALTERED=1` and left one
voucher; it did not qualify a resend after a lost response, restart or an
intervening change. Each measured `Payment`, `Receipt` and `Contra` bank file
was imported once. Neither observation authorizes another write to discover
what happened to the first one.
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
