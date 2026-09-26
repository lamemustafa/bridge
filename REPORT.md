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

## 2026-09-26T06:27Z: #696 PR #733, ready
Head `3792b5f84dc5e9d35cacf7812e7e648e260f27ad`. Net -33 LOC. Sonnet: a P2 (CDATA in LINEERROR, in the shared protocol reader, pre-existing for the `&` hard failure) is filed as **#735**, not fixed here. The P3 (empty LINEERROR) is fixed.

## 2026-09-26T06:27Z: #680 draft PR #734
Head `959c2e15839abfa00e5ba11eaab3b12c8cd6496b`. Test-only (+109). The mutant that disables the pre-split branch fails only the new test (151 other window tests pass). Sonnet review is pending.

## 2026-09-26T06:27Z: #632 draft PR #736 (pinned; part of #632)
Head `4e6000e944d0259fde56eff9c4a0af5dbb2fa0be`. Option (b)-text only, put in AMENDMENT_WARNING/NEXT_STEP. The catalog line is left alone because #708 edits it. (a) and the (b) refusal stay deferred. Sonnet review is pending.

## 2026-09-26T06:27Z: #557 skipped
Blocked on #686, per the owner's comment. The tool is disabled, and the fix lands with the qualification.

## 2026-09-26T06:27Z: #667 skipped
The owner's trade-off ("Not implemented; the owner's call"). The mutation runner also sits beside `lane-e`'s `mutations.json` work.

## 2026-09-26T06:27Z: new issue #735
parse_import_outcome keeps CDATA markup in LINEERROR and refuses the whole outcome on a CDATA with a bare `&`. Verified by a direct probe. Latent: not observed live.

## 2026-09-26T06:32Z: #632 PR #736, ready (pinned), scope corrected
Head `f8d904d5830e0a61a8aa9d5a9ec2cbf2242f5146`. The review found that #639 already put the (b)-text in the `build_import_xml` description, so #736 only repeats that sentence verbatim in the build result (warning and next step). It is optional; close it if the description is enough. Sonnet had no P1/P2; its two P3s (wording, placement) are fixed. Resealed in its own commit.

## 2026-09-26T06:32Z: CI note
The red "Required checks" events on #727 (d34e77c, 7c7a739), #731 (f222f2e) and #733 (db04bd2) were all `native`/`bundle-smoke` jobs **cancelled** by a newer push, not failures. #731's current head passes Required checks. #727's current head is Lane D's merge of master, 109d6bf.

## 2026-09-26T06:37Z: #680 PR #734, ready
Head `cb57941d301920b6f110cff1f912a4866fb0f73a`. Sonnet: no P1/P2. Two independent mutants of the pre-split predicate each fail only the new test (plus the predicate's own unit test), and the control stays green. The P3 (doc comment) is fixed.

## 2026-09-26T06:37Z: all five Lane Q PRs are ready
#727 (#662), #731 (#689, pinned, approval path: route an Opus independent reviewer), #733 (#696), #734 (#680), #736 (#632, pinned, optional). None is merged by me.

## 2026-09-26T06:42Z: #727 merged
#662 is fixed on master by Lane D's merge of PR #727. The session is unsubscribed from it.

## 2026-09-26T07:18Z: CI status
- #731 (`b2865cb`), #733 (`3792b5f`), #734 (`cb57941`): green.
- #736 (`f8d904d`): green after one re-run. Bundle smoke on macOS had died in `rustup toolchain install` on a runner DNS failure before any build step; it is explained on the PR.
- #727: merged.

All four open PRs are ready and wait only on Lane D's independent reviewer. #731 is on the approval path and needs Opus.

## 2026-09-26T07:37Z: check-in
Master gained #727 (mine) and #708 (lane-f). #731 and #736 conflicted only in the compatibility surface/matrix JSON. I merged master into each: master's JSON was taken and `scripts/reseal.sh` was re-run. Each changed 1 pin, and `--verify` is clean.
- #731 is now `5da9116007ba2447cb2e805d3560301fe8bea3a3`. The approved_import tests (14) and approval_seam_gate (10) pass. Its merge commit carries git's default message, without the session trailer. I noted this rather than force-push.
- #736 is now `3fd05f5cfee3429ba223db8dce2be48dc1ee5d09`. The amend tests (27) pass.
- #733 and #734 are behind master with no conflict and are left for Lane D's pre-merge update.

No review threads are open on any of them. The only bot comments are Codex usage-limit notices.

Since #708 merged, #717 part 1 (the cause on the in-queue group re-read) is no longer blocked by overlap. I'll take it next.
