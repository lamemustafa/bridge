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
