# Lane V (independent reviewer, cloud) report

One entry per PR head reviewed. The PR comment is the canonical record; this file is an index.

## 2026-09-25, first queue

| PR | Head | Result | Comment |
| --- | --- | --- | --- |
| #665 post-dialog nonce token | cb711fc8689178da0c87fdc629b7b573dbf25b46 | No P1/P2. P3: a no-token clean exit is reported as `import_approval_declined`, not unavailable; the macOS checkbox is ticked with no live dialog. | https://github.com/lamemustafa/bridge/pull/665#issuecomment-5834759547 |
| #682 tax-audit mutation check required | 0b4061daeaa6fbe8578508ab5dd26c13aa00389d | P2: `changed_files` lacks `-z`, so a quoted (non-ASCII) crate path skips the job (reproduced). The `native` and `bundle` gates share it. | https://github.com/lamemustafa/bridge/pull/682#issuecomment-5834768920 |
| #684 batch post slice C `target_voucher_step` | 6cc206644c41a7a022ead25a6f34bf29b62105be | No P1. Report-only is confirmed. P2: "step above CREATED means another voucher changed" is contradicted by the PR's own ALTERED 50 row. P3: no test pins that the verdict ignores the step. | https://github.com/lamemustafa/bridge/pull/684#issuecomment-5834793283 |

What was run (Linux, pinned 1.96.0, `--features rfd/gtk3`):

- #665: `approved_import` tests 7 passed; `approval_seam_gate` 8 passed; a constant-nonce mutation was killed.
- #684: `agent_import::post` 119 passed; `catalog` 51 passed.
- #682: the `ci.yml` seal matches; `check-ci-workflow-consistency` passes; the quoting miss was reproduced in a scratch repo.

## 2026-09-25, open PRs without an independent review (oldest first)

| PR | Head | Result | Comment |
| --- | --- | --- | --- |
| #642 MCP outstandings on several-currency books | 7afce72f8f1504b0101e1614297319680fd9aec3 | No P1/P2. Foreign bills are excluded or refused; the omission is loud. The PR's named blocking capture gap still stands. | https://github.com/lamemustafa/bridge/pull/642#issuecomment-5834878917 |
| #666 post_queue_read_failed before the intent | 76a99f50842801572d98f3b0de99c33011365de8 | No P1. P2: a merge hazard with master's #641 arm and test (keep `post_catalogue_unreadable` first). P3: the gate forbids only `post_probe_xml(`. A mutation marking the POST's own error was killed by the e2e test. | https://github.com/lamemustafa/bridge/pull/666#issuecomment-5834950609 |
| #672 listing snapshots | 7f11e4143f281ba68cb1d5edce43407295daec58 | No P1/P2. The extent pinning was verified for all three kinds. P3: the TB text calls a gateway delete's effect unmeasured, though §11c.5 records +2; a served page's top-level `read_at` is the call time. The PR is conflicting after #665. | https://github.com/lamemustafa/bridge/pull/672#issuecomment-5835019269 |
| #688 merge-driver test on Git 2.43's upload-pack | bdb8e5369793ade09dbc6948ea76b33b8f2a595b | No P1/P2. Verified on Git 2.43. Both mutations killed: the fix removed, and `safe.directory=*`. P3: the over-trust control does not run on a Git that serves untrusted sources. | https://github.com/lamemustafa/bridge/pull/688#issuecomment-5835035500 |

## 2026-09-25, delta reviews after the heads moved

| PR | Head | Result | Comment |
| --- | --- | --- | --- |
| #682 | 72968866b2d715630ce5989f51c11229841a8768 | Master merge plus reseal only; `ci.yml` is byte-identical and its seal matches. The earlier P2 stands. | https://github.com/lamemustafa/bridge/pull/682#issuecomment-5835043500 |
| #684 | 373b167f8ff1e6db25f87079c55cda496e96f94d | Master merge plus reseal only; the net diff hash is identical and all 280 pins match. The earlier P2/P3 stand. | https://github.com/lamemustafa/bridge/pull/684#issuecomment-5835048200 |

Also: #665 merged (54eb3271) after this lane's review.

## 2026-09-25, heads messaged by lanes directly

| PR | Head | Result | Comment |
| --- | --- | --- | --- |
| #695 slice B LINEERROR text | b4d7bd5041975835c7a57c85ec57f7f091d3c947 | No P1/P2. No verdict reads the text; reading back is idempotent; a Format mutation was killed. The PR is dirty against master. | https://github.com/lamemustafa/bridge/pull/695#issuecomment-5835866526 |
| #684 slice C | 70cdda00af34ef6f88f0acd48647fdae12efa28b | The P2 and P3 are resolved (step-2 posted_verified test); 168 tests pass; 280 pins match. | https://github.com/lamemustafa/bridge/pull/684#issuecomment-5835899884 |
| #682 | merged as a8324c66 | Merged with the `-z` fix (ci.yml:69-70): the P2 is resolved. No comment posted on the merged PR. | n/a |
| #700 ledger_masters as_of | 591f40182b37dac00ed891146fb8b3873a5c0ca5 | No P1. P2: a merge hazard with #672 (the snapshot must be keyed by as_of). P3: the impossible-date test asserts no typed code. | https://github.com/lamemustafa/bridge/pull/700#issuecomment-5835935942 |
| #704 dialog decline test (#687) | a7945232790b2a5966ef4dca5a75097b41f9e1ef | No P1/P2. The #687 mutation was killed; a clean exit without the token is now unavailable. P3: a crash still reads as a decline. | https://github.com/lamemustafa/bridge/pull/704#issuecomment-5835982687 |
| #706 foreign-opening cause (#675) | 7a5e70eee6c5af7ed286ddde1638962564ab9265 | No P1/P2. Accept/refuse is unchanged; the cause is typed and surfaced; tests take the composite from the live capture. P3: the remediation says "held in a foreign currency" but the check is structural. | https://github.com/lamemustafa/bridge/pull/706#issuecomment-5836021590 |
| #672 listing snapshots | cfb8f7b38b1454fa121e3694ecef1a442877a856 | Master merge plus reseal; the src-tauri net diff is byte-identical to 7f11e414 and all 280 pins match. The #700 as_of snapshot-key hazard was repeated. | https://github.com/lamemustafa/bridge/pull/672#issuecomment-5836027317 |
| #701 egress gate | f51eaa55985e9953c4359b61c21ae7ff8b5dc4e6 (moved from 1fb9fb32) | No P1. P2: the cargo-tree check passes "sealed" when it sees nothing (reproduced with an empty cargo shim); it should be exact-set in both directions. P3: other clients, `TcpSocket`, no gate self-test. | https://github.com/lamemustafa/bridge/pull/701#issuecomment-5836045614 |

Not reviewed: #647 and #649 are stacked on #642's branch, not master, and no lane has named them.

## 2026-09-25, further lane requests

| PR | Head | Result | Comment |
| --- | --- | --- | --- |
| #707 slice D1 (N-voucher generalization) | 2670dc6a34345fd7effb875a35cbd0fc730324a7 | No P1/P2. A differential probe showed request bytes and intent JSON identical to b4d7bd50 for a Journal and a 3-party Payment; admission still refuses 2+ first; 3/3 mutations on the latent N code were killed. | https://github.com/lamemustafa/bridge/pull/707#issuecomment-5836480661 |
| #708 CR LF ledger names / folded twins | f4498332f35e58e667ec0ec2637cf39dd5345df8 | No P1/P2. Twin checks at build, pre-approval and the queue; CR LF is refused at the native preview; 2/2 mutations killed. P3: a test doc says "no post code"; CR LF readback in a voucher export is not captured (fails closed). | https://github.com/lamemustafa/bridge/pull/708#issuecomment-5836563925 |
| #695 slice B | 0a96c2e32272bf89747ea1a1437acf21a03eea41 | Master (#684) merge only; the src-tauri diff is identical; 492+23 tests pass; the MCP text copy is regenerated inside `encoded_len`, so the drop cannot be bypassed. | https://github.com/lamemustafa/bridge/pull/695#issuecomment-5836606819 |
| #672 | 4375f63131c41c67d23355bf618a9d661aa944a6 | Master merge; the hunks are identical; 253 tests pass; the pins match. The #700 hazard stands. | https://github.com/lamemustafa/bridge/pull/672#issuecomment-5836641015 |
| #700 | 362904c2122ccb69c0ac99cf8c52a1a91da12233 | Reseal only; the P2 (a snapshot key with #672) stands. | https://github.com/lamemustafa/bridge/pull/700#issuecomment-5836641779 |
| #701 | a9b9e4e5b0199f409102465ead4f4265ef227cc5 | Reseal only; the P2 (a vacuous cargo-tree pass) stands, unfixed. | https://github.com/lamemustafa/bridge/pull/701#issuecomment-5836642599 |
| #704 dialog decline test | ccfdaff010d8927cbcde80e255766fcdc54163af | Master merge; hunks identical; the seal matches. **New P2:** the stub tests flake with ETXTBSY (captured errno 26; 4/25 and 1/6 runs), a fork-inheritance race in `stub()`. Two affected tests are already on master from #665. Rows expecting `unavailable` can pass on a spawn failure. | https://github.com/lamemustafa/bridge/pull/704#issuecomment-5836879119 |

Note: #672 has merged (b587f94a), so the as_of snapshot-key fix is now #700's to make.
| #710 bank_reconciliation port (E2a) | b163010d047196bd50b5f0ddfae83d4bd993f162 | No P1. P2: nothing checks opening + Σ(credit−debit) == closing (BANK-1 skips null balances), so a dropped or duplicated statement row turns a cleared entry into a timing difference, silently; likely parity with the reference, so a deliberate refusal or a BANK-1 invariant was proposed. Signs, tolerance and window bounds verified; 2/2 boundary mutations killed. P3: the not_found title names the books side, which can never be not_found. | https://github.com/lamemustafa/bridge/pull/710#issuecomment-5837983713 |
| #704 | 5e0dda357cabb9ad586ca9b913819f6159516229 | The ETXTBSY P2 is resolved: stubs are written by a child `sh`, and every row asserts its `.ran` marker. The Linux loop passed 25/25, and 25/25 at `--test-threads=32` (it was 4/25 failing before). A no-marker mutation was killed by all 3 stub tests. The seal matches. | https://github.com/lamemustafa/bridge/pull/704#issuecomment-5838183110 |
| #707 slice D1 rollback gate | dd45f793e43ebb8bb6a5eed44164d6646a455d03 | No P1/P2. One voucher is unchanged: `post_doubt` equals `masters_doubt` at N=1, and nothing writes `batch_step`. Widening the gate to N>=1 was killed by 5 one-voucher e2e tests; treating an absent step as matched was killed by 2. P3: `read_verified_baseline` uses `masters_doubt` (moot while native posts refuse amendment). | https://github.com/lamemustafa/bridge/pull/707#issuecomment-5838723158 |
| #712 slice D2a batch post | deb66f394a4957782ad8890f2ddc8bbdd6ff38d9 | No P1. P2: no test covers a null or absent `matches_created` (after-snapshot unavailable); the mutation `!= false` (null counts as matched) survived. Admission, summary honesty, record order and the seam were verified; the overlay-skip mutation was killed. P3: the step doubt message overclaims its cause; the test seam skips the cap and distinctness. | https://github.com/lamemustafa/bridge/pull/712#issuecomment-5838873907 |
| #666 pre-intent refusal (ready) | 3c3470fa47bd2d51dacacc490e76a8b3db7e727a | The earlier P2 is resolved: the catalogue arm comes before the #656 arm (a mutation moving it was killed); ADR row 12 is combined; the race test changed only comments; 121+10 tests pass; the seal matches. The P3 (gate forbids only `post_probe_xml(`) stands. | https://github.com/lamemustafa/bridge/pull/666#issuecomment-5838928804 |
| #713 high_value_register port (E2b) | ab804665800c96c8629556f8198047ab72e31d5e | **P1 (wrong amount):** a mixed cash+bank voucher is a cash row for the whole party amount. The golden's h15 (Rs 50k cash + Rs 2L bank) raises "Cash paid … at or over the s.269ST(a) … reportable in clause 31(bc)" for Rs 2.5L, with no limit disclosing it, and double-counts in the cash and bank totals. It is parity with the reference; a disclosure or divergence was proposed. Thresholds, the Contra skip and exclusions were verified; the `>=` mutation was killed. | https://github.com/lamemustafa/bridge/pull/713#issuecomment-5838961717 |
| #642 | 097533f647da22988efeaf88244736935da56fcb | Master merge plus reseal; the hunks are identical; 69+107 tests pass; the seal matches. | https://github.com/lamemustafa/bridge/pull/642#issuecomment-5839180891 |
| #647 601b-2 desktop and sweep, classified read | a5fc1b16861e6740ac8e9202222a603f7b9ba417 | No P1/P2. An assertion cannot admit a book with several masters; the base-only partial withholds totals on the desktop and in the sweep; the mutation (no-assertion arm binds INR) was killed; 122 Rust + 33 node + 8 vitest tests pass; fixtures are in provenance. | https://github.com/lamemustafa/bridge/pull/647#issuecomment-5839228328 |
| #649 601c remove the INR override | 97369fb11f91292b9fb772f158263ceb3b3f66ef | No P1/P2. One admission (`admit_inr_classified`); the override button is removed; fails closed; 122+33+7 tests pass. P3 (pre-existing): the voucher-scan feature path still takes an assertion. | https://github.com/lamemustafa/bridge/pull/649#issuecomment-5839261123 |
| #706 | b024ba9e84ae58d29cb574538840462f8ae9bba9 | Master merge; the net diff is identical; 11+139 tests pass; the seal matches. | https://github.com/lamemustafa/bridge/pull/706#issuecomment-5839288121 |
| #715 601d several-currency compliance read and MCP Trial Balance | 137beb94ff629972833ad3fcfb1e7c571deef708 | No P1/P2. Foreign rows are skipped unparsed; base rows with composite values are set aside by name; only plain rows are admitted. MCP scope fields are carried on every page; the desktop still refuses; the party-ledger workbook is withheld on any exclusion. 161 + 219 tests pass; the foreign-`continue` mutation was killed; the seal matches (280); six fixtures are in provenance. P3: the single-currency parser's error-code order changed (it still refuses). | https://github.com/lamemustafa/bridge/pull/715#issuecomment-5839355779 |
| #700 | 7b6df42c45c5a4b2a3d5bb64bdb699868848aef9 | The earlier P2 and P3 are resolved. Compliance snapshots are keyed by `gstin_as_of`; both date spellings give one key; the key mutation (discriminant only) was killed by the two new tests; 58 tests pass. The seal is stale (3 files) until the planned reseal. P3: the description does not say later pages must repeat `as_of` (otherwise `snapshot_not_held`). | https://github.com/lamemustafa/bridge/pull/700#issuecomment-5839509095 |
| #719 typed native collection errors (#676) | 68938d0b35321d3fb7034f3c1d1a8585f410d3e6 | No P1. P2: the `RowCutOff` arms in the group row (`lib.rs:1482`) and in the voucher ledger entry (`lib.rs:1707`) are untested; reverted, every test still passes, and a probe shows captured cuts turn MalformedResponse into RowUnusable. The other three mutations were killed; the seal matches. | https://github.com/lamemustafa/bridge/pull/719#issuecomment-5839546523 |
| #700 | 5d8bbcf39093c5f5e2c84d59b9d0007909446dec | Master merge (c6e07424) plus reseal. The net diff is identical apart from the context around the new remediation arm; both arms are kept; 740 `agent::` tests pass; 280 pins match; the compatibility gate passes. The P2 and P3 stay resolved; the optional P3 about `as_of` on later pages is still open. | https://github.com/lamemustafa/bridge/pull/700#issuecomment-5839758323 |
