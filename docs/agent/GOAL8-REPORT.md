# Goal 8 report — fifth-round review fixes

All thirteen accepted items were addressed on `feat/agent-connector` with Rust
1.96.0. Item 6 changed sealed `src-tauri/src/tally/runtime.rs` under Goal 3B
authority; `rehash-surface` reported one changed entry, followed by
`seal-surface` and `repoint-matrix`. Nothing was pushed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent_import.rs:865` | Verified imports did not prove accounting effect. | `83609d8` | `verified_import_vouchers_require_observed_effective_accounting_flags` | Readback fetches both flags; only explicit `No`/`No` verifies. |
| `src-tauri/src/agent.rs:923` | Movement could read before BOOKSFROM. | `ea842d0` | `ledger_movement_rejects_a_window_before_the_observed_books_from` | Returns `window_precedes_books_from`. |
| `src-tauri/src/agent.rs:386` | Egress could retain the pre-cap row count. | `20e2bf5` | `response_byte_cap_covers_the_serialized_jsonrpc_result_and_keeps_content_short` | Cap helpers return surviving rows. |
| `src-tauri/src/agent.rs:867` | Unallocated parties were unbounded. | `f53c425` | `unallocated_parties_are_bounded_without_dropping_the_aggregate` | Aggregate remains while parties page and trim independently. |
| `src-tauri/src/agent.rs:1226` | `top` could not exceed protocol summary’s ten parties. | `606643a` | `outstandings_top_ranking_uses_all_open_bills_not_the_report_cap` | Decision: rank uncapped `statement_open_bills`, which already exposes party, direction, amount, and age. |
| `src-tauri/src/tally/runtime.rs:1444` | Company evidence hashed reserialized parsed data. | `a9731bd`, `6425be4` | `agent_company_list_evidence_hashes_the_exact_raw_response_bytes` | Adapter returns raw byte count/SHA; sealed surface resealed. |
| `src-tauri/src/agent.rs:1519` | Non-string optional filters widened reads. | `e5bee0e` | `optional_filters_reject_non_string_values_before_widening_a_read` | Returns `argument_invalid:<name>`. |
| `src-tauri/src/agent.rs:116` | Invalid configured limits silently defaulted. | `f0620a6` | `configured_agent_limits_reject_malformed_or_out_of_range_values` | Defaults apply only when absent. |
| `src-tauri/src/agent.rs:1101` | Egress lines could interleave across processes. | `534d82e` | `concurrent_egress_appends_leave_two_parseable_json_lines` | One preformatted line is locked and written atomically. |
| `src-tauri/src/agent.rs:1860` | Pipe-delimited temporary voucher entries corrupted ledger names. | `4ac6a1c` | `voucher_parser_keeps_pipe_characters_inside_structured_ledger_names` | Entries remain structured while parsing. |
| `src-tauri/src/agent.rs:374` | Stored evidence lacked per-read timing. | `dbbbda4` | `stored_evidence_records_have_individual_timestamps_and_durations` | Each record receives RFC 3339 `read_at` and `duration_ms`. |
| `src-tauri/src/agent.rs:155` | IPv6 status endpoint lacked brackets. | `21a7fb3` | `status_endpoint_uses_the_transport_canonical_loopback_origin` | Uses transport canonical origin for IPv4 and IPv6. |
| `.gitignore:1` | Local MCPB staging dirtied the tree. | `8d57541` | `node scripts/package-mcpb.mjs` then clean `git status` | Binary and staged resources are ignored and documented. |

## Thread replies

1. Import verification now fetches `ISCANCELLED` and `ISOPTIONAL`; only two explicit `No` values produce `posted_verified`, either `Yes` becomes `posted_not_effective`, and an absent flag fails closed.

2. Movement rejects a requested start before the observed BOOKSFROM boundary with `window_precedes_books_from` before any ledger or voucher result is released.

3. Egress now receives the post-cap surviving row count, not the original requested count.

4. Unallocated totals and count are retained while party rows are offset/limit bounded with their own truncation cursor; byte trimming preserves the aggregate.

5. The agent now ranks from uncapped bill rows rather than the protocol report’s deliberately capped `top_parties`, so `top: 12` returns twelve parties within `max_rows`.

6. The sealed runtime adapter now hashes and sizes the exact CompanyListV2 bytes before parsing; the one changed sealed hash was rehashed, resealed, and repointed.

7. Optional strings and enum inputs fail closed when present with a non-string value, preventing an accidental broad read.

8. Configured row and byte limits default only when absent; malformed or out-of-range settings stop startup with the stable limit error.

9. Egress serializes first, then holds the advisory lock for one complete append and sync, preventing mixed JSONL lines.

10. Voucher parser entries are accumulated as JSON objects, preserving ledger names such as `A|B`.

11. Evidence records now carry their individual completion timestamp and observed call duration.

12. Status uses the transport formatter, producing `http://[::1]:9000` for IPv6 loopback.

13. MCPB binary and staged resource outputs are ignored and README-documented; an end-to-end staging run left Git clean.

## Final verification

```text
$ cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 465 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 121.04s

$ cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.19s

$ (cd tools && cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
pass 104

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Tests  6 passed (6)

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173
$ corepack pnpm exec playwright test
2 passed (1.3s)

$ node scripts/package-mcpb.mjs
Prepared packaging/mcpb/bin/bridge_mcp and verified required license resources for local .mcpb assembly; do not commit this host binary.

$ git status --short
```
