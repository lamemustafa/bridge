# Changelog

All notable changes to Bridge are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

The next release line is `0.2.x`. This creates an unambiguous version boundary
between the published MIT-licensed `v0.1.0` release and Apache-2.0 builds from
current source.

### In plain words: since the `mcp-preview-0.2.0` build (16 Sep 2026)

These changes are in the source. They are not yet in a published package.
Each line names the pull requests it comes from, except where it names an open
issue.

**What you can do now**

- Post a Payment, Receipt or Contra voucher, as well as a Journal. Each one
  still waits for your approval in a separate ComplyEaze Bridge window
  (#585, #600).
- Read password-protected bank statement PDFs into voucher proposals, including
  Union Bank of India statements (#444, #481).
- See a party's GSTIN as it stood on a date you choose, from its dated
  registration history (#657, #700).
- See each ledger's group ancestry, and the date its opening balance is as of
  (#498, #499, #569).
- See the post-dated flag, invoice and reference details, and the party GSTIN
  on voucher reads (#498).
- Pick vouchers by voucher class the way Tally classifies them (#663).
- Work with books of more than 1,000 ledgers in the ledger catalogue (#643).
- Get a whole `verify_import` verdict at once, with the verified vouchers in
  pages (#673).
- Source builds only: with `BRIDGE_AGENT_ENABLE_BATCH_POST` on as well as
  posting, post 2 to 50 vouchers of one saved batch after one approval, which
  shows a summary by ledger. It is off by default and not in the extension
  until a live batch post is proven. A batch counts as clean only when Tally's
  counts match exactly; otherwise you review it (#712, #721).

**Safer posting**

- Voucher posting is off by default in the Claude Desktop extension. Turn it
  on in the extension settings (#577).
- An approval counts only when the approval window returns a fresh one-time
  token. How the window closes no longer decides it (#665, #704).
- ComplyEaze Bridge confirms the company as its last step before posting, and
  reports where the voucher landed (#607).
- It refuses to post if a ledger changed since the voucher was prepared, if
  the company's masters changed between its final checks and the post, or if
  the saved file no longer matches its record (#578, #615, #616).
- If the company's masters change while a post is landing, the post is
  reported as not verified and needing reconciliation, so you check it in
  Tally (#623).
- It records each voucher's import identity before sending it, and never sends
  one twice (#582, #678).
- It refuses to post into a book with more than one currency defined (#613).
- When Tally rejects a line, you see Tally's own error text, kept short and
  safe to display (#695).
- When you correct a voucher it built, ComplyEaze Bridge checks the
  correction against the book as it prepares the file. It does not post
  corrections itself; you import the file in Tally (#439, #620, #639, #660).

**Clearer answers when it cannot answer**

- A refusal now names its cause instead of a generic failure. Causes include:
  - a read that ran out of time (#493);
  - an empty book (#489);
  - a missing company identity (#514);
  - no reply from Tally (#659);
  - a foreign-currency opening balance, or a book with more than one currency
    (#606, #706, #720);
  - a voucher type that doesn't exist (#677);
  - a list Tally would not serve (#719);
  - two ledgers whose names differ only by a hidden line break (#708).
- Education-mode Tally: ComplyEaze Bridge refuses a read that Education would
  return empty without saying so (#588, #598).

**Big books**

- ComplyEaze Bridge splits a long voucher window when Tally cannot serve it in
  one request, and sizes each request by expected response size, not date span
  (#494, #506).
- The `vouchers` tool reports how long each request to Tally took (#608).

**Tax-audit engine (in development, not in the extension)**

- A Rust tax-audit engine, `bridge-tax-audit`, now covers:
  - cash payments and receipts (s.40A(3), s.269ST, s.269SS/T);
  - depreciation;
  - s.44AB applicability;
  - financial statements;
  - the trial balance;
  - stale balances;
  - ledger scrutiny;
  - the cash book;
  - TDS payees;
  - s.43B and s.43B(h);
  - book-keeping quality;
  - loans and interest;
  - partners (s.40(b), s.194T);
  - bank reconciliation;
  - a high-value register.
  (#501, #504, #508, #560, #561, #571, #592, #593, #618, #636, #710, #713)
- Every module is mutation-tested in CI (#646, #682).
- Its accuracy is **not yet proven publicly**. Issue #738 proposes how to
  prove it.

**Security and upkeep**

- The tools workspace moved off a yanked `chacha20` and a vulnerable `rustls`,
  and CI audits its lockfile (#455, #458).
- CI now fails if an HTTP call appears outside the files allowed to make
  one. The README's promise that Tally data is never uploaded now has a test
  behind it (#701).
- We deleted legacy code that nothing reached (#473, #495).

### Removed

- DSC (digital-signature certificate) hardware-token detection, certificate
  extraction, and their AXAL sync path have been withdrawn from Bridge's
  scope, along with the `pkcs11` and `cryptoki` dependencies that reached the
  PKCS#11 driver. `pkcs11` 0.5.0 was unsound (RUSTSEC-2022-0034) and
  unmaintained; the capability may be rebuilt properly later if needed. AXAL's
  Tally and Documents integrations are unaffected.

### Changed

- `build_import_xml` now reports `live_evidence` as an array of
  `{observation, report, voucher_types}` records rather than a single string,
  and no longer emits `live_evidence_report`. The previous shape could name
  only one source for a whole batch, so a Payment build cited a report that
  records Payment being refused. A client branching on the old string value
  needs updating; the accompanying voucher types make the provenance readable
  without one.
- Relicensed future Bridge distributions from the MIT License to the Apache
  License, Version 2.0. The previously published `v0.1.0` release remains
  available under the MIT License that accompanied that release.
- Core snapshot canaries now authorize attempts through stable observed
  sealed-profile execution evidence without claiming field support from an
  incidental first-day dataset; snapshot rows are always re-fetched after the
  durable run starts.
- A probe that no longer returns the selected company now clears and
  invalidates every company-scoped evidence, proof, mirror, diagnostic, and
  snapshot view before installing the replacement probe, so its fresh review
  remains usable without displaying stale company data.
- Snapshot lifecycle probes no longer replace interactive setup-review state;
  restart admission uses the exact sealed Core receipt, and ambiguous duplicate
  live company identities fail before any snapshot read with their concrete
  terminal proof reason preserved.
- Snapshot recovery now durably replays backward-clock abandonment evidence,
  enforces a 100,000-record aggregate hydration ceiling, and recovers an exact
  already-committed receipt from compact hash-bound proof authority without
  rehydrating canonical membership.

### Added

- Local import files may now carry Payment, Receipt and Contra vouchers as well
  as Journals, so a bank statement can be expressed in the voucher types Tally
  files it under. Each of the three is admitted only as two entries over two
  distinct ledgers carrying neither a voucher number nor a reference. The side
  that must hold money is refused unless that ledger's live group ancestry
  reaches a reserved Bank Accounts or Cash-in-Hand identity — the two where a
  captured ledger is observed sitting under a captured group. The counterparty
  side must be established as holding no money: any money group there means the
  voucher is really a Contra, and a ledger whose group ancestry cannot be
  resolved is refused as well, because neither leg is admitted on an absence of
  evidence. A build that names a
  counterparty warns that its amount lands On Account. Native posting is
  unchanged and still accepts only one unnumbered Journal.
- A local-first Tally Truth Layer with capability passports, explicit truth
  states, encrypted mirror evidence, resumable/adaptive snapshots, Proof of
  Sync and Gap Map output, and a safer operator console. The migrations are
  additive; rollback requires restoring the prior application and retaining
  the encrypted database for forward recovery rather than deleting evidence.
- Portable, bounded Tally protocol, canonicalization, transport, runtime,
  compatibility, incremental-policy, qualification, observability, and
  write-safety crates backed by a synthetic loopback protocol simulator.
- Reviewed single-use setup authority, exact selected-read qualification, and
  fail-closed compatibility manifests/runbooks. Live Education behavior and
  every write capability remain unknown or disabled until exact reviewed
  evidence exists.
- Native Windows and macOS CI coverage for formatting, tests, builds, and
  Clippy.
- Repository-local Windows and macOS application icons.
- Open-source contribution, security, review, and rectification guidance.
- Reproducible Node and Rust toolchain baselines, installer smoke builds, and
  complete lockfile-to-license-inventory checks.
- Automated legal-resource inspection for Windows MSI/NSIS installers and the
  staged and DMG-packaged macOS app bundles.

### Security

- SQLCipher/keyring-backed local Tally state, immutable proof/checkpoint
  receipts, loopback-only proxy-free HTTP, bounded incremental decoding,
  cancellation and lease enforcement, idempotent crash replay, and sealed
  no-write qualification boundaries.
- SQLCipher pool replacement connections now receive raw key bytes from
  zeroizing storage without retaining a key-derived pragma string. Proof
  contract v3 binds detailed record counts, and historical crash recovery no
  longer depends on current checkpoint ownership.
- File-backed snapshot ownership now uses per-run kernel advisory locks, so a
  crash can be reclaimed after wall-clock rollback without allowing a live
  owner to be stolen. Persisted/live company profiles correlate through an
  opaque endpoint-scoped identity key, and macOS qualification reports
  `ru_maxrss` in its native byte units.
- Losing checkpoint compare-and-swap decisions terminalize as durable failed
  proofs and close staging attempts instead of remaining falsely resumable;
  unrelated checkpoint advances do not rewrite Failed or Cancelled outcomes.
- Compatibility claims now require verified synthetic-fixture identity before
  an explicit parsed Tally application rejection can establish `Unsupported`;
  fixture, context, sentinel, parser, malformed-response, and transport
  failures remain fail-closed observations rather than incompatibility claims.
- Updated the XML parsing graph and removed unused Linux-only dialog
  dependencies from the supported Windows and macOS build graph.
- Updated the Tauri runtime to 2.11.5 and tauri-runtime-wry to 2.11.4.
- HTTPS-only AXAL endpoints with redirect blocking, bounded responses, and
  credential validation.
- Safer DSC PIN transport and PKCS#11 library discovery without exposing
  arbitrary native-library loading to the webview.
- Bounded Tally and document responses with endpoint and upload validation.

## [0.1.0] - 2026-07-12

### Added

- Initial open-source Bridge application with React, Rust, and Tauri support
  for Tally, GST, DSC, document, sync, and local database workflows.

[Unreleased]: https://github.com/lamemustafa/bridge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lamemustafa/bridge/releases/tag/v0.1.0
