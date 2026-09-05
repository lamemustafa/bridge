# Goal 14 report — rounds 11–13 review fixes

All nineteen accepted findings were remediated on `feat/agent-connector` using
Rust 1.96.0. Each behavioural fix has a dedicated regression test and commit;
no finding was closed as won't-fix and nothing was pushed. Finding 18 changed
the Goal 3B-authorised sealed file `src-tauri/src/tally/runtime.rs`. The
documented `rehash-surface` → `seal-surface` → `repoint-matrix` sequence
reported one changed sealed file, changed that entry from
`191251c22653414bbdfa94e94835c521dacc0d4276e79f0b1ddba6140277ffff` to
`1f67deec68dca59ad3776c83f8718aec3980b9f1eb7ee332c326c41fda3b5a8e`, and
changed the surface/matrix hash from
`c0c58373e11e2d65da6dd764800ccf36e0492a97af9cef19c61aa402d2e6d749` to
`8ed43eb8cd21c7156197c96a02cdda46d09f4d0ed82256032e0142f451609f22`.
The matrix remains at 11 unknown claims and makes no new public compatibility
claim.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:1328` | Ledger-movement exports could include an out-of-window voucher. | `373e690` | `ledger_movement_rejects_an_out_of_range_voucher_before_aggregation` | Uses the literal-filter voucher renderer and rejects any returned outlier as `window_not_honoured` before aggregation. |
| `src-tauri/src/agent.rs:2621` | Voucher and changed-feed entries could omit a required ledger field. | `6fb902b` | `voucher_and_changed_parsers_reject_incomplete_ledger_entries` | Missing `LEDGERNAME`, `AMOUNT`, or `ISDEEMEDPOSITIVE` now fails the whole parse as `agent_read_protocol_invalid`. |
| `src-tauri/src/agent_import.rs:878` | Import read-back could accept an incomplete ledger entry. | `0461e91` | `verification_rejects_incomplete_ledger_entries` | The verification parser rejects every incomplete required-field combination as `import_verification_export_invalid`. |
| `src-tauri/src/agent_import.rs:583` | User text could forge the reserved batch-marker namespace. | `559f910` | `narration_and_reference_reject_reserved_markers_after_entity_decoding` | Narration and reference are entity-decoded then reject case-insensitive `[BRIDGE:` with `narration_reserved_marker`. |
| `src-tauri/src/agent.rs:1405` | Undeclared tool arguments could reach dispatch. | `7303ffc` | `tool_arguments_reject_unknown_keys_before_tool_dispatch` | The selected tool's allowlist is enforced before dispatch; `ledgre` returns `argument_unknown:ledgre`. |
| `src-tauri/src/agent.rs:2343` | Voucher entity text lost field-adjacent whitespace. | `2d7784d` | `voucher_parsers_preserve_entity_adjacent_whitespace` | Voucher and change-feed parsers use `trim_text(false)` while ignoring structural whitespace. |
| `src-tauri/src/agent.rs:2501` | Changed masters lacked a required stable identity. | `de7d59e` | `changed_master_rows_require_guid_or_master_id` | The render fetches GUID and MASTERID and parsing rejects a blank pair as `change_row_identity_invalid`. |
| `scripts/package-mcpb.mjs:68` | A staged MCPB manifest could advertise binaries not present in the archive. | `9b2e4ef` | `MCPB verifier rejects a manifest platform without its staged binary` | Staging writes a host-only manifest and target-triple binary path; its verifier refuses every advertised missing binary. |
| `src-tauri/src/agent.rs:1646` | A movement entry absent from the initial ledger snapshot could be silently skipped. | `49e1dc8` | `ledger_movement_refuses_snapshot_drift_except_unselected_entries` | It now fails closed as `ledger_snapshot_drifted`, except an entry outside an explicit selector. |
| `src-tauri/src/agent_import.rs:488` | A failed import-ledger append could leave a partial line. | `a86d5d4` | `import_ledger_append_rolls_back_a_partial_failing_write` | The admitted writer restores its original length after write or sync failure. |
| `src-tauri/src/agent.rs:1975` | Response trimming could corrupt non-change-feed results. | `135f4fd` | `only_change_feed_responses_may_trim_cursor_rows` | Only cursor-bearing change feeds trim; oversized verification and master-validation responses fail as `agent_response_too_large`. |
| `src-tauri/src/agent.rs:1457` | Unknown `fields` values silently selected the basic ledger view. | `1a4ca9b` | `ledger_master_fields_reject_unknown_schema_values` | Values other than `basic` or `compliance` return `argument_invalid:fields`. |
| `src-tauri/src/agent.rs:2757` | `ledger_movement` omitted pagination from its declared schema. | `782db5b` | `ledger_movement_schema_exposes_offset_and_limit` | The public input schema declares validated `offset` and `limit`. |
| `src-tauri/src/agent.rs:2822` | A `tools/call` notification could execute without a reply or receipt. | `dd86fd3` | `tools_call_notifications_are_refused_and_receipted_without_dispatch` | No-id calls are receipted refusals before dispatch and the following ping remains usable. |
| `src-tauri/src/agent_import.rs:419` | External import-ledger reads could race an admitted append. | `cb86c64` | `external_import_ledger_read_waits_for_the_append_admission_lock` | External reads acquire the shared admission lock; the already-admitted helper remains internal. |
| `src-tauri/src/agent.rs:2288` | Ledger names were passed into an unverified TDL filter. | `afee0b8` | `client_side_ledger_filter_accepts_unquoted_tdl_ledger_names` | Voucher TDL is date-only; parsing filters against the resolved live ledger spelling, including `%`, formula-like, and non-ASCII names. |
| `src-tauri/src/agent.rs:2879` | A post-build receipt failure discarded the built batch response. | `72e4885` | `built_batch_stays_in_band_when_its_egress_receipt_fails` | Build results retain `batch_id` and report `egress_recorded:false` in-band; non-build failures remain fail-closed. |
| `src-tauri/src/tally/runtime.rs:1525` | Company-list evidence hashed decoded rather than transport-encoded bytes. | `bf74bdd` | `agent_company_list_evidence_hashes_the_encoded_utf16_response_bytes` | The paired transport result supplies `encoded_bytes` and `encoded_sha256`; the UTF-16 fixture proves the recorded commitment is encoded-byte based. |
| `src-tauri/src/agent.rs:1572` | Changed-since egress evidence did not enumerate every released field. | `71c315e` | `changed_since_egress_fields_cover_every_released_voucher_and_master_field` | Receipt metadata now declares all released voucher and master fields, including identifiers and accounting state. |

## Thread replies

1. Ledger movement now reads vouchers through the date-filter profile used by `vouchers`, validates each observed voucher date before aggregation, and performs the existing wider-window corroboration. The regression supplies an out-of-range voucher and proves it cannot produce a movement result.

2. Both the ordinary voucher and changed-feed parsers now treat each ledger entry as an all-or-nothing protocol record. The regression removes `LEDGERNAME`, `AMOUNT`, and `ISDEEMEDPOSITIVE` one at a time and gets the typed `agent_read_protocol_invalid` error in both paths.

3. Import verification now applies the same complete-entry rule before it can fingerprint or mark a voucher verified. Its regression covers each missing required field and observes `import_verification_export_invalid`.

4. Payload validation decodes entities before examining narration and reference text, then reserves the `[BRIDGE:` namespace case-insensitively. The regression covers both literal and numeric-entity spellings in both user-controlled fields.

5. Tool argument validation now runs before dispatch against the selected schema's declared property names. The `ledgre` regression confirms the request is rejected as `argument_unknown:ledgre` instead of widening or reaching a read.

6. Voucher and changed-feed XML readers now preserve entity-adjacent field whitespace with `trim_text(false)`, while parser structure still ignores non-field whitespace. The regression verifies the exact leading and trailing text survives both read paths.

7. Changed master rendering now requests GUID and MASTERID, and parsing requires at least one non-blank stable identifier. The regression demonstrates that an identity-free row is rejected and that the request fetch list contains both identifiers.

8. MCPB packaging now stages a generated host-only manifest and a binary under its target triple; the committed multi-platform manifest remains a template. The verifier regression adds an advertised Windows entry without its binary and confirms staging is refused.

9. Movement calculation no longer treats a ledger absent from the snapshot as zero. It fails closed with `ledger_snapshot_drifted`, retaining only the explicitly selected-other-ledger exception demonstrated in the regression.

10. Import-ledger writes record the original length and truncate back to it if either append or sync fails under admission. The failing-writer regression proves a partially written byte is removed and the pre-existing line remains intact.

11. Cursor trimming is now restricted to results carrying the change-feed cursor contract. Oversized `verify_import` and `validate_masters` shapes are refused rather than silently truncated, as the regression asserts.

12. Ledger-master field selection now has only two admitted values, `basic` and `compliance`. The typo `complaince` returns the explicit `argument_invalid:fields` error.

13. `ledger_movement` now publicly declares `offset` and `limit` with their bounds, aligning the schema with the implementation. The schema regression asserts both properties and minima.

14. No-id `tools/call` messages are now refused, logged through the receipt path, and never dispatched. The async regression sends a notification followed by ping and proves the refusal receipt is present and the server stays usable.

15. External import-ledger reads now share the admission lock with appends, while the internal already-admitted path avoids recursive locking. The concurrency regression blocks the reader until the append admission is released.

16. Ledger selection is now entirely client-side after parsing against the resolved live spelling; the voucher TDL has only a date filter. The regression covers `Input CGST 9%`, `=BVL Zeta Formula`, and `खर्चा`, proving none enters TDL and each remains selectable.

17. When egress receipt recording fails after a build has completed, the JSON-RPC response retains the built batch id and adds `egress_recorded:false` as an in-band error. The regression verifies that transformation and confirms it does not apply to a response without a build batch.

18. The company adapter now retains the paired transport's encoded bytes and SHA-256 instead of reconstructing a commitment from decoded XML. The authorised sealed runtime change was resealed, and the UTF-16 regression proves the recorded hash is over the encoded response bytes.

19. `changed_since` evidence now declares every voucher and master field released by the result, including both identity families and cancelled/optional state. The exact-list regression prevents a future release from silently omitting receipt metadata.

## Final verification

```text
$ rustup run 1.96.0 rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 510 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 130.80s
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.12s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ rustup run 1.96.0 cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
exit 0

$ rustup run 1.96.0 cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.76s

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
ℹ pass 106
ℹ fail 0
ℹ duration_ms 1252.861625

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)
Duration  773ms

$ corepack pnpm exec playwright test  # direct Vite server at 127.0.0.1:4173
2 passed (916ms)

$ PATH="/Users/tapishkhandelwal/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH" node scripts/package-mcpb.mjs
Prepared /Users/tapishkhandelwal/Desktop/dev/worktrees/bridge-agent-connector/packaging/mcpb/stage/bin/aarch64-apple-darwin/bridge_mcp with a darwin-arm64-only manifest and verified its staged resources; do not commit host artifacts.
```
