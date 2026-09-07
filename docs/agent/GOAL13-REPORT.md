# Goal 13 report — tenth-round review fixes

Both accepted findings were addressed on `feat/agent-connector` with Rust
1.96.0. No finding was closed as won't-fix and nothing was pushed. The
party-ledger-master evidence adapter changed the Goal 3B-authorised sealed file
`src-tauri/src/tally/runtime.rs`; the documented `rehash-surface`,
`seal-surface`, and `repoint-matrix` sequence changed its entry hash from
`9bf70e64f98ce139c13d6914b78d4999b4d569cbacaf6b5c73fb44a990b05a6a` to
`191251c22653414bbdfa94e94835c521dacc0d4276e79f0b1ddba6140277ffff`, and
the manifest/matrix hash from
`b1784781f8944b711a1d710100411142c8fcaaf8670d0b51af5e8a967d8af1ea` to
`c0c58373e11e2d65da6dd764800ccf36e0492a97af9cef19c61aa402d2e6d749`.
The matrix retains 11 unknown claims and makes no new public compatibility
claim.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `src-tauri/src/agent_import.rs:786` | Verification silently discarded malformed text/entity fragments. | `173d476` | `verification_rejects_unknown_entities_in_ledger_and_narration_fragments` | Text and general-reference decoding now propagate `import_verification_export_invalid`; malformed wire fragments cannot contribute to a fingerprint or `posted_verified`. |
| `src-tauri/src/agent.rs:676`; `src-tauri/src/tally/runtime.rs:1659` | Compliance masters used an unrelated second plain-ledger export only to obtain evidence. | `6538c46` | `party_ledger_master_evidence_tracks_source_responses_without_plain_ledger_read` | The one evidence-bearing party-master runtime read supplies the returned compliance records and aggregated paired source commitments; the compliance branch no longer calls `fetch_ledgers_with_evidence`. |

## Thread replies

1. Import verification now propagates the typed decode failure from both ordinary text and general-reference fragments before deciding whether a voucher is active. The regression injects `A&bogus;B` independently into a ledger name and narration and confirms both fail closed as `import_verification_export_invalid`, preventing malformed data from satisfying an `AB` fingerprint.

2. The compliance `ledger_masters` branch now calls exactly one runtime adapter, `fetch_agent_party_ledger_masters_with_evidence`, which returns the compliance records and the source’s paired master, balance, and group response commitments together. The plain `fetch_ledgers_with_evidence` hash-only read is removed; the regression changes a source response commitment, observes changed evidence, and verifies the aggregated paired byte count without accepting a plain-ledger commitment.

## Final verification

```text
$ rustup run 1.96.0 rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast -q
test result: ok. 493 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 124.92s

$ rustup run 1.96.0 cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.45s

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0

$ node --experimental-strip-types --test scripts/*.test.mjs
ℹ pass 105
ℹ fail 0

$ corepack pnpm exec vitest run scripts/evidence-drawer-focus.test.tsx scripts/local-evidence-no-read.test.tsx
Test Files  2 passed (2)
Tests  6 passed (6)

$ corepack pnpm exec playwright test  # direct Vite server at 127.0.0.1:4173
2 passed (1.1s)
```
