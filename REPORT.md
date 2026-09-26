# Cloud Lane P (process and CI): report

One dated entry per step, times from `date -u`. This branch is append-only, fast-forward pushes only, never merged.

## 2026-09-26 05:57 UTC: Item 1, #723 Windows sccache flake → draft PR #728

**Status: draft PR open ([#728](https://github.com/lamemustafa/bridge/pull/728)), Sonnet and Opus reviews running.**

**Decision recorded (conservative choice, gate kept at full strength).** The brief asked me to tolerate only the exact "already stopped" case of `--stop-server`. I did not, because the measurement shows that case is **not** separable from "stats unreadable":
- In sccache 0.17.0, `--show-stats` with no server running prints **zeroed** statistics and exits 0 (`src/commands.rs`, `Command::ShowStats`).
- Throwaway Windows run [36221632800](https://github.com/lamemustafa/bridge/actions/runs/36221632800) on master's exact step, after 630 s idle and one real compiler write: `{"stepExit":2,"couldntConnect":true,"cacheWrites":0}`. Tolerating exit 2 would have passed a write count of 0 after a real write. On master that means `has-writes=false`, and the compiler cache is silently not saved.

**Root cause, verified:** sccache's default 600 s idle shutdown.
- In failed run 36196180348 (Windows), cargo finished at 22:30:04 and the stats step started at 22:39:50, 586 s later.
- In between ran the MSI and NSIS builds, the 6 min test-seam proof and MCPB packing. None of them go through sccache.

**Fix:** `SCCACHE_IDLE_TIMEOUT: '0'` on the bundle job, with `--stop-server` kept strict and a comment saying why. `ci.yml` is pinned, so the reseal is a separate commit and `reseal.sh --verify` reports current.

**Proof, with every prediction holding:**

| Case | Result |
| --- | --- |
| old code, idle | step exit 2, writes 0 (false) |
| new code, idle | exit 0, writes 1 |
| new code, server stopped beforehand | exit 2, writes 0: still fails closed |

**Throwaway branches I created:** `cloud-p/sccache-proof-723` and `cloud-p/cache-inventory-669`. They are not for merging. Deleting them is Lane D's call.

**Needs a decision:** none for #723. If the owner wants the literal "tolerate exit 2" behaviour anyway, that is a P5 downgrade and should be their explicit call.

## 2026-09-26 05:59 UTC: Item 2, #688 / #527 orphaned draft → ready for review

**Status: #688 is ready for review, green, no P1 or P2. Merging is Lane D's.**
- **Brought up to master.** I merged master `8ad6544` into `claude/merge-driver-upload-pack-trust` as a merge commit `cbf06e4`, with no rebase. None of the 16 new master commits touched the file under change. No merge driver is configured in this clone, and I did not use `git merge-tree`.
- **Re-measured on Git 2.43.0:**

  | Head | Tests | Pass | Fail | Failing |
  | --- | ---: | ---: | ---: | --- |
  | master | 19 | 15 | 4 | the ownership test, plus the 3-entry merge-driver group |
  | PR | 20 | 17 | 3 | only the merge-driver group |

  These are the "two merge-driver-harness tests" from the brief: two subtests plus their parent.
- **CI:** all green on `cbf06e4`.
- **Fresh Sonnet review: no P1 or P2.** It killed both mutations. I replied on the PR to both P3s: one is a known CI-Git limitation, the other test-only duplication. Neither is changed.
- **Does it still solve #527?** Partly, and it never claimed more.
  - It fixes the Git 2.43 ownership failure.
  - #527's noexec acceptance item was measured by lane T on 25 Sep. Without the exec root there are +5 failures; with it, the failing set is the Git 2.43 baseline. It passes, and that evidence is on the lane-T report branch, not yet on #527 itself.
- **Needs an owner decision (unchanged):** Git 2.43 passes `%S/%X/%Y` to the merge driver unexpanded. The options are a documented minimum Git version (probably 2.44), and/or a driver error that names that cause. Until then the merge-driver group stays red on stock Ubuntu 24.04 Git.

## 2026-09-26 06:18 UTC: Item 4, #669 Actions caches → proposal PR #729 (draft, owner's decision)

**Measured read-only** (throwaway workflow with `actions: read`, run 36221828972, 05:47 UTC): 9.90 GiB in 20 entries.

| Class | Entries | MiB |
| --- | ---: | ---: |
| Live (newest per family) | 11 | 4,610 |
| Package sccache, 2nd-newest per OS | 2 | 114 |
| **Superseded master Rust caches** (same restore prefix, older lockfile hash) | 5 | **3,986** |
| **PR-only Rust caches** (#695 and #707, both merged; the *same key* as master's live entry) | 2 | **1,430** |

Removing the two waste classes leaves **4.61 GiB**.

**Proposed rule, [#729](https://github.com/lamemustafa/bridge/pull/729):**
1. rust-cache `save-if: github.event_name != 'pull_request'` on all 3 jobs;
2. the existing fail-closed retention script also keeps only the newest master `v0-rust-*` entry per restore prefix, env hash included;
3. the retention job `needs` every Rust-cache-saving job.

**No cache deleted by hand.** A dry run of the script against the measured inventory selects exactly the 5 superseded ids.

**Reviews:**
- Round 1 (Opus + Sonnet) found a **real P1 in my change.** A duplicate `save-if` key made GitHub reject `ci.yml`, so Bridge CI ran with 0 jobs. `check-ci-workflow-consistency.mjs` does not parse YAML, so it didn't catch this.
- Round 1 also found a P2: grouping without the env hash could delete the only cache for a runner image still in use.
- Both are fixed.
- Round 2 (Opus): no P1 or P2. Its P3, the build guide still stating the old rule, is fixed.
- Head `ccef193`; CI is running.

**Process gap worth a follow-up:** nothing in CI loads the workflow YAML strictly. A duplicate-key or invalid workflow shows up only as a 0-job run, which reads as "no checks" rather than "red". A 10-line duplicate-key check in `check-ci-workflow-consistency.mjs` would have caught my P1 locally. I have not built it (scope); it is proposed here.

**Needs a decision:**
- Adopt the rule? Merge #729 (Lane D and the owner).
- The one-off deletion of today's 5 + 2 stale entries, or leave it to LRU and the 7-day eviction.

## 2026-09-26 06:51 UTC: Item 3, CI measurement (Bridge CI, last 50 runs)

**Sample:** the 50 most recent completed `ci.yml` runs on master pushes and PRs, 25 Sep 17:41 to 26 Sep 05:44 UTC: 39 PR runs and 11 master pushes. That is 618 job records, every attempt included, collected with the GitHub MCP Actions API.
- Per-step timings come from the 21 most recent runs (264 jobs). The re-fetch for the other 29 was lost to a container restart, and I did not repeat it, because the sample is already stable (n≥17 per step).
- Cache hits come from `Bundle smoke (windows-latest)`, the one job that touches all four caches, over the 25 most recent runs.
- Raw data is not committed. It is reproducible from the run ids.

**Runs**

| Conclusion | Count |
| --- | ---: |
| success | 33 |
| cancelled (superseded by a newer push to the same ref) | 11 |
| failure | 6 |

- 3 runs needed a rerun: 36205041132 took 3 attempts; 36196180348 and 36186882972 took 2 each.
- **Wall time**, run created to last job done, attempt 1, success or failure:

| Scope | n | p50 (min) | p90 (min) | Max (min) |
| --- | ---: | ---: | ---: | ---: |
| All | 39 | **16.0** | **19.9** | |
| PR | 29 | 15.9 | 20.2 | 22.4 |
| Master push | 10 | 16.1 | 17.4 | 19.8 |

**Per job** (successful and failed jobs; minutes; queue is the job's created→started time)

| Job | n | p50 | p90 | Queue p50 | Queue p90 | Failed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Bundle smoke (windows-latest) | 41 | **14.7** | **17.5** | 0.03 | 0.07 | 3 |
| Native checks (macos-latest) | 42 | 13.6 | 15.5 | 0.13 | **2.28** | 5 |
| Native checks (windows-latest) | 42 | 10.4 | 12.0 | 0.03 | 0.05 | 4 |
| Bundle smoke (macos-latest) | 42 | 10.0 | 12.0 | 0.12 | 0.18 | 0 |
| Tally portable core | 45 | 2.9 | 3.1 | 0.03 | 0.05 | 1 |
| Frontend build | 45 | 1.9 | 2.2 | 0.03 | 0.03 | 1 |
| Workflow consistency | 46 | 1.1 | 1.2 | 0.03 | 0.05 | 0 |
| Rust format | 46 | 0.4 | 0.4 | 0.03 | 0.05 | 0 |
| Tax-audit mutation records | 32 | 0.3 | 0.3 | 0.03 | 0.05 | 4 |

Queue wait is negligible except on macOS. Outside the sample, on 26 Sep, one `Bundle smoke (macos-latest)` in a #728 run waited about 22 min for a runner.

**Per step, the top steps of the heavy jobs** (successful steps of successful jobs, 21 runs; minutes)

| Job | Step | n | p50 | p90 |
| --- | --- | ---: | ---: | ---: |
| Bundle smoke (win) | **Prove shipped executables lack the test-only approval seam** | 17 | **6.3** | 6.9 |
| Bundle smoke (win) | tauri:build | 17 | 4.0 | 5.4 |
| Bundle smoke (win) | Set up Windows native prerequisites | 17 | 0.9 | 1.1 |
| Bundle smoke (mac) | **Prove … test-only approval seam** | 19 | **4.4** | 5.6 |
| Bundle smoke (mac) | Build macOS bundles | 19 | 2.7 | 4.2 |
| Native (mac) | Test native workspace | 17 | 5.1 | 6.1 |
| Native (mac) | Test legacy voucher-scan and calibration harness features | 17 | 3.5 | 3.9 |
| Native (mac) | Lint legacy harness features / Test PDF extraction | 17 | 1.3 / 1.3 | 1.6 / 1.6 |
| Native (win) | Test native workspace | 17 | 5.0 | 5.2 |
| Native (win) | Test PDF extraction through PDFium | 17 | 1.4 | 1.5 |
| Tally portable core | Test portable Tally truth layer | 20 | 0.7 | 0.7 |
| Frontend build | pnpm test / playwright install | 20 | 1.0 / 0.6 | 1.1 / 0.8 |

**Cache hits** (Bundle smoke windows, 25 runs; runs that never reached the step excluded)

| Cache | Result |
| --- | --- |
| pnpm | **22/22** |
| Strawberry Perl | **22/22** |
| rust-cache | **20/22** exact, 2 partial (restore-key fallback) |
| package sccache restore | **18/20** hit, 2 miss |

Inside sccache, the #728 PR run on Windows served 6 of 6 cacheable Rust compiles from cache.

**Failures, grouped:** 37 failed jobs.
- 19 are only the `Required checks` aggregate echoing another job's failure or cancellation.
- Of the 18 substantive failures:

| Class | Jobs | Runs | Cause |
| --- | ---: | ---: | --- |
| **Code** | 4 | 4 | Tax-audit mutation gate: "N selected, 0 proven on this tree" after non-source input changed |
| Code | 4 | 2 | `every_registered_test_matches_its_synthetic_golden` (bridge-tax-audit registry golden), mac and win |
| Code | 4 | 1 | `real_tree_has_complete_migration_and_report_surface_coverage` / compatibility-surface Frontend test (36188402169) |
| Code | 2 | 1 | clippy `needless_borrow`, mac and win |
| **Infra** | 3 | 2 | sccache `couldn't connect to server` in the Bundle smoke (win) stats step. **All passed on rerun; fixed by #728** |
| Infra | 1 | 1 | PDFium download HTTP 500 (Native mac). Passed on rerun |

- Totals: **code 14, infra 4.** Every infra failure passed on rerun, and the sccache failures cost 3 extra Windows bundle runs.
- The 4 mutation-gate failures are code by definition, but they look like a *process* choke point: the committed mutation results must be regenerated whenever non-source inputs move. Worth checking whether that regeneration needs the Mac.

**What this says about throughput**
- One CI cycle is p50 16 / p90 20 min, and `strict: true` makes every merge cost every other PR a cycle.
- The critical path is Bundle smoke (windows), and 43% of it is the test-seam proof. That step builds a release test harness **without** `RUSTC_WORKSPACE_WRAPPER=sccache` (it runs `cargo` after the tauri build).
- **Candidate, not built:** route that build through the same sccache server, or reuse artifacts. The Windows job would fall towards ~9–10 min and CI wall towards the macOS native job's ~14 min. That is a rough 2-min p50 saving per cycle, and more on p90. It needs its own measured PR.
- 11 of 50 runs (22%) were cancelled by newer pushes. That is wasted runner time, but not wall-clock on the merge path.

## 2026-09-26 06:53 UTC: Item 5, pinned-surface bottleneck → proposal issue #740

**Proposal only; no code.** Filed as [#740](https://github.com/lamemustafa/bridge/issues/740) (`type:feature`, `area:infra`).
- **Measured:**
  - 80% of master merges touch the surface (40 of 50);
  - `strict: true` with a p50 16 / p90 20 min CI puts the cap at about 3–3.75 merges an hour;
  - pinned PRs additionally need a local re-merge, reseal and re-review after every pinned merge;
  - **63% of consecutive pinned merges touch disjoint pinned files**, so their *only* conflict is the two stored aggregate digests (`manifest_sha256`, `compatibility_surface_sha256`).
- **Options:**
  - **A.** An order-independent seal: store per-file hashes only, and let the gate compute the aggregate. It keeps the per-file "unreviewed Tally-path change" check intact and is code-negative. **Recommended first.**
  - **B.** GitHub merge queue, which requires A. There is a table of how each seal design behaves when the queue tests B on top of A:
    - stored aggregate: B is ejected;
    - reseal in queue CI: impossible, CI can't push;
    - post-merge bot reseal: rejected, it bypasses protection;
    - order-independent seal: works.
  - **C.** Auto-reseal bot: not recommended, because of P8 and a weaker attestation.
  - **D.** Narrow the surface: about 10% gain, and it touches the `MAX_SURFACE_FILES = 280` cap.
- Each option has its security, rollback and expected-gain analysis in the issue. It also covers Lane D's merge-queue questions and flags an unverified risk: whether GitGuardian reports on `merge_group` commits.

## 2026-09-26 06:53 UTC: FINAL, Cloud Lane P

**Ready for review (Lane D merges):**
- [#728](https://github.com/lamemustafa/bridge/pull/728), #723 sccache.
  - `SCCACHE_IDLE_TIMEOUT: '0'`, with `--stop-server` kept strict.
  - Windows proof: old fails, new passes, and a stopped server still fails closed. Full CI green on `a52748f` and `75846c4`.
  - Sonnet and Opus reviews: no P1/P2 open; all 5 findings answered.
  - It is **behind** master, with no conflict. It touches a pinned file, so an update needs a local re-merge and reseal.
- [#688](https://github.com/lamemustafa/bridge/pull/688), #527 Git 2.43 upload-pack trust.
  - Merged up to master; CI green; fresh Sonnet review has no P1/P2.

**Draft, awaiting an owner decision:**
- [#729](https://github.com/lamemustafa/bridge/pull/729), #669 cache retention rule.
  - Round 1 found my P1 (duplicate YAML key, so CI ran 0 jobs) and a P2; both are fixed.
  - Round 2 has no P1/P2. CI is green on `ccef193`.
  - It stays a draft because automated cache deletion is the owner's option-3 call on #669.

**Proposal:** [#740](https://github.com/lamemustafa/bridge/issues/740), the pinned-surface bottleneck.

**Decisions needed:**
1. #723: I deliberately did **not** tolerate the "already stopped" exit 2, because that case comes with fabricated zero statistics. If the owner still wants tolerance, it is an explicit P5 downgrade.
2. #527: a documented minimum Git version (probably ≥ 2.44) for the merge driver's `%S/%X/%Y` placeholders, and/or a driver error naming that cause.
3. #669: adopt #729. Separately, decide whether to delete today's 5 superseded + 2 PR-only entries (5,416 MiB) by hand, or let LRU and the 7-day eviction take them.
4. #740:
   - accept losing the stored self-checksum (option A)?
   - accept review evidence naming the PR head under a merge queue (option B)?
   - choose a batch size.
5. Follow-ups I noticed but did not build:
   - a strict duplicate-key YAML load of the workflows in `check-ci-workflow-consistency.mjs` (it would have caught my #729 P1 locally);
   - routing the test-seam proof build (6.3 min p50, 43% of the Windows bundle job) through sccache;
   - posting the lane-T noexec evidence onto #527 itself.

**Throwaway branches I created (not for merging; deletion is Lane D's call):** `cloud-p/sccache-proof-723`, `cloud-p/cache-inventory-669`.

**Deviations from the brief, recorded:**
- Item 1's fix does not tolerate the stopped case (reason above).
- Item 3 per-step timings come from 21 of the 50 runs, because a container restart lost the full re-fetch; the job-level and failure numbers cover all 50.
- GitHub access was through the MCP API rather than `gh`, which is not available here.
- The cache inventory came from a read-only throwaway workflow, since no MCP cache endpoint exists.

## 2026-09-26 07:46 UTC: Status

- #688 (Git 2.43 upload-pack trust, #527) **merged** by Lane D at 07:46 UTC on head `926f50e`.
- #728 and #729 are unchanged: green, and each trial-merges cleanly with master at `b5039c8`.
