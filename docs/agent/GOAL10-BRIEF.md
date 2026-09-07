# Goal 10 — Seventh-round Codex review threads on PR #228 (12 findings, 2026-09-05 10:18 UTC)

Same worktree, branch and rules as `GOAL9-BRIEF.md` (Rust 1.96 on PATH, one regression test per fix, commit per fix, do not push, sealed edits only where `GOAL3B-BRIEF.md` authorises them plus the documented reseal, no won't-fix without file:line counter-evidence). Report as `docs/agent/GOAL10-REPORT.md` in the Goal 9 format with a reply paragraph per thread. Finish with the workspace suite, clippy on `bridge_mcp` with `-D warnings`, the compatibility gate with the five CI arguments, the Node/Vitest part of `corepack pnpm test`, Playwright via the direct-vite fallback, and one run of `node scripts/package-mcpb.mjs`. All twelve are accepted; three are P1 and go first.

## P1
1. **`agent.rs:648` — enforce the ledger filter on parsed vouchers.** After parsing, retain only rows with at least one entry whose ledger matches the requested ledger (same normalised key as `matching_ledger_name`, resolved to the live spelling first); if Tally returned rows that do not match, treat it as `filter_not_honoured` evidence: drop them and mark the result `partial` with that reason. Test with a mixed response.
2. **`agent.rs:1657` — cancellation state in the change feed.** Fetch `ISCANCELLED` and `ISOPTIONAL` in the changed-vouchers profile, parse as required typed booleans (`cancelled`, `optional` on every row), and fail the scan with `voucher_accounting_state_not_observed` when either is absent. Test.
3. **`agent.rs:1405` — unallocated cursor from the page start.** Keep the requested offset separately and compute `next_offset = requested_offset + surviving_rows` when the byte cap trims the unallocated page (and check the same for open bills and items). Test the 500-row-page-trimmed-to-100 case.

## P2
4. **`agent.rs:1889` — decode and append XML entity fragments** in the voucher-row parser the same way the import parser already does (Goal 5 fix), failing closed on decode errors. Test `R&amp;D` through `vouchers` and `changed_since`.
5. **`agent.rs:1219` — unique ledger match.** Prefer an exact live spelling; if more than one live ledger shares the normalised key and none is exact, return `ledger_ambiguous` with the candidates. Test `A-B` vs `AB`.
6. **`agent.rs:1023` — bound ledger-movement rows** with `offset`/`limit` (≤ `max_rows`), `truncated` and `next_offset`, and teach the byte-cap trimmer about `result.ledgers`. Test.
7. **`agent_import.rs:417` — lock import-ledger appends** under the existing admission lock, writing one preformatted line. Test two concurrent appends.
8. **`agent.rs:1429` — count `result.masters` in egress receipts** for `validate_masters` while keeping the combined changed-feed count. Test.
9. **`agent.rs:90` — invalid `BRIDGE_TALLY_PORT` fails startup** with `port_setting_invalid`; default only when absent. Test.
10. **`scripts/package-mcpb.mjs:40` — build `--release` and copy from `target/release`**; update the README line. Test via the script's own verifier plus a path assertion.
11. **`agent.rs:1027` — commit movement evidence to the financial reads.** Combine the evidence (request hash, response hash, bytes) of the ledger catalogue read, the pre-window read and the in-window read into the `ledger_movement` evidence, not only company discovery. Test that the evidence hash changes when the voucher response changes.
12. **`docs/agent/README.md:11` — host-independent toolchain command** (`rustup run 1.96.0 …`), and the same in any brief that names the ARM64 triple.
