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
