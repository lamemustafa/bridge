# T4-2 — review-finding rectification

## Outcome

- An `On Account` bill allocation with an empty `NAME` no longer aborts its
  voucher read. Its reference is represented as `{"kind":"on_account"}`;
  no empty string, `null`, or placeholder name is emitted. Reference-bearing
  allocations use `{"kind":"named","name":"..."}` and still fail closed
  when `NAME` is absent or empty.
- An empty returned `GSTDUTYHEAD` now has the same classification meaning as
  an omitted duty-head element. `TAXTYPE=Others` produces `NotTaxLedger` and
  `TAXTYPE=GST` produces `Absent`. Non-empty unknown duty-head values remain
  `Unrecognized` with their raw spelling preserved.

## Files changed

- `src-tauri/src/agent_voucher_parse.rs`
- `src-tauri/src/agent_voucher_parse_tests.rs`
- `src-tauri/crates/bridge-tally-protocol/src/lib.rs`
- `src-tauri/src/agent_ledgers.rs`
- `runs/T4-2-report.md`

## Verification

All commands ran offline from this worktree and exited `0`.

```text
rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib on_account_bill_allocation_with_empty_name_is_explicitly_unnamed
  1 passed; 0 failed; 715 filtered out

rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib reference_bearing_bill_allocation_with_empty_name_fails_closed
  1 passed; 0 failed; 715 filtered out

rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib compliance_ledger_duty_heads_preserve_raw_values_and_name_attributes
  1 passed; 0 failed; 715 filtered out

rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib captured_empty_duty_head_uses_tax_type_classification
  1 passed; 0 failed; 715 filtered out

git diff --check
  exit 0
```

The capture-backed duty-head test reads the exact `TAXTYPE` and self-closing
`GSTDUTYHEAD` fields from
`master_fields_lab_partial_alter_after.response.xml`, then wraps those bytes in
the parser's collection envelope because the retained fixture is a full-object
Alter response rather than a collection response.

No Tally instance was contacted. The compatibility surface was not resealed.

## Commit handoff

The requested local commit could not be created in this environment. The
exact-file staging command exited `128` before staging any file because the
linked worktree Git metadata is not writable:

```text
git add src-tauri/src/agent_voucher_parse.rs src-tauri/src/agent_voucher_parse_tests.rs src-tauri/crates/bridge-tally-protocol/src/lib.rs src-tauri/src/agent_ledgers.rs runs/T4-2-report.md
  fatal: Unable to create linked-worktree index.lock: Operation not permitted
  exit 128
```

No task file was staged or committed. `TASK.md` and `runs/T4-1-report.md` were
pre-existing untracked files and were left untouched.
