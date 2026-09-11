# T4-1 — bill-allocation fetch and parse port

## Outcome

The agent's windowed and changed-voucher profiles now fetch
`ALLLEDGERENTRIES.BILLALLOCATIONS.NAME`, `.BILLTYPE`, and `.AMOUNT`. Parsed
ledger entries expose `bill_allocations` as a list of `{name, bill_type,
amount}` objects.

An empty `BILLALLOCATIONS.LIST` from the captured response produces `[]`.
A list that contains any scalar is admitted only when all three scalars are
present and nonblank; incomplete or blank scalars return
`bill_allocation_field_missing`. Allocation amounts are checked as exact
decimals. The raw `NAME` value is retained unchanged after admission.

## Files changed

- `src-tauri/src/agent_read_profiles.rs`
- `src-tauri/src/agent_voucher_scalars.rs`
- `src-tauri/src/agent_voucher_parse.rs`
- `src-tauri/src/agent_tests.rs`
- `src-tauri/src/agent_voucher_parse_tests.rs`
- `runs/T4-1-report.md`

## Verification

All commands ran from the task worktree and exited `0` on the final run.

```text
rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib voucher_profiles_fetch_accounting_state_and_bill_allocations
  1 passed; 0 failed; 712 filtered out

rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib captured_bill_allocations_preserve_raw_fields_and_empty_entries
  1 passed; 0 failed; 712 filtered out

rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib incomplete_bill_allocations_are_refused_instead_of_becoming_empty
  1 passed; 0 failed; 712 filtered out

rustup run 1.96.0 rustfmt --check --edition 2021 src-tauri/src/agent_read_profiles.rs src-tauri/src/agent_voucher_scalars.rs src-tauri/src/agent_voucher_parse.rs src-tauri/src/agent_tests.rs src-tauri/src/agent_voucher_parse_tests.rs
  exit 0

git diff --check
  exit 0
```

The first focused parse attempt revealed that the retained captured response
uses an empty `BILLALLOCATIONS.LIST` for an entry without allocations. The
parser was corrected before the final passing run: only a wholly empty list is
accepted as `[]`; a list with any incomplete scalar fails closed.

## Port rationale

The reference change was read for intent, but this branch's parser is a
scoped, streaming `NativeCollectionScope` state machine with scalar claims and
separate text, CDATA, and entity handling. The port extends those existing
scope predicates and claim paths for the nested list instead of transplanting
the older surrounding parser code. It also retains this branch's existing
ledger-entry JSON convention: the observed `ISDEEMEDPOSITIVE` string is kept
as received rather than being rewritten while bill allocations are attached.

No redaction rule was added for bill references. Compatibility JSONs were not
modified or resealed.

## Not verified

This was intentionally offline: no Tally instance was contacted. The focused
library tests cover the request render and captured parse boundary; no full
workspace build or full suite was run because a concurrent release build holds
the shared target directory and the task explicitly excludes broad builds.

## Commit handoff blocker

The requested local commit could not be created in this environment. The
explicit `git add` for the six task files exited `128` because Git could not
create the linked worktree index lock:

```text
fatal: Unable to create '.../.git/worktrees/bridge-pr228-takeover/index.lock': Operation not permitted
```

No task file was staged and no commit was created. The pre-existing untracked
`TASK.md` was not touched.
