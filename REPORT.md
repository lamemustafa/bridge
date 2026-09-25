# Cloud Lane T (trial) report

Append-only log. Newest entries at the bottom. This branch is never merged.

## Fri Sep 25 14:23:36 UTC 2026 — start

- Base: `origin/master` at `a51ae783054fddb77e73ed9f044f7516335e8826`.
- Host: Ubuntu 24.04.4 LTS, x86_64, 4 vCPU, 15 GiB RAM, running as root (sudo not needed).
- Toolchain: `rustup show` reports `1.96.0-x86_64-unknown-linux-gnu`, active via `rust-toolchain.toml`.
- Installed via `apt-get install` in 41 s (179 new packages, 19 upgraded): `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev`. Already present: `libssl-dev build-essential pkg-config perl`.
- Next: cold `cargo test --locked -p bridge --lib` from `src-tauri`.

## Fri Sep 25 14:33:52 UTC 2026 — step 1.3: `cargo test --locked -p bridge --lib`

**As written, the command does not build on Linux.** Exit 101 after 178 s of cold build. Failure, first lines:

```
error: failed to run custom build command for `rfd v0.17.2`
  thread 'main' panicked at …/rfd-0.17.2/build.rs:16:17:
  You need to choose at least one backend: `gtk3` or `xdg-portal` features for x86_64-linux
```

- Classification: **Linux-specific, and deliberate**. `src-tauri/Cargo.toml:113-114` declares `rfd = { version = "0.17", default-features = false }` with the comment "Bridge supports Windows and macOS; rfd's defaults add Linux portal/Wayland code." Linux has no dialog backend, so rfd's build script refuses. This is a missing platform configuration. It is not a code defect.
- Diagnostic workaround, with no file changes: `cargo test --locked -p bridge --lib --features rfd/gtk3`. It builds under `--locked`: `Cargo.lock` is unchanged and `git diff` is empty. A fix for Linux would enable `gtk3` for rfd only under `[target.'cfg(target_os = "linux")'.dependencies]`. That needs owner review of whether `Cargo.lock` changes. I did not verify this, and I made no change.
- Side effect: the Tauri build script generated the untracked `src-tauri/gen/schemas/linux-schema.json`. `.gitignore` does not cover it. I left it uncommitted.

Result with the workaround (`--features rfd/gtk3`), from the `test result:` line:

| Metric | Value |
| --- | --- |
| Cold build to first error, then the rest of the build with gtk3 | 178 s (the wrapper's \`date\` delta for the failing run) + 100 s (Cargo's own \`Finished … in 1m 40s\` for a \`--no-run\` build, which was printed to the console and not logged) ≈ 4.6 min on 4 vCPU. This is split across two runs, so it is not one clean cold number. |
| Test run | 248 s wall, including a small incremental relink; the harness reports 247.07 s |
| Passed / failed / ignored | **1332 / 1 / 6** (0 filtered) |
| Peak `src-tauri/target` | 3.2 GiB, sampled with `du -sb` every 20 s |

The two extra `test result: ok. 1 passed … 1338 filtered out` lines are child re-executions of the test binary, not separate suites.

Failure:

- `db::encrypted::tests::readonly_directory_returns_typed_storage_without_resetting_the_mirror`, which panicked at `src/db/encrypted_tests.rs:351:10` with: `readonly directory must reject SQLCipher callback configuration: ()`.
  - Classification: **environment (running as root), not a defect.** The test sets the directory mode to `0o500` and expects writes to fail. Root has CAP_DAC_OVERRIDE, so the write succeeds.
  - Proof: the same test binary run on that test alone passed as uid/gid 65534 (`setpriv --reuid=65534 --regid=65534 --clear-groups`), and failed again as root. Any root container, which is the default for cloud sessions, will hit this. Possible options for the owner: detect euid 0 and fail with a clear message, or run the gate as non-root.

The 6 ignored tests are by design: 5 need PDFium (`BRIDGE_PDFIUM_LIBRARY`) and 1 is a manual live-lab read.

Next: step 1.4 gates, run as root. `bridge` is built both plain and with `--features rfd/gtk3` where it applies.
