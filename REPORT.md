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

## 2026-09-26T07:48Z: #717 (part 1) draft PR #743 (pinned)
Head `2db5ba5ee9f1c3d30057bef8ebc91556ad5c378a`. Before this change, the queue's group re-read reported `post_queue_read_failed` with no cause. It now reports `group_export_invalid` with the snapshot parser's cause, through the same helper the pre-approval read uses. The tests failed first; the reverted-cause proof is in the PR. Posting path, so both Sonnet and Opus reviews are pending. Part 2 (the STATUS class) is still waiting on the owner's decision.

## 2026-09-26T07:55Z: #743 (#717 part 1), ready (pinned, posting path)
Head `d8f4a626ec3dfdac10f2c14be51cb447d5d601b1`. Opus: no P1/P2; its P3 (ADR 0004 row 12 count and list) is fixed and resealed. Sonnet: no findings (e2e module 82/82). Needs an Opus independent reviewer.
CI: #731 (`5da9116`) and #736 (`3fd05f5`) are green after the master merges.

## 2026-09-26T08:49Z: check-in
Master gained #712 (batch posting D2a), #688 and #741. #731, #736 and #743 conflicted only in the compatibility JSON. For each I merged master, took master's JSON, re-ran the reseal, verified it, and tested the merged tree. Merge commits carry the session trailer.
- #731 `253b74fc2288f73d0d2554acd7976a9192e01f8a`: approved_import (14) and seam gate (10) pass.
- #736 `e7c42681333769b8d39a93e39017a0fcceffffa7`: amend tests (27) pass.
- #743 `e499614bb36dc18d9fc4721ea6e494765d8b0543`: the full bridge suite passes on the merged tree with #712 (1414; only the known root-host red fails), and clippy is clean.
- #733 and #734: trial merges with master are clean; left for Lane D.

No human review threads are open.

## 2026-09-26T10:20Z: check-in, and #711 taken up
Master gained #721 (batch posting D2b) and #742. I merged master again into #731 (`a13239a26e1dedbf56a4213e9028c683d01b927f`), #736 (`6b8e1a358364c78c3809b0ef1ef771ec97912dce`) and #743 (`69c1f6a09d75dd3c269bc892d8c9cea609e86e10`). Each conflicted only in the surface JSON and was resealed and tested; the full suite passes on #743's tree. #733 and #734 trial-merge cleanly.

With #712 and #721 merged, #711 was no longer blocked by overlap. **Draft PR #749** is at head `5e9ba027bca141a7b9cae4ea801365b5fa38a63e` (pinned, posting path). The four under-lock refusals keep their own codes, and the append and later steps keep the catch-all. The approval seam gate caught an `impl From` in approved_import.rs, and I reworked the code to build the variants explicitly. `attempt_recorded` reports what the journal shows (null / true / false / false), not false in every case as the issue asked; the reason is in the PR. The mutant proof is in the PR. Sonnet and Opus reviews are pending. ADR 0004 row 12 is one line that #743 also edits, so whichever merges second needs a line merge.

## 2026-09-26T10:32Z: #749 (#711) ready (pinned, posting path)
Head `463d2aed62bd5a3f63196a9454247c0852f37e26`. Opus found no P1. Its P2 (ADR row 14 still named the catch-all) and one P3 (README codes) are fixed and resealed; the other two P3s are answered on the PR. Sonnet had no findings. Needs an Opus independent reviewer.

## 2026-09-26T10:38Z: #734 merged
#680 is fixed on master. Lane D merged master into the branch (`3b45db6`) and then merged it. The session is unsubscribed.

## 2026-09-26T11:25Z: FINAL (budget end)

### PRs
| PR | Issue | Head | State |
|---|---|---|---|
| #727 | #662 unbounded manifest.json read | `b5039c8` on master | **merged** |
| #734 | #680 pre-split carry-forward test | `bd0d4077` on master | **merged** |
| #731 | #689 review child exits 0 without its token → unavailable | `267c603965b58a6bb453221dab879735575e924c` | ready, pinned; **approval path: needs an Opus independent reviewer** |
| #733 | #696 lab tools read LINEERROR from the bounded outcome | `8831cc9c088a81a5a983045a70dfed097a6bec6a` | ready; Lane D's merge-of-master head is green |
| #736 | #632 (b)-text in the build result (partial) | `b2e689b9938a0dfabf9ce7fac9dc7f66bb1fc4f9` | ready, pinned; **optional**, since #639 already covers the tool description |
| #743 | #717 part 1: cause on the in-queue group re-read | `76046e4c7b77090ca21d3ee05e8945dee5ed93bb` | ready, pinned; **posting path: needs Opus** |
| #749 | #711 under-lock refusals keep their own codes | `5e15d1000d56348d838acab5d7d6f6ce65def181` | ready, pinned; **posting path: needs Opus** |

In this final check-in I merged master (#739, #734, #750) into #731, #736, #743 and #749. Each conflicted only in the surface JSON, was resealed with `--verify` clean, and passed its targeted tests and the seam gate. Their CI is re-running on these heads. #733 merges cleanly.
#743 and #749 both edit ADR 0004 row 12, which is one line, so whichever merges second needs a line merge.
None of the open PRs has an open review thread. Every Sonnet/Opus finding was answered on its PR.

### Skipped
- #644: owned by Lane E per the plan on the issue (crate-wide `fig`→`Result`), and overlaps the lane-e files.
- #717 part 2: STATUS classing is a design decision; the question is on the issue.
- #718: over 300 lines plus overlap with #715; a 3-slice split is proposed on the issue.
- #702: design choice (`.cmd` or a test binary) plus a Windows mutation run; the question is on the issue.
- #557: blocked on #686 (tool disabled).
- #667: the owner's trade-off.

### Needs lab
None. No fix in this lane needed a new Tally capture.

### New issue
- #735: `parse_import_outcome` keeps CDATA markup in LINEERROR text, and refuses the whole outcome when a CDATA holds a bare `&`. Verified by a direct probe. Latent: not observed live.

### Process notes
- The first overlap checks used a stale origin/master; they were corrected and re-run.
- One merge commit on #731 (`5da9116`) lacks the session trailer. It was not force-pushed.
- CI "Required checks" reds on superseded heads were cancellations. One macOS bundle-smoke failure was a runner DNS failure; it passed after one re-run.

## 2026-09-26T11:31Z: #735 taken up (at Lane D's request)
Branch `cloud-q/735-lineerror-cdata`. The five open PRs are parked; I am not re-merging master into them.

## 2026-09-26T11:37Z: #735 pushed
Branch `cloud-q/735-lineerror-cdata`, head `2e66550a4462f9434994d9e02f6a626b66a9fef1` (pinned: 2 files resealed). One LINEERROR reader for outcome and evidence, handling CDATA and references. The derived tests failed first; the captured digest is pinned unchanged; a mutant that drops the CDATA arm fails. PR and Sonnet review next.

## 2026-09-26T11:37Z: #735 draft PR #763
Head `2e66550a4462f9434994d9e02f6a626b66a9fef1`. Sonnet review running. The five parked PRs (#731, #733, #736, #743, #749) were not touched after the final check-in, per Lane D.

## 2026-09-26T11:46Z: #735 PR #763 ready (pinned)
Head `59cf3da6a3c8e6e7b5b8b72d623f4e93588767f8`. Sonnet found no functional defect; its mutation test confirms the CDATA test fails without the fix and that the captured digest is unchanged. Its P3 (a doc comment overclaimed sanitisation for the new caller) is fixed and resealed. Lane Q is done: #763 and the five parked PRs are waiting on Lane D.

## 2026-09-26T12:18Z: #763 merged up with master
CI was green on `59cf3da`, but GitHub reported a merge conflict after master took #751. The conflict was only in the compatibility surface and matrix. I took master's copies, resealed (`--verify` clean), and pushed merge head `753d7c7ea7b28bd6696812d333df61ceba287dce`. `bridge-tally-protocol`: 350 passed; the targeted `bridge` tests: 119 passed; fmt and clippy clean. The five parked PRs were not touched.

## 2026-09-26T12:52Z: #763 merged up with master again
CI was green on `753d7c7` (14/14 completed, 12 passed, 2 skipped), then master took #757 and the compatibility JSON conflicted again. Same procedure: took master's surface and matrix, resealed (`--verify` clean), and pushed merge head `288c982bbdef4c0463e4d7c5ce4f3d6e15141f35`. `bridge-tally-protocol`: 350 passed; the targeted `bridge` tests: 119 passed; fmt and clippy clean. The five parked PRs are all green on their heads and were not touched. Their mergeability against current master is Lane D's, per its instruction.

## 2026-09-26T13:13Z: #763 merged up with master (#755)
CI was green on `288c982` (14/14 completed, 12 passed, 2 skipped). Master then took #755, which also touches the compatibility surface. Same resolution: took master's JSON, resealed (`--verify` clean), and pushed merge head `569abd6a0c3eb2dc469782e1e70c4c49e6c3817f`. 350 protocol tests and 119 targeted `bridge` tests passed; fmt and clippy are clean. Each master change to the surface will conflict this PR again, and each needs only a reseal.

## 2026-09-26T13:57Z: #763 merged up with master (#762)
CI was green on `569abd6` (14/14 completed, 12 passed, 2 skipped). Master then took #762, which also touches the surface. I took master's JSON, resealed (`--verify` clean), and pushed merge head `e7c43ed2bdf6d9b4bd0e2eab4c54bae5a37659f9`. 350 protocol tests and 119 targeted `bridge` tests passed; fmt and clippy are clean.
