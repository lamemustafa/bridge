# Cloud Lane E report

Append-only log. Newest entry at the bottom. This branch is never merged.

## 2026-09-25 14:22 UTC — session start

- Toolchain: rustc 1.96.0 (pinned by rust-toolchain.toml), 4 CPUs.
- master = a51ae78. Remote heads match Lane D's table (gate 3eb9db45, E2a 7330fa85, E2b 1cd0d02d, E3a f94811e8, E4 2dd8bb63).
- Starting queue item 1 (gate branch).

## 2026-09-25 14:32 UTC — item 1 (gate) pushed, PR #682

- `lane-e/require-mutation-records` @ f802d6f (fast-forward from 3eb9db4). origin/master a51ae78 merged with --no-ff; reseal as separate commits; `scripts/reseal.sh --verify` current.
- Sonnet review 1 found a P2: `git diff --name-only` hides a file renamed out of the crate. Fixed with `--no-renames` (f60fa7c) and resealed again (f802d6f). Sonnet review 2 found no P1 or P2; a P3 (the nightly workflow file isn't in the gate) is noted in the PR body.
- PR https://github.com/lamemustafa/bridge/pull/682, opened as a **draft** (this session's harness opens every PR as a draft; mark it ready when you want it). Stopping there on this PR.

## 2026-09-25 14:32 UTC — item 2 (E2a) in progress

- Merged origin/master into `lane-e/e2a-bank-recon` (f473a44, local).
- Failing-first at 4a6b9a9 (4 fail): edge_books::{a_repeated_books_guid_is_refused_not_panicked, edge_runners_agree_across_the_two_sides, every_edge_book_matches_the_reference}, registry::every_registered_test_matches_its_synthetic_golden. At the port commit ddcfdfd, 3 pass; `edge_runners_agree_across_the_two_sides` passes only from bdaa814 (EDGE_TESTS wiring).
- Merged head: 334 tests pass, clippy clean, fmt clean.
- **Finding:** sampled the E2A mutations. E2A-01 (equivalent: `(gap,si) <= x` can never tie because si is unique), E2A-02 (exact Re 1), E2A-03 (exact 7 days), E2A-07 (unreachable: books rows are never zero) and E2A-09 (voucher on the window's first day) **survive**. No fixture row sits on those bounds. Fixing with 2 small unit tests plus redefinitions of E2A-01 and E2A-07. No golden touched.
- Timing: about 8 s per mutation at 4 jobs with warm workers, so a full run of about 487 is roughly 65–75 min. It fits this session.
