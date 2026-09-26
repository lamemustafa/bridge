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

## 2026-09-25 14:48 UTC — E2a review round 1 and fixes (local, not pushed yet)

- Nightly workflow guard re-checked: the issue step is `if: github.ref == 'refs/heads/master'` (line 116), and the file is identical to master on all five lane-e branches.
- Opus review (round 1). Code: no panic or wrap; paise arithmetic is all checked. Findings:
  - **P1:** the reader accepted negative, two-sided and out-of-period rows.
  - **P1:** test gaps. The window's last day could be dropped, or a statement row matched twice, and every test still passed.
  - **P2:** window outside the FY; empty charge term; missing rows or balance keys; row number, doc or account mismatch.
  - **P2:** untested split orchestration, invariants that don't check money, a missing TB row read as 0.
- Sonnet review (round 1): gates pass; no HashMap; fixtures synthetic and balanced. **P2:** the binding of `bank_reconciliation_ledger` is untested. **P3:** E2A-13's wording. Its "P1" was the missing mutation records, which is expected before the full run.
- Fixed in 3 commits (59452ee, 539d449, plus a third). Reader refusals; window-in-FY refusal; empty-term refusal; unit assertions for the last day, (date, GUID) order and one-use statement rows; binding assertions. Mutations E2A-16..23 added, and E2A-01, 07, 13 and 15 redefined. **All E2A-01..23 killed** (sampled, --full). Tests 336 pass; clippy and fmt clean. No golden or fixture touched. 6 tests added in total, all small.
- **Left for Lane D (needs a reference golden, which I can't make):** an edge book that exercises split orchestration. Opus showed these wrong ports all pass today:
  - a forward split that doesn't retire its statement rows;
  - a reverse-split pool that reuses forward-split books rows;
  - a charge pass that overwrites split_settlement;
  - statement order by index only;
  - a split window made exclusive at 7 days;
  - the BANK-2 duplicate check disabled.
  Suggested fixture rows: two books rows competing for one statement row, a split at exactly 7 days, a split statement row whose narration holds a charge term, and a duplicate statement row.
- **Not changed, for Lane D's call:**
  - no invariant checks money (books opening + rows = closing); adding one is new behaviour beyond the reference's check_invariants;
  - a missing TB row reads as 0, as the reference's `map_or(0)` parity does;
  - opening + Σrows ≠ closing is not refused, because bankrec_paths deliberately tests a closing that doesn't tie.
- Round-2 reviews (fresh Sonnet and Opus) running. Then freeze and run the full mutation list.

## 2026-09-25 15:09 UTC — E2a round 2 (local, not pushed yet)

- PR #682 (gate): every check green on f802d6f, and the tax-audit job ran and passed. Waiting on Lane D.
- Round-2 Sonnet: no findings. Round-2 Opus: the refusals are sound and change no accepted figure, but 12 wrong implementations still passed every test. Two are P1: the match ignoring the day gap, and books ordered by GUID only. The rest are split-pass reuse, window and order, a charge pass over splits, and three reader gaps.
- Fixed in 0502d6a:
  - the split unit test now also runs a whole synthetic book, with rows competing for splits by date, index and reuse, and asserts every row's reason;
  - more match assertions;
  - a blank (space-only) charge term is refused;
  - the module docs list the deliberate divergences (window outside FY, blank term, reader refusals).
  - E2A-24..35 added. **All 35 E2A mutations killed** (--full, sampled). 336 tests pass; clippy and fmt clean. Still 6 tests added.
- **For Lane D (P2, can't check here):** the reader now refuses a row whose `row` is not its 0-based position, or whose doc/account differs from the document's, plus negative, two-sided or out-of-period rows. The real statement adapter (`tae.adapters.bank_documents`) isn't in this repo, so I can't confirm it writes rows that way. If it numbers rows from 1, the real-book parity run will refuse every statement. It fails closed, but please check on the first local run.
- A final Sonnet and Opus pass on 0502d6a is running. Then freeze and run the full list.

## 2026-09-25 15:35 UTC — E2a frozen at 6a1fad0; full mutation run started

- Rounds 3–4 of review (fresh Sonnet and Opus each time) found P2 test gaps, now fixed:
  - 657b8b2 pins run's own Re 1 tolerance and the split pools' 7-day bounds and order;
  - b218c8e pins the reverse split's order and its retirement of the statement row.
  The final Opus and Sonnet passes on b218c8e found **no P1 or P2**.
- E2A mutations are now E2A-01..47, all killed (sampled with --full). 336 tests pass; clippy and fmt clean. 6 tests added in all. No golden or fixture touched.
- Merged origin/master (54eb327; no crate or crate-input change) and **pushed `lane-e/e2a-bank-recon` @ 6a1fad0** (fast-forward from 7330fa8). This is the frozen head.
- Full run started: `mutations.py --full --jobs 4`, about 519 mutations, estimated 70–80 min. I'll commit the records and open the draft PR when it finishes.
- PR #682: Lane D pushed 7296886 (merge + reseal). The red "Required checks" is from the superseded run, whose jobs were cancelled by that push. The new run is in progress.

## 2026-09-25 15:57 UTC — PR #682: quoted-path fix pushed

- A reviewer comment on #682 raised a P2: without `-z`, git quotes non-ASCII, `"` and `\` paths, so such a crate file skipped the gate. It was valid; I reproduced it in a scratch repo.
- Fixed in a7b883a (`git diff --name-only --no-renames -z … | tr '\0' '\n'`) and resealed in 1b482dd (`reseal.sh --verify` current).
- A fresh Sonnet review found no P1 or P2. It exercised the step's bash on an empty diff and on ASCII, non-ASCII, space, quote, backslash, deleted and renamed paths. No case selects less than before.
- **Pushed `lane-e/require-mutation-records` @ 1b482dd** (fast-forward from Lane D's 7296886). Replied on the PR and updated the body (candidate SHA, net LOC +27/−6).
- E2a full run: 230/519 done at this point. The only survivors so far are the five in accepted-survivors.json (X11, X12, A04, A19, S08).

## 2026-09-25 16:27 UTC — container restart; E2a full run restarted in shards

- The cloud container restarted at about 16:15 UTC and wiped all local state. The E2a full run (386/519 done) was lost; its records were never committed. Everything pushed is intact: gate 1b482dd, E2a 6a1fad0, this report.
- PR #682: CI on 1b482dd completed with no failures.
- Re-running on the same frozen head 6a1fad0 in 4 shards (`--full --jobs 4 --shard k/4`). Each shard's results and log are pushed to a scratch branch **`cloud/lane-e-e2a-shards`** (never to be merged) as the shard finishes, so another restart loses at most one shard. At the end I'll `--merge` them into `parity/mutation-results.json` on the E2a branch.
- Before the restart the run had found no survivors except the five accepted ones (X11, X12, A04, A19, S08).

## 2026-09-25 17:38 UTC — PR #682 merged; second restart; E2a shards 1–3 done

- PR #682 was merged by Lane D. Afterwards an independent verification comment confirmed the -z fix. Its P3 (the ci.yml comment names only non-ASCII bytes) is not pushed, because it's a wording-only change.
- The container restarted again at about 16:55 UTC. Shards 1–3 of the E2a full run had already finished and been pushed to `cloud/lane-e-e2a-shards`: 130/130, 130/130 and 130/130 pass, with X11 and S08 as their accepted survivors. Shard 4 was lost mid-run and has been restarted.
- If shard 4 also can't finish here, then **E2a is frozen at 6a1fad0 and needs a branch run** for the last shard (or a nightly dispatch on `lane-e/e2a-bank-recon`). Shards 1–3 are on the scratch branch.

## 2026-09-25 17:55 UTC — E2a done: records committed, draft PR #710

- Shard 4 finished (129/129). The whole list on the frozen head 6a1fad0: **519 run, 514 killed, 5 accepted survivors** (X11, X12, A04, A19, S08). No timeouts. All 47 E2A mutations killed. Shards took 12–15 min each.
- Records commit b96afe8 (only `parity/mutation-results.json`), then a master merge b163010 (nothing under the crate or its inputs). `--verify --changed-since origin/master`: 519 selected, 519 proven on crate tree 60079575465d99cc. Tests 336 pass; clippy and fmt clean. A fresh Sonnet pre-push review found none.
- **Pushed `lane-e/e2a-bank-recon` @ b163010**, fast-forward.
- **Draft PR https://github.com/lamemustafa/bridge/pull/710** "E2a: port bank_reconciliation". It has "Real books: pending (local, Lane D)", the failing-first names, the mutation numbers and the P3 list. The two items for Lane D (the adapter row shape, and a split-orchestration edge book needing a reference golden) are in the body.
- Per the queue I'm not opening E2b until #710 merges. Meanwhile I'll do only work that changes nothing under the crate.

## 2026-09-25 18:13 UTC — #710 green; E2b re-stacked, compiled and reviewed (no PR yet)

- #710 (E2a): every check green on b163010, including the now-required "Tax-audit mutation records". Waiting on Lane D.
- **E2b**: merged the E2a head b163010 into `lane-e/e2b-hvr` (273a7fa). The one conflict was `parity/mutations.json`, resolved as the union: E2a's E2A-01..47 (including its redefinitions) plus E2b's E2B2-01..29.
  - This is E2b's **first build**. 340 tests pass.
  - clippy `-D warnings` refused the lakh/crore digit grouping in `high_value_register.rs` (`inconsistent_digit_grouping`). Rewritten as plain groups with rupee comments, values unchanged (a1ebadf). Pushed a1ebadf.
- **Failing first:** at 1884864, 5 tests fail:
  - `high_value_register::tests::{config_values_are_typed_or_refused, the_recipient_type_follows_pack_py}`
  - `edge_books::{a_journal_on_one_ledger_is_refused_not_panicked, every_edge_book_matches_the_reference}`
  - `registry::every_registered_test_matches_its_synthetic_golden`

  At port commit c1bf117 all pass (338).
- **Mutations:** E2B2-01..29 sampled with --full; 28 killed. E2B2-03 (Contra walked) survived: no fixture's Contra has a party line.
- **Reviews, round 1:**
  - Sonnet: gates pass; loans_interest is unchanged by the shared [loans] reader; fixtures are synthetic and balanced; 4 tests added. P2: no direct test that an unknown ledger in `counterparty_type_by_ledger` is refused.
  - Opus: no P1; overflow checked everywhere; no panic. P2: `s194n_terms` accepted a blank term, and every debit then counted. P2, inherited from the reference: a cash row carries the party-side amount, so a party paid partly in cash and partly by bank is a cash row for the whole amount. That over-states, never under-states, and the golden pins it (h15).
- **Fixed in b09ef7c:**
  - blank-term refusal (E2B2-30);
  - Contra unit test (5 tests added in total, all small);
  - the binding assertion;
  - the over-statement written into the module docs as a parity limit.
  E2B2-03 and E2B2-30 are now killed; all 30 E2B2 are killed. 341 tests pass; clippy and fmt clean.
- **P3s noted, not changed:**
  - a paise overflow is reported as `AuditError::Config` (crate-wide pattern);
  - an unknown counterparty type string is kept silently;
  - the CA threshold and the s.269ST limit are both 2 lakh and the threshold is never configurable today, so swapping them is untested;
  - a zero-amount party line is untested.
- Next: round-2 check on b09ef7c, then the E2b full mutation run (its crate tree doesn't change when #710 merges unchanged). I'll open the E2b PR only after #710 merges.

## 2026-09-25 18:57 UTC — E2b frozen at fb9f6e2; sharded full run

- Round-2 Opus on b09ef7c: no P1. Two doc P2s, fixed in fb9f6e2 (docs only):
  - The new bullet claimed rows "never under-state". They do under-state: a voucher naming two or more parties gives each only its own line, and its tax goes to no party (hvr_paths h11).
  - `s194n_terms`' own doc now lists the blank-term divergence.
  Sonnet checks on b09ef7c and fb9f6e2 found none.
- **Pushed `lane-e/e2b-hvr` @ fb9f6e2** (the frozen head).
- **For Lane D, a reference-level audit concern, not fixable here without breaking parity.** `high_value_register` takes a row's amount from the party side, so an s.269ST cash row can **under-state**. Example: a multi-party cash receipt with tax whose cash per party reaches 2 lakh while each party's own line is below it. That row is missed. It can also over-state (a party paid partly in cash and partly by bank; h15). The goldens pin both. It is now documented in the module docs. It may be worth raising against the reference engine itself.
- The container restarted a third time (about 18:50 UTC). The E2b full run is sharded to `cloud/lane-e-e2b-shards`. Shards 1 and 2 are done: 138/138 and 137/137 killed, no survivors. Shards 3 and 4 were rerun after the restart.

## 2026-09-25 19:31 UTC — #710 real books equal; E2b records pushed, PR held

- #710: Lane D's local real-book parity is byte-identical on clients A and B. Client C has no statement, so no bank_reconciliation run and nothing to compare. So the reader's row-shape refusals pass the real adapter's output. #710 is marked ready, is mergeable and green.
- **E2b** full run on the frozen head fb9f6e2 (4 shards on `cloud/lane-e-e2b-shards`, 15–17 min each): **549 run, 544 killed, 5 accepted survivors** (X11, X12, A04, A19, S08). No timeouts. All 30 E2B2 mutations killed.
- Records commit 0f04441 (only `mutation-results.json`). `--verify` against both origin/master and origin/lane-e/e2a-bank-recon: 549 selected, 549 proven on crate tree 8cf87d249d9b4f6b. A Sonnet pre-push check found none. **Pushed `lane-e/e2b-hvr` @ 0f04441.**
- E2b PR: **held until #710 merges** (one port PR at a time). When it merges I'll merge master into E2b (crate unchanged if #710 merges as-is), re-verify, and open "E2b: port high_value_register" as a draft.

## 2026-09-25 19:40 UTC — E3a re-stacked and compiled (local; no PR)

- Merged the E2b head 0f04441 into `lane-e/e3a-stock` (fafe3ad). The one conflict was `mutations.json`, resolved as the union (E3A-01..32 appended).
- E3a adds `Book::stock`. The two field-built unit-test books from the E2a and E2b reviews needed `stock: None` (02027c7).
- This is E3a's **first build**. 344 tests pass; clippy and fmt clean.
- **Failing first:** at 7ce1d55, 3 tests fail, each with `stock: not ported yet`:
  - `edge_books::a_goods_line_without_a_quantity_field_is_refused`
  - `edge_books::every_edge_book_matches_the_reference`
  - `registry::every_registered_test_matches_its_synthetic_golden`

  At port commit 01cb619 all pass (341).
- Mutations: **E3A-01..32 all killed** (sampled with --full).
- **Lane D's condition, the string literals the stock-part reader compares against.** Counted over non-test code in `src/stock_read.rs` and E3a's lines in `src/book.rs`:
  - **Tally data values: 2.** `"Not Applicable"` is BASEUNITS, compared exactly after Tally's reserved-value marker via `xml::reserved_value`. `"yes"` is COMPANY/ISINTEGRATED, after `py_lower`.
  - **Engagement config values: 2**, `"from_masters"` and `"from_read"` (`[stock].{opening,closing}_summary`). Plus one key-presence check, `"items"` (the mixed-config refusal).
  - **Manifest part kinds: 2**, `"stock_items"` and `"stock_summary"`.
  - **XML names looked up: 12 distinct.** STOCKITEM, @NAME, GUID, PARENT, BASEUNITS, OPENINGBALANCE, OPENINGVALUE, CLOSINGBALANCE, CLOSINGVALUE, CLOSINGRATE, COMPANY, ISINTEGRATED.
  - The quantity and rate parsers are pre-existing (master) and match a numeric pattern, not literals.
- The review round (Sonnet + Opus) on 02027c7 is running.

## 2026-09-25 20:00 UTC — E3a reviewed, frozen at 5beb691, full run started

- **Opus round 1 on E3a:** no P1. Paise arithmetic is checked throughout; f64 quantities never reach a paise figure; no panics; the reader fails closed on nearly everything. The fixable P2s were wrong implementations that passed every test while moving a paise figure (all fixed in bd9b3dd):
  - a repeated Stock Summary name keeping its first row instead of its last;
  - nameless summary rows kept;
  - the quantity-field check reading every voucher instead of the population.
  Pinned in bd9b3dd, E3A-33 and E3A-34; both killed. The module docs overclaimed the value-only exclusion; fixed in bd9b3dd and 5beb691 (docs).
- **Sonnet:** no P1 or P2. It confirmed `book.rs` changes are additive only, with no other module's figure moved; 3 tests added.
- **Round 2 (Sonnet and Opus):** no P1 or P2 beyond the doc precision fixed in 5beb691.
- **Pushed `lane-e/e3a-stock` @ 5beb691** (the frozen head). The sharded full run is on `cloud/lane-e-e3a-shards`.
- **For Lane D (design, reference-level, not changed):**
  - `[stock].opening_date` and `closing_date` are never checked against the engagement period, and not for opening ≤ closing either. A closing_date of 2025-12-31 gives `gap_closing_paise` against the full-year TB movement, without refusal. Requiring them to equal the period bounds would be a divergence that could refuse real configs (for example, an opening as of 31 March). It needs a decision from real configs.
  - Unverified live: an opening Stock Summary taken "as of" the period start may already include 1 April movements, which the walk then adds again. This question is about the reference's design.
- **P3s:**
  - `(-)5 Nos` reads as +5;
  - `N/A` reads as None;
  - item names are matched case-sensitively;
  - the pre-existing `paise("+-1.00")` reads as positive;
  - the pre-existing `number_match_end` is quadratic (40k commas take 1.6 s);
  - M06/B05 is a duplicate mutation, from before this branch.

## 2026-09-25 20:04 UTC — #710 merged; E2b draft PR #713

- #710 was merged (squash, 72a1eb7). Master's crate tree is byte-identical to E2a head d263950.
- E2b: merged origin/master into `lane-e/e2b-hvr`. The squash made 12 crate files conflict. I resolved them by restoring the crate directory from the branch's own head 0f04441, which is exact because master's crate equals E2a's.
  - HEAD's crate tree equals 0f04441's (c4a9908c). The 6 non-crate master files are carried byte-for-byte.
  - `--verify` against master: 549/549 proven. A Sonnet check found none.
- **Pushed `lane-e/e2b-hvr` @ ab80466.** **Draft PR https://github.com/lamemustafa/bridge/pull/713** "E2b: port high_value_register", with "Real books: pending (local, Lane D)".
- **Note for the stack:** because the ports are squash-merged, each later branch (E3a, E4) will conflict the same way when master takes its predecessor. I resolve it the same way, after checking that master's crate tree equals the predecessor's head.
- E3a full run in progress on `cloud/lane-e-e3a-shards`.

## 2026-09-25 20:30 UTC — #713 green; fourth container restart; E3a shards 2–4 rerunning

- #713 (E2b): every check green on ab80466, including the required mutation-records check. Waiting on Lane D (real books, merge).
- The container restarted again at about 20:25 UTC. E3a shard 1 had been pushed (146/146 killed). Shards 2–4 are rerunning on the frozen head 5beb691.

## 2026-09-25 20:48 UTC — #713 held by Lane D

- #713 real-book parity is byte-identical on clients A, B and C (Lane D). #713 was then **held** by Lane D. The h15 behaviour (a mixed cash-and-bank voucher raising a 31(bc) cash finding for the party's whole amount) is to be fixed in the reference engine first. #713 then takes new goldens, plus a `limits` line and a figure definition naming the cash-line amount, followed by a fresh review.
- I can't produce goldens here. **E2b is blocked on Lane D's reference change and goldens.** When they arrive (pushed to `lane-e/e2b-hvr`, or as a message), I'll port the code change, redo the E2b mutation records on the new tree, and re-stack E3a and E4.
- Meanwhile: E3a's full run continues. Its records will be made on E3a's current tree (5beb691), so they'll need redoing only if the E2b change alters files E3a carries, which a golden or `high_value_register.rs` change will. E3a's PR stays unopened (one port PR at a time).

## 2026-09-25 21:41 UTC — E3a records pushed (no PR; the stack is held at #713)

- A fifth container restart (about 21:15 UTC) cost only shard 4, which was rerun. E3a full run on the frozen head 5beb691: **583 run, 578 killed, 5 accepted survivors** (X11, X12, A04, A19, S08). No timeouts. All 34 E3A mutations killed. Shards on `cloud/lane-e-e3a-shards`.
- Records commit e1b6550 (only `mutation-results.json`). `--verify --changed-since origin/lane-e/e2b-hvr`: 583/583 proven on crate tree 538b799cc815a446. A Sonnet pre-push check found none. **Pushed `lane-e/e3a-stock` @ e1b6550.**
- **Status of the stack:**
  - E2a merged (#710).
  - E2b #713 is held for Lane D's reference change and goldens (h15, cash-line amount).
  - E3a is ready, but its records will need redoing after the E2b change: a golden or `high_value_register.rs` change moves the crate tree. The PR stays unopened until #713 merges.
  - E4 is not started (its build would compete with nothing now, but it would have to be redone after E2b and E3a change, too).
- **Waiting on Lane D for:** the E2b reference change and goldens. When that lands on `lane-e/e2b-hvr` (or as instructions), I'll port the code side, rerun E2b's full list, re-stack E3a and redo its records, then continue with E4.

## 2026-09-25 21:48 UTC — E4 re-stacked and compiled (local; no PR)

- Merged the E3a head e1b6550 into `lane-e/e4-party-monthly` (c8389f1). `mutations.json` union: E4-01..25 appended, no id clash.
- **E4's first build:** 347 tests pass. clippy `-D warnings` refused six explicit derefs in `party_monthly.rs` (`explicit_auto_deref`). Fixed mechanically, and E4-22's `from` follows its line (3459afc). clippy and fmt now clean; no stale mutation.
- **Failing first:** at 4efcf7c, 5 tests fail, each with `party_monthly: not ported yet`:
  - `edge_books::{a_party_tag_equal_to_a_fixed_row_is_refused_not_panicked, pwm_1_fires_on_a_total_or_movement_that_does_not_tie, pwm_2_fires_on_a_party_row_or_an_others_label_that_is_wrong, every_edge_book_matches_the_reference}`
  - `registry::every_registered_test_matches_its_synthetic_golden`

  At port commit 4e68199 all pass (344).
- Mutations: **E4-01..25 all killed** (sampled with --full).
- Review round (Sonnet and Opus) on 3459afc running. The E4 full run and records wait until E2b and E3a settle, since any E2b change moves every later crate tree.

## 2026-09-25 22:09 UTC — E4 reviewed and pushed (no records, no PR); stack waiting on Lane D

- **Opus round 1 on E4:** no P1. The arithmetic is checked (the i128 sums in PWM only widen) and no panic was found. The P2s:
  - wrong implementations passed every test while moving paise: the period's first day excluded, a TB difference of exactly Re 1 made a finding, excluded vouchers matched strictly or of any date, ranking by signed year or counting outside amounts, ties by descending name, a note after the period counted in returns, the opening balance published without its sign;
  - `batch-e4.md` claimed pm_paths reaches a Cust C/D tie, but the golden has them at 120,000 and 100,000.
- **Fixed** in d85e238 and 1e7f14d:
  - one whole-run unit test on a synthetic book (E4 now adds 4 tests);
  - E4-26..35 recorded;
  - the provenance note corrected. The fixture's own comment is left as is, because its bytes are what the goldens were made from.
- **Sonnet round 1** found no P1 or P2. Round 2 (Opus and Sonnet) found only the lower date bound, which is now pinned. **All 35 E4 mutations killed.** 348 tests pass; clippy and fmt clean.
- **Pushed `lane-e/e4-party-monthly` @ 1e7f14d.** No full run and no records yet: they would be invalidated by the E2b change.
- **Where the stack stands:**
  - E2a merged (#710).
  - **E2b #713 is held by Lane D:** the reference fix, then new goldens, a `limits` line and a cash-line figure definition. Nothing on the branch yet as of 22:10 UTC.
  - E3a is ready (e1b6550, records on its current tree).
  - E4 is reviewed (1e7f14d).
- **Next, once Lane D pushes the E2b change:**
  1. port and review it;
  2. rerun E2b's full list;
  3. re-stack E3a, then E4, redoing their records;
  4. open each PR in turn.

## 2026-09-26 01:59 UTC — E2b: Lane D's reference change ported (local, reviews running)

- Lane D pushed df9ac5e7 on `lane-e/e2b-hvr`: `parity/PORT-NOTE-HVR.md` and the two regenerated goldens (reference 140bc7d3).
- **Ported** in 01cc4d46:
  - each row keeps per-GUID (share, money line), added per occurrence;
  - a (party, day) row that raises a finding and differs gains the definition suffix, the `{mode}_line` figure and fact, and the limit line (the below-threshold sentence only when the line total is under the threshold), placed before the unidentified-party limit;
  - both regenerated goldens now match byte-for-byte.
- E2B2-31..36 record the new logic. Four of them survived the goldens (marking on any vs all vouchers, the line's direction, the line summed per voucher, a repeated GUID). One unit test pins them (59577a29), which is the 6th test E2b adds, all small. **All 36 E2B2 killed.** 342 tests pass; clippy and fmt clean.
- Fresh Sonnet and Opus reviews of the port against the note are running. Next: push, the E2b full mutation run (sharded to `cloud/lane-e-e2b-shards-2`), records, then the fresh independent review Lane D asked for on #713. After that, re-stack E3a and E4 and redo their records.

## 2026-09-26 02:12 UTC — E2b port reviewed and pushed (fa9a9d0c); full run started

- Opus review of the port found no P1. Its two P2 test gaps were an unidentified row that differs, and marking by row totals instead of per GUID. Both are pinned by extending the unit test (fa9a9d0c), and E2B2-37..39 added. **All 39 E2B2 killed** (sampled). A Sonnet review of the port and a check of fa9a9d0c found none.
- **Pushed `lane-e/e2b-hvr` @ fa9a9d0c** and commented on #713. E2b full run started, sharded to `cloud/lane-e-e2b-shards-2`.

## 2026-09-26 03:20 UTC — E2b records pushed (6e2b984f); #713 ready for Lane D's review

- E2b full run on fa9a9d0c: 4 shards (13–14 min each), all on `cloud/lane-e-e2b-shards-2`, merged with `--merge`. **558 run, 553 killed**; the 5 survivors are the accepted X11, X12, A04, A19, S08. All 39 E2B2 killed. `--verify --changed-since origin/master`: 558/558 proven on crate tree e6a7aab84c779eb1.
- 342 tests pass at 6e2b984f. Sonnet pre-push check: none (records-only; records equal the shard merge key-for-key; fast-forward).
- **Pushed `lane-e/e2b-hvr` @ 6e2b984f**, updated #713's body (candidate SHA, 342 tests, E2B2-01..39, 558/553, review record, net LOC +6921/−565 crate, +1526/−24 source and tests, 6 tests) and commented there.
- Waiting on: Lane D's fresh independent review and the local real-book re-run at 6e2b984f. Next for this lane: re-stack E3a (and then E4) on 6e2b984f locally; their PRs open only after #713 merges.

## 2026-09-26 03:20 UTC — E2b real books EQUAL; E3a re-stacked locally

- Lane D merged master into `lane-e/e2b-hvr` (b47e154b; crate tree identical to 6e2b984f) and reported real-book parity **EQUAL on all three clients** against reference 140bc7d3. #713's body now names b47e154b as the candidate and carries the real-book line. CI's earlier "Required checks" failure on 6e2b984f was a superseded, cancelled run; the mutation-records check passes.
- Follow-up Lane D named (not for #713): re-vendor the rules at 140bc7d3. Deferred here, since the crate stays frozen until the stack merges.
- E3a: merged 6e2b984f into `lane-e/e3a-stock` locally (16fb0f06): `mutations.json` union (592 ids, none edited), one `stock: None` in E2b's new test book. 345 tests pass; clippy and fmt clean. Full run started, sharded to `cloud/lane-e-e3a-shards-2`; a Sonnet review of the merge runs before any push.

## 2026-09-26 03:50 UTC — #713 held again (Lane D: reference fix for the "below" rows); E3a run resumed after a restart

- Lane D holds #713 on the independent review's narrowed P1: a flagged row whose money line is `below` the threshold keeps its at-or-over title, its clause tags and its place in the at-or-over count. The reference is fixed first (row stays listed with a true title, drops the statutory tags, leaves the at-or-over count, a separate count reconciles), then goldens and port note regenerate. This lane ports it when it lands on `lane-e/e2b-hvr`, then E2b full run, records, fresh review.
- A container restart killed E3a's full run on 16fb0f06 after shard 1 (148 killed, pushed). Worktrees rebuilt from origin; shards 2–4 rerunning. These records will be superseded once E2b changes again (E3a must re-stack), but the run is not interrupted.
