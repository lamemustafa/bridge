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
HEAD = "0123456789abcdef0123456789abcdef01234567"
SHORT = HEAD[:7]

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

    def run_gate(self, scenario="pass", extra_args=()):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["GATE_SCENARIO"] = scenario
        return subprocess.run(
            [str(SCRIPT), "321", "--repo", "lamemustafa/bridge", *extra_args],
            cwd=ROOT,
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

    def test_base_pinned_path_removed_from_head_is_indeterminate(self):
        self.assert_indeterminate("surface-unpins", "base-pinned path(s) are absent from the head surface")

    def test_touched_pinned_path_without_reseal_blocks(self):
        self.assert_blocked("surface-reseal-missing", "changed pinned paths omit the compatibility surface reseal")

    def test_touched_pinned_path_with_reseal_passes(self):
        self.assert_pass("surface-reseal-present", "changed pinned paths include the compatibility surface")

    # -- Privacy / PII scan (KEEP #2) ----------------------------------------

    def test_email_in_title_blocks(self):
        self.assert_blocked("privacy-email-blocker", "customer email shape")

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


if __name__ == "__main__":
    unittest.main()
