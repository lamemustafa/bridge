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

## Fri Sep 25 14:41:57 UTC 2026 — step 1.4 gates and verdict

Run as root, in order, after step 1.3. Where the plain form fails on the rfd build error, the same command also ran with `--features rfd/gtk3`. The `node --test` run was repeated after the first run's install race (see below).

| Command | Exit | Counts | Time |
| --- | --- | --- | --- |
| `cargo test --locked -p bridge --lib` (plain) | 101 | did not build: rfd has no Linux backend | 178 s |
| `… --lib --features rfd/gtk3` | 101 | 1332 passed / **1 failed** / 6 ignored | 248 s |
| `cargo test --locked -p bridge --test approval_seam_gate` (plain) | 101 | did not build: rfd | 1 s |
| `… --features rfd/gtk3 --test approval_seam_gate` | 0 | 7 / 0 / 0 | 42 s |
| `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings -A clippy::pedantic` (plain) | 101 | did not build: rfd | 149 s |
| same with `--features rfd/gtk3` | 0 | no warnings | 60 s |
| `node --experimental-strip-types --test scripts/*.test.mjs`, first run | 1 | 282 tests: 273 pass / 5 fail / 4 skipped | 32 s |
| same, rerun with `node_modules` present | 1 | 285 tests: 277 pass / **4 fail** / 4 skipped | 23 s |
| `cd tools && cargo test --locked --workspace` | 0 | 57 / 0 / 0 across 13 `test result:` lines | 51 s |
| `scripts/reseal.sh --verify` | 0 | "compatibility surface and matrix are current" | <1 s |

Peak `src-tauri/target` across steps 1.3 and 1.4 was **5.73 GiB**, sampled every 20 s (53 samples). `tools/target` was 1.3 GiB at the end, from a one-off `du -sh` printed to the console; it is not in a saved log.

### Classification of the step 1.4 failures

1. **rfd build error** (plain lib, seam and clippy). **Linux-specific and deliberate**: see step 1.3.
2. **`scripts/release-workflows.test.mjs`: `Cannot find package 'yaml'`** (first node run only). **Environment.** No `pnpm install` ran first, whereas CI installs first. `node_modules` timestamps (14:37:55–56 UTC) fall inside that node run (14:37:46–14:38:18). My inference, not verified: `generate-frontend-licenses.test.mjs`, which runs `corepack pnpm run …` under pnpm 11, installed dependencies mid-run, and this file raced it. After the install, the file passes 4/4. Separately, a test that can trigger an install while sibling files run in parallel is a possible latent race; the owner may want to look at it.
3. **`git merge driver: reconciles disjoint pinned-file changes, refuses genuine ones`**. Subtests: `disjoint pinned files merge cleanly…`, where stderr shows `reseal-merge-driver: invoked for … but could not read it at both refs -- falling back`, and `both sides changing … is refused`, which cascades with `you need to resolve your current index first`. **Git-version-specific (host Git 2.43.0, stock Ubuntu 24.04). It is a portability gap, not a wrong answer.** I verified in a throwaway repo that Git 2.43.0 passes `%S %X %Y` to a custom merge driver **literally**: the driver received the strings `%S`, `%X`, `%Y`, and none resolves as a revision. `mergeRefsFromArgs` accepts any non-empty string, so the driver takes the documented safe fallback, a plain 3-way merge, and the "clean merge" assertion fails. My recollection is that Git added these placeholders in 2.44; I have not verified that here. The repo documents no minimum Git version for the driver, and the fallback warning names "could not read at both refs" rather than the Git version.
4. **`captured checkout trusts only its differently-owned source path`**, which failed with `fetch captured source failed: fatal: detected dubious ownership in repository at '<tmp>/repo/.git'`. **Git-version-specific (2.43.0).** In a throwaway reproduction with `GIT_TEST_ASSUME_DIFFERENT_OWNER=1`, `safe.directory` supplied via `GIT_CONFIG_COUNT` env or via `-c` is honoured by `rev-parse`. It is **not** honoured by the `upload-pack` child of a local-path `fetch`, which fails with the same message. The identical fetch passes when the three `safe.directory` values come from a `GIT_CONFIG_GLOBAL` file. The harness scopes trust through env config (`withGitConfig`), so on this Git it cannot trust a differently-owned source. This also affects the real case the test models: a CI or container checkout owned by another uid.

Items 3 and 4 fall in the scope of #527. The #527 status comment records that "a local Git 2.43 run [was] not performed", so these are new evidence.

The 4 skipped tests are `check-advisory-delta` cases, and each skip says `cargo-audit not installed`.

### Verdict

**Linux cannot run the app-crate gates as committed.** `bridge` does not build on Linux, because rfd deliberately has no Linux backend. With a command-line `--features rfd/gtk3`, which needs no file or lockfile change, Linux can run them. Under that flag the lib tests, the approval-seam gate and clippy all pass, except one test that fails only because this host runs as root (passes as uid 65534). The tools workspace and reseal verification pass unmodified. The Node script suite has two Git-2.43-specific merge-driver-harness failures, items 3 and 4 above.

Practical conditions for sending app-crate work to Linux cloud sessions:
- (a) pass `--features rfd/gtk3`, or have the owner add a Linux-only rfd feature;
- (b) treat `readonly_directory_returns_typed_storage_without_resetting_the_mirror` as expected-red when running as root, or run it as non-root;
- (c) run `corepack pnpm install --frozen-lockfile` before the Node suite;
- (d) treat the two merge-driver tests as expected-red on Git 2.43, or install a newer Git.

Environment timings: apt install took 41 s, measured by a `date +%s` delta printed to the console and not in a saved log. The Tauri build also leaves the untracked file `src-tauri/gen/schemas/linux-schema.json`, which I deleted after the run.

Nothing was fixed in step 1. Next: #527, starting from items 3 and 4.

## Fri Sep 25 14:45:14 UTC 2026 — #527 measurement: real noexec `TMPDIR`

This is the acceptance run that #527's 2026-09-21 status comment asks for. It is a measurement only, with no code change.

Setup, as root on this host: `mount -t tmpfs -o noexec,nosuid,size=6g` on a scratch mount point. `findmnt` reported `rw,nosuid,noexec,relatime`. A shell script copied there and marked executable was refused with `Permission denied`, so noexec was proven with a real file, not a string check. The exec root was a directory on the executable ext4 root filesystem. Each condition ran `node --test scripts/reseal-merge-driver.test.mjs` once, with only the two variables below changing. `BRIDGE_RESEAL_TEST_EXEC_ROOT` was unset in the environment, except where set. Git 2.43.0.

| Condition | `TMPDIR` | `BRIDGE_RESEAL_TEST_EXEC_ROOT` | Exit | tests / pass / fail | Time |
| --- | --- | --- | --- | --- | --- |
| baseline | default (`/tmp`, executable) | unset | 1 | 19 / 15 / 4 | 14 s |
| noexec, no root | noexec tmpfs | unset | 1 | 19 / 10 / **9** | 4 s |
| noexec, with root | noexec tmpfs | executable dir | 1 | 19 / 15 / 4 | 13 s |

- The baseline's 4 failures are the Git 2.43 items 3 and 4 from step 1.4. They are present in every condition, so this comparison uses the set of failing test names, not the exit code.
- **noexec without the root** adds 5 failures:
  - `an explicit execution root owns executable scratch and build files`
  - `an unavailable committed toolchain is checked locally without resolving it`
  - `a locally installed committed toolchain resolves only after the inventory check`
  - `rustup inventory and resolution run outside the pinned checkout`
  - `toolchain preflight reads the captured revision despite dirty or missing source file`

  The log shows exec denials: Cargo reported `could not execute process <noexec>/…/cargo-target/debug/build/{proc-macro2,quote}-…/build-script-build (never executed)` (3 such lines), plus 3 `Permission denied (os error 13)` lines.
- **noexec with the root**: the failing set is **byte-identical** (`diff` of the `not ok` lines) to the baseline. The exec root removes every noexec-caused failure.

For #527's remaining item this is real noexec-host evidence: it fails without the explicit root and passes with it. The confound is that the suite as a whole cannot be green on Git 2.43, for the reasons in step 1.4 items 3 and 4, so "passes" here means "no failures beyond the Git 2.43 baseline". A clean run on Git ≥ 2.44 plus noexec has not been performed. The mount and the scratch directories were removed afterwards.

Next: decide whether a code fix is in scope for items 3 and 4 in a separate branch.

## Fri Sep 25 14:52:37 UTC 2026 — #527 fix: draft PR #688

- Branch: `claude/merge-driver-upload-pack-trust`, commit `1cc5b65`, based on `a51ae783`. The branch name was already `claude/`-prefixed, so no substitution was needed. Draft PR: https://github.com/lamemustafa/bridge/pull/688
- Fixes step 1.4 item 4 only. The captured-source fetch repeats exactly the caller's `safe.directory` entries on `--upload-pack`, shell-quoted. Two tests are added: a wrong-trust fetch control, and a quoting test on a path containing a space and a single quote.
- On Git 2.43.0: 20 tests, 17 pass, 3 fail. The 3 failures are the item 3 merge-driver group only. Three inverse mutations (dropping `--upload-pack`, trusting `*`, dropping the quoting) each fail the intended test. The Sonnet review found nothing blocking.
- Supporting evidence from CI: the `Workflow consistency` job on master `a51ae783` ran Git 2.55.0 and passed. This is only partial attribution evidence, because the runner also differs in uid and image. I could not get a newer Git here: GitHub archive, kernel.org and the git-core PPA were each refused by the proxy with 403.
- **Not fixed, needs an owner decision:** item 3, where Git 2.43 passes `%S/%X/%Y` unexpanded. The options are a documented minimum Git version, and/or making the driver name the cause instead of "could not read it at both refs". Either would change the driver or the docs, not only the harness.
- I have subscribed to PR #688 activity.

Queue item 3: stopping here and waiting for Lane D.
