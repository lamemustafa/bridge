# Tally roadmap execution log

One line per merged PR, appended by the orchestrator after merge (see
[PROMPT_PLAYBOOK.md](./PROMPT_PLAYBOOK.md) §7.1). This log is the
orientation input for phase selection: the current phase is the lowest-
numbered phase whose exit criterion is not yet evidenced here.

Format:

```
| date | PR | phase | invariant established | evidence |
```

Evidence must name a real artifact: a test (crate::module::test_name), a
signed compatibility-matrix receipt id, a migration version, or a demo
scenario transcript reference. "Done" is not evidence.

| Date | PR | Phase | Invariant established | Evidence |
| --- | --- | --- | --- | --- |
| _(no merged PR yet — see working state below)_ | | | | |

## Unmerged working state — 2026-07-29

Recorded here because this log is the orientation input for phase selection, and
leaving it empty while substantial evidence exists would mis-select the next
phase. These rows move into the table above, with PR numbers, on merge. Branch
commits may exist, but nothing below is merged.

| Unit | Status | Invariant established | Evidence |
| --- | --- | --- | --- |
| P0 reality probe | Complete, unmerged | Live Tally reachable over a loopback forward; extended `FETCH` returns narration, party ledger, `PARTYGSTIN`, `ALTERID`, ledger and inventory lists — full-fidelity reads are not blocked by Tally | `IMPROVEMENT_PLAN_2026H2.md` §8.1–8.7; raw captures in `.bridge-live/captures/` (gitignored) |
| P0 write probes | Complete, unmerged | Imports succeed on Education mode; `ERRORS=0` on a rejected write; no natural voucher idempotency; master re-create silently becomes Alter; `LASTMID=0` on master create; company AlterID high-water marks move | `IMPROVEMENT_PLAN_2026H2.md` §8.1, §8.2, §8.4, §8.5, §8.6, §8.7 |
| P0b D1 | Complete, unmerged | Import success classification is fail-closed against the real `ERRORS=0 / EXCEPTIONS=1` rejection shape | `bridge_tally_protocol::import_evidence::live_education_import_counter_shapes_are_clean_only_when_the_intended_write_applied` |
| P0b D3 | Complete, unmerged | Export and import STATUS rules are distinct; reported import `STATUS=0` overrides clean counters | `bridge_tally_protocol::import_evidence::wrapped_status_zero_rejects_clean_import_counters`; `PROMPT_PLAYBOOK.md` §1 GLOBAL RULES |
| P0b D4 | Complete, unmerged | `tally_prime_edit_log` is representable without allowing any Education SKU to reach a positive claim | `bridge_tally_compatibility::tests::edit_log_product_family_has_a_distinct_stable_wire_value` |
| P0b D2 | Complete, unmerged | The exact V2 request produces no response, so its date boundary cannot be measured; four of seven shipped read profiles violate the spaced-identifier rule for `$$` functions | `IMPROVEMENT_PLAN_2026H2.md` §§8.9–8.11; `compatibility/evidence/p0b-live-evidence-defects-2026-07-29.md` D2 |
| P0b rectify P1-1 | **FIXED, unmerged** | A clean import result cannot succeed without exact non-zero intended mutation counters | `bridge_tally_protocol::import_evidence::clean_import_success_requires_each_live_evidence_condition_independently`; Bug #97 |
| P0b rectify P1-2 | **FIXED, unmerged** | Education mode cannot reach `Observed` or `Supported` for any product; licensed evidence remains eligible | `bridge_tally_compatibility::tests::edit_log_education_cannot_reach_a_positive_claim_with_valid_signed_evidence`; `positive_claim_requires_fresh_signed_exact_scope_evidence`; Bug #98 |
| P0b rectify P1-3 | **FIXED, unmerged** | Bare imports use §8.2 counters; reported `STATUS=0` fails regardless of counters | `PROMPT_PLAYBOOK.md` §1; `bridge_tally_protocol::import_evidence::wrapped_status_zero_rejects_clean_import_counters`; Bug #99 |
| P0b rectify P2-1 | **FIXED, unmerged** | The w4 golden retains numeric `LASTVCHID=295`; non-numeric values fail closed | `bridge_tally_protocol::import_evidence::non_numeric_lastvchid_is_rejected_even_when_import_counters_are_clean`; Bug #101 |
| P0b rectify P2-2 | **FIXED, unmerged** | D2 transport claims are limited to durable evidence and reconciled with §§8.9–8.11 | `compatibility/evidence/p0b-live-evidence-defects-2026-07-29.md` D2; Bug #100 |
| Unit A outstandings | **Blocked, unmerged — ruling 4 rectified offline; sizing uncalibrated** | The only correct bill payload remains the closed outstandings-only `ALLLEDGERENTRIES.*` profile under the 40 MiB exception, 28 MiB target and immutable 20-second deadline. A broad reporting period can no longer render a request: the sealed type requires a `NarrowDateWindow` (at most 31 calendar days) plus `$AlterID > N AND $AlterID <= M`. Date legality is compatibility-profiled: explicit detected Education keeps day 01/02/31, while licensed/unknown evidence permits ordinary boundaries and relies on I12. Each narrow date partition must prove exact `0..ALTVCHID` coverage before the full reporting-period scan can complete. Paired digest/length/row equality, company pinning, duplicate rejection, `CompleteScan`/`PartialScan` separation, trend-stop behavior, and the single adjacent wider empty-range proof remain fail-closed. Production has no initial width and returns typed Partial before endpoint admission while sizing is uncalibrated. | Owner ruling 3 retracted AlterID-only segmentation and single-sample tuning. Owner ruling 4 removed the unsafe universal Education boundary invariant without weakening I12 or the 31-day cap. Offline regressions prove detected Education is strict, licensed/unknown accepts day 15, and an out-of-span returned voucher remains Partial. Aarav is protocol/completeness-only and forbidden for sizing. The ignored reconciliation exit remains blocked until the bill-bearing company is generated in ascending date order. |

The ignored `unit_a_ordered_corpus_calibration_sample` target now provides the bounded manual
evidence path for the pending corpus: one sample per invocation, a durable reservation written
before Tally contact, fresh non-overwriting files, status checks around both paired requests, no
retry or loop, and no automatic policy change.

**Process gate before PR:** the adversarial review produced the five
rectifications recorded above. A fresh-context re-review and the preservation
gate required by `PROMPT_PLAYBOOK.md` §7.1 step 3 remain mandatory before this
unit opens or merges a PR.
