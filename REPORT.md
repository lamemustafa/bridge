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
