# Goal 9 report — sixth-round review fixes

All six accepted items were addressed on `feat/agent-connector` with Rust
1.96.0. The changes are confined to unsealed `agent.rs` and `agent_import.rs`,
so Goal 3B did not authorize or require a reseal. Nothing was pushed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:41` | Key-based masking could omit party-bearing response fields. | `c04f25c` | `mask_parties_walks_every_tool_sample_response_without_leaking_party_names` | `PartyName` markers make the central redactor materialize or mask marked values independently of JSON key names; every tool sample is walked under `mask_parties`. |
| `src-tauri/src/agent_import.rs:866` | A narration tag could attribute a voucher that predated its import mark. | `7a7d0ba` | `narration_tag_verification_requires_a_post_mark_voucher` | Tagged vouchers now require `ALTERID >` the persisted company high-water mark; older matches are `not_attributable` with `tag_precedes_pre_import_voucher_mark`. |
| `src-tauri/src/agent.rs:1015` | Missing ledger openings could become fabricated zero openings after BOOKSFROM. | `a8e5a75` | `ledger_movement_marks_an_unobserved_post_books_opening_partial` | The typed movement row preserves `opening` and `closing` as null and marks both row and tool evidence partial with `opening_balance_not_observed`. |
| `src-tauri/src/agent.rs:2280` | Missing or non-string `params.name` could exit the stdio server. | `4ff0356` | `malformed_tool_name_is_in_band_and_the_same_session_serves_the_next_request` | The stdio loop now converts the malformed tool call to JSON-RPC `-32602` and serves the next request in that same session. |
| `src-tauri/src/agent.rs:910` | `direction` only filtered open bills, leaving other outstandings views combined. | `031451a` | `payable_outstandings_views_exclude_mixed_receivable_rows` | Decision: recompute selected-side totals, ageing buckets, ranking, bills, and unallocated aggregate from typed bill/residual directions; bill ages are available, so `ageing_buckets` remains meaningful rather than omitted. |
| `src-tauri/src/agent.rs:1582` | Empty voucher windows lacked a meaningful corroboration control. | `8077b5d` | `empty_voucher_window_corroboration_handles_all_three_control_branches` | A widened control outside the original window completes; a row inside it returns `window_contradicted`; a second empty read consults high-water and distinguishes no vouchers from an uncorroborated partial. |

## Thread replies

1. Party redaction no longer depends on a growing key allowlist. Party-bearing fields are explicitly represented by an internal `PartyName` marker, which the central redactor masks under `mask_parties` regardless of where the marker appears. The regression walks all thirteen tool response samples, including compliance names, master candidates, bills, unallocated parties, and voucher-entry ledgers, and verifies no known party text survives.

2. Narration tags now have the same temporal attribution boundary as fingerprints. A matching `[BRIDGE:<txn>]` voucher must have an `ALTERID` strictly above the persisted company high-water voucher mark; a tagged voucher at or below that mark is reported as `not_attributable` with `tag_precedes_pre_import_voucher_mark`, while a later tagged voucher still verifies normally.

3. Ledger movement no longer substitutes a numeric zero for an absent observed opening when the requested window begins after BOOKSFROM. The affected ledger row returns null opening and closing values with its own partial state and stable reason, and the enclosing tool evidence is also partial so callers cannot mistake the result for a complete movement calculation.

4. A malformed `tools/call` name is now an in-band JSON-RPC invalid-params response (`-32602`) rather than an error propagated out of `run_stdio`. The regression sends a malformed request and then `ping` through one duplex session, proving the process remains available for the following request.

5. Direction now applies consistently to every outstandings view. The implementation filters typed bill and residual rows first, then recalculates totals, party ranking, bills, unallocated count/amount, and ageing buckets from the selected side. Because bill ages are present at this boundary, the documented decision is to provide direction-specific ageing buckets rather than emit the not-available fallback.

6. Empty voucher windows now require a discriminating control. A widened response may corroborate the original empty window only when it has rows exclusively outside that window; any in-window row contradicts it. If the widened control is also empty, the company high-water separates a genuinely voucherless company (`company_has_no_vouchers`) from a partial `empty_uncorroborated` result.

## Final verification

```text
$ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 471 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 124.44s

$ cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.87s

$ (cd tools && cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
pass 104

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173
$ corepack pnpm exec playwright test
2 passed (1.3s)
```
