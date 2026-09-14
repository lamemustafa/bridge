#!/usr/bin/env python3
"""Focused offline controls for scripts/merge-gate.sh.

The fake gh command models server responses, including paginated REST and
GraphQL pages. No network or merge operation is used.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "merge-gate.sh"
HEAD = "0123456789abcdef0123456789abcdef01234567"
FAKE_GH = ROOT / "scripts" / "merge_gate_fake_gh.py"
PRIVACY = ROOT / "scripts" / "merge_gate_privacy.py"


def load_privacy_module():
    spec = importlib.util.spec_from_file_location("merge_gate_privacy", PRIVACY)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MergeGateControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="merge-gate-controls-")
        cls.bin = Path(cls.tmp.name)
        gh = cls.bin / "gh"
        shutil.copyfile(FAKE_GH, gh)
        gh.chmod(0o755)
        jq = cls.bin / "jq"
        jq.write_text("""#!/usr/bin/env python3
import os, sys
if os.environ.get("GATE_SCENARIO") == "context-report-jq-failure" and "--rawfile" in sys.argv and "contexts" in sys.argv:
    raise SystemExit(1)
os.execv(os.environ["GATE_REAL_JQ"], [os.environ["GATE_REAL_JQ"], *sys.argv[1:]])
""")
        jq.chmod(0o755)
        cls.real_jq = shutil.which("jq")
        if not cls.real_jq:
            raise RuntimeError("jq is required for merge-gate controls")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_gate(self, scenario="pass", extra_args=()):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["GATE_SCENARIO"] = scenario
        env["GATE_REAL_JQ"] = self.real_jq
        counter = self.bin / f"{scenario}-counter-{os.getpid()}"
        counter.write_text("0")
        env["GATE_COUNTER"] = str(counter)
        check_counter = self.bin / f"{scenario}-checks-{os.getpid()}"
        check_counter.write_text("0")
        env["GATE_CHECK_COUNTER"] = str(check_counter)
        status_counter = self.bin / f"{scenario}-statuses-{os.getpid()}"
        status_counter.write_text("0")
        env["GATE_STATUS_COUNTER"] = str(status_counter)
        base_tip_counter = self.bin / f"{scenario}-base-tip-{os.getpid()}"
        base_tip_counter.write_text("0")
        env["GATE_BASE_TIP_COUNTER"] = str(base_tip_counter)
        return subprocess.run(
            [str(SCRIPT), "321", "--repo", "lamemustafa/bridge", *extra_args],
            cwd=ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def assert_blocked(self, scenario, phrase):
        result = self.run_gate(scenario)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(phrase, result.stdout)
        self.assertNotIn("MAY MERGE", result.stdout)

    def assert_indeterminate(self, scenario, phrase):
        result = self.run_gate(scenario)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn(phrase, result.stdout)
        self.assertNotIn("MAY MERGE", result.stdout)

    def test_server_full_sha_and_paginated_pages_can_pass(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("provider review records the full current head", result.stdout)
        self.assertIn("MAY MERGE", result.stdout)
        self.assertIn("--match-head-commit 0123456789abcdef0123456789abcdef01234567", result.stdout)

    def test_allowlisted_docs_only_skipped_checks_do_not_block(self):
        result = self.run_gate("skipped-docs-matrix")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("allowlisted skipped native CI matrix job", result.stdout)

    def test_skipped_ci_jobs_require_an_explicit_allowlist_and_matching_path_condition(self):
        self.assert_blocked("skipped-unallowlisted", "skipped CI job outside the explicit workflow allowlist")
        self.assert_blocked("skipped-native-scope", "skipped native CI matrix job despite native-scope changed files")
        self.assert_blocked("skipped-bundle-scope", "skipped bundle CI matrix job despite bundle-scope changed files")

    def test_required_skipped_check_blocks(self):
        self.assert_blocked("required-skip", "required check 'Required checks' is not passing")

    def test_incomplete_check_run_is_indeterminate(self):
        self.assert_indeterminate("check-run-incomplete", "head-bound check-run evidence")

    def test_empty_pending_legacy_status_does_not_block(self):
        result = self.run_gate("status-empty-pending")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_required_context_blocks(self):
        self.assert_blocked("missing-required", "required check 'Rust format' was not reported")

    def test_unexpected_required_context_blocks(self):
        self.assert_blocked("required-context-unexpected", "undocumented required check context")

    def test_cancelled_check_blocks(self):
        self.assert_blocked("cancel-check", "failing, cancelled, or pending")

    def test_surface_transport_failure_is_indeterminate(self):
        self.assert_indeterminate("surface-fail", "could not read and validate compatibility surface")

    def test_silent_checks_response_is_indeterminate(self):
        self.assert_indeterminate("checks-silent", "checks query returned no JSON")

    def test_summary_only_short_sha_is_indeterminate(self):
        self.assert_indeterminate("short-review", "independent-review-sha")

    def test_manual_full_sha_attestation_accepts_zero_finding_summary(self):
        result = self.run_gate("manual-summary", ("--independent-review-sha", HEAD))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("manual independent review attestation", result.stdout)

    def test_provider_review_requires_completed_summary(self):
        self.assert_blocked("summary-failed", "review run for 0123456 is not completed")

    def test_blocked_merge_state_cannot_pass(self):
        self.assert_blocked("blocked-state", "merge state BLOCKED")

    def test_unrecognised_final_merge_state_is_indeterminate(self):
        self.assert_indeterminate("final-unrecognized", "unrecognised value")

    def test_short_thread_page_is_indeterminate(self):
        self.assert_indeterminate("threads-short", "returned 1 of 101 nodes")

    def test_unresolved_second_thread_page_blocks(self):
        self.assert_blocked("threads-unresolved-second", "1 of 101 review threads unresolved")

    def test_malformed_thread_pagination_is_indeterminate(self):
        self.assert_indeterminate("threads-malformed-pagination", "no advancing cursor")

    def test_first_thread_page_omits_the_empty_cursor(self):
        result = self.run_gate("threads-first-empty-cursor-rejected")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_thread_total_drift_is_indeterminate(self):
        self.assert_indeterminate("threads-total-drift", "totalCount changed")

    def test_head_change_is_blocked(self):
        self.assert_blocked("head-moves", "PR head moved during preflight")

    def test_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone", "privacy scan found")

    def test_hidden_metadata_identifier_is_still_scanned(self):
        self.assert_blocked("hidden-comment-identifier", "privacy scan found")

    def test_unterminated_comment_cannot_supply_validation(self):
        self.assert_blocked("unterminated-html-comment", "actual test or reproduction command")

    def test_multiline_comment_cannot_supply_functional_summary(self):
        self.assert_blocked("policy-multiline-comment", "non-empty functional summary")

    def test_raw_html_comment_drift_blocks_revalidation(self):
        self.assert_blocked("body-comment-drift", "description changed during preflight")

    def test_repeated_digit_phone_is_scanned(self):
        self.assert_blocked("repeated-phone", "privacy scan found")

    def test_grouped_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone-grouped", "privacy scan found")

    def test_separated_indian_mobile_styles_are_scanned(self):
        for scenario in ("phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized", "unicode-phone", "unicode-phone-tab"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")
        result = self.run_gate("unicode-phone-two-lines")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_grouped_indian_landline_is_scanned_without_global_separator_joining(self):
        self.assert_blocked("landline-grouped", "privacy scan found")

    def test_standard_indian_landline_is_scanned_without_global_separator_joining(self):
        for scenario in ("landline-standard-hyphen", "landline-standard-space", "landline-standard-four-digit"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")
        result = self.run_gate("landline-standard-underscore")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pem_certificate_envelope_is_blocked_without_echoing_payload(self):
        for scenario, begin, body in (
            ("pem-certificate-envelope", "-" * 5 + "BEGIN CERTIFICATE" + "-" * 5, "MII" + "A" * 48),
            ("trusted-pem-certificate-envelope", "-" * 5 + "BEGIN TRUSTED CERTIFICATE" + "-" * 5, "MII" + "B" * 48),
        ):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("PEM certificate envelope", result.stdout)
                self.assertNotIn(begin, result.stdout)
                self.assertNotIn(body, result.stdout)

    def test_header_shaped_added_payload_is_still_scanned(self):
        self.assert_blocked("hunk-header-phone", "privacy scan found")

    def test_header_shaped_added_literals_count_as_payload(self):
        result = self.run_gate("hunk-header-literals")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_binary_shaped_added_literal_is_not_binary_metadata(self):
        result = self.run_gate("hunk-binary-literal")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unrelated_separator_groups_are_not_fused_into_a_phone(self):
        result = self.run_gate("separated-dates")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_new_year_date_ranges_are_not_fused_into_a_phone(self):
        result = self.run_gate("separated-dates-new-year")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_year_month_date_ranges_are_not_fused_into_a_phone(self):
        result = self.run_gate("separated-dates-year-month")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_adr_identifier_with_underscores_remains_clean(self):
        result = self.run_gate("adr-identifier")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_destination_path_is_scanned(self):
        self.assert_blocked("path-id", "privacy scan found")

    def test_binary_deletion_is_not_treated_as_added_content(self):
        result = self.run_gate("binary-delete")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_binary_additions_require_two_current_head_manual_attestations(self):
        self.assert_blocked("binary-review", "require matching --binary-review-sha and --independent-review-sha")
        binary_only = self.run_gate("binary-review", ("--binary-review-sha", HEAD))
        self.assertEqual(binary_only.returncode, 1, binary_only.stdout + binary_only.stderr)
        self.assertIn("require matching --binary-review-sha and --independent-review-sha", binary_only.stdout)
        malformed = self.run_gate("binary-review", ("--binary-review-sha", "not-a-sha", "--independent-review-sha", HEAD))
        self.assertEqual(malformed.returncode, 1, malformed.stdout + malformed.stderr)
        self.assertIn("binary review attestation must be a full 40-hex", malformed.stdout)
        stale = self.run_gate("binary-review", ("--binary-review-sha", "f" * 40, "--independent-review-sha", HEAD))
        self.assertEqual(stale.returncode, 1, stale.stdout + stale.stderr)
        self.assertIn("binary review attestation names a different commit", stale.stdout)
        current = self.run_gate("binary-review", ("--binary-review-sha", HEAD, "--independent-review-sha", HEAD))
        self.assertEqual(current.returncode, 0, current.stdout + current.stderr)
        self.assertIn("explicit current-head binary and independent review attestations", current.stdout)
        moved = self.run_gate("binary-review-head-moves", ("--binary-review-sha", HEAD, "--independent-review-sha", HEAD))
        self.assertEqual(moved.returncode, 1, moved.stdout + moved.stderr)
        self.assertIn("PR head moved during preflight", moved.stdout)

    def test_binary_attestation_does_not_bypass_privacy_metadata_scan(self):
        result = self.run_gate("binary-review-private", ("--binary-review-sha", HEAD, "--independent-review-sha", HEAD))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("privacy scan found", result.stdout)

    def test_malformed_surface_is_indeterminate(self):
        self.assert_indeterminate("surface-malformed", "could not read and validate compatibility surface")

    def test_surface_with_no_v1_manifest_schema_is_indeterminate(self):
        self.assert_indeterminate("surface-schema-malformed", "could not read and validate compatibility surface")

    def test_removed_base_pin_requires_human_hold(self):
        self.assert_indeterminate("surface-unpins", "base-pinned path(s) are absent")

    def test_compare_lineage_mismatch_is_indeterminate(self):
        self.assert_indeterminate("lineage-mismatch", "base/head compare did not prove")

    def test_check_run_wrong_head_is_indeterminate(self):
        self.assert_indeterminate("check-run-wrong-head", "head-bound check-run evidence")

    def test_check_run_count_mismatch_is_indeterminate(self):
        self.assert_indeterminate("check-run-count-mismatch", "head-bound check-run evidence")

    def test_final_check_and_status_snapshots_do_not_inherit_earlier_success(self):
        self.assert_blocked("late-check-run-failure", "final check run(s) are failed")
        self.assert_blocked("late-check-run-unallowlisted-skip", "final refreshed check-run evidence reports a skipped CI job outside the explicit workflow allowlist")
        self.assert_blocked("late-status-failure", "final combined commit-status evidence reports a failure")

    def test_base_oid_mismatch_is_indeterminate(self):
        self.assert_indeterminate("base-oid-mismatch", "PR base OID")

    def test_final_base_tip_recheck_rejects_a_late_move_or_read_failure(self):
        self.assert_blocked("final-base-tip-moves", "base tip moved after final CI evidence")
        self.assert_indeterminate("final-base-tip-failure", "could not revalidate base tip after final CI evidence")

    def test_malformed_commit_status_is_indeterminate(self):
        self.assert_indeterminate("status-malformed", "head-bound commit-status evidence")

    def test_malformed_changed_file_is_indeterminate(self):
        self.assert_indeterminate("malformed-files", "could not read the complete changed-file set")

    def test_missing_changed_file_status_is_indeterminate(self):
        self.assert_indeterminate("missing-file-status", "could not read the complete changed-file set")

    def test_changed_file_count_mismatch_is_indeterminate(self):
        self.assert_indeterminate("files-count-mismatch", "changed-file response has")

    def test_empty_file_array_is_not_one_filename(self):
        self.assert_indeterminate("files-empty", "contained no filenames")

    def test_fractional_thread_count_is_indeterminate(self):
        self.assert_indeterminate("threads-fractional", "could not read review threads")

    def test_empty_page_cannot_claim_more_threads(self):
        self.assert_indeterminate("threads-empty-more", "no nodes while claiming another page")

    def test_template_checklist_permalink_on_continuation_passes(self):
        result = self.run_gate("checklist-template-continuation")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("description links a completed review-checklist item", result.stdout)

    def test_checklist_permalink_must_bind_the_full_current_head(self):
        self.assert_blocked("checklist-stale-ref", "same-repository line-specific")
        self.assert_blocked("checklist-anchor-suffix", "same-repository line-specific")

    def test_foreign_checklist_link_blocks(self):
        self.assert_blocked("checklist-foreign", "same-repository line-specific")

    def test_unlinked_checklist_text_blocks(self):
        self.assert_blocked("checklist-unlinked", "same-repository line-specific")

    def test_omitted_nonremoved_diff_file_is_indeterminate(self):
        self.assert_indeterminate("diff-omits-file", "privacy diff coverage failed")

    def test_truncated_textual_diff_payload_is_indeterminate(self):
        self.assert_indeterminate("diff-truncated-payload", "privacy diff coverage failed")

    def test_metadata_only_zero_line_diff_can_pass(self):
        result = self.run_gate("metadata-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("metadata-only diff section", result.stdout)

    def test_metadata_only_examples_are_redacted_and_bounded(self):
        result = self.run_gate("metadata-private")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("metadata-only diff section", result.stdout)
        self.assertNotIn("privacy diff coverage failed", result.stdout)
        self.assertNotIn("tester", result.stdout)
        examples = next(line for line in result.stdout.splitlines() if "metadata-only diff section" in line)
        self.assertLess(len(examples), 260)
        self.assertNotIn("x" * 161, examples)

    def test_metadata_only_diff_requires_rest_zero_totals(self):
        self.assert_indeterminate("metadata-incomplete", "metadata-only 'docs/example.md' conflicts")

    def test_pr_selector_must_be_numeric(self):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        result = subprocess.run(
            [str(SCRIPT), "321;echo unsafe", "--repo", "lamemustafa/bridge"],
            cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("PR selector must be numeric", result.stderr)

    def test_repo_identity_must_be_safe(self):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        result = subprocess.run(
            [str(SCRIPT), "321", "--repo", "lamemustafa/bridge;echo unsafe"],
            cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("--repo must be OWNER/NAME", result.stderr)

    def test_other_repository_policy_is_not_assumed(self):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        result = subprocess.run(
            [str(SCRIPT), "321", "--repo", "example/other"],
            cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("unsupported repository", result.stderr)

    def test_missing_functional_summary_blocks(self):
        self.assert_blocked("missing-functional-summary", "functional summary")

    def test_missing_test_summary_blocks(self):
        self.assert_blocked("missing-test-summary", "test or reproduction command")

    def test_final_changed_body_revalidates_test_evidence(self):
        self.assert_blocked("body-loses-evidence", "description changed and no longer carries test or reproduction evidence")

    def test_final_changed_body_revalidates_functional_summary(self):
        self.assert_blocked("body-loses-functional", "description changed and no longer carries a non-empty functional summary")

    def test_draft_merge_state_blocks(self):
        self.assert_blocked("draft-state", "merge state DRAFT")

    def test_documented_required_context_cannot_be_omitted(self):
        self.assert_blocked("protection-missing-gitguardian", "branch protection omits 1 documented")

    def test_failing_combined_commit_status_blocks(self):
        self.assert_blocked("status-failed-combined", "combined commit-status evidence reports a failure")

    def test_failing_individual_commit_status_blocks(self):
        self.assert_blocked("status-failed-context", "combined commit-status evidence reports a failure")

    def test_privacy_uuid_policy_is_exact_context_aware_and_value_redacting(self):
        privacy = load_privacy_module()
        candidate = "12345678-abcd-4abc-8abc-123456789012"
        nil = "00000000-0000-0000-0000-000000000000"
        provenance = self.bin / "fixture-uuid-provenance.json"
        provenance.write_text(json.dumps([{
            "uuid": candidate,
            "head": HEAD,
            "source": "fixture",
        }]))

        for text in (
            "credential_session_id=" + candidate,
            "bearerToken=" + candidate,
            "credentialSessionId=" + candidate,
        ):
            with self.subTest(text=text):
                result = privacy.scan(text, HEAD, str(provenance))
                self.assertTrue(result["blockers"])
                self.assertNotIn(candidate, json.dumps(result))

        for text in (
            "urn:uuid:" + candidate,
            "synthetic fixture uuid=" + candidate,
        ):
            with self.subTest(text=text):
                result = privacy.scan(text, HEAD)
                self.assertTrue(result["indeterminate"])
                self.assertNotIn(candidate, json.dumps(result))

        nil_result = privacy.scan("{\"id\": \"" + nil + "\"}", HEAD)
        self.assertFalse(nil_result["blockers"] + nil_result["indeterminate"])

        approved = privacy.scan("fixture_value=" + candidate, HEAD, str(provenance))
        self.assertFalse(approved["blockers"] + approved["indeterminate"])
        reused = privacy.scan("token=" + candidate, HEAD, str(provenance))
        self.assertTrue(reused["blockers"])

    def test_privacy_uuid_provenance_failure_is_indeterminate_without_echoing_input(self):
        privacy = load_privacy_module()
        candidate = "12345678-abcd-4abc-8abc-123456789012"
        result = privacy.scan("urn:uuid:" + candidate, HEAD, str(self.bin / "missing-provenance.json"))
        self.assertTrue(result["indeterminate"])
        self.assertNotIn(candidate, json.dumps(result))

    def test_privacy_module_blocks_trusted_pem_and_four_digit_landline_without_echoing_values(self):
        privacy = load_privacy_module()
        trusted_pem = "-----BEGIN TRUSTED CERTIFICATE-----\nMII" + "B" * 48
        landline = "0120-2345678"
        for value, expected in ((trusted_pem, "PEM certificate envelope"), (landline, "identifier shape")):
            with self.subTest(expected=expected):
                result = privacy.scan(value, HEAD)
                self.assertTrue(result["blockers"])
                self.assertIn(expected, "\n".join(result["blockers"]))
                self.assertNotIn(value, json.dumps(result))

    def test_duplicate_thread_ids_are_indeterminate(self):
        self.assert_indeterminate("threads-duplicate-id", "pagination repeated thread IDs")

    def test_checklist_anchor_must_reference_an_existing_line(self):
        self.assert_blocked("checklist-missing-anchor", "review-checklist link to an existing line")

    def test_template_functional_prompt_does_not_count_as_summary(self):
        self.assert_blocked("template-functional-prompt", "non-empty functional summary")

    def test_template_validation_prompts_do_not_count_as_evidence(self):
        self.assert_blocked("template-validation-prompt", "test or reproduction command")

    def test_pr_title_identifier_is_scanned(self):
        self.assert_blocked("metadata-title-id", "privacy scan found")

    def test_pr_commit_metadata_identifier_is_scanned(self):
        self.assert_blocked("metadata-commit-id", "privacy scan found")

    def test_standard_commit_identity_fields_are_validated_and_scanned(self):
        self.assert_blocked("metadata-author-id", "privacy scan found")
        result = self.run_gate("metadata-unlinked-identities")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_capped_commit_metadata_is_indeterminate(self):
        self.assert_indeterminate("metadata-capped", "complete head-bound PR commit metadata")

    def test_truncated_commit_metadata_is_indeterminate(self):
        self.assert_indeterminate("metadata-truncated", "complete head-bound PR commit metadata")

    def test_duplicate_commit_metadata_is_indeterminate(self):
        self.assert_indeterminate("metadata-duplicate", "complete head-bound PR commit metadata")

    def test_commit_metadata_head_mismatch_is_indeterminate(self):
        self.assert_indeterminate("metadata-head-mismatch", "complete head-bound PR commit metadata")

    def test_all_a_pan_is_not_exempted_as_a_placeholder(self):
        self.assert_blocked("all-a-pan", "privacy scan found")
        for scenario in ("grouped-pan-space", "grouped-pan-hyphen"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")
        result = self.run_gate("grouped-masked-pan-space")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_explicit_masked_pan_remains_a_placeholder(self):
        result = self.run_gate("masked-pan")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_quoted_git_destination_path_is_covered(self):
        result = self.run_gate("quoted-path")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_crlf_and_ambiguous_unquoted_diffs_preserve_coverage(self):
        for scenario in ("crlf-diff", "ambiguous-unquoted-path", "ambiguous-rename-path"):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_gitlink_requires_explicit_provenance_license_notice_review(self):
        for scenario in ("gitlink", "gitlink-existing"):
            with self.subTest(scenario=scenario):
                self.assert_indeterminate(scenario, "gitlink change(s) require explicit provenance, license, and NOTICE review")

    def test_control_character_in_destination_path_is_indeterminate(self):
        self.assert_indeterminate("control-path", "could not read the complete changed-file set")

    def test_populated_sha_without_command_is_not_validation_evidence(self):
        self.assert_blocked("template-validation-sha-only", "test or reproduction command")

    def test_validation_heading_without_command_is_not_evidence(self):
        self.assert_blocked("template-validation-empty", "actual test or reproduction command")

    def test_protection_requires_strict_status_checks(self):
        self.assert_indeterminate("protection-nonstrict", "required status-check response was malformed")

    def test_duplicate_check_run_ids_are_indeterminate(self):
        self.assert_indeterminate("check-run-duplicate-id", "head-bound check-run evidence")

    def test_grouped_12_and_16_digit_identifiers_are_scanned(self):
        for scenario in ("grouped-identifier-12", "grouped-identifier-16", "grouped-identifier-mixed"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")

    def test_workflow_change_requires_rollback_and_migration_notes(self):
        self.assert_blocked("workflow-notes-missing", "workflow change lacks non-empty rollback notes")
        self.assert_blocked("workflow-sibling-migration", "workflow change lacks non-empty rollback notes")
        self.assert_blocked("workflow-placeholders", "workflow change lacks non-empty rollback notes")
        self.assert_blocked("workflow-punctuated-placeholders", "workflow change lacks non-empty rollback notes")

    def test_workflow_change_with_required_notes_can_pass(self):
        result = self.run_gate("workflow-notes-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_workflow_delete_and_rename_out_require_notes(self):
        for scenario in ("workflow-delete", "workflow-rename-out"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "workflow change lacks non-empty rollback notes")

    def test_workflow_delete_and_rename_out_with_notes_can_pass(self):
        for scenario in ("workflow-delete-notes", "workflow-rename-out-notes"):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_renamed_file_requires_a_string_previous_filename(self):
        for scenario in ("renamed-previous-missing", "renamed-previous-null", "renamed-previous-false"):
            with self.subTest(scenario=scenario):
                self.assert_indeterminate(scenario, "could not read the complete changed-file set")

    def test_sensitive_paths_require_security_impact_notes_including_renames(self):
        for scenario in ("security-notes-missing", "security-rename-out", "security-crate", "security-agent-import", "security-dsc"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "security-impact notes")
        result = self.run_gate("security-notes-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("security-none")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_blocked("security-pending", "security-impact notes")

    def test_sensitive_paths_without_security_attestation_are_indeterminate(self):
        self.assert_indeterminate("security-review-missing", "security-focused reviewer comment")

    def test_refreshed_failed_or_required_skipped_runs_block(self):
        for scenario in ("check-run-failed", "check-run-required-skip", "check-run-optional-skip"):
            with self.subTest(scenario=scenario):
                phrase = "skipped CI job outside the explicit workflow allowlist" if scenario == "check-run-optional-skip" else "refreshed check run(s)"
                self.assert_blocked(scenario, phrase)
        result = self.run_gate("check-run-allowlisted-skip")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_nonempty_pending_legacy_status_is_indeterminate(self):
        self.assert_indeterminate("status-nonempty-pending", "commit-status evidence")

    def test_truncated_legacy_status_pages_are_indeterminate(self):
        self.assert_indeterminate("status-truncated", "complete head-bound commit-status evidence")

    def test_complete_combined_status_pages_bind_head_and_contexts(self):
        result = self.run_gate("status-two-page")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for scenario in ("status-truncated", "status-duplicate", "status-mixed-head"):
            with self.subTest(scenario=scenario):
                self.assert_indeterminate(scenario, "complete head-bound commit-status evidence")

    def test_validation_commands_must_be_concrete_and_in_validation(self):
        for scenario in ("validation-command-outside", "validation-tool-prose", "validation-placeholder-command", "validation-html-comment"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "actual test or reproduction command")
        result = self.run_gate("validation-node-command")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("validation-tilde-command")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_empty_fenced_functional_summary_is_not_content(self):
        for scenario in ("empty-fenced-summary", "empty-tilde-summary", "empty-typed-tilde-summary", "empty-rule-summary", "inline-fence-summary"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "non-empty functional summary")
        for scenario in ("nonempty-fenced-summary", "nonempty-tilde-summary"):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_checklist_heading_is_not_a_completed_item(self):
        self.assert_blocked("checklist-heading", "review-checklist link")

    def test_context_report_jq_failure_is_indeterminate_without_fabricated_count(self):
        result = self.run_gate("context-report-jq-failure")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("could not compute bounded required-check diagnostics", result.stdout)
        self.assertNotIn("1 of 0 required check contexts", result.stdout)
        self.assertNotIn("all 0 required check contexts passed", result.stdout)

    def test_required_context_diagnostics_are_bounded_for_failures(self):
        result = self.run_gate("required-context-output-bound")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("50 of 55 required check contexts", result.stdout)
        self.assertLess(len(result.stdout.encode()), 6000)
        self.assertNotIn("z" * 100, result.stdout)

    def test_security_review_is_focused_independent_and_current(self):
        for scenario in ("security-review-stale", "security-review-author", "security-review-unrelated", "security-review-no-scope", "security-review-bare-scope", "security-review-placeholder-rationale", "security-review-hidden", "security-review-hidden-unterminated", "security-review-outsider", "security-review-dismissed", "security-camel-dsc", "security-camel-credential", "security-axal-frontend", "security-axal-native", "security-encrypted-keystore", "security-documents-consumer", "security-documents-consumer-rename-out", "security-documents-screen", "security-documents-screen-rename-out", "security-tauri-cargo", "security-tauri-cargo-rename-out", "security-tauri-cargo-lock", "security-tauri-cargo-lock-rename-out", "security-tauri-lib", "security-tauri-lib-rename-out", "security-commands-facade", "security-bank-statement-import", "security-prune-package-compiler-cache", "security-prune-package-compiler-cache-rename-out", "security-ci-workflow", "security-ci-workflow-rename-out", "security-release-preview", "security-release-preview-rename-out", "security-deploy-install-page", "security-deploy-install-page-rename-out"):
            with self.subTest(scenario=scenario):
                self.assert_indeterminate(scenario, "security-focused reviewer comment")
        result = self.run_gate("security-review-valid")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("security-ci-workflow-valid")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("non-sensitive-cargo-lock")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("security-deploy-install-page-valid")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("security-documents-screen-valid")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_sync_rename_out_requires_migration_notes(self):
        self.assert_blocked("sync-migration-missing", "migration compatibility")
        result = self.run_gate("sync-migration-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_implementation_additions_need_all_three_p4_answers(self):
        self.assert_blocked("implementation-p4-missing", "all three substantive P4")
        self.assert_blocked("implementation-p4-shell", "all three substantive P4")
        self.assert_blocked("implementation-p4-powershell", "all three substantive P4")
        self.assert_blocked("implementation-p4-sql", "all three substantive P4")
        self.assert_blocked("p4-placeholders", "all three substantive P4")
        result = self.run_gate("implementation-p4-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("implementation-p4-continuation")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_dependency_manifest_additions_need_a_substantive_rationale(self):
        self.assert_blocked("dependency-manifest-missing", "dependency manifest addition lacks a substantive dependency justification")
        result = self.run_gate("dependency-manifest-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_platform_sensitive_paths_need_substantive_both_host_evidence(self):
        self.assert_blocked("platform-evidence-missing", "substantive Windows validation")
        result = self.run_gate("platform-evidence-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("platform-evidence-heading")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("platform-checkbox-evidence")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_blocked("platform-checkbox-comment", "substantive Windows validation")
        self.assert_blocked("platform-inline-prose", "substantive Windows validation")
        self.assert_blocked("platform-negative-outcome", "substantive Windows validation")
        self.assert_blocked("platform-unaffected-bare", "substantive Windows validation")
        self.assert_blocked("platform-evidence-bare-label", "substantive Windows validation")
        self.assert_blocked("platform-evidence-sibling-list", "substantive Windows validation")
        self.assert_blocked("platform-evidence-package-manager", "substantive Windows validation")
        self.assert_blocked("platform-evidence-empty-fence", "substantive Windows validation")
        self.assert_blocked("platform-evidence-punctuated-placeholder", "substantive Windows validation")
        result = self.run_gate("platform-evidence-fenced-continuation")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("platform-unaffected-rationale")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_windows_native_setup_action_needs_both_host_evidence(self):
        self.assert_blocked("platform-windows-native-action", "substantive Windows validation")
        self.assert_blocked("platform-windows-native-action-rename-out", "substantive Windows validation")

    def test_ci_workflow_needs_both_host_evidence(self):
        self.assert_blocked("platform-ci-workflow", "substantive Windows validation")

    def test_release_mcpb_preview_needs_both_host_evidence(self):
        self.assert_blocked("platform-release-mcpb-preview", "substantive Windows validation")

    def test_ci_workflow_rename_out_needs_both_host_evidence(self):
        self.assert_blocked("platform-ci-workflow-rename-out", "substantive Windows validation")

    def test_powershell_paths_need_both_host_evidence(self):
        self.assert_blocked("platform-powershell-missing", "substantive Windows validation")
        result = self.run_gate("platform-powershell-evidence")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_checklist_fetch_failure_stays_indeterminate(self):
        result = self.run_gate("checklist-fetch-fail")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("could not read review-checklist content", result.stdout)
        self.assertNotIn("description changed and no longer carries a completed", result.stdout)

    def test_database_migration_paths_need_rollback_notes(self):
        self.assert_blocked("migration-rollback-missing", "database migration path lacks non-empty rollback notes")
        result = self.run_gate("migration-rollback-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_wrapped_canonical_migration_template_field_is_recognized(self):
        result = self.run_gate("migration-template-wrapped")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_blocked("migration-template-other-field", "database migration path lacks non-empty rollback notes")

    def test_developer_home_path_shapes_are_scanned_without_echoing_values(self):
        unicode_user = chr(0x03BB) + chr(0x00E9)
        for scenario in ("home-macos", "home-unix", "home-root", "home-root-home", "home-windows", "home-macos-root", "home-unix-root", "home-windows-forward", "home-windows-escaped", "home-macos-unicode", "home-unix-unicode", "home-windows-unicode"):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("developer-home path shape", result.stdout)
                self.assertNotIn("tester", result.stdout)
                self.assertNotIn(unicode_user, result.stdout)
        result = self.run_gate("home-regex-source")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_diff_parser_uses_python38_compatible_prefix_removal(self):
        import importlib.util
        parser = ROOT / "scripts" / "merge_gate_diff.py"
        spec = importlib.util.spec_from_file_location("merge_gate_diff", parser)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        class NoRemovePrefix(str):
            def removeprefix(self, _prefix):
                raise AssertionError("Python 3.8 lacks str.removeprefix")
        self.assertEqual(module.diff_destination(NoRemovePrefix("diff --git a/x b/y")), "y")
        self.assertEqual(module.textual_destination(NoRemovePrefix("+++ b/y")), "y")

    def test_diff_parser_accepts_git_generated_mixed_quote_rename_headers(self):
        import importlib.util
        parser = ROOT / "scripts" / "merge_gate_diff.py"
        spec = importlib.util.spec_from_file_location("merge_gate_diff_mixed", parser)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        unicode_name = '"b/\\351\\233\\252.txt"'
        self.assertEqual(module.diff_destination('diff --git "a/\\351\\233\\252.txt" b/plain.txt'), "plain.txt")
        self.assertEqual(module.diff_destination('diff --git a/plain.txt ' + unicode_name), "雪.txt")
        self.assertEqual(module.diff_destination("diff --git a/foo b/bar b/foo b/bar"), "foo b/bar")

    def test_definite_blocker_wins_over_indeterminate_evidence(self):
        self.assert_blocked("draft-surface-fail", "merge state DRAFT")
        result = self.run_gate("draft-surface-fail")
        self.assertNotIn("INDETERMINATE — do not merge", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
