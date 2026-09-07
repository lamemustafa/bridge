# Goal 7 report — fourth-round review fixes

All nine accepted findings were addressed on `feat/agent-connector` with Rust
1.96.0. No sealed-surface path changed: `agent.rs`, `agent_import.rs`, and
`scripts/package-mcpb.mjs` have no entry in the compatibility surface, so the
Goal 3B reseal sequence was not applicable. Nothing was pushed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent_import.rs:347` | A pre-import voucher mark could be derived from a truncated voucher scan. | `66a8e48` | `company_high_water_mark_refuses_voucher_scan_shapes_and_preserves_attribution_boundary`; `simulator_build_then_manual_import_readback_verifies_every_voucher` | Build reads the company `ALTVCHID` and `ALTMSTID` high-water values, persists `company_high_water`, and refuses an unobserved mark. |
| `scripts/package-mcpb.mjs:9` | MCPB staging omitted license and third-party inventory resources. | `81eb647` | `MCPB stage verifier requires every license and inventory resource` | Staging copies `LICENSE`, `NOTICE`, JS and Rust inventories; verification names every missing resource. |
| `src-tauri/src/agent.rs:371` | The byte cap applied to the inner payload rather than the complete MCP/JSON-RPC response. | `ceb6714` | `response_byte_cap_covers_the_serialized_jsonrpc_result_and_keeps_content_short` | The envelope and JSON-RPC result are serialized and bounded; `content[0].text` is a short tool/company/rows/truncation/evidence summary. |
| `src-tauri/src/agent.rs:1006` | `ledger_movement` recorded voucher count instead of released ledger-row count. | `9866a1f` | `ledger_movement_receipt_counts_three_ledgers_from_two_vouchers` | Receipt rows are measured from the constructed ledger rows before response ownership moves. |
| `src-tauri/src/agent.rs:525` | Basic `ledger_masters` disclosed `party_gstin` / receipt fields were imprecise. | `6aa2ce7` | `ledger_master_release_shapes_keep_gstin_compliance_only` | GSTIN is emitted only in the compliance shape; both receipt shapes enumerate the exact released fields. |
| `src-tauri/src/agent.rs:932` | A supplied unknown movement ledger returned an empty successful result. | `7bdc148` | `ledger_lookup_normalizes_separators_and_refuses_absent_ledgers` | Case/separator-insensitive matching returns the live canonical name or typed `ledger_not_found`. |
| `src-tauri/src/agent.rs:1390` | Present but malformed row-pagination values were treated like defaults. | `7796064` | `pagination_rejects_present_invalid_values_in_helpers_and_row_tools` | Only absent values default; negative, fractional, and string `limit`/`offset`/`top` values return `pagination_invalid`. |
| `src-tauri/src/agent.rs:1169` | Egress-log paging read the complete log. | `18bc92b` | `egress_log_tail_reads_only_the_last_bounded_chunks` | Tail reads seek backward in 64 KiB chunks, scan at most 256 KiB, and retain the existing `max_rows` cap. |
| `src-tauri/src/agent_import.rs:927` | Unrelated duplicate records in the verification window could fail a valid batch. | `a028b34`, `a5a3a56` | `unrelated_window_duplicates_do_not_block_a_verified_batch`; `verification_reports_absence_divergence_and_duplicate_fingerprints` | Status considers only duplicate remote IDs/fingerprints touching batch-attributable rows; unrelated duplicates remain informational in `unrelated_duplicates_in_window`. |

## Thread replies

1. Pre-import attribution now uses the company high-water query, rather than a
potentially truncated voucher collection. Both company counters are required
and persist as `company_high_water`; a missing or malformed observation returns
`pre_import_mark_unobserved`. The regression proves a scan-shaped response
cannot create the mark and preserves the boundary for a pre-existing voucher.

2. MCPB assembly now stages `LICENSE`, `NOTICE`,
`THIRD_PARTY_LICENSES.txt`, and `THIRD_PARTY_LICENSES_RUST.txt` beside the
binary. The staging verifier checks the assembled directory and fails with the
specific missing resource; the regression exercises a missing Rust inventory
and then the complete set.

3. The response budget now covers the complete serialized MCP result and the
outer JSON-RPC response used by stdio. Structured payload is emitted once in
`structuredContent`; the content text is a bounded human summary containing
the tool, company, row count, truncation state, and evidence state. The
regression proves the final serialized JSON-RPC object fits its cap.

4. `ledger_movement` now counts `rows_returned` from the fully constructed
ledger-row vector before it is moved into the response. The receipt can no
longer report two voucher rows when three ledger rows were released; the
regression fixes that exact two-voucher/three-ledger case.

5. Basic ledger-master output now contains only name, parent, and opening
balance. `party_gstin` and compliance data are released exclusively by the
compliance branch, and `fields_returned` is derived from the exact selected
shape; both lists are asserted by regression coverage.

6. A caller-supplied ledger is resolved against the live ledger catalogue using
the existing case- and separator-insensitive key. A match releases the
canonical live name; no match returns `ledger_not_found` rather than a
misleading empty complete result. The regression covers both paths.

7. Pagination parsing now distinguishes omission from invalid input across the
shared helper. An omitted value keeps its documented default, while negative,
fractional, and string values fail closed as `pagination_invalid`; the test
also confirms the typed error through a row-bearing tool.

8. `egress_log` now seeks from the file end in 64 KiB chunks and never scans
more than the documented 256 KiB ceiling, while limiting returned rows to
`max_rows`. The regression writes a file larger than one chunk and proves the
last two lines are available without consuming the head.

9. Duplicate records are now classified relative to the batch-attributable
remote IDs and accounting fingerprints. Only those duplicates affect batch
status; unrelated window duplicates are retained in
`unrelated_duplicates_in_window`. The two regressions cover both a fully
verified batch beside unrelated duplicates and a divergent batch fingerprint
that must still block verification.

## Final verification

```text
$ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 454 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 126.91s

$ cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.36s

$ (cd tools && cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ corepack pnpm test
Test Files  2 passed (2)
Tests  6 passed (6)
[WebServer] [ERROR] This project is configured to use 11.7.0 of pnpm. Your current pnpm is v11.5.3
Error: Process from config.webServer was not able to start. Exit code: 1

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173
$ corepack pnpm exec playwright test
2 passed (1.1s)
```

`corepack pnpm test` completed its Node test portion (104 passed) and Vitest
portion (6 passed) but its Playwright-configured nested web-server invokes
Corepack's pnpm 11.5.3 while `package.json` requires 11.7.0. The package
metadata was not changed because that is outside these nine findings. No
`.fixture-integrity-*` directory was present or removed. The required direct
Vite fallback then ran the same two Playwright specs successfully.
