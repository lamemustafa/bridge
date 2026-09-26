# 0019 — A stored audit read is client book data at rest

- Status: Proposed
- Date: 2026-09-18
- Related: [0012 — Tally live compatibility evidence](0012-tally-live-compatibility-evidence.md); `docs/tally/privacy-model.md`; `docs/tax-audit/read-format-v1.md` (`tally-read-v1`, §6 P7)
- Does not amend: [0015 — Selected-read qualification authority](0015-tally-selected-read-qualification-authority.md), which #495 withdrew on 2026-09-17. 0015 covers a different, now-removed setup flow; this ADR does not touch it and should not be read as reviving it.

## Context

`audit_read` will store a whole financial year of one client company, as Tally returned it, under
Bridge's data directory, for `bridge-tax-audit` and the reference Python engine to read. The read
holds ledgers, parties, GSTINs and PANs, every voucher and narration, and inventory. `docs/tax-audit/read-format-v1.md`
already names the problem this ADR exists to answer: "A whole-year book stored in plaintext is a
new at-rest data class for any producer that previously kept hashes only. Encryption, retention
and deletion policy for that class need their own design decision; this spec does not decide
them" (§6, rule P7). For a large trading book, one whole-year read is about 300 MiB of content.

What Bridge does today:

- **The data directory.**
  - macOS: `~/Library/Application Support/Bridge`.
  - Windows: `%LOCALAPPDATA%\Bridge\agent` (falling back to `%APPDATA%`, then a bare relative path).
  - Other platforms fall back to the temp directory.
  - `BRIDGE_AGENT_DATA_DIR` overrides all of these (`src-tauri/src/local_files/paths.rs:5-26`,
    `src-tauri/src/agent.rs:256-268`).
- **How the directory is admitted.** `ensure_private_directory` creates it `0700`, refuses
  symlinks, and checks the owner on Unix. On Windows it refuses reparse points but "Ordinary
  directories retain their inherited Windows ACLs" (`src-tauri/src/local_files/directory.rs:11-48`,
  comment at `:37-44`).
- **The MCP binary writes client data in plaintext today.** Bank-statement proposals, including
  every parsed statement record, are written as pretty JSON with mode `0600`, staged then renamed
  (`src-tauri/src/agent_bank_statement.rs:329-382`). Import files and lab evidence live in the same
  directory. No code on master expires or prunes any of these files: a search for retention,
  prune, purge and expire in the agent and local-files modules finds only test names and fixtures
  (`agent_import_bank_tests.rs:999-1013`, a ledger called "Retention Party"; `agent_tests.rs:1910`,
  a test about the in-memory evidence buffer's eviction), and none of them touches a file on disk.
- **The desktop app's Tally mirror is encrypted.** It is SQLCipher with a random 32-byte key held
  in the OS credential store: service `com.complyeaze.bridge.tally-mirror`, account
  `database-key-v1:<sha256(path)>`. It runs an integrity check and sets `cipher_memory_security`
  and `secure_delete` (`src-tauri/src/db/encrypted.rs:9-127,179-235`), and fails closed with
  "an explicit local reset is required" when the database exists but its key is missing
  (`:107-111`). The key store is constructed only in the desktop entry point
  (`src-tauri/src/lib.rs:64-70`, specifically `:68`).
- **The MCP binary never touches the credential store today.** `OsMirrorKeyStore` is constructed
  only at `lib.rs:68` (the desktop app's `run`). `bridge_mcp`'s `main` runs `agent::run_stdio`
  (`src-tauri/src/bin/bridge_mcp.rs`) and never reaches it.
- **The credential-store backends differ by platform.** `keyring` 4.2.0 with feature `v1` is
  resolved in `Cargo.lock` (pinned at `src-tauri/Cargo.toml:110`).
  - macOS uses the legacy file keychain (`apple-native-keyring-store` 1.0.2, `src/lib.rs:12-16`).
  - Windows uses Credential Manager, whose credentials default to `Enterprise` persistence
    (`windows-native-keyring-store` 1.1.0, `src/lib.rs:36-44`). That persistence is meant to roam
    with a domain roaming profile; Bridge does not currently choose a different one.
- **The MCP binary already has native-dialog paths, and they are narrower than they look.**
  `bridge_mcp`'s own `main` checks for a `--confirm-journal` argument before starting Tokio and,
  if present, calls `agent::run_confirmation()` instead of serving stdio
  (`src-tauri/src/bin/bridge_mcp.rs`, `run_journal_confirmation_child_from_args` in
  `src-tauri/src/lib.rs`); `--confirm-review`, for the review dialog of #239, works the same way.
  The post path exists because posting one approved voucher spawns the same
  executable as a child process with that flag (PR #236; today `nonce_bound_dialog` in
  `src-tauri/src/tally/approved_import.rs`). Since #635 the parent does not take the
  child's exit status as the answer. It sends a fresh nonce on stdin and approves only when the child
  prints exactly the post token for that nonce and exits cleanly. `run_confirmation`'s doc comment
  says it "Runs before Tokio starts, because macOS dialogs require the main thread". The post
  child shows an approve/cancel `rfd::MessageDialog` over a bounded text preview (`show_review`),
  whose title and post button name the voucher count the parent sends after the nonce (#746).
  On Windows it is a raw `MessageBoxW`, not `rfd`, because `rfd` without common-controls-v6 discards the
  custom button labels (that file's own comment). This is
  a real precedent for showing *some* native prompt from a process `bridge_mcp` controls, but it is
  not a general file dialog: every `rfd::FileDialog` call site on master — the save/open/folder
  picker `audit_read_export` would need — is reachable only from a `#[tauri::command]` taking Tauri
  `State`, which a plain stdio server cannot construct (`commands.rs:2289,2303,2575`;
  `agent_desktop_journal_review.rs:131`; `source_draft/files.rs:24,45`).
- **The privacy model already requires more than the MCP binary does.** It says book data belongs
  in the encrypted mirror only when required, raw response bodies are not normal diagnostic output,
  and "Operators must be able to delete the mirror and associated OS credential as a single
  documented reset operation. Until that workflow is implemented and tested, no UI should claim
  that local Tally data has been fully erased" (`docs/tally/privacy-model.md:9-47`, the "Book data"
  row is at `:12`).
- **Egress is recorded.** Every tool call appends a receipt to `agent-egress.jsonl` in the data
  directory (`Server::append_framed_egress`, `src-tauri/src/agent_delivery.rs:32-72`).
- **Files on disk bypass that receipt.** A process with a shell or a filesystem tool, running as
  the same user as Bridge, can read any file in the data directory and send it to its own model
  provider without redaction and without a receipt.

External context, held to the hedge each source itself uses:

- **DPDP Act 2023.** s.8(5) requires "reasonable security safeguards"; s.8(6) requires notifying
  the Board and affected principals of a breach; s.8(7) requires erasing personal data once the
  purpose is served, "unless retention is necessary for compliance with any law for the time being
  in force" (<https://www.dpdpa.com/dpdpa2023/chapter-2/section8.html>,
  <https://indiankanoon.org/doc/157637354/>).
- **DPDP Rules 2025, Rule 6** lists encryption, obfuscation, masking or virtual tokens, access
  control, access logs and backups, and asks that logs and personal data be retained for one year
  unless another law requires otherwise (<https://www.dpdpa.com/dpdparules/rule6.html>).
- **When the Rules take effect.** The Rules were notified on 13 November 2025. Secondary sources
  say the substantive obligations, including s.8, take effect 18 months later, around 13 May 2027
  (<https://www.sansalegal.com/post/dpdp-act-2023-and-rules-2025-phased-implementation-timeline-and-business-compliance-deadlines>,
  <https://www.seclore.com/fundamentals/dpdp-rules-2025-compliance-guide/>). This has not been
  confirmed from a primary source.
- **Legal, unresolved.** Client books contain personal data (proprietors' PANs, individual
  parties, salary ledgers). A CA firm processing them for an audit is probably a Data Fiduciary for
  that purpose. ComplyEaze Bridge receives nothing on the Tally path, so it is probably neither
  fiduciary nor processor for a local read. Whether a professional retention rule such as SA 230
  counts as "law" under s.8(7) is a legal question this ADR does not resolve.
- **SA 230 and SQC 1.** Audit documentation is ordinarily kept for no shorter than seven years from
  the date of the auditor's report; ICAI amended SA 230 para A23 from ten years to seven
  (<https://www.icai.org/post/announcement-on-amendment-to-sa-230-retention-period-for-engagement-documentation-working-papers-28-05-2010>).
  Assembly of the final file is ordinarily within 60 days of the report (ICAI implementation
  guide, <https://cpeapp.icai.org/downloadBGM/5b39b17b010f9.pdf>, via search summary). This
  obligation belongs to the firm's audit file; it says nothing about which program's cache holds a
  copy.
- **What the OS already provides.** FileVault encrypts whole volumes with AES-XTS, with keys
  handled in the Secure Enclave on Apple silicon and T2 Macs
  (<https://support.apple.com/guide/security/volume-encryption-with-filevault-sec4c6dc1b6e/web>).
  DPAPI data can "typically" be decrypted only by a user with the same logon credential on the same
  computer (<https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata>).
  Neither protects against a process running as the same logged-in user — which includes an AI
  agent's own shell.

Threats this ADR must address, and what stops each one:

- **T1. Laptop lost or stolen.** Full-disk encryption, or app-level encryption.
- **T2. Another OS account on the same machine.** `0700` on macOS; an explicit ACL, not an
  inherited one, on Windows.
- **T3. Copies Bridge does not control** (Time Machine, a zipped support folder,
  `BRIDGE_AGENT_DATA_DIR` inside a synced folder, desktop search indexing). Only app-level
  encryption stops these.
- **T4. Data kept longer than its purpose (DPDP s.8(7)).** Retention with real deletion.
- **T5. Residue after deletion.** A deleted file is not reliably erased on an SSD. Only destroying
  the key (crypto erasure) gives a real answer.
- **T6. A same-user process reading the read**, including the agent's own shell sending it to its
  model provider. App encryption only partly stops this: on macOS the legacy keychain's ACL is
  bound to the creating program, so another program is expected to trigger a user prompt; on
  Windows a generic Credential Manager entry is readable by any process of that user. Everywhere,
  the real control is a consent step before plaintext leaves Bridge.

## Decision

**Bridge stores each audit read encrypted with its own key. Plaintext leaves Bridge only through a
person-confirmed export. A read is a working copy with a short default lifetime, not an archive.**

### 1. Storage

- **One file per read.** Each read is one SQLCipher database, `data_dir/audit_reads/<read_id>.db`,
  holding the manifest bytes, each part's stored blob exactly as `read-format-v1.md` §2 defines it
  (gzip or identity), and `stored_sha256`/`sha256` checked on every open.
- **Reuse, not new crypto.** The file is opened through the existing `connect_encrypted` path:
  integrity check, `cipher_memory_security`, `secure_delete`. There is no new cryptographic
  primitive.
- **The v1 directory layout becomes the export layout.** `read-format-v1.md` §1 would be amended to
  say so. Hash definitions do not change, because the stored blob is identical.
- **Keys.** Each read gets a random 32-byte key in the OS credential store, under a service name
  separate from the mirror's: `com.complyeaze.bridge.audit-read`, account `<read_id>`.
  - **Known blocker, not an open question.** Bridge pins `keyring` 4.x with only the `v1` feature
    (`src-tauri/Cargo.toml:110`), and the `v1` `Entry` type has no way to pass a persistence
    modifier — the Windows store defaults every credential to `Enterprise`
    (`windows-native-keyring-store-1.1.0/src/store.rs:121`). `Local` persistence, so the credential
    does not roam, needs a direct dependency on `keyring_core`/`windows_native_keyring_store` and
    `Entry::new_with_modifiers` (`keyring-core-1.0.0/src/lib.rs:152`), bypassing the `v1` shim. The
    mirror's existing key has the same roaming default today; fixing both together is the honest
    scope.
  - On macOS it uses the legacy keychain the mirror already uses.
- **Fail closed.** If the credential store is unavailable or refuses, `audit_read` refuses. It
  never falls back to plaintext.
- **The directory must be private.** On Unix, `audit_reads/` is admitted by
  `ensure_private_directory`. On Windows it gets an explicit protected DACL for the owner and
  SYSTEM only, instead of the inherited ACL. `audit_read` refuses if the resolved data directory is
  not the platform default, unless the operator has set an explicit acknowledgement — closing the
  `BRIDGE_AGENT_DATA_DIR`-into-a-synced-folder path.

### 2. Handle and access

The tool returns only `read_id` and `manifest_sha256`, plus counts, as `read-format-v1.md` §1
specifies. No MCP tool returns part bytes. Later consumers — the Rust tax-audit engine and any
derived-layer builder — open a read inside Bridge.

### 3. Export: the only way to plaintext

- **Where it runs.** Export is a desktop-app action, not an MCP tool. `bridge_mcp` does have one
  existing native-dialog path (the journal-post approve/cancel prompt described in Context), but
  that is a fixed message box shown by a short-lived child process, not a save-file picker, and
  every `rfd::FileDialog` call site on master requires Tauri `State`. Putting export behind the
  desktop app keeps the consent step in a process that already has this capability reviewed, rather
  than building a second, picker-capable native-dialog path for `bridge_mcp` that this ADR does not
  need.
- **What it does.** `audit_read_export(read_id, manifest_sha256)` opens a native save dialog, then
  writes the v1 directory to the folder the person picked, with the same no-overwrite,
  staged-then-renamed discipline the bank proposals use.
- **What it records.** It appends an egress receipt naming the read, the manifest hash, and the
  fact of export. It does not record the path, following this repository's rule that local paths
  are excluded from receipts and support export (`docs/tally/privacy-model.md:41-43`).
- **Who owns the copy.** The exported copy is outside Bridge's policy, and the dialog says so in
  one sentence.
- **Until the Rust engine consumes reads directly**, this is how the Python engine gets its input.
  Export does not make plaintext book data anywhere it was not already going to exist; it makes the
  step deliberate and recorded.

### 4. Retention

- **Default lifetime.** Every read gets `expires_at` set to 90 days after `created_at`. That
  covers the audit plus SQC 1's 60-day file-assembly window with margin — a judgement call, not a
  measured deadline.
- **Holds.** The operator can place a hold, or extend the date, per read, through a native
  confirmation, not a model decision.
- **Expiry is enforced lazily.** Bridge has no scheduler by design. An expired read is erased at
  the next Bridge start or `audit_read*` call, and each erasure writes a receipt.
- **Warning before expiry.** From 14 days before `expires_at`, every `audit_read*` result and the
  desktop app's read list name the reads about to expire, so a hold can be placed before, not
  after, the erasure.
- **Listing.** `audit_read_list` shows every read's company, period, size, `expires_at` and hold
  state. It shows no book content.
- **Archival retention (SA 230, seven years) is the firm's audit file, reached through export.**
  Bridge makes no retention claim for its own store beyond `expires_at`.

### 5. Deletion

- **Order of operations.** `audit_read_delete(read_id, manifest_sha256)` and expiry both run these
  steps in order: delete the credential; delete the database file and its WAL/SHM; append a
  receipt.
- **What erasure means, stated plainly.** It means the key has been destroyed. The macOS login
  keychain file is itself backed up by Time Machine, so restoring an old backup can bring back both
  a key and a database — this needs testing before any UI claims erasure. The ADR therefore claims
  only "not readable by this installation after deletion", never "erased from every copy".
- **A per-company purge** deletes every read whose `company.guid` matches, supporting the end of a
  client engagement.
- **Reset.** The existing privacy-model promise of a single documented reset extends to "all audit
  reads and their credentials".

### 6. What this forbids

- A plaintext read under the data directory. Any staging file that exists during a write must be
  inside the encrypted database, not beside it.
- Any MCP tool returning raw part bytes or a filesystem path to them.
- Writing a read to a caller-chosen path other than through the native export dialog.
- Claiming erasure beyond "key destroyed in this installation".
- Storing a read in, or exporting it to, the Bridge repository, CI, fixtures, or a support bundle.
  ADR 0012 already excludes company names/GUIDs, tax identifiers, amounts, and narrations from live
  compatibility evidence; this ADR applies the same exclusion to audit reads.
- A retention default longer than 90 days without an explicit per-read hold.

### Why not plaintext

An earlier version of this decision kept the v1 directory in plaintext under `0700`/`0600`,
relying on FileVault/BitLocker and a 90-day expiry. That was rejected on review for reasons that
still bound the design above:

- It contradicts `docs/tally/privacy-model.md:12`, which puts book data only in the *encrypted*
  mirror; a plaintext whole-year book is a larger store of book data than anything the encrypted
  mirror holds today.
- "Rely on FileVault or BitLocker" cannot be checked from an ordinary user process (`manage-bde`
  needs admin rights to query), so the warning would be unverifiable, and neither protects T3 or
  T6 regardless.
- On Windows, `directory.rs`'s inherited ACL is acceptable under the platform default
  `%LOCALAPPDATA%`, but `BRIDGE_AGENT_DATA_DIR` can point anywhere, including a shared or synced
  folder, and nothing on master checks that.
- Deletion of a plaintext file beside `agent-egress.jsonl` is not erasure on an SSD and is not
  covered by any receipt if another process reads it first — silently contradicting the egress log's
  own promise for the most sensitive data Bridge will ever hold.
- One undifferentiated plaintext store has no per-client erasure; a firm ending one engagement
  needs to erase that client alone and be able to say so.

### Options considered

1. **Plaintext directory, `0700`/`0600`** (today's pattern for bank proposals). Needs no new code
   and the Python engine reads it directly, but covers T2 on macOS and T1 only if FileVault/
   BitLocker happens to be on; does not cover T3, T5, or T6. Rejected — see "Why not plaintext".
2. **Encrypted, with the key in the OS credential store** (chosen). Covers T1–T3 and T5 (with a
   per-read key) and partly covers T6. Costs key management in the MCP binary and an explicit
   export/decrypt step for the plaintext-only Python engine.
3. **No persistence: stream to the engine only.** Would cover all of T1–T6 for Bridge's own
   storage, but is not possible today because the engine is a separate process and MCP responses
   are capped and redacted — a roughly 300 MiB book cannot pass through a model. It would also lose
   reproducibility, since the pack's evidence would point at bytes that no longer exist. Left open
   for later, once the Rust engine runs inside Bridge.
4. **Mirror hashes only, then re-read Tally to reproduce.** The strongest option on privacy, but
   reproduction fails whenever the book has changed since the read (common, since staff keep
   editing) or the client's Tally is unreachable from the CA's machine. A hash can then only verify
   a copy held elsewhere, not recreate one. Rejected.

## Consequences

- **The agent's shell (T3, T6)** finds ciphertext in the data directory. On macOS, reading the key
  from another program is expected to raise a keychain prompt; this needs testing before it is
  relied on. On Windows it does not — the ADR states plainly that on Windows, the control against a
  same-user process is the export consent step, not the encryption.
- **Hashes stay usable.** The Python engine and parity checks keep working from an export, and
  every `sha256` in the format keeps its meaning.
- **A real per-client "forget"** exists that a CA can use when an engagement ends.
- **New operational risks** (see Open items): credential-store access from `bridge_mcp` under
  Claude Desktop or another MCP host is untested; a binary update can change the code signature
  that gates a macOS keychain item; the key and the database can go missing independently, which
  fails closed with "explicit local reset required", matching the existing mirror behavior
  (`encrypted.rs:107-111`).
- **Option 3 (no persistence) stays open.** Once the Rust engine runs in-process, a mode that never
  writes a read can be added without changing this ADR's rules for reads that are written.
- **Bank-statement proposals are left as they are.** They stay plaintext; this ADR does not change
  them. A follow-up should decide whether the same rule should cover them.

## Open items before this can be Accepted

**Primary risk.** Both load-bearing mechanisms — storage, which fails closed without the
credential store, and export, the only path to plaintext — depend on capabilities no Bridge code
exercises today from the headless `bridge_mcp` process: nothing there touches the mirror's
credential store (`OsMirrorKeyStore` is built only in the desktop `lib.rs:64-70`), and no
`rfd::FileDialog` call site is reachable from it either (Context). Item 1 below is a go/no-go
measurement for the whole design, not one item among five: if a credential-store access from
`bridge_mcp` prompts or blocks under a real MCP host, `audit_read` itself has to move to the
desktop app, and the MCP server would only list reads and run the engine against them.

1. On both platforms, measure whether a credential-store access from `bridge_mcp` launched by
   Claude Desktop (or another MCP host) shows a prompt, and whether it blocks the stdio call.
   Repeat after a signed binary update, since a macOS keychain ACL can be bound to the signing
   identity.
2. Decide whether to add the direct `keyring_core` dependency needed for `Local` persistence on
   Windows (the `v1` API cannot express it; see Keys above), and whether the mirror's own key
   moves with it.
3. Legal: confirm the firm's role under DPDP, and whether SQC 1 retention counts as "law" for
   s.8(7).
4. Confirm the DPDP Rules commencement date from a primary source; the secondary sources cited
   above have not been cross-checked against an official notification.
5. Measure SQLCipher open-and-verify time on a large trading book's whole-year read (about 300 MiB)
   against a stdio tool-call budget.
