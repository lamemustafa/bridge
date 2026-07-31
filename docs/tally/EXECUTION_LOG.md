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
| Unit A outstandings | **Complete for Unit A scope per ruling 8, unmerged** | The only correct bill payload remains the closed outstandings-only `ALLLEDGERENTRIES.*` profile under the 40 MiB exception, 28 MiB target and immutable 20-second deadline. Whole-scan completeness is proven by exact `[BooksFrom, LastVoucherDate]` tiling; AlterID ranges tile only the budget axis. Interior empty partitions are admissible, whole-book false-empty fails closed, and no AlterID-adjacent proof remains. Preflight/runtime admit at most 128 segment pairs; paired reads are separated and followed by `/status`. As-of is explicit and every unprovable read is visible as typed Partial with totals withheld. | The first port-9000 exit attempt proved I12 by failing closed on an Education-invalid mode-agnostic boundary. After binding the target-only harness to the owner-attested Educational profile, exact runs completed 27 paired partitions on 9000/9001 in 8.30/8.93 s: 220 vouchers, ₹45,14,597 receivable, ₹1,05,000 payable, 48 open bills, ageing 4/4/4/36. Payload sizes differ (3,443,776/3,639,306 bytes) but accounting agrees. Production-scale conditional subdivision and licensed-Tally coverage are Unit B; no positive licensed claim is made. |

The `unit_a_ordered_corpus_calibration_sample` harness has been **deleted**, not fixed. Review
found that it dispatched directly through the transport, bypassing the runtime's per-endpoint
serial queue, request budget and trend guard — so its timing evidence did not represent the
product path it existed to calibrate — and that it retained *decoded* response text as the
"byte-exact" evidence whose encoded length and SHA-256 were asserted.

Both are real, but repairing them would have preserved dead machinery. Ruling 7 established
that this corpus **cannot** calibrate an AlterID width at all (the proposed width equalled the
corpus high-water, so no segmentation occurred), and ruling 8 replaced calibrated-width sizing
with conditional subdivision measured from the book being read. The harness therefore measured
a quantity nothing consumes. Unit B must measure through the product path, not beside it.

**Process gate before PR:** the adversarial review produced the five
rectifications recorded above. A fresh-context re-review and the preservation
gate required by `PROMPT_PLAYBOOK.md` §7.1 step 3 remain mandatory before this
unit opens or merges a PR.
