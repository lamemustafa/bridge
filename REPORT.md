# Cloud Lane Q report

Fast-forward only; never merged. Newest entry last.

## 2026-09-26T05:42Z: #644 skipped
Owned by Lane E per the plan on the issue (comment 2026-09-25): a crate-wide `fig` -> `Result` change of about 290 sites, after the E stack. It also overlaps `lane-e/e2b-hvr` (`high_value_register.rs`, `bank_reconciliation.rs`, `lib.rs`). No action.

## 2026-09-26T05:42Z: #662 draft PR #727
Branch `cloud-q/662-manifest-cap`, head `d34e77c4347525622c8dbfa2c5963b93f1ecac93`. The manifest read is capped at 64 MiB and refused as `C1-size`, and the scanner's stale pointer is fixed. Net +49/-5. Reverted-fix proof is in the PR. Sonnet review is pending. Open on the issue: whether to wire the scanner into CI (it would touch ci.yml/package.json, which other PRs are changing).

## 2026-09-26T06:11Z: correction
My first overlap checks used a stale `origin/master` (a51ae78, 16 commits behind 8ad6544). I recomputed them against 8ad6544. #727's base is behind, but its files are unchanged since, and it merges cleanly.

## 2026-09-26T06:11Z: #662 PR #727, ready
Head `7c7a7399c3a56c4ff6b2bedb922fcaa0c8fe9099`. Sonnet review: no P1/P2. One P3 (the part-count estimate) is fixed. Marked ready for the independent reviewer.

## 2026-09-26T06:11Z: #711 skipped (overlap)
The fix is in the `before_dispatch` closure of `agent_import_post.rs` (lines ~425-445). #712 and #721 both edit that hunk (`@@ -430,7 +451,9`). #666's `PreIntentQueueRefusal` is on master. Redo after #712/#721.

## 2026-09-26T06:11Z: #717 skipped (overlap + design)
Part 1: #708 (`lane-f/626`) rewrites the lines around the in-queue group re-read in `recheck_import_admission`. Part 2 needs an empty/absent STATUS class decision. The question is on the issue.

## 2026-09-26T06:11Z: #718 skipped (size + overlap)
There are 36 `bail!` sites plus the sibling parsers, well over 300 lines. #715 touches `native_ledger_collection.rs`. A 3-slice split is proposed on the issue.

## 2026-09-26T06:11Z: #689 PR #731, ready (pinned)
Head `b2865cb463ef558f785ba995e9298ed8016b06cf`. Approval path. Sonnet: no findings. Opus: no P1/P2; P3-a (seam-gate mutation rows) fixed, P3-b replied. Reseal is its own commit. Needs the independent (Opus) reviewer from Lane D.

## 2026-09-26T06:11Z: #702 skipped (design + Windows)
It needs a choice between a `.cmd` stand-in and a test binary, plus a Windows mutation run. The question is on the issue.

## 2026-09-26T06:11Z: #696 draft PR #733
Head `db04bd28aa1d61a6984fda7024ad32a0c2e3b36c`. Net -49 LOC. Sonnet review is pending.
