# Goal 4 report — PR #228 review rectification

Completed on `feat/agent-connector`; local commits only. `REMOTEID` remains in
the generated import for Tally-side upsert idempotency, but is deliberately not
used as a readback correlation key: the implementation guide's verified
2026-07-30 observation is that Tally overwrites it. The implemented proof uses
the persisted narration marker first and the date/type/sorted signed-entry
fingerprint second, so either observed REMOTEID behaviour still verifies.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| agent_import.rs | REMOTEID readback | `10aca81` | import simulator | narration tag then accounting fingerprint; proof includes voucher number, GUID and MASTERID |
| agent.rs | TDL interpolation | `f89ff5e` | agent unit | `TdlStringValue` rejects quote, `$$`, colon, controls and non-safe characters |
| agent.rs/runtime.rs | bypassed custom reads | `10aca81` | import simulator | custom reads use serialized identity-bracketed runtime dispatch |
| agent.rs | incomplete redaction | `10aca81` | recursive redaction unit | central recursive response redaction occurs before output hashing |
| agent_import.rs | verification dates | `10aca81` | import simulator | normalize to `YYYYMMDD` before ledger persistence/rendering |
| agent_import.rs | invalid envelopes | `f89ff5e` | import unit | empty/error/non-collection responses are typed protocol failures |
| agent.rs | missing voucher entries | `10aca81` | agent parser unit | parser returns ledger, signed amount and polarity; malformed envelope fails closed |
| agent.rs | wrong movement opening | `f89ff5e` | workspace regression suite | pre-window entries derive an opening at `from`; method is declared |
| agent.rs | incorrect signed closing | `f89ff5e` | workspace regression suite | debit is negative, credit positive, closing sums signed values |
| agent.rs | silent change truncation | `f89ff5e` | schema/unit regression | independent rows are capped with `truncated` and `next_offset` |
| agent.rs | Windows temp data | `10aca81` | compile review | `%LOCALAPPDATA%\\Bridge\\agent`, with app-data fallback |
| agent.rs | wrong company high water | `10aca81` | parser regression | parses company rows and selects the verified GUID only |
| agent.rs | parser error as complete | `10aca81` | parser regression | XML parser errors propagate as typed failures |
| agent.rs | egress write hidden | `10aca81` | focused MCP suite | append/sync/permission errors fail tool call; unreadable differs from absent |
| agent.rs | incomplete commitments | `10aca81` | import simulator | every custom data read combines identity and response commitments |
| packaging/mcpb | missing binary step | `10aca81` | manual command review | `scripts/package-mcpb.mjs`; generated host binary remains untracked |
| agent.rs | coupled checkpoints | `f89ff5e` | tools schema regression | separate `voucher_alter_id` and `master_alter_id` are accepted/returned |
| agent_import.rs | import race | `10aca81` | import suite | advisory file lock spans ledger check, mark, file write and append |
| agent.rs | malformed tools/call exits | `f89ff5e` | stdio loop regression | request errors are returned and loop continues |
| GOAL1/2-BRIEF.md | absolute paths | `10aca81` | `rg` check | wording is worktree-relative |
| agent.rs | false verified tuple | `10aca81` | company unit | incomplete tuple has `incomplete_tuple` and missing field |
| agent.rs | fixed protocol version | `f89ff5e` | protocol unit | supports `2025-06-18` and `2024-11-05`, rejects unknown version |
| agent.rs | party-only ledger filter | `10aca81` | request unit | filter is entry-membership oriented rather than `PARTYLEDGERNAME` |
| agent.rs | partial direction application | `f89ff5e` | workspace regression suite | direction is validated and applied to returned bill/party views |
| agent.rs | success wording on error | `10aca81` | focused MCP suite | summary derives from result error state and says `read withheld` |

## Sealed surface

Changed sealed entries were `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and
`src-tauri/src/tally/runtime.rs`. The documented reseal order was run from
`tools`: `rehash-surface` reported `rehash_surface_changed:3`, then
`seal-surface`, then `repoint-matrix`. This updates the code digest only; all
11 compatibility claims remain `unknown` and no live-Tally claim is made.

## Verification

```text
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check       passed
cargo test --manifest-path src-tauri/Cargo.toml --lib agent::tests    2 passed
cargo test --manifest-path src-tauri/Cargo.toml --lib agent_import::tests  3 passed
cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings  passed
corepack pnpm test                                                     103 Node, 6 Vitest, 2 Playwright passed
compatibility gate                                                     passed: unknown_claims=11:evidenced_claims=0
```

The requested full workspace command was started twice:
`cargo test --manifest-path src-tauri/Cargo.toml --workspace`. Both processes
completed after reporting the running 430-test library suite without a shown
failure, but this terminal's shortened capture did not return their final exit
status. It is therefore an execution attempt, not a claimed green full-gate
result. No live Tally or native Windows validation was performed; those remain
release evidence gaps.
