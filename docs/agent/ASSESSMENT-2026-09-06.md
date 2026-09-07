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
The PR adds `unicode-normalization` as a direct production dependency for NFC
ledger-name near-miss suggestions. Its version and dependencies already existed
in the baseline lockfile; the generated license inventory includes their use.
Exact live spelling remains required for import admission. File locking uses
the Rust standard library; no separate locking crate remains. [Official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)

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
| Ledger movement | Use Tally's observed period opening at the requested start. Apply only in-window, non-cancelled, non-optional voucher entries. Eliminate the earlier-history scan. Corroborate opening snapshots and require freshly observed licence mode for both book-start and caller-specified opening dates. |
| Change enumeration | Hide and refuse `changed_since`. A client result cap is not a server-work bound, and unqualified snapshot continuation is not a reliable change feed. No bypass setting is added. |
| Import verification | Reserve explicit markers before fallback matching, consume each observed row once, and distinguish attributed postings from matching content. Equivalent decimal spellings compare equally without changing stored XML bytes. |
| Response recovery | Preserve a persisted batch ID through result caps, final framing caps, and receipt failures. The client can recover without generating another transaction. |
| Source commitments | Hash the UTF-16LE request entities sent by transport; retain status, selector-catalogue, and currency-probe evidence. Count both accepted bodies of paired source reads. |
| Receipts and proofs | Derive released field paths from final redacted output. Roll back handled proof-publication failures and retain a recovery journal after interruption or uncertain rollback. Use portable receipt locking and decode only complete UTF-8 tail lines. |
| Financial summaries | Label gross exposure explicitly and keep billed/unallocated receivable and payable directions separate. Preserve future-due or unknown ages in `unaged`. |
| MCPB packaging | Validate the actual per-platform manifest, packaged executable, legal resources, and extracted archive launch. |
| Admission and pagination | Bound master-name inputs and suggestions before report expansion. Refuse active entryless movement rows. Keep shared-offset pages advancing or return an explicit size error. Search page widths with logarithmically bounded serialization probes. |

Receipt writes use a readable and writable handle, seek to the end under an
exclusive lock, and restore the original length if appending or syncing fails.
The access rights support both Windows locking and failed-write rollback. Shared
readers and exclusive writers use the standard-library file-lock API, available
within the pinned Rust 1.96 toolchain.
[Rust file locking](https://doc.rust-lang.org/1.96.0/std/fs/struct.File.html#method.lock)
[Microsoft file-lock contract](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)

MCPB staging follows the actual manifest 0.1 schema, including executable command,
entry point, and environment mappings; official CLI validation is part of bundle
CI. A ZIP file with a manifest-shaped object is insufficient evidence.
[Official schema](https://github.com/anthropics/mcpb/blob/70fe3b34cd6dff1b3bba046638edc72a6467a4fb/src/schemas/0.1.ts)

### Final review corrections

Voucher amounts, polarity flags, and calendar dates must parse before either
ordinary or accounting reads release them. Ledger selectors bracket voucher reads
with matching catalogues and check observed entry membership, refusing ambiguous
or changing names. Empty-window corroboration applies to the unfiltered source; a selector
with no matches does not erase evidence that source rows were observed. Movement
metadata preserves cancelled and optional source-row counts while excluding their
entries from balances. Response-cap refusals retain partial source commitments
in the in-process evidence store. Import verification is independent of the MCP
output-row limit, and repeated verification appends compact hash-bound status
records instead of duplicating the batch payload. Typed failures retain completed
source observations through parsing, window validation, and downstream read errors.
The egress tail reports truncation for both row and scan-byte omissions, and
refuses incomplete or malformed retained JSONL records. Ordinary voucher rows
expose typed cancellation and optional state. Import readback applies the same
accounting-domain validation before any verdict. Fully attributed expected
multiplicity is allowed while one observed identity can satisfy only one
transaction and unexpected duplicates remain blocking.

The voucher and import paths now share one XML parser. It admits successful
export status, complete scalar text (including entities and CDATA), singleton
fields, exact sign/polarity agreement, and valid numeric identifiers before
selectors or verification can use a row. Import source admission then rejects
duplicate GUIDs or numeric master IDs and multiple or malformed reserved
transaction markers across the whole collection, before attribution narrows it.
The same reserved transaction ID on two distinct vouchers is rejected even when
one voucher predates the batch high-water mark; filtering cannot hide a collision.
Company checkpoints preserve fragmented text and require one matching identity.
Voucher and ledger collections reject duplicate GUIDs or numeric master IDs
before returning complete reads or calculating movement. Wrong object types in
a voucher collection are refused instead of becoming an empty observation.
Evidence and egress history reads obey the configured global row cap as well as
the requested limit and retention ceiling. Omitted history is marked truncated.
All JSON-RPC responses, including control messages, obey the final newline-inclusive
byte cap. Oversized controls return a bounded refusal without invalidating the
session. Status product identity comes from an observed gateway capability; an
unrecognized, conflicting, or unavailable status-page banner cannot override it.
An unobserved gateway capability returns `not_observed`.
The education-mode flag is boolean for observed modes and null otherwise. Caller
company UUIDs are admitted before any read, and malformed observed GUIDs cannot
produce verified identities or complete unscoped receipts.

Both book-start and explicit-date ledger openings now obtain a fresh mode probe
before date admission and bracket the read with a closing mode observation. A
stale cached mode cannot admit an unsupported Education-mode date. Captured-source
regressions cover absent/stale cache, unsafe book starts, and closing mode drift.

The same fresh-mode requirement covers compliance ledgers and native
outstandings, whose balance fields also lack an independently returned period.
The corrected boundary is the runtime source, rather than a requirement that a
client call status first. Mode/date refusals retain completed wire observations
through the runtime and adapter error mappings. MCP outstandings also binds the
INR observation to its company and master extent and requires that witness to
match the opening financial extent. Closing extent drift remains partial. The
desktop operator-assertion contract remains separate.

The final date-admission audit traced every MCP route:

| Read family | Admission evidence |
| --- | --- |
| Basic/compliance ledger balances, movement openings, native outstandings | Fresh recognized mode, typed permitted period, closing mode observation. |
| Voucher reads and import readback | Literal date predicates and returned-row window validation; corroborated empty reads, with fresh Silver 7.1 profile qualification before import absence is persisted. |
| Catalogue, currency, company identity, import high-water | Metadata only; no period-dependent balance is released. |
| Legacy calibrated scan and change enumeration | Unavailable through the MCP evidence path. |

Import verification fingerprints each expected and observed voucher once and
indexes marker/content candidates before matching. A captured-derived local
1,000-expected/10,000-observed regression retained the same verdicts while reducing
matching from 19.4 seconds to 0.48 seconds without markers, and from 30.8 seconds
to 0.39 seconds with markers. These are local comparison measurements, not Tally
response-time claims. New voucher-file generation and its schema admit only
Journal, the type established by live import/readback. Payment, Receipt, and
Contra fail before network or file effects; historical records remain readable.
Fresh observed TallyPrime Silver 7.1 is required before and after the build
reads, before any file or batch record is published. The fixed native company
collection observes the release label and exclusive licence-tier flags; two
identical 16-company captures establish 7.1/Silver (§3.1 of the protocol reference).
A missing, conflicting or different release, Gold or ambiguous tier, Education,
ERP9 and Edit Log have no new-file qualification. Persisted `not_found` import
verdicts require the same opening and closing profile; positive historical rows
remain directly observable. This does not qualify future changes to Tally. Unknown argument names produce a fixed
error code so large property names cannot expand responses or retained evidence.

Existing caller-selected data directories are admitted without changing their
permissions. Newly created Unix directories use private creation permissions;
shared Unix leaves, wrong owners, and symlink/reparse leaves are refused. Windows
directories retain their inherited ACLs. Import-file
builds repeat the ledger catalogue before publication and require its full source
commitment to match, covering changes to identities and parents as well as names.
This is repeated-observation stability, not an atomic guarantee at later import.
Duplicate detection hashes the structured accounting fingerprint, so ledger names
containing punctuation cannot collide through delimiter concatenation.

Concurrent verification uses a per-batch journal generation captured at admission.
Publication compares it under the existing exclusive lock before staging proofs;
a competing same-batch publication causes an explicit retry refusal. Identical
status appends still advance the generation, while unrelated batches remain
independent. No persisted schema change or network-held file lock is introduced.

Import absence has a separate qualification requirement. A result containing
`not_found` requires licensed TallyPrime observations before and after readback;
an unqualified observation withholds the negative verdict and preserves prior
proof/status. Positive historical postings remain directly observable. The
Education-mode variable-predicate failure does not describe the connector's
literal predicates; [protocol reference §5.3](../tally/TALLY_PROTOCOL_REFERENCE.md#53-rejected-period-boundaries-widen-silently--the-key-trap)
records the actual counter-observation and its limits. The negative-verdict guard
addresses the unqualified absence case, without claiming that Education mode was
observed to reject the connector's literal request.

Receipt records distinguish `response_prepared` from `stdio_write_completed`,
linked by receipt ID and frame hash. Preparation records use `*_prepared` fields;
completion records confirm successful stdout write and flush, not client
consumption. A missing completion leaves delivery unconfirmed. Failure to append
a completion stops further dispatch. The archive smoke and client checks verify
both records, and failed-writer regressions preserve persisted import recovery.

Movement also repeats its voucher source after the final opening-ledger read.
An unchanged opening cannot hide an in-window posting, edit, or deletion between
those reads. Captured-source replay covers stable reads and each drift case.
This establishes repeated-observation stability, not an atomic Tally snapshot.

### Source volume and current-window admission

Output pagination follows a complete, validated source read. It does not limit
Tally's export. The voucher and import profiles fetch named scalar fields and
three ledger-entry fields, whereas the large-volume measurements in protocol
reference §12a.8 used wildcard expansion. Their bytes-per-voucher and timing
constants cannot be applied to this projection. No dense-book throughput or
server-work bound is claimed. The existing 32 MiB/20-second transport limits
refuse unreadable sources, and completed observations survive that refusal;
they cannot cancel work already being performed inside Tally.

File generation now exercises its exact verification projection and date window
before publication. The source must be paired, company-bound and valid for the
window, with the same GUID/AlterID admission used by verification, and a closing qualified profile must still be observed. The result
records the current source row count, paired bytes and response commitment.
This prevents issuing a file whose verification window is already unreadable.
It does not reserve capacity for the import or concurrent future changes.

Adaptive partitioning was not added. Its completeness, mutation stability and
request budget would require a separately observed end-to-end slice. Applying
wildcard sizing constants or assuming AlterID/date locality would add unsupported
behavior to the connector. Dense-window support remains an explicit limitation.

Every voucher now carries a GUID bound to the selected company before filtering,
movement or import attribution. Missing GUIDs cannot fall back to unscoped master
IDs. Failed pairs retain completed source commitments, including drift and
closing identity failures. Narration redaction preserves only the known
`voucher_schema` metadata subtree; accounting narration remains redacted and
text content matches the structured result.

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

A later identity qualification reused the first file's caller transaction label
in a separate local batch, generating a new `batch_v1` wire UUID and narration
marker. The `12.61` Journal dated September 7 created exactly one new voucher
(master ID 5, assigned number 2, AlterID 9). Repeating the exact file altered that
same voucher to AlterID 10 with zero creations, errors, exceptions or deletions.
Immediate before/after verification retained the old voucher's GUID, master ID 4,
number 1 and AlterID 8 unchanged. The new file SHA-256 was
`e39eb3c0bfe53144bdd9c0f4afcb88c3d63a2050214233ee77465d42a54245ef`.
Its unchanged 7,990-byte readback and metadata are committed as a regression
fixture. The regression reproduces the exact generated file hash, verifies the
new posting, and loses attribution when only the expected batch ID changes.
This qualifies exact-file repeat and independent batch identity in the observed
Silver 7.1 Journal slice. Rebuilding after losing the local batch state creates a
new identity; it does not deduplicate the underlying business event. UUIDv8 is
used as the representation for a domain-separated hash of the random batch UUID
and caller label; UUIDv8 itself does not promise uniqueness.
[RFC 9562 §5.8](https://www.rfc-editor.org/rfc/rfc9562.html#section-5.8)

This qualifies the recorded synthetic slice only. It does not qualify every
voucher type, account nature, licence mode, release, customer dataset, or desktop
client. Direct Tally imports remain outside the connector. The compatibility
matrix remains `unknown`; these observations are not substituted for its signed
qualification process. Tally's official integration documentation describes the
XML interface, but returned bytes and readback establish the behavior above.
[Official XML integration documentation](https://help.tallysolutions.com/xml-integration/)

## Regression provenance

The protocol fixture tree contains unchanged UTF-16LE captures for an empty
native collection, a three-voucher response, the ledger catalogue, licensed
company discovery with and without the later release field, book extents, native period opening, compliance master/balance/group
sources, and the four native ageing sources. The
simulator adopts the catalogue's captured synthetic identities; it does not
rewrite the captured response. Metadata records the exact original wire hash.
Fixture-integrity policy remains enforced. Simulators test regressions after
live observation.

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

The final failure-path inventory covers completed status observations before
company discovery or fallback fails, and both differing bodies at every legacy
financial pair stage: four outstandings reports, three party-ledger reports,
currency and basic-ledger reads. Thirteen captured-source fault scenarios retain
exact commitments and preserve their existing typed error or partial verdict.
Two additional MCP status cases check the final response, text/structured parity
and retained `read_evidence` history. A failed child with no completed source
leaves earlier commitments unchanged; a real zero-byte response retains its
hashes. Company-number fault injection checks the
shared 1–16 ASCII-digit rule at discovery, verified identity and scoped tools;
malformed values release no company report. Existing source-parser name admission
remains in place.

Independent-build regression reproduces the obsolete cross-batch label refusal,
then verifies retained legacy records plus two successful new batches using the
same labels. Within-batch duplicates remain refused before dispatch. Publication
failure regression uses the recorded namespace XML commitment, partial staging
and a real rename obstruction: the recovery marker and batch ID survive, including
the smallest framing budget. The partial response does not promise a complete XML
file or import-ledger row.

Local artifact admission is shared across journals, locks, staging and delivery
receipts. It validates the opened regular file before reading or changing it;
transaction directories are private at creation. Temporary-file regressions cover
refusal without changing original bytes or permissions, alongside normal append,
locking and recovery. The same helper cross-compiled with warnings denied for
Windows using existing dependencies; hosted Windows runtime evidence remains
required. This does not claim protection against same-user ancestor-directory
replacement.

The readable voucher-schema module publishes text, positive amount and ledger
constraints alongside runtime admission. Optional text is limited to 2,000 Unicode
characters, matching JSON Schema, with the separate 5 MB request cap unchanged.
An independent Ajv 8.20.0 validator and official MCP SDK client checked 471 cases;
448 invalid payloads returned typed refusals with zero Tally source bytes. Cases
include multibyte limits, all controls at the start/middle/end, terminal line
separators, zero and oversized amounts, and blank ledger names. Both schema
advertisements match and narration redaction preserves the complete schema.
Calendar/company/master/balance and decoded-marker checks remain runtime domain
admission. [JSON Schema string constraints](https://github.com/json-schema-org/json-schema-spec/blob/main/specs/jsonschema-validation.md#maxlength)

Movement now rejects each active voucher whose signed entries do not sum
exactly to zero, before ledger selection or arithmetic. A captured-response
regression omits one balancing side from both paired reads and checks the
`voucher_entries_unbalanced` refusal with retained source evidence. Cancelling
imbalances across separate vouchers do not pass. This is a necessary integrity
check, not proof that no balanced subset was omitted. Tally itself rejects
unequal voucher debit/credit totals. [Official XML examples](https://help.tallysolutions.com/sample-xml/)

Storage paths are also
admitted as Unicode before state creation, preventing JSON path serialization
from failing after publication. Startup regressions cover native invalid encoding
and exact preservation of multilingual paths; Windows runtime checks are required.

Local candidate verification: **1,011 Rust workspace tests**, **242 agent tests
within that workspace**, **48 tools-workspace tests**, **107 Node tests**, **6
Vitest tests**, and **2 Playwright tests** passed. Both Rust workspace Clippy
runs passed with warnings denied. Frontend build, formatting, licensing,
fixture-integrity, workflow, read-boundary, request-hazard, compatibility gate,
and matrix-Markdown checks passed. Compatibility gate: 11 unknown claims,
zero evidenced claims. These counts describe the settled local source; fresh
hosted checks are still required for its published commit.

The final macOS arm64 release binary passed twenty-seven live checks with party masking and separate narration-redaction checks:
twenty-six complete responses and one expected `empty_uncorroborated` refusal for a
window with no nearby voucher evidence. A fresh process then completed movement
from August 3 through September 1 without any prior status call, and a second
window correctly carried the test Journal's Cash opening. A separate fresh process
read all nine basic ledgers without a preceding status call. Separate fresh
processes also completed compliance ledgers and native outstandings. Status reported the observed release `7.1` and licence tier `silver`. Master
validation, filtered and unfiltered vouchers, compliance masters, outstandings,
restored-batch verification, and egress-log readback also completed.
Every emitted frame matched its linked preparation/completion receipts and
text/structured representations.
Status, filtered-voucher, compliance-master, basic-ledger, native-outstandings,
licensed Journal-file build, and qualified import-readback commitments and source
byte counts
were independently recomputed from the captured transport request/response files.
The voucher-type filter and a nonmatching ledger selection both completed.
Ordinary voucher rows exposed observed boolean cancellation and optional flags.
Movement completed with repeated voucher-source corroboration after the final
opening read.
Two verifications with a one-row output cap retained an attributed complete
result and appended 408 bytes total to the local ledger. A previously generated
staged file still correctly reported as not imported. A fresh process built one
additional Journal file for `12.63` using the same caller label in the same intact
local journal after catalogue, verification-window and Silver 7.1 profile
observations. This final file contains 2,000-character narration and reference
values whose Unicode text is preserved exactly in the generated XML; it was not
imported, so this checks file generation, not Tally storage limits for long text. Its batch ID and XML hash differ from the earlier file, whose
bytes remain unchanged; both files are retained. Its preflight observed the existing Journal;
readback correctly reported the new file as `not_found`. The file was not imported.
A separate narration-redaction process preserved the schema's narration property
while removing accounting narration from the returned Journal. The earlier
controlled exact-file repeat, bracketed by observed Silver 7.1 status, returned
zero created and one altered; its GUID, master ID, assigned number and exact signed
amount remained stable while AlterID advanced from 7 to 8. The final read pass made
no operator import and created no Tally voucher; it also verified the new namespaced
Journal at master ID 5 and AlterID 10. The expected read refusal retained its actual completed source
commitments and byte count, retrievable through `read_evidence`. A captured-source regression also proves that
a final JSON-RPC frame refusal records partial evidence with the cap reason while
retaining the original source commitments. Nonpageable company/master/history
responses refuse at all cap layers without inventing continuation offsets; omitted egress
rows set `truncated`. A one-record `read_evidence` call returned one observation
and explicitly reported the omitted history as truncated. The recording proxy passed an explicit readiness check
before these calls. An earlier candidate run had a proxy startup failure; those
failed observations and its successful repeat remain retained separately.
Separate release-process checks confirmed port zero and a non-loopback host
fail at startup before creating the data directory. An existing shared directory
was refused without changing its mode or creating files. At the minimum 256-byte
cap, control refusals and malformed-request errors fit and a subsequent ping
succeeded. Both diagnostic tools returned one truncated row under a global
one-row cap despite a larger requested limit.
A two-process release test replayed retained transport responses and delayed the
first verifier at its final corroboration response. The old executable overwrote
the newer proof and status; this executable returned the retry conflict, preserved
both proof files and the ledger byte-for-byte, retained its read evidence, and
succeeded on retry. This is controlled replay evidence, not a concurrent live
Tally mutation experiment.
That same binary passed eight checks using official MCP
JavaScript SDK 1.30.0, negotiating 2025-11-25 down to 2025-06-18. Official MCPB
CLI 2.1.2 validated and packed the archive. Its extracted executable and all four
legal resources matched the staged bytes; executable mode survived extraction;
the manifest command initialized and listed ten default tools successfully.

- Release executable SHA-256: `45907fc17d1fd97a3d11534c9816280746419c481bed805820134bd0d038a5bc`.
- MCPB archive SHA-256: `d9c025162b5819c810d0772b746e6ca9d107c35df3ecf07afb99a719bdad8142`.
- Source fingerprint (363 build-input files, unchanged through the settled-source rebuild): `bf82c83f214ae0eef0ecaa1c269c4fa831eb56d73b9029413d60f17b8f3ce0c7`.

CI builds, validates, packs, extracts, and launches the actual MCPB on Windows
and macOS. The portable smoke checks initialization, ten default tools, the local
voucher schema, and its linked preparation/completion receipts with bounded execution and
output. Six harness regressions and the final local archive passed this check.
Hosted results must confirm the same check for the published candidate; desktop
client installation remains separate. The local archive is unsigned and is not
a production release.

The final verification record must distinguish local tests, actual archive
launch, official client smoke, live Tally evidence, and hosted Windows/macOS
checks. None substitutes for the others. Graphify data and its refresh script
were unavailable in this checkout; structural discovery used focused source
tracing instead.

The 163-entry sealed surface was audited before each reseal. No sealed path changed in the final artifact/schema correction. The preceding reseal
updated six existing paths for shared company-number admission and retained
probe/drift evidence across financial readers. No paths were added or
removed. Previous seals cover observed release/tier profile version 4, typed
native-ledger validation, the protocol observation, and the qualified file-identity
and numbering clarification, literal-date counter-observation,
negative-verdict qualification limit, currency witness,
standard-library file locking, fresh financial mode admission and retained refusal
evidence. Earlier
changes covered request commitments, report-source fields, CI packaging, and
the period-opening protocol reference. The existing pin
set was rehashed, sealed, and repointed using the release-process commands.
No compatibility cell was promoted.

The numbering review was adjudicated against the actual generated request and
retained import/readback bytes. Failed `Alter` behavior does not establish a
duplicate on this `Create` + client-`REMOTEID` workflow. The earlier exact-file
repeat omitted `VOUCHERNUMBER` and retained one voucher with stable GUID, numeric
master ID and assigned number. The renderer regression preserves that request
identity with or without an optional number. [Protocol reference §9.8](../tally/TALLY_PROTOCOL_REFERENCE.md#98-voucher-numbering-method-changes-everything--use-manual)
records the scoped exception and its limits; no unobserved voucher-type preflight
was added.

## Migration, rollback, and security impact

See [README migration notes](README.md#protocol-and-migration-notes) for output
field changes, unavailable change enumeration, strict argument admission, and
fingerprint-only attribution. There is no database migration. The new reader
accepts legacy full batch records and compact hash-bound verification status
records; older binaries refuse the compact format. Preserve the ledger and
proofs, and disable imports after a binary downgrade instead of truncating
history. Rolling back the connector does not undo an operator import into Tally.

The connector remains loopback-only and never dispatches import XML. Company
identity, response redaction, bounded input/output, local private-file handling,
and exact emitted-byte receipts remain review requirements. Public fixtures
contain synthetic data and no credentials or operational machine paths. Required
hosted Windows checks must confirm the platform-specific file-lock correction.
