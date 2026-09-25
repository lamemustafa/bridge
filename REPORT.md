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
