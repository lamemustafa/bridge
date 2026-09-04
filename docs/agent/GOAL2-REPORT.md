# Goal 2 report — historical voucher-file import work (not live-verified)

> Status superseded by Goal 6: this report describes synthetic/local
> voucher-file generation and readback behavior. It records no live-Tally
> import/read-back evidence and must not be used to claim live verification.

## Delivered

- Extended `bridge_mcp` with `voucher_schema`, `validate_masters`,
  `build_import_xml`, and `verify_import`.
- `build_import_xml` validates the strict payload schema, transaction-ID
  uniqueness, exact two-decimal arithmetic, per-voucher debit/credit balance,
  company `BOOKSFROM..today`, exact live ledger names, and previously built
  transaction IDs. It writes UTF-8 import XML to
  `<data_dir>/imports/<batch_id>.xml` and appends a private
  `agent-import-ledger.jsonl` entry.
- `validate_masters` returns `exact`, `near_miss`, or `missing`, with NFC,
  case, whitespace/NBSP, dash/quote, and prefix diagnostics. It never creates
  a master.
- `verify_import` only exports the stored batch window. It compares remote ID,
  date, type, signed ledger entries, and polarity; reports verified, divergent,
  or absent rows; detects remote-ID and accounting-fingerprint duplicates;
  writes `.proof.json` and `.proof.md`; and appends the verification status.
- The transport boundary rejects any `TALLYREQUEST` beginning with `Import`.
  No Goal 2 path posts a Tally import envelope.

## Sign convention basis

The generated XML uses the Goal 2 contract's stated voucher convention:
debits render as negative `AMOUNT` with `ISDEEMEDPOSITIVE=Yes`, credits as
positive `AMOUNT` with `ISDEEMEDPOSITIVE=No`. The protocol reference confirms
that these direct voucher-entry fields are available; its bill-allocation note
also warns not to generalize bill-allocation polarity to direct ledger entries.
No `.bridge-live` capture was present in this worktree, so the implementation
does not claim a new live measurement. The golden test fixes the requested
voucher convention byte-for-byte.

## Verification

```text
rustup run 1.96.0-aarch64-apple-darwin cargo fmt --manifest-path src-tauri/Cargo.toml --check
rustup run 1.96.0-aarch64-apple-darwin cargo test --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
5 passed; 0 failed
rustup run 1.96.0-aarch64-apple-darwin cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
finished with no warnings
```

The test coverage includes schema/balance failure, all requested near-miss
matcher cases, exact two-voucher XML bytes, sign convention and escaping,
batch/import-ledger uniqueness, absent/divergent/duplicate proof outcomes,
and an end-to-end loopback simulator path: build file, simulate the accountant
loading its vouchers, then verify both as `posted_verified`.

## Limits

- No live Tally or Windows/macOS installer test was run.
- Manual import remains required; Bridge does not send import XML.
- Masters must already exist. Exact spelling is mandatory.
- Education-mode date acceptance (days 1, 2, or 31 on the observed profile)
  remains an operator constraint because this MCP server does not infer the
  connected licence mode.
- Readback proves only the stored batch date window and fields requested by the
  verification collection; it is not a qualification of every Tally SKU or
  configuration.
