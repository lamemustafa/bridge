# Goal 6 report — third-round review fixes

All six findings were addressed on `feat/agent-connector` with Rust 1.96.0.
No sealed-surface file changed; the Goal 3B reseal sequence was therefore not
required. Nothing was pushed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent.rs:41` | Unknown redaction settings previously selected a permissive fallback. | `92f0d4a`, `4913eea` | `redaction_setting_defaults_only_when_unset_and_rejects_unknown_values` | Unset retains the default; any unrecognised value returns `redaction_setting_invalid` during startup. |
| `src-tauri/src/agent.rs:653` | Change-feed pagination used a positional offset that a newly inserted row could shift. | `2d8a840` | `change_feed_snapshot_cursor_excludes_rows_inserted_after_first_page` | First page captures both high-water marks; later pages require both snapshots, filter the bounded AlterID window, order by AlterID, and return page-max cursors until a complete, corroborated page. |
| `src-tauri/src/agent.rs:1534` | A malformed voucher/ledger/group `ALTERID` could be silently omitted from a change scan. | `ddbe16f` | `change_scan_rejects_any_missing_or_malformed_row_alter_id` | Every change row now fails closed with `change_row_alterid_invalid`, before a checkpoint can be returned. |
| `src-tauri/src/agent_import.rs:41` | Import inputs could not optionally carry a Tally voucher number. | `148b7b9` | `optional_voucher_number_is_rendered_only_when_valid_and_supplied` | Optional numbers are bounded and safe, render only when supplied, participate in readback proof, and README documents voucher-type numbering when absent. |
| `src-tauri/src/agent.rs:508` | Import planning appeared available without live-Tally import/readback evidence. | `a4a0bf7`, `03e10a6` | `imports_are_hidden_and_refused_without_explicit_live_evidence_opt_in` | Import tools are hidden and refuse by default; an enabled build says `live_evidence: none_recorded`. README, connector brief, MCPB description, and the historical Goal 2 report avoid a live-verified claim. |
| `src-tauri/src/agent.rs:803` | Open bills could be silently cut by the party-ranking limit. | `9696c37`, `e167069` | `open_bill_paging_marks_remaining_rows_and_returns_a_cursor` | `top` applies only to ranking; `open_bills` now uses bounded offset/limit and reports `truncated` with `next_offset`. |

## Thread replies

1. Redaction configuration now fails closed at server startup. An unset
`BRIDGE_AGENT_REDACTION` keeps the documented default, while every unrecognised
value returns the stable `redaction_setting_invalid` error rather than serving
under an unexpected policy; the regression covers both paths.

2. `changed_since` now creates an AlterID snapshot on its first page and
requires both snapshot values for a continuation. Both server filters use the
strict lower cursor and inclusive snapshot upper bound, output is AlterID
ordered, and truncated pages return their actual page maximum rather than a
high-water mark. The regression injects ALTERID 4 after a page pinned at 3 and
shows that ALTERID 3 is returned without admitting the later row.

3. The scan now rejects a missing, empty, or non-numeric `ALTERID` from either
voucher or master output with `change_row_alterid_invalid`. This happens before
page/cursor construction, so a malformed response cannot yield a checkpoint.

4. Voucher numbers remain optional because numbering belongs to the configured
Tally voucher type. When present, Bridge accepts only a non-empty, at-most-32
character, control-free value without `$`, renders `VOUCHERNUMBER`, and compares
it in readback proof; when absent, Tally applies the type configuration.

5. Import planning is now an explicit operator opt-in, not a claim of live
qualification. Without `BRIDGE_AGENT_ENABLE_IMPORT=1`, both import tools are
absent from discovery and reject direct calls; enabled builds are labelled
`live_evidence: none_recorded` and point to the historical report until live
evidence exists.

6. Open-bill paging is independent of the party-ranking `top` value. The tool
now pages all qualifying bills with bounded offset/limit and exposes both the
truncation flag and `next_offset`, so callers can retrieve the remaining rows.

## Final verification

```text
$ cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 447 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 118.67s

$ cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.48s

$ (cd tools && cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ corepack pnpm test (disposable exact-commit worktree; see note below)
Error: Process from config.webServer was not able to start. Exit code: 1
Cause: the nested Corepack web-server invocation used pnpm 11.5.3 while package.json requires 11.7.0; packageManager was not changed.

$ corepack pnpm exec vite --host 127.0.0.1 --port 4173
$ corepack pnpm exec playwright test
2 passed (1.1s)
```

The initial frontend command was first attempted in this worktree but its
fixture-integrity test also sees the pre-existing untracked
`.fixture-integrity-spYvAc/` directory. To preserve that user state, the same
command and direct Playwright fallback were rerun from a disposable worktree at
commit `03e10a6`, sharing the installed dependencies read-only. The initial
Corepack mismatch reproduced there; the direct Playwright fallback passed.
