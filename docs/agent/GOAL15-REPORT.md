# Goal 15 report — round 14 review fixes

All nine accepted findings were remediated on `feat/agent-connector` with
Rust 1.96.0. Each behavioural fix has one dedicated regression test and a
separate local commit; no finding was closed as won't-fix and nothing was
pushed. Finding 6 changed the Goal 3B-authorised sealed file
`src-tauri/src/tally/runtime.rs`. The documented `rehash-surface` →
`seal-surface` → `repoint-matrix` sequence reported one changed sealed entry,
changed the runtime hash from
`1f67deec68dca59ad3776c83f8718aec3980b9f1eb7ee332c326c41fda3b5a8e` to
`a016d922c472cec43d6a0950b4e22cdfa09d6436011bdbaefa13fccb9a54c5e1`, and
changed the manifest/matrix hash from
`8ed43eb8cd21c7156197c96a02cdda46d09f4d0ed82256032e0142f451609f22` to
`97f4094c2a86bf3208fe39d4c3f9a9f58638ef7320c0c72e96c693ebda8d3f96`.
The matrix retains 11 unknown claims and makes no new public compatibility
claim.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent_import.rs:1015` | Fingerprint fallback reused one observed voucher for multiple expected vouchers. | `93edf93` | `fingerprint_fallback_consumes_an_observed_voucher_once_per_batch` | Fallback candidates are consumed by observed-row index; identical expected fingerprints are exposed as `ambiguous_within_batch`. |
| `src-tauri/src/agent.rs:1622` | Post-resolution voucher ledger filtering still used a lossy lookup key. | `720d345` | `resolved_ledger_filter_keeps_only_the_exact_live_spelling` | Resolution remains tolerant, but the selected live spelling is matched by exact string equality. |
| `src-tauri/src/agent.rs:1639` | Voucher window validation ran after ledger selection. | `612a83b` | `voucher_window_is_validated_before_the_ledger_selector` | The original and widened reads validate every parsed row before applying an optional ledger filter. |
| `src-tauri/src/agent.rs:2647` | Changed voucher rows could omit DATE or VOUCHERTYPENAME. | `bc9b46e` | `changed_voucher_rows_require_date_and_voucher_type` | Both non-empty core fields are required before identity/cursor processing; failure is `change_row_core_field_invalid`. |
| `src-tauri/src/agent.rs:1415` | `ledger_movement` schema pagination was not accepted by dispatch. | `82591ba` | `every_declared_tool_property_is_allowed_by_dispatch` | `offset`/`limit` are admitted, and the test derives every declared property from `tool_definitions` to prevent future drift. |
| `src-tauri/src/agent.rs:310`; `src-tauri/src/tally/runtime.rs:1848` | Custom agent reads recorded decoded-text evidence. | `ee4a799` | `voucher_read_evidence_uses_utf16_transport_bytes` | `post_read` uses stable paired transport bytes/SHA; the UTF-16 `vouchers` test exercises the full identity and custom-read path. |
| `src-tauri/src/agent.rs:2286` | Voucher company names were unnecessarily restricted as TDL literals. | `85f94b1` | `voucher_company_name_is_validated_and_xml_escaped_without_a_tdl_literal` | `ValidatedCompanyName` plus XML escaping protects `SVCURRENTCOMPANY`; no `TdlStringValue` remains because no user value enters a formula. |
| `src-tauri/src/agent.rs:2075` | Outstandings receipts omitted unallocated-party rows. | `44de0ff` | `outstandings_receipt_counts_wholly_unallocated_party_rows` | Receipt rows count `open_bills` plus `unallocated.parties`; derived `top_parties` remain a summary, not a paged row collection. |
| `src-tauri/src/agent_import.rs:973` | Verification treated any free-text `TRUNCAT` substring as structural truncation. | `dd2a516` | `verification_narration_with_truncated_text_is_not_a_completeness_marker` | The structural-marker decision is documented below; user narration no longer affects completeness. |

## Structural completeness decision

No structural completeness marker currently exists in the parsed verification
envelope or paired transport result: neither exposes a trusted total/count or
completion-status element for the voucher collection. Verification therefore
uses only a returned row count strictly below `max_rows` and the identical
GUID/ALTERID set from the second paired read. It deliberately does not infer
truncation from user-controlled text such as narration.

## Thread replies

1. Fingerprint fallback now consumes an observed voucher as soon as it is attributed, so a single posted voucher cannot satisfy two otherwise identical expected vouchers. The proof marks each affected expected-voucher row with `ambiguous_within_batch`; the regression produces one verified and one not-found result from one observed voucher.

2. Ledger-name normalization is now limited to resolving the requested name against the live ledger catalogue. Once resolved, filtering uses exact equality, so a request for `AB` cannot return an `A-B` entry.

3. The voucher-window check now runs over the complete parsed response before any ledger selector can remove rows, including the wider corroboration request. An out-of-window entry for another ledger now returns `window_not_honoured` rather than being hidden by the selector.

4. Changed-feed parsing now requires non-empty DATE and VOUCHERTYPENAME before building any cursor-relevant row. The regression removes each field independently and gets `change_row_core_field_invalid`.

5. `ledger_movement` now admits its declared `offset` and `limit` properties at dispatch. The regression iterates `tool_definitions` and asserts every declared property is present in that tool's dispatch allowlist, preventing the schema and implementation from diverging again.

6. Custom agent reads now use the stable paired transport result and carry its encoded byte count and SHA-256 through `post_read`; decoded XML is used only for parsing. The authorised runtime change was resealed, and the UTF-16 voucher-path regression verifies encoded-byte accounting end to end.

7. `SVCURRENTCOMPANY` is now treated as an XML element value, not a formula operand. It accepts a validated comma/plus/non-ASCII name, XML-escapes it at rendering, and rejects control-character-invalid names through `ValidatedCompanyName`.

8. Outstandings receipt row counts now include every released paged row collection: open bills and unallocated parties. `top_parties` is a derived ranking summary, so it is documented as intentionally excluded; a wholly unallocated response now reports a non-zero row count.

9. The whole-document `TRUNCAT` heuristic is removed. There is no structural collection-completeness marker available today, so completeness remains the bounded row count plus stable identical GUID/ALTERID set across the paired read; a narration containing “truncated payment” no longer blocks verification.

## Final verification

```text
$ rustup run 1.96.0 rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ rustup run 1.96.0 cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
exit 0

$ rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 519 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 125.69s
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.12s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ rustup run 1.96.0 cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.74s

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
ℹ pass 106
ℹ fail 0
ℹ duration_ms 859.562208

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)
Duration  697ms

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173  # started before browser gate
VITE v8.2.2  ready in 171 ms

$ corepack pnpm exec playwright test
2 passed (1.5s)
```
