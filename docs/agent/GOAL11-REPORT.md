# Goal 11 report — eighth-round review fixes

All eleven accepted findings were addressed on `feat/agent-connector` with
Rust 1.96.0. No finding was closed as won't-fix and nothing was pushed. The
outstandings evidence adapter changed the Goal 3B-authorised sealed file
`src-tauri/src/tally/runtime.rs`; the documented `rehash-surface`,
`seal-surface`, and `repoint-matrix` sequence changed its entry hash from
`35b8823f635e12fbf4588df1399fc22af264b18146ee78b1cfa2686592b79f15` to
`0b044830c6cf5bd383769ab4d6865d8f37e6579f9fbea3a468760326359077ab`, and
the manifest/matrix hash from
`1c8c527c4f5375d23377f843f5db3031c840f5f53f3bae916387bbf8b1c53fdf` to
`e80a7d73f59d408ad18e548f9a079f1690893b3fd71a9b9cb27d75fd35a280c2`.
The matrix retains 11 unknown claims and makes no new public compatibility
claim.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:1588` | Future-due open bills were omitted from ageing buckets. | `aee4157` | `future_due_open_bills_remain_in_the_first_ageing_bucket` | Nullable per-bill age remains visible; the amount contributes to `days_0_30`. |
| `src-tauri/src/agent.rs:2285` | Changed masters without a usable name could advance a cursor. | `47db3c4` | `change_scan_rejects_any_missing_or_malformed_row_alter_id` | Empty or missing names fail closed as `change_row_name_invalid` before cursor construction. |
| `src-tauri/src/agent_import.rs:280` | Import verification trusted one potentially incomplete window read. | `af96573`, `fdb64b4` | `verification_window_corroboration_rejects_each_unsafe_branch`; `simulator_build_then_manual_import_readback_verifies_every_voucher` | Enforces date bounds, identical GUID/ALTERID sets across two reads, and no max-row/truncation boundary. |
| `src-tauri/src/agent.rs:1812` | Byte trimming did not preserve changed-feed cursor semantics. | `8c0ac06` | `byte_trimming_change_feed_rows_stops_checkpoint_advancement` | Trims the larger feed axis, retains its surviving max cursor, and disables checkpoint advancement. |
| `src-tauri/src/agent.rs:975`; `src-tauri/src/tally/runtime.rs:61` | Outstandings evidence omitted financial read wire evidence. | `25287f5` | `outstandings_wire_evidence_changes_with_any_native_report_response` | Currency and every paired native report provide exact request hash, response hash, and paired byte count to returned evidence. |
| `src-tauri/src/agent.rs:2285` | Changed-master XML entity fragments were not decoded safely. | `9539f10` | `changed_master_parser_decodes_entity_fragments` | Decodes and appends name/parent fragments, including `R&amp;D`; malformed decoding fails closed. |
| `src-tauri/src/agent.rs:875`; `docs/agent/README.md:49` | Master snapshot high-water covered types outside the scanned domain. | `95f3a0b` | `master_domain_high_water_ignores_unsupported_master_types` | Snapshot query is ALTERID-only for Ledger and Group; README explicitly bounds master change detection to those types. |
| `src-tauri/src/agent.rs:2010` | Explicit zero `limit` and `top` values bypassed pagination validation. | `bb08d8e` | `pagination_rejects_present_invalid_values_in_helpers_and_row_tools` | Present zero values now return `pagination_invalid`; offsets remain independently valid at zero. |
| `src-tauri/src/agent_import.rs:255` | A failed import-ledger append could leave the XML behind. | `ad6dee0`, `e420e99` | `unwritable_ledger_path_removes_the_written_import_file` | A real directory-at-ledger-path failure returns the append error after deleting the written XML; cleanup failure names the orphan path. |
| `src-tauri/src/agent_import.rs:454` | Batch GUID comparison was presentation-sensitive. | `2f83f70` | `batch_guid_is_canonicalized_and_compared_case_insensitively` | Persists lowercase GUIDs and accepts mixed-case verification input. |
| `src-tauri/src/agent.rs:1674` | Egress-tail readers could observe a concurrent partial append. | `a4aa93c` | `egress_tail_waits_for_an_exclusive_append_lock` | Reader holds a shared advisory lock through the tail read; regression waits on an exclusive writer lock. |

## Thread replies

1. Future-due bills remain open exposure, so their nullable age is now placed in the first ageing bucket; the regression confirms bucket totals still reconcile with all open-bill totals.

2. The changed-master parser now rejects missing or whitespace-only names before it builds any row or checkpoint, returning `change_row_name_invalid` rather than advancing on an unnamed master.

3. Verification now performs a second identical window read and certifies only if dates are in range, GUID/ALTERID pairs are stable, and neither response reaches a configured truncation boundary; the simulator additionally models all fourteen identity-bracketed requests.

4. The generic byte-cap trimmer now treats changed vouchers and masters as independent cursor axes, removes from the larger array, and makes a truncated feed explicitly non-advanceable.

5. The sealed runtime adapter exposes actual paired-read wire facts. The agent combines currency and outstandings request hashes, response hashes, and byte counts instead of inventing a synthetic financial evidence record.

6. Changed-master `NAME` and `PARENT` text uses the same fail-closed entity decoding and append discipline as the earlier voucher parser correction, including `R&amp;D` fragments.

7. The master cursor is now deliberately domain-matched: only Ledger and Group ALTERIDs establish its high-water, and unsupported master changes therefore cannot stall the feed. The README states that scope.

8. Pagination validation now rejects explicit zero values for both `limit` and `top` at the boundary with `pagination_invalid`, matching the schemas.

9. When the JSONL ledger cannot be opened after XML creation, cleanup removes the XML and preserves the original ledger error. The regression creates a directory at the ledger path to exercise that real failure mode.

10. Import batches persist a lowercase canonical company GUID and verify it case-insensitively, matching the identity boundary.

11. Egress-log tails acquire a shared lock, so a concurrent exclusive append cannot expose a partial JSONL line to readers.

## Final verification

```text
$ rustup run 1.96.0 rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast -q
test result: ok. 486 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 132.36s

$ rustup run 1.96.0 cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.35s

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
ℹ pass 105
ℹ fail 0

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)

$ env -u CI corepack pnpm exec playwright test  # direct Vite server at 127.0.0.1:4173
2 passed (1.0s)
```
