# Goal 10 report — seventh-round review fixes

All twelve accepted findings were addressed on `feat/agent-connector` using
Rust 1.96.0. Only unsealed agent, import, package, and documentation files
changed, so Goal 3B did not require a reseal. Nothing was pushed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:699` | Parsed vouchers could retain ignored ledger-filter rows. | `e8b3849` | `voucher_ledger_filter_drops_mixed_response_rows_that_do_not_match_live_spelling` | Resolves the live spelling, drops mismatches, and marks evidence partial as `filter_not_honoured`. |
| `src-tauri/src/agent.rs:876` | Change rows lacked required accounting state. | `69aadbc` | `changed_voucher_rows_require_typed_accounting_state` | Fetches and fails closed on typed `cancelled` and `optional`. |
| `src-tauri/src/agent.rs:1730` | Byte trimming advanced cursors from the wrong base. | `7147c53` | `byte_trimming_keeps_each_cursor_relative_to_its_requested_offset` | Items, bills, and unallocated pages retain the requested offset. |
| `src-tauri/src/agent.rs:2211` | XML entities were lost in voucher parsing. | `08847f5` | `voucher_parsers_decode_entities_in_vouchers_and_change_feeds` | Decodes and appends text/reference fragments fail-closed. |
| `src-tauri/src/agent.rs:1391` | Normalized ledger lookup could select an arbitrary collision. | `6d6409f` | `ledger_lookup_prefers_exact_live_spelling_and_rejects_ambiguous_matches` | Exact spelling wins; unresolved collisions return `ledger_ambiguous`. |
| `src-tauri/src/agent.rs:1207` | Movement rows had no page contract and escaped byte trimming. | `6e4c9bf` | `byte_trimming_keeps_each_cursor_relative_to_its_requested_offset` | Adds bounded offset/limit, truncation/cursor, and `ledgers` trimming. |
| `src-tauri/src/agent_import.rs:406` | Ledger JSONL appends could interleave. | `d352091` | `schema_balance_matcher_rendering_and_ledger_append_are_fail_closed` | Admission lock wraps preformatted single-line writes; concurrent appends parse. |
| `src-tauri/src/agent_import.rs:181` | Receipt count used catalogue size rather than returned masters. | `467b89b` | `schema_balance_matcher_rendering_and_ledger_append_are_fail_closed` | Egress count is `result.masters.len()`; changed-feed count remains combined. |
| `src-tauri/src/agent.rs:166` | Invalid Tally port silently defaulted. | `6e56ea9` | `tally_port_defaults_only_when_the_environment_value_is_absent` | Only an absent setting defaults; invalid values return `port_setting_invalid`. |
| `scripts/package-mcpb.mjs:40` | MCPB used debug binary output. | `1a96cbb` | `MCPB packaging reads the release binary` | Builds `--release` and copies `target/release`. |
| `src-tauri/src/agent.rs:1069` | Movement evidence omitted financial read bytes. | `23e7d1f` | `ledger_movement_evidence_changes_when_a_voucher_response_changes` | Catalogue, pre-window, and window read evidence are combined. |
| `docs/agent/README.md:11` | Toolchain documentation was host-specific. | `c599c6c` | `rg` absence check | Uses `rustup run 1.96.0` in README and affected briefs. |

## Thread replies

1. Voucher ledger filters are now verified after parsing against the resolved live ledger spelling; any rows Tally returns outside that filter are removed and make the result partial with `filter_not_honoured`.

2. The changed-voucher profile now requests `ISCANCELLED` and `ISOPTIONAL`; both appear as typed fields on every row or the entire scan fails closed.

3. Byte-cap pagination now computes each next cursor from the requested page offset and surviving count, including unallocated parties.

4. Voucher text and entity reference fragments are decoded and appended consistently for ordinary and changed reads; malformed decoding is an in-band parser failure.

5. Ledger lookup no longer chooses the first normalized collision: exact live spelling is preferred, otherwise ambiguous candidates return `ledger_ambiguous`.

6. Ledger movement now has a bounded page shape and the generic trimmer can remove `result.ledgers` while maintaining a valid cursor.

7. Every import-ledger append acquires the admission lock and emits one already-serialized JSONL line; the concurrent regression confirms parseable output.

8. `validate_masters` egress receipts now state the actual number of returned master results, rather than the larger catalogue consulted to create them.

9. A malformed explicit port is rejected at startup; only an absent environment variable uses port 9000.

10. MCPB assembly now builds and stages the release binary, and its test asserts the release path.

11. Ledger movement now performs its catalogue and voucher reads through the existing identity-safe agent adapter, so all raw financial request/response hashes and byte counts are combined into the returned evidence.

12. Agent invocation instructions no longer encode an ARM64 host triple and instead use the portable Rustup toolchain selector.

## Final verification

```text
$ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 477 passed; 0 failed

$ cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.79s

$ (cd tools && cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
pass 105

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173
$ env -u CI corepack pnpm exec playwright test
2 passed (889ms)

$ PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH" node scripts/package-mcpb.mjs
Prepared packaging/mcpb/bin/bridge_mcp and verified required license resources for local .mcpb assembly; do not commit this host binary.
```
