#!/usr/bin/env python3
"""Focused offline controls for the shrunk scripts/merge-gate.sh.

The gate was cut from 3977 lines of merge-gate.sh/.test.py/_fake_gh.py/
_privacy.py/_diff.py down to only what runs as a real required CI check:
compatibility-surface validation, the privacy/PII scan, and a minimal
review-evidence-names-current-head check. This suite exercises only that
kept surface, plus a dedicated regression test for each of the 7 defects
fixed in the kept code (#346, #358, #360, #365, #366, #373, #377).

The fake `gh` (merge_gate_test_gh.py) models server responses; no network or
merge operation is used.
"""
from __future__ import annotations

import importlib.util
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "merge-gate.sh"
FAKE_GH = ROOT / "scripts" / "merge_gate_test_gh.py"
PRIVACY_MODULE_PATH = ROOT / "scripts" / "merge_gate_privacy.py"
HEAD = "0123456789abcdef0123456789abcdef01234567"
SHORT = HEAD[:7]


def load_privacy_module():
    """Load scripts/merge_gate_privacy.py fresh, bypassing sys.modules.

    A fresh load (rather than a cached import) matters here: several of the
    PrivacyScannerFindingsPR335 tests are run by hand against a deliberately
    reverted copy of the file to prove they fail on the pre-fix behavior
    described in PR #335 review, then re-run against the restored file. A
    cached import would silently keep serving the first version loaded.
    """
    spec = importlib.util.spec_from_file_location("merge_gate_privacy", PRIVACY_MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

MKTEMP_WRAPPER = """#!/usr/bin/env bash
# Only scan-input-write-failure pre-occupies the privacy-scan-input path (as
# a directory) so the write inside merge-gate.sh's redirect group fails.
real=$(command -v -p mktemp)
if [ "${GATE_SCENARIO:-}" = "scan-input-write-failure" ] && [ "$1" = "-d" ]; then
  dir=$("$real" -d)
  mkdir "$dir/privacy-scan-input"
  printf '%s\\n' "$dir"
else
  exec "$real" "$@"
fi
"""


class MergeGateControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="merge-gate-controls-")
        cls.bin = Path(cls.tmp.name)
        gh = cls.bin / "gh"
        shutil.copyfile(FAKE_GH, gh)
        gh.chmod(0o755)
        mktemp = cls.bin / "mktemp"
        mktemp.write_text(MKTEMP_WRAPPER)
        mktemp.chmod(mktemp.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
        if not shutil.which("jq"):
            raise RuntimeError("jq is required for merge-gate controls")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_gate(self, scenario="pass", extra_args=(), cwd=None):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["GATE_SCENARIO"] = scenario
        return subprocess.run(
            [str(SCRIPT), "321", "--repo", "lamemustafa/bridge", *extra_args],
            cwd=str(cwd) if cwd else ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def assert_pass(self, scenario, phrase=None, extra_args=()):
        result = self.run_gate(scenario, extra_args)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("MAY MERGE", result.stdout)
        if phrase:
            self.assertIn(phrase, result.stdout)
        return result

    def assert_blocked(self, scenario, phrase, extra_args=()):
        result = self.run_gate(scenario, extra_args)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(phrase, result.stdout)
        self.assertNotIn("MAY MERGE", result.stdout)
        return result

    def assert_indeterminate(self, scenario, phrase, extra_args=()):
        result = self.run_gate(scenario, extra_args)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn(phrase, result.stdout)
        self.assertNotIn("MAY MERGE", result.stdout)
        return result

    # -- Baseline -----------------------------------------------------------

    def test_pass_scenario_may_merge(self):
        self.assert_pass("pass", "review evidence names the current head")

    # -- Compatibility-surface validation (KEEP #1) --------------------------

    def test_surface_fetch_failure_is_indeterminate(self):
        self.assert_indeterminate("surface-fetch-fail", "could not read and validate compatibility surface at " + SHORT)

    def test_surface_malformed_is_indeterminate(self):
        self.assert_indeterminate("surface-malformed", "could not read and validate compatibility surface at " + SHORT)

    def test_surface_head_on_schema_1_is_indeterminate(self):
        self.assert_indeterminate("surface-head-schema1", "could not read and validate compatibility surface at " + SHORT)

    def test_base_pinned_path_removed_from_head_is_indeterminate(self):
        self.assert_indeterminate("surface-unpins", "base-pinned path(s) are absent from the head surface")

    def test_touched_pinned_path_without_reseal_blocks(self):
        self.assert_blocked("surface-reseal-missing", "changed pinned paths omit the compatibility surface reseal")

    def test_touched_pinned_path_with_reseal_passes(self):
        self.assert_pass("surface-reseal-present", "changed pinned paths include the compatibility surface")

    # -- Privacy / PII scan (KEEP #2) ----------------------------------------

    def test_email_in_title_blocks(self):
        self.assert_blocked("privacy-email-blocker", "customer email shape")

    def test_known_public_agent_address_in_a_well_formed_commit_trailer_is_identity_metadata(self):
        self.assert_pass("public-agent-coauthor-trailer", "review evidence names the current head")

    def test_known_public_agent_address_in_a_well_formed_crlf_commit_trailer_is_identity_metadata(self):
        self.assert_pass("public-agent-coauthor-trailer-crlf", "review evidence names the current head")

    def test_arbitrary_customer_address_in_a_coauthor_trailer_still_blocks(self):
        self.assert_blocked("customer-coauthor-trailer", "customer email shape")

    def test_arbitrary_customer_address_in_a_crlf_coauthor_trailer_still_blocks(self):
        self.assert_blocked("customer-coauthor-trailer-crlf", "customer email shape")

    def test_known_public_agent_address_outside_a_commit_trailer_still_blocks(self):
        self.assert_blocked("public-agent-email-in-pr-body", "customer email shape")

    def test_known_public_agent_address_in_source_payload_still_blocks(self):
        self.assert_blocked("public-agent-email-in-source-payload", "customer email shape")

    def test_spoofed_coauthor_header_still_blocks(self):
        self.assert_blocked("public-agent-spoof-header", "customer email shape")

    def test_coauthor_line_outside_the_trailer_footer_still_blocks(self):
        self.assert_blocked("public-agent-nonfooter", "customer email shape")

    def test_crlf_coauthor_line_outside_the_trailer_footer_still_blocks(self):
        self.assert_blocked("public-agent-nonfooter-crlf", "customer email shape")

    def test_mixed_line_endings_still_block(self):
        self.assert_blocked("public-agent-mixed-line-endings", "customer email shape")

    def test_malformed_coauthor_trailer_still_blocks(self):
        self.assert_blocked("public-agent-malformed-trailer", "customer email shape")

    def test_malformed_crlf_coauthor_trailer_still_blocks(self):
        self.assert_blocked("public-agent-malformed-trailer-crlf", "customer email shape")

    def test_coauthor_trailer_with_extra_payload_still_blocks(self):
        self.assert_blocked("public-agent-trailer-extra-payload", "customer email shape")

    def test_public_agent_address_in_the_author_name_still_blocks(self):
        self.assert_blocked("public-agent-email-in-author-name", "customer email shape")

    def test_customer_address_in_author_name_with_public_agent_trailer_still_blocks(self):
        self.assert_blocked("customer-email-in-author-name-with-public-agent-trailer", "customer email shape")

    def test_credential_literal_blocks(self):
        self.assert_blocked("privacy-credential-blocker", "literal credential, bearer, or API token value")

    def test_unknown_uuid_is_indeterminate(self):
        self.assert_indeterminate("privacy-uuid-indeterminate", "without exact current-head fixture provenance")

    def test_binary_addition_without_attestation_blocks(self):
        self.assert_blocked("binary-missing-attestation", "require matching --binary-review-sha and --independent-review-sha")

    def test_binary_addition_with_attestation_passes(self):
        self.assert_pass(
            "binary-with-attestation",
            "have explicit current-head binary and independent review attestations",
            extra_args=("--binary-review-sha", HEAD, "--independent-review-sha", HEAD),
        )

    def test_gitlink_change_is_indeterminate(self):
        self.assert_indeterminate("gitlink-change", "gitlink change(s) require explicit provenance")

    # -- Minimal head-SHA review-evidence check (KEEP #3, new) ---------------

    def test_review_object_naming_head_passes(self):
        self.assert_pass("review-names-head-via-review", "review evidence names the current head")

    def test_summary_comment_naming_head_passes(self):
        self.assert_pass("review-names-head-via-comment", "review evidence names the current head")

    def test_no_review_evidence_blocks(self):
        self.assert_blocked("review-missing", "no review evidence (review or comment) names current head")
        self.assertIn("#317", self.run_gate("review-missing").stdout)

    # -- Regression tests for the 7 fixes in the kept code -------------------

    def test_346_parenthesized_date_range_not_flagged(self):
        # Before the fix, GROUPED_NUMBER_RE's captured boundary character
        # (here "(" and ")") was left in match.group(0), so DATE_RANGE_RE's
        # fullmatch never matched and the date range was misreported as an
        # unexplained long digit run.
        result = self.assert_pass("date-range-parens")
        self.assertNotIn("unexplained long digit run", result.stdout)

    def test_358_scan_input_write_failure_is_indeterminate(self):
        # Before the fix, `{ printf ...; cat ...; } >"$scan_input_file"` had
        # no `|| status=$?`, so a failed write was never detected and a
        # truncated/empty file was silently fed to the privacy classifier.
        self.assert_indeterminate("scan-input-write-failure", "could not assemble complete privacy scan input")

    def test_360_lfs_pointer_requires_binary_attestation(self):
        # Before the fix, an added Git LFS pointer (ordinary text, no "GIT
        # binary patch" marker) was reconciled as a normal textual change and
        # never required a binary-review attestation.
        self.assert_blocked("lfs-pointer-binary", "require matching --binary-review-sha and --independent-review-sha")

    def test_365_removed_record_line_total_mismatch_is_flagged(self):
        # Before the fix, `[ "$status" = "removed" ] && continue` skipped a
        # removed REST record before its line totals were ever compared
        # against the diff, so a mismatch went unreported.
        self.assert_indeterminate("removed-file-mismatch", "line totals for 'docs/retired.md' differ from REST metadata")

    def test_366_coverage_example_redacts_identifier_shape(self):
        # Before the fix, only home-directory paths were redacted from a
        # coverage-issue filename before it was echoed into gate output; an
        # identifier-shaped filename reached the message verbatim. (The same
        # filename is also, independently, real scan input -- path_text is
        # always scanned raw -- so this PR is separately and correctly
        # BLOCKed by the classifier itself; that is not what this test
        # checks. It checks that the *coverage diagnostic message* never
        # echoes the raw identifier shape.)
        result = self.assert_blocked("coverage-example-redaction", "omits or duplicates 'docs/<identifier>.md'")
        self.assertNotIn("ABCDE1234F", result.stdout)

    def test_373_diff_destination_without_rest_record_is_flagged(self):
        # Before the fix, reconciliation only walked REST records looking for
        # a matching diff section; a diff section with no REST record at all
        # (the reverse direction) was never flagged.
        self.assert_indeterminate("diff-extra-destination", "diff destination 'docs/smuggled.md' has no corresponding REST record")

    def test_377_localhost_author_email_is_accepted(self):
        # Before the fix, the author/committer email regex required a dotted
        # two-letter TLD, so a Git-valid address like dev@localhost rejected
        # the whole commit-metadata fetch and the PR went INDETERMINATE.
        result = self.assert_pass("author-email-localhost")
        self.assertNotIn("could not prove complete head-bound PR commit metadata", result.stdout)

    def test_finding374_helpers_resolve_relative_to_the_script(self):
        """#374: the gate must locate its own helper scripts by $script_dir.

        Every other test runs with cwd=ROOT, so a working-directory-relative
        invocation of merge_gate_diff.py passes there and fails anywhere else.
        Running the gate from an unrelated directory is the only thing that
        exercises the difference.
        """
        with tempfile.TemporaryDirectory(prefix="merge-gate-cwd-") as elsewhere:
            result = self.run_gate("pass", cwd=elsewhere)
        combined = result.stdout + result.stderr
        self.assertNotIn("merge_gate_diff.py: No such file or directory", combined)
        self.assertNotIn("can't open file", combined)
        self.assertEqual(result.returncode, 0, combined)
        self.assertIn("MAY MERGE", result.stdout)



class PrivacyScannerFindingsPR335(unittest.TestCase):
    """Direct unit coverage of scripts/merge_gate_privacy.py, one pair of
    tests (positive + negative) per surviving PR #335 review finding against
    the promoted-to-required privacy/PII scanner. Thread ids are the ones
    from the PR #335 review; all 11 were investigated and found to already
    be fixed on this branch (see the PR description / task report for the
    commit-by-commit evidence) -- these tests exist to lock that behavior in
    as regression coverage, since no prior test exercised these specific
    sub-cases directly.

    Every positive test is paired with a negative test asserting a
    legitimate, similarly-shaped value is NOT flagged -- issue #328 was a
    phone normalizer that fused adjacent numbers and flagged ordinary date
    ranges, so a widened/whole-line/multi-match PII pattern gets a
    false-positive check alongside its detection check.
    """

    def setUp(self):
        self.privacy = load_privacy_module()

    def blockers(self, text):
        return self.privacy.scan(text, HEAD)["blockers"]

    def assert_blocked(self, text, phrase):
        blockers = self.blockers(text)
        self.assertTrue(any(phrase in b for b in blockers), blockers)

    def assert_not_blocked(self, text):
        blockers = self.blockers(text)
        self.assertEqual(blockers, [], blockers)

    # Finding 1 -- PRRT_kwDOTWMyis6h8G4J: normalize standard 3-digit-area-
    # code Indian landline formats (e.g. 022-23456789, 011 23456789).
    def test_finding1_standard_3digit_landline_blocked(self):
        self.assert_blocked("Customer landline: 022-23456789", "long digit run")
        self.assert_blocked("Customer landline: 011 23456789", "long digit run")

    def test_finding1_negative_no_landline_not_blocked(self):
        self.assert_not_blocked("This changelog entry references no landline number at all.")
        self.assert_not_blocked(
            "Build step 0-1 ran before step 2345 in the pipeline; step 6789 followed."
        )

    # Finding 2 -- PRRT_kwDOTWMyis6h8Z5U: recognize variable-length (4-digit)
    # landline area codes (e.g. 0120-2345678).
    def test_finding2_variable_length_area_code_blocked(self):
        self.assert_blocked("Customer landline: 0120-2345678", "long digit run")

    def test_finding2_negative_non_landline_shapes_not_blocked(self):
        # Leading digit isn't 0: not a landline area code.
        self.assert_not_blocked("Invoice 9120-2345678 was issued to a vendor.")
        # Subscriber half is only 6 digits: one short of the 7-digit form.
        self.assert_not_blocked("Ticket 0120-234567 was closed.")

    # Finding 3 -- PRRT_kwDOTWMyis6h8G4L: hold raw certificate payloads
    # (bare "-----BEGIN CERTIFICATE-----") for human review.
    def test_finding3_bare_certificate_envelope_blocked(self):
        self.assert_blocked(
            "-----BEGIN CERTIFICATE-----\n"
            "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0000000000000000\n"
            "-----END CERTIFICATE-----",
            "PEM certificate envelope",
        )

    def test_finding3_negative_plain_mention_not_blocked(self):
        self.assert_not_blocked("Renew the TLS certificate before it expires next month.")

    # Finding 4 -- PRRT_kwDOTWMyis6h8Z5W: block trusted-certificate PEM
    # envelopes ("-----BEGIN TRUSTED CERTIFICATE-----"), not just bare/X509.
    def test_finding4_trusted_certificate_envelope_blocked(self):
        self.assert_blocked(
            "-----BEGIN TRUSTED CERTIFICATE-----\n"
            "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0000000000000000\n"
            "-----END TRUSTED CERTIFICATE-----",
            "PEM certificate envelope",
        )

    def test_finding4_negative_non_certificate_pem_not_blocked(self):
        # A different PEM envelope kind (not a certificate) is out of this
        # finding's scope and must not trip the certificate-specific rule.
        self.assert_not_blocked(
            "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A\n-----END PUBLIC KEY-----"
        )

    # Finding 5 -- PRRT_kwDOTWMyis6h8Z5Z: block UUID-shaped credential
    # session IDs instead of exempting every UUID wholesale.
    def test_finding5_uuid_credential_session_id_blocked(self):
        self.assert_blocked(
            "credential_session_id: 3fae1c2b-9d4e-4a11-8f2c-7b6d5e4a3c21",
            "credential, session, token, or bearer",
        )

    def test_finding5_negative_nil_uuid_sentinel_not_blocked(self):
        # The all-zero UUID is an explicit, documented sentinel and must stay
        # exempt even in credential context, or every fixture using it as a
        # placeholder session id would wrongly block.
        self.assert_not_blocked("credential_session_id: 00000000-0000-0000-0000-000000000000")

    # Finding 6 -- PRRT_kwDOTWMyis6h_z-h: block non-UUID credential tokens
    # (credential context must not be UUID-only).
    def test_finding6_non_uuid_bearer_token_blocked(self):
        self.assert_blocked(
            "Authorization: Bearer example-live-token-abcdefghijklmnop",
            "literal credential, bearer, or API token",
        )

    def test_finding6_negative_env_substitution_not_blocked(self):
        self.assert_not_blocked("Authorization: Bearer $BEARER_TOKEN")

    # Finding 7 -- PRRT_kwDOTWMyis6h_z-m: detect customer email addresses in
    # added payloads, distinct from admitted commit-author identities.
    def test_finding7_customer_email_blocked(self):
        self.assert_blocked("Customer contact: jane.doe@customerdomain.test", "customer email shape")

    def test_finding7_negative_example_domain_not_blocked(self):
        self.assert_not_blocked("Reviewer contact: dev@example.com")

    # Finding 8 -- PRRT_kwDOTWMyis6iBarl: recognize quoted credential keys
    # (e.g. {"api_key": "..."}), not just bare/unquoted assignments.
    def test_finding8_quoted_credential_key_blocked(self):
        self.assert_blocked(
            '{"api_key": "customerproductioncredential"}',
            "literal credential, bearer, or API token",
        )

    def test_finding8_negative_quoted_placeholder_not_blocked(self):
        self.assert_not_blocked('{"api_key": "your_api_key"}')

    # Finding 9 -- PRRT_kwDOTWMyis6iBarn: stop exempting credential values
    # that merely resemble type names (...Token, ...Secret) when lower-case.
    def test_finding9_lowercase_typelike_values_not_exempted(self):
        self.assert_blocked("access_token: productionToken", "literal credential, bearer, or API token")
        self.assert_blocked("client_secret: supersecret", "literal credential, bearer, or API token")

    def test_finding9_negative_real_type_annotation_not_blocked(self):
        # Genuine type-syntax spellings (leading-capital ...Token/...Secret,
        # or the established lower-case type names) must stay exempt.
        self.assert_not_blocked("access_token: AccessToken")
        self.assert_not_blocked("client_secret: ClientSecret")
        self.assert_not_blocked("client_secret: str")

    # Finding 10 -- PRRT_kwDOTWMyis6iC356: scan password/passphrase/private-
    # key assignments as credential literals, not just api_key/token/secret.
    def test_finding10_password_assignment_blocked(self):
        self.assert_blocked(
            '{"password":"customerproductionpassword"}',
            "literal credential, bearer, or API token",
        )

    def test_finding10_negative_password_placeholder_not_blocked(self):
        self.assert_not_blocked('{"password":"REDACTED"}')

    # Finding 11 -- PRRT_kwDOTWMyis6iC36H: inspect every credential
    # assignment on a line, not just the first.
    def test_finding11_second_assignment_on_line_inspected(self):
        mixed = self.blockers(
            '{"api_key":"example_api_key","client_secret":"customerproductionsecret"}'
        )
        self.assertEqual(len(mixed), 1, mixed)
        self.assertIn("1 literal credential", mixed[0])
        both_real = self.blockers(
            '{"api_key":"customerprodkeyabc","client_secret":"customerprodsecretxyz"}'
        )
        self.assertEqual(len(both_real), 1, both_real)
        self.assertIn("2 literal credential", both_real[0])

    def test_finding11_negative_all_placeholders_not_blocked(self):
        self.assert_not_blocked(
            '{"api_key":"your_api_key","client_secret":"replace_me_client_secret"}'
        )


if __name__ == "__main__":
    unittest.main()
