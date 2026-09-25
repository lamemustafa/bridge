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
