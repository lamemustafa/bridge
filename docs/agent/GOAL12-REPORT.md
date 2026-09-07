# Goal 12 report — ninth-round review fixes

All six accepted findings were addressed on `feat/agent-connector` with Rust
1.96.0. No finding was closed as won't-fix and nothing was pushed. The
ledger-export evidence adapter changed the Goal 3B-authorised sealed file
`src-tauri/src/tally/runtime.rs`; the documented `rehash-surface`,
`seal-surface`, and `repoint-matrix` sequence changed its entry hash from
`0b044830c6cf5bd383769ab4d6865d8f37e6579f9fbea3a468760326359077ab` to
`9bf70e64f98ce139c13d6914b78d4999b4d569cbacaf6b5c73fb44a990b05a6a`, and
the manifest/matrix hash from
`e80a7d73f59d408ad18e548f9a079f1690893b3fd71a9b9cb27d75fd35a280c2` to
`b1784781f8944b711a1d710100411142c8fcaaf8670d0b51af5e8a967d8af1ea`.
The matrix retains 11 unknown claims and makes no new public compatibility
claim.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:2366` | Changed-master entity-adjacent whitespace was removed. | `6d70f07` | `changed_master_parser_decodes_entity_fragments` | The parser retains meaningful decoded spaces with `trim_text(false)` while whitespace-only tags still fail validation; high-water and envelope validation are unchanged. |
| `src-tauri/src/agent.rs:1311`; `src-tauri/src/tally/runtime.rs:1555`; `docs/agent/README.md:54` | Ledger movement could obtain `OPENINGBALANCE` from Tally's display period. | `12b4722` | `ledger_movement_opening_export_is_pinned_to_admitted_books_from` | The native ledger export pins `SVFROMDATE=BOOKSFROM`; the endpoint profile admits that boundary before dispatch and rejects a mismatch as `opening_period_not_honoured`. |
| `src-tauri/src/agent.rs:2535` | Changed vouchers without a stable identity could enter a scan. | `65093fa`; `9c28da5` | `changed_voucher_rows_require_a_stable_identity`; `voucher_parsers_decode_entities_in_vouchers_and_change_feeds` | A changed row requires nonblank `GUID` or `MASTERID` before any cursor is built; the entity fixture now supplies the required GUID. |
| `src-tauri/src/agent.rs:1674` | Top-party ranking excluded unallocated-only exposure. | `8378f35` | `outstandings_top_ranking_includes_wholly_unallocated_parties` | Ranking aggregates selected bills and selected unallocated residuals, and returns `billed`, `unallocated`, and total components. |
| `src-tauri/src/agent.rs:676`; `src-tauri/src/tally/runtime.rs:1555` | `ledger_masters` omitted its ledger-read wire evidence. | `52bef57` | `ledger_masters_evidence_changes_when_a_ledger_response_changes` | Basic and compliance results combine the actual paired native ledger request hash, response hash, and byte count into returned evidence. |
| `src-tauri/src/agent.rs:584`; `src-tauri/src/agent.rs:2752` | Egress receipt could describe a pre-envelope result instead of the emitted JSON-RPC frame. | `37505c6` | `egress_receipt_uses_the_final_jsonrpc_replacement_when_only_the_envelope_exceeds_cap` | Receipt generation now runs after JSON-RPC trimming/replacement and hashes/counts the exact serialized stdout line; an envelope-only overflow records the final `agent_response_too_large` frame. |

## Thread replies

1. The changed-master reader now uses `trim_text(false)`, preserving the meaningful whitespace around decoded entity fragments such as `Income &amp; Expense`; end-tag handling still clears the active tag, so whitespace between elements does not become a field value. The regression now asserts `Income & Expense`, while the existing high-water and envelope-validation paths remain unaffected.

2. `ledger_movement` now reads its ledger opening through the native export pinned to `BOOKSFROM`, then applies only literal pre-window voucher movement. This follows the verified `OPENINGBALANCE`/`SVFROMDATE` behavior in protocol reference §5.5; because Tally does not return a response period span, the endpoint profile admits the exact boundary before dispatch rather than asserting an unobservable response field, and the regression covers both admitted and mismatched periods.

3. Changed voucher parsing now requires a non-empty `GUID` or `MASTERID` before creating the parsed row or allowing cursor construction, returning `change_row_identity_invalid` otherwise. The follow-up fixture-only commit supplies a GUID to the existing entity-decoding test so it remains a valid changed-voucher observation under that rule.

4. The top-party calculation now aggregates selected-direction open bills with selected-direction unallocated residuals before sorting. Each returned party shows the billed and unallocated components alongside the combined total, and the regression proves a wholly unallocated party can rank first.

5. `ledger_masters` now carries the actual native ledger read evidence into its response. The authorised runtime adapter returns paired wire facts and both the basic and compliance paths combine them with company evidence; the regression changes only the ledger response commitment and observes changed returned evidence.

6. Egress metadata is now written at the stdio boundary after the final JSON-RPC response has been bounded or replaced. The regression sets a cap that fits the tool result but not its envelope, then confirms the receipt records the replacement frame's exact serialized byte count and SHA-256 with zero returned fields and rows.

## Final verification

```text
$ rustup run 1.96.0 rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast -q
test result: ok. 491 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 136.23s

$ rustup run 1.96.0 cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.09s

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
ℹ pass 105
ℹ fail 0

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)

$ corepack pnpm exec playwright test  # direct Vite server at 127.0.0.1:4173
2 passed (1.2s)
```
