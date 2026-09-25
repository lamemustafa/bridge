# Cloud Lane FB (builder) report

Append-only log. Newest entry at the bottom. This branch is never merged.
Each branch step gets a dated entry; a "built <sha>" line marks a head as done.

## 2026-09-25 15:02 UTC — session start

- Toolchain: rustc 1.96.0, cargo 1.96.0, node v22.22.2, corepack 0.34.6, git 2.43.0, 4 CPUs, Linux.
- origin/master = f41a96c.
- Queue (oldest head first), all already contain origin/master (0 behind):
  - `lane-f/gold-docs-evidence` @ e3bf0e2
  - `lane-f/egress-gate` @ 883dc5f
  - `lane-f/653-ledger-masters-as-of` @ d0f59ab

## 2026-09-25 15:03 UTC — order note

- Lane D's message (15:02 UTC) put 653 first, then egress-gate, then gold-docs-evidence, with 626-crlf-ledger-names to follow.
- The gold-docs-evidence gates had already started (oldest head first, per the brief), so it finishes first. Then 653, then egress-gate. 626 gets picked up when it appears.
- gold-docs-evidence: 0 behind master, so no merge. `scripts/reseal.sh` changed both JSONs; `--verify` reports current. Local reseal commit 6e89298 (not pushed). Gates running.

## 2026-09-25 15:21 UTC — gold-docs-evidence run 1 (on master f41a96c), superseded

- Host setup: the container had no GTK/WebKit dev libraries. I installed them with apt (libwebkit2gtk-4.1-dev, libgtk-3-dev, libsoup-3.0-dev, libayatana-appindicator3-dev, librsvg2-dev, libxdo-dev). That's a host change only, not a repo change.
- Clippy feature spelling: `--features bridge/rfd/gtk3` is rejected ("multiple slashes in feature"). **`--features rfd/gtk3`** works (src-tauri/Cargo.toml is both the workspace root and the `bridge` package). I use it from here on.
- Run 1 on e3bf0e2 + reseal 6e89298: every gate was green apart from the known reds (the counts are the same as run 2 below). A Sonnet review of 6e89298 found nothing.
- Known-red check: `scripts/reseal-merge-driver.test.mjs` on origin/master f41a96c fails the same way (4 fail: two subtests of "git merge driver: reconciles disjoint pinned-file changes, refuses genuine ones", their parent, and "captured checkout trusts only its differently-owned source path").
- **Not pushed:** origin/master moved to 54eb327 (#665) before the push. Redoing from step 1, as local `fb/gold-2`: merge b0af14e, reseal 7fc5afc (`--verify` current). Run 2 gates and a Sonnet review are running.

## 2026-09-25 15:33 UTC — lane-f/gold-docs-evidence: built d91b2d4: green

- Built head: 1317146 (Lane F's README fix; it replaced e3bf0e2 mid-build). An in-flight run on the old head plus merge (b0af14e, reseal 7fc5afc) was stopped and discarded; those commits were never pushed.
- Merged origin/master 54eb327 --no-ff → 410beb4 (clean, no conflicts). Reseal → **d91b2d4** "Reseal the compatibility surface". Four pinned reference parts rehashed. `--verify`: current.
- Sonnet review of 410beb4 + d91b2d4: no findings. The merge is a byte-identical union both ways, all changed pins match their bytes, and the pin list, claims and bridge_commit_sha are unchanged.
- Pushed as a fast-forward: `lane-f/gold-docs-evidence` 1317146..d91b2d4.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1335 passed, 1 failed (known: `db::encrypted::tests::readonly_directory_returns_typed_storage_without_resetting_the_mirror`, root), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair in reseal-merge-driver.test.mjs, same 4 on master), 4 skipped |
| check-tally-live-read-boundary | 0 | ok |
| check-fixture-byte-integrity | 0 | ok |
| check-fixture-provenance | 0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- `src-tauri/gen/schemas/linux-schema.json` was created by the build. I deleted it; it was not committed.

## 2026-09-25 15:41 UTC — lane-f/653-ledger-masters-as-of: built d0f59ab: RED

- Head d0f59ab. Merged origin/master 54eb327 --no-ff → c6d5672 (clean). Reseal → 9134e35 (agent.rs, agent_catalog.rs, agent_ledgers.rs rehashed); `--verify` current. Sonnet review of c6d5672 + 9134e35: no findings. **Not pushed**, because of the real reds below. Both commits stay local only.
- **REAL 1: `cargo fmt --check` fails** in Lane F's code: `src-tauri/src/agent_ledgers_tests.rs` around line 1107. rustfmt wants two lines wrapped:
  - `assert_eq!(error["code"], "ledger_masters_as_of_requires_compliance", "{args}");`
  - `assert!(error["remediation"].as_str().is_some_and(|text| text.contains("fields=compliance")));`
  Both lines come from 01568e5.
- **REAL 2: 2 lib tests fail**, and they fail identically on d0f59ab without the master merge (I ran them in a separate worktree at d0f59ab):
  - `agent::ledgers::tests::through_the_tool::an_impossible_as_of_date_is_refused_before_any_request`
  - `agent::ledgers::tests::through_the_tool::as_of_without_compliance_fields_is_refused_before_any_request`
  - The first panic lines are the same for both:
    `panicked at src/agent_ledgers_tests.rs:1135:57: called Result::unwrap() on an Err value: Custom { kind: InvalidInput, error: "simulator sequence request count is out of range" }`
  - Line 1135 is `SequenceSimulator::spawn(plans).unwrap()`, and both tests call `call(Vec::new(), …)`. The simulator refuses an empty plan list, so a "refused before any request" test can't use that helper as written.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 1 | **REAL**: diff in agent_ledgers_tests.rs (above) |
| bridge --lib (rfd/gtk3) | 101 | 1336 passed, 3 failed (**2 REAL** above + known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- I'll rebuild when Lane F pushes a new head.

## 2026-09-25 15:48 UTC — lane-f/egress-gate: built 1fb9fb3: green

- Head 883dc5f. Merged origin/master 54eb327 --no-ff → 236de00 (clean). Reseal → **1fb9fb3** (.github/workflows/ci.yml and package.json pins rehashed); `--verify` current.
- Sonnet review of 236de00 + 1fb9fb3: no findings. The merge is a byte-identical union both ways; all 280 pins match their bytes; pin list, claims and bridge_commit_sha are unchanged.
- The dependency-mutation proof was not repeated (per Lane D). As an extra check, `node scripts/check-tally-egress-boundary.mjs` on the merged head exits 0: "reqwest/hyper are confined to the pinned crates and, inside the app crate, to 4 pinned files."
- Pushed as a fast-forward: `lane-f/egress-gate` 883dc5f..1fb9fb3.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1335 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

## 2026-09-25 15:55 UTC — lane-f/626-crlf-ledger-names: built c808bd7: RED

- Head c808bd7 already contains origin/master 54eb327, so there was no merge. Reseal → 3f0c5a8 (master_binding.rs, agent_catalog.rs, agent_import.rs, agent_import_post.rs, approved_import.rs rehashed); `--verify` current. Sonnet review of 3f0c5a8: no findings (all 280 pins match their bytes). **Not pushed**, because of the real red below. The commit stays local only.
- **REAL: clippy fails** (`-D warnings` → `-D dead-code`) on the lib target:
  ```
  error: function `source_entities` is never used
      --> src/agent_import.rs:2515:4
  2515 | fn source_entities(requested: &[String]) -> Result<Vec<SourceEntity>, String> {
  ```
  Cause: 48be0ce ("Name a ledger whose stored name ends in CR LF by its exact bytes (#626)") removed both non-test callers that master has (agent_import.rs:433 and :2313 on master). The only remaining uses are in agent_import_tests.rs (lines 1678 and 1738), so the non-test lib build sees it as dead. Tests still compile and pass because the test build uses it.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1348 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 101 | **REAL**: dead `source_entities` (above) |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- Next: 653's new head edb3258.

## 2026-09-25 16:03 UTC — lane-f/653-ledger-masters-as-of: built 591f401: green

- New head edb3258 ("Count requests with a held plan in the #653 refusal tests, and format them"). It fixes both reds reported for d0f59ab. Merged origin/master 54eb327 --no-ff → 3e76bef (clean). Reseal → **591f401** (agent.rs, agent_catalog.rs, agent_ledgers.rs rehashed); `--verify` current.
- Sonnet review of 3e76bef + 591f401: no findings. The merge is a byte-identical union both ways; all 280 pins match their bytes; pin list, claims and bridge_commit_sha are unchanged.
- Both previously failing tests now pass: `through_the_tool::an_impossible_as_of_date_is_refused_before_any_request` and `through_the_tool::as_of_without_compliance_fields_is_refused_before_any_request`.
- Pushed as a fast-forward: `lane-f/653-ledger-masters-as-of` edb3258..591f401. (The earlier local c6d5672/9134e35 were never pushed.)

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1338 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- Queue now: 626 is still RED at c808bd7 and waits for a new head. Idle-polling every 10 minutes.

## 2026-09-25 16:34 UTC — lane-f/gold-docs-evidence: built 1cde915: green

- New head 1cde915 (969b259 + 1cde915 on top of d91b2d4: README.md and docs/agent/README.md only, neither pinned). 0 behind master 54eb327, so no merge. `scripts/reseal.sh` produced no change and `--verify` reports current, so there is **no reseal commit and nothing to push**. The head stays 1cde915. No Sonnet review, since there is no commit of mine to review.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1335 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- Queue widened per Lane D to `lane-a/*` and `lane-c/*` (lane-a first). None exist yet. 626 is still RED at c808bd7. Back to polling.

## 2026-09-25 16:42 UTC — lane-f/653-ledger-masters-as-of: built 362904c: green

- Head 89edda1 (Lane F's merge of master a8324c6, with the JSONs on master's side). 0 behind. Reseal → **362904c** (agent.rs, agent_catalog.rs, agent_ledgers.rs rehashed); `--verify` current.
- Sonnet review of 362904c: no findings. All 280 pins match their bytes; pin list, claims and bridge_commit_sha are unchanged; a8324c6 is an ancestor.
- Pushed as a fast-forward: `lane-f/653-ledger-masters-as-of` 89edda1..362904c.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1338 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

- Queue state:
  - egress f51eaa5: reseal a9b9e4e ready, review clean, gates pending.
  - 626 5d6d6d7: merge d1b7ccd + reseal f449833 ready, review clean, gates pending.
  - gold: moved to 7927967 (Lane F's own master merge). My local 28eea71/e9888fe on 1cde915 are discarded and will be redone.
  - Lane A asked for an iteration build of `lane-a/d1-wip` @ ddfcdc2 (per Lane D, lane-a first). Running that next.
  - `lane-c/601d-wip` @ 7543f8a has appeared.

## 2026-09-25 16:45 UTC — lane-a/d1-wip: built ddfcdc2: RED (iteration build for Lane A, WIP)

- Built exactly Lane A's seven commands on ddfcdc2, detached. No merge, reseal or push, and the branch is unchanged. The branch is 1 behind master a8324c6 and 18 ahead.
- **Host note:** without a backend feature, the `rfd` build script panics on Linux: "You need to choose at least one backend: `gtk3` or `xdg-portal` features for x86_64-linux". Steps 2 and 3 as given therefore never reach Bridge's code. I reran both with `--features rfd/gtk3` (2b, 3b), which produce the real errors below.

| # | Command | Exit | Result |
|---|---|---|---|
| 1 | cargo fmt --all -- --check | 1 | 20 diff hunks (list below) |
| 2 | clippy --workspace --all-targets (no feature) | 101 | rfd build script: no Linux backend (host) |
| 2b | same + `--features rfd/gtk3` | 101 | 2 × E0061 in `bridge` (lib test) |
| 3 | cargo test -p bridge --lib (no feature) | 101 | rfd build script: no Linux backend (host) |
| 3b | same + `--features rfd/gtk3` | 101 | same 2 × E0061; no tests ran |
| 4 | cargo test -p bridge-tally-protocol | 0 | all pass (148 lib + 20 test binaries, 0 failed) |
| 5 | tools cargo test --workspace | 101 | 23 passed, 1 failed (below) |
| 6 | node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| 7 | scripts/reseal.sh --verify | 1 | expected FAIL: 4 stale pins (below) |

**Compiler errors (2b and 3b; identical, the only errors, no warnings):**
```
error[E0061]: this function takes 2 arguments but 1 argument was supplied
   --> src/agent_import_post_e2e_tests.rs:216:13
216 |     assert!(import_outcome_is_clean(Some(&outcome)));
    |             argument #2 of type `usize` is missing
note: function defined here --> src/agent_import_post.rs:789:4
789 | fn import_outcome_is_clean(
790 |     outcome: Option<&bridge_tally_protocol::TallyImportOutcome>,
791 |     voucher_count: usize,

error[E0061]: this function takes 2 arguments but 1 argument was supplied
    --> src/agent_import_post_e2e_tests.rs:1443:13
1443 |     assert!(import_outcome_is_clean(Some(&outcome)));
     |             argument #2 of type `usize` is missing
error: could not compile `bridge` (lib test) due to 2 previous errors
```
The non-test lib target reported no errors under clippy; only the lib-test target failed.

**Tools failure (5):** `bridge-tally-compatibility` `tests::real_tree_has_complete_migration_and_report_surface_coverage` panicked at `bridge-tally-compatibility/src/lib_tests.rs:909:46`: `called Result::unwrap() on an Err value: Invalid { code: "surface_file_changed" }`. This follows from the unsealed surface (step 7), so a reseal should clear it.

**Stale pins (7):** src-tauri/src/agent_import.rs, src-tauri/src/agent_import_ledger.rs, src-tauri/src/agent_import_post.rs, src-tauri/src/tally/approved_import.rs.

**fmt diffs (1), file:line of each hunk.** Plain `cargo fmt` fixes them all:
- agent_import_ledger.rs: 89, 348
- agent_import_ledger_stream_tests.rs: 356, 371, 388, 400, 410
- agent_import_post.rs: 436, 826, 966
- agent_import_post_e2e_tests.rs: 523
- agent_import_post_tests.rs: 690, 768, 1456, 1490, 1523, 1542, 1555, 1562, 1582

## 2026-09-25 16:53 UTC — lane-a/d1-wip: built 375caf3: RED (iteration build for Lane A, WIP)

- Same seven commands, on 375caf3 detached (a fast-forward from ddfcdc2). No merge, reseal or push; the branch is unchanged. Steps 2 and 3 without a feature still stop on the host's `rfd` backend panic, so 2b and 3b add `--features rfd/gtk3`.
- **Both E0061 compile errors are gone.** Clippy with gtk3 is clean (0 errors, 0 warnings).

| # | Command | Exit | Result |
|---|---|---|---|
| 1 | cargo fmt --all -- --check | 1 | 22 diff hunks (list below) |
| 2 / 3 | clippy / lib test, no feature | 101 / 101 | rfd build script: no Linux backend (host only) |
| 2b | clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | clean |
| 3b | cargo test -p bridge --lib --features rfd/gtk3 | 101 | 1352 passed, **2 failed** (1 real + known root-only db::encrypted), 6 ignored |
| 4 | cargo test -p bridge-tally-protocol | 0 | all pass |
| 5 | tools cargo test --workspace | 101 | 23 passed, 1 failed (surface not resealed, as before) |
| 6 | node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only) |
| 7 | scripts/reseal.sh --verify | 1 | expected FAIL: stale pins agent_import.rs, agent_import_ledger.rs, agent_import_post.rs, tally/approved_import.rs |

**REAL test failure (3b):** `agent::agent_import::post::e2e_tests::ack_tests::a_batch_of_several_vouchers_is_refused_before_any_request`
```
panicked at src/agent_import_ack_tests.rs:668:88:
called `Result::unwrap()` on an `Err` value: "import_post_remote_ids_mismatch"
```
Line 668 is `native_post_request(&line, RemoteIds::from_ids(vec![Uuid::new_v4()])).unwrap()`. The test pushes a second voucher (so 2 vouchers and 2 txn_ids) but passes one remote id, and the new "id count checked against the batch" rule refuses that before the test reaches its assertion.

**Tools (5):** `tests::real_tree_has_complete_migration_and_report_surface_coverage` panicked at `bridge-tally-compatibility/src/lib_tests.rs:909:46`: `Invalid { code: "surface_file_changed" }`. This follows from step 7.

**fmt hunks (1), file:line.** Plain `cargo fmt` fixes them:
- agent_import_ledger.rs: 91, 356
- agent_import_ledger_stream_tests.rs: 359, 374, 396, 408, 418
- agent_import_post.rs: 436, 826, 966, 1232
- agent_import_post_e2e_tests.rs: 523
- agent_import_post_tests.rs: 690, 768, 1454, 1488, 1534, 1553, 1566, 1573, 1593, 1614

## 2026-09-25 16:59 UTC — lane-f/egress-gate: built a9b9e4e: green

- Head f51eaa5 (Lane F's merge of master a8324c6, with the JSONs on master's side). 0 behind. Reseal → **a9b9e4e** (.github/workflows/ci.yml and package.json rehashed); `--verify` current.
- Sonnet review of a9b9e4e: no findings. All 280 pins match their bytes; pin list, claims and bridge_commit_sha are unchanged; a8324c6 is an ancestor.
- As an extra check, `check-tally-egress-boundary.mjs` exits 0 ("sealed … 4 pinned files").
- Pushed as a fast-forward: `lane-f/egress-gate` f51eaa5..a9b9e4e.

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1335 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |

## 2026-09-25 17:07 UTC — lane-f/626-crlf-ledger-names: built f449833: green

- New head 5d6d6d7 ("Drop source_entities, which requested_masters replaced (#626)"). It fixes the clippy dead-code red reported for c808bd7. Merged origin/master a8324c6 --no-ff → d1b7ccd (clean). Reseal → **f449833** (master_binding.rs, agent_catalog.rs, agent_import.rs, agent_import_post.rs, approved_import.rs rehashed); `--verify` current.
- Sonnet review of d1b7ccd + f449833: no findings. The merge is a byte-identical union both ways; all 280 pins match their bytes; pin list and claims are unchanged.
- Pushed as a fast-forward: `lane-f/626-crlf-ledger-names` 5d6d6d7..f449833. (The earlier local 3f0c5a8 on c808bd7 was never pushed.)

| Gate | Exit | Result |
|---|---|---|
| cargo fmt --check | 0 | clean |
| bridge --lib (rfd/gtk3) | 101 | 1348 passed, 1 failed (known root-only db::encrypted), 6 ignored |
| approval_seam_gate | 0 | 8 passed |
| clippy --workspace --all-targets --features rfd/gtk3 -D warnings | 0 | 0 warnings |
| pnpm install --frozen-lockfile | 0 | ok |
| node --test scripts/*.test.mjs | 1 | 285 tests, 277 pass, 4 fail (known merge-driver pair only), 4 skipped |
| live-read-boundary / byte-integrity / provenance | 0/0/0 | ok |
| tools cargo test --workspace | 0 | 57 passed |
