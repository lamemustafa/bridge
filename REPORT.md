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
