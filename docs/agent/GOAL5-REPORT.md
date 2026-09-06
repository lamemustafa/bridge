# Goal 5 report — PR #228 second-round rectification

Completed on `feat/agent-connector`; commits are local only and have not been
pushed. Every response-oriented change stays outside the sealed compatibility
surface. The existing `fs2` admission lock was retained because it provides a
real cross-process advisory lock across the required read-check-create-append
critical section; its generated Rust license inventory is now refreshed.

| path:line | finding | fix commit | test | note |
| --- | --- | --- | --- | --- |
| `agent_import.rs:694` | Escaped XML comparison | `232c21d` | `verification_unescapes_every_record_text_node_before_fingerprinting` | XML text and reference events are decoded before proof/fingerprint construction. Reply: This now decodes named and numeric entities in every parsed voucher text field, so a successful `R&D` import is not reported divergent. |
| `agent_import.rs:790` | Pre-mark fingerprint attribution | `0f0d65a`, `bcffdf4` | `fingerprint_only_verification_requires_a_post_mark_voucher` | A no-tag fingerprint must have `ALTERID` greater than the persisted voucher mark; otherwise it is `not_attributable`. The batch ledger also persists the explicitly unobserved master axis rather than manufacturing it from vouchers. Reply: A pre-existing identical voucher cannot now become posting proof; only a narration marker or post-mark fingerprint can verify it. |
| `agent.rs:611` | Change-feed checkpoint completeness | `7e317bf` | `truncated_change_feed_never_advances_the_checkpoint` | Checkpoints are advanceable only when both axes reach the selected company's numeric high-water without truncation. Reply: A parsed prefix no longer advances the cursor; the response names the failed high-water corroboration. |
| `agent.rs:554` | Voucher-window completion | `92fa189` | `voucher_window_rejects_out_of_range_rows_and_requires_a_wider_empty_check` | Out-of-window rows fail closed; an empty result issues a one-day-wider corroborating read. Reply: Tally date-window widening/collapse is no longer presented as complete literal-window evidence. |
| `agent.rs:1241` | Numeric checkpoints | `e564d45` | `changed_since_checkpoints_round_trip_as_numbers_with_numeric_string_compatibility` | High-water values are JSON integers; malformed values fail closed and numeric strings remain accepted at input. Reply: A returned checkpoint can now be sent directly to the next request without silent full replay. |
| `agent.rs:75` | `BRIDGE_AGENT_MAX_BYTES` | `c094433` | `response_byte_cap_truncates_trailing_rows_and_refuses_a_single_oversized_row` | Default is 200,000 bytes; trailing page rows are removed with `next_offset`, while a single oversized row is refused. Receipts include `enforced_bytes`. Reply: The documented byte cap is enforced at the final serialized egress boundary. |
| `agent_import.rs:490` | Tally-host accounting date | `9cf8c67` | `accounting_day_uses_tally_host_local_calendar_at_utc_midnight` | Import bounds and outstandings `as_of` use `chrono::Local`, documented as Tally-host calendar time. Reply: Defaults now follow the local Tally machine at a UTC-midnight boundary. |
| `agent.rs:523` | Egress page row count | `3275e76` | `paginated_egress_rows_returned_is_the_final_page_length` | Receipts report the actual page after offset/limit, not a capped total. Reply: Near-end pages now record their real one-row/zero-row length. |
| `agent.rs:208` | Status evidence probe | `b4778ab` | `tally_status_uses_the_runtime_probe_observation` | `tally_status` calls the runtime Capability Passport probe and binds its observed payload hash. Reply: Status fields are now observed probe output, not a fabricated `/status` commitment. |
| `src-tauri/Cargo.toml:69` | Rust dependency inventory | `126a44d` | `node scripts/check-dependency-inventory.mjs --rust`; `node scripts/check-license-metadata.mjs` | Retained the existing `fs2` advisory lock and regenerated `THIRD_PARTY_LICENSES_RUST.txt`; inventory reports 384 locked components. Reply: The shipped notice now includes `fs2` and every branch-lockfile dependency. |
| `agent_import.rs:90` | Full company tuple per batch | `4627731` | `batch_company_tuple_rejects_a_same_guid_different_book` | Name, GUID, company number, and books-from are persisted and exactly compared during verification. Reply: A same-GUID different-book/year-end tuple now fails closed as `company_identity_mismatch`. |
| `agent.rs:851` | Cancelled/optional movement | `ccaffd3` | `ledger_movement_excludes_cancelled_and_optional_vouchers_and_rejects_unknown_flags` | Both non-posting flags are excluded from pre-window and in-window arithmetic; absent flags fail closed. Reply: Ledger balances now cannot include cancelled, optional, or state-unobserved vouchers. |

## Compatibility surface

No Goal 5 code edit touched a GOAL3B-authorized sealed file. Consequently no
reseal was required; the current compatibility gate passes with all 11 claims
remaining `unknown`.

## Final verification

```text
cargo test --locked --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 441 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 120.63s

cargo clippy --locked --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.60s

corepack pnpm test
Error: Process from config.webServer was not able to start. Exit code: 1
Cause: nested Corepack resolved pnpm 11.5.3 while this repo requires 11.7.0; Node/Vitest passed before Playwright startup.

compatibility gate
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0
```

## Current-head replay (2026-09-06)

The following replay was run at the current local `feat/agent-connector` head
with Rust 1.96.0. It confirms the later review work did not regress these
twelve fixes. `corepack pnpm test` remains an honest environment gap: its Node
and Vitest phases pass, but Playwright's configured web-server child receives
Corepack pnpm 11.5.3 even though the outer command uses 11.7.0. No package
manager policy was changed to mask that failure.

```text
$ rustup run 1.96.0 cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
exit 0

$ rustup run 1.96.0 cargo test --locked --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
test result: ok. 519 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 123.65s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.80s

$ rustup run 1.96.0 cargo clippy --locked --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s

$ corepack pnpm test
tests 106; pass 106; fail 0
Test Files 2 passed (2); Tests 6 passed (6)
[WebServer] This project is configured to use 11.7.0 of pnpm. Your current pnpm is v11.5.3
Error: Process from config.webServer was not able to start. Exit code: 1

$ (cd tools && rustup run 1.96.0 cargo run --locked -p bridge-tally-compatibility -- gate ../docs/tally/compatibility/compatibility-matrix.json ../docs/tally/compatibility/compatibility-surface.json ../docs/tally/compatibility/trusted-evidence-keys.json ../docs/tally/compatibility/evidence ..)
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0
```

## Thread replies

1. Every parsed voucher text field now uses the tolerant XML unescape path before it participates in display, fingerprint, or proof construction. The regression covers ledger and party values with named and numeric entities, so a correct `R&D` import no longer becomes a divergent proof.

2. A marker-free accounting-fingerprint match must now postdate the persisted voucher-axis pre-import mark. An otherwise identical pre-existing voucher is `not_attributable`, while a post-mark match is `posted_verified`; a narration marker remains an independently observable attribution route.

3. Change-feed checkpoints now advance only after the independent high-water corroboration succeeds for both voucher and master axes. A well-formed but truncated prefix returns a non-advanceable checkpoint and a reason rather than silently skipping history.

4. Voucher results are checked against the requested date window before reporting completion, and an empty range receives a strictly wider corroborating read. A widened/collapsed Tally response is therefore partial evidence, not a completed empty segment.

5. Returned high-water checkpoints are JSON integers, and the next request accepts either those integers or a legacy numeric string. Missing or malformed values fail closed, so round-tripping a response no longer silently replays from zero.

6. `BRIDGE_AGENT_MAX_BYTES` is parsed with its documented 200,000-byte default and enforced at serialized egress. The agent removes trailing rows with a cursor when possible and refuses a single oversized row rather than emitting an over-cap response.

7. Default accounting dates now use the local calendar of the host that runs Bridge and Tally, via `chrono::Local`, rather than UTC. The midnight-boundary regression demonstrates the expected local-date behavior for both import bounds and outstandings defaults.

8. Egress receipts now record the actual post-offset/post-limit page length. Near-end pages consequently report one or zero rows as delivered rather than the total capped by the requested limit.

9. `tally_status` now binds its reported product, release, mode, bytes, and hashes to the runtime capability probe. If that probe cannot observe the data, the response says `not_observed` rather than fabricating a `/status` commitment.

10. The existing `fs2` advisory lock was retained because it protects the required cross-process admission critical section, and the Rust dependency inventory was regenerated to include it. The inventory and license-metadata checks are named in the table above; no unreviewed lock substitution was introduced.

11. Each batch now persists the complete company tuple and requires exact equality at verification. A same-GUID book with a different company number or books-from date fails closed as `company_identity_mismatch`.

12. Ledger movement excludes cancelled and optional vouchers from every balance-affecting aggregation, and missing state flags fail closed. The regression covers one cancelled and one optional voucher in the requested window.
