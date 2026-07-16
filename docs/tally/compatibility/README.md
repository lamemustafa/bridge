# Tally compatibility evidence

`compatibility-matrix.json` is the machine-readable authority for exact Tally
support cells. Every cell starts as `unknown`; absence of evidence is never
success. `compatibility-surface.json` binds evidence freshness to the exact
Bridge Tally request, parser, transport, runtime, lockfile, and gate sources.

An evidenced `observed`, `supported`, or `unsupported` cell requires all of the
following:

- a privacy-reduced live-read receipt for the exact product, release, mode,
  platform, architecture, transport, ODBC state, company-load state, locale,
  encoding, fixture-owned dataset tier, source surface, commit, and operation
  profiles, plus an explicit operator attestation that no customer data was
  loaded;
- an unexpired maintainer review attestation signed by a non-revoked key in
  `trusted-evidence-keys.json`;
- a clean source tree at observation time and evidence no older than the cell's
  declared maximum age;
- a successful executable gate run against the current source surface.

The receipt never establishes responder authenticity, source completeness or
atomicity, accounting correctness, performance support, or any write behavior.
Checksums detect accidental change; only the reviewed signature supplies claim
authority.

Run the checked-in gate from `src-tauri`:

```sh
cargo run --locked -p bridge-tally-compatibility -- gate \
  ../docs/tally/compatibility/compatibility-matrix.json \
  ../docs/tally/compatibility/compatibility-surface.json \
  ../docs/tally/compatibility/trusted-evidence-keys.json \
  ../docs/tally/compatibility/evidence ..
```

`Observed` and `supported` require every required operation to pass the full
fixture sentinel contract. `Unsupported` requires an actual required-profile
failure observed in a response; an unreachable port or absent evidence is not
unsupported. JSONEX, large-dataset, no-company, and UTF-16 cells are currently
non-promotable and remain `unknown` until their qualification paths exist.

The standalone live controller is documented in
[`live-education-runbook.md`](./live-education-runbook.md). It dispatches only
the sealed production read profiles through a typed adapter, requires two
interactive confirmations, and cannot overwrite a receipt. Never create a
receipt manually or promote parser-only qualification evidence.

The separate [`bills-native-probe-runbook.md`](./bills-native-probe-runbook.md)
defines the synthetic fixture and stop conditions for the dormant native
`Ledger Outstandings` probe. Its non-default typed runner can create only a
structurally separate observation receipt. It is not a production read profile,
cannot create a compatibility receipt, and cannot satisfy a support claim.
