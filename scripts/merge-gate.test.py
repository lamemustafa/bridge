#!/usr/bin/env python3
"""Focused offline controls for scripts/merge-gate.sh.

The fake gh command models server responses, including paginated REST and
GraphQL pages. No network or merge operation is used.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "merge-gate.sh"
HEAD = "0123456789abcdef0123456789abcdef01234567"
FAKE_GH = r'''#!/usr/bin/env python3
import base64, json, os, sys
args = sys.argv[1:]
scenario = os.environ.get("GATE_SCENARIO", "pass")
head = "0123456789abcdef0123456789abcdef01234567"
new_head = "fedcba9876543210fedcba9876543210fedcba98"
base = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

def emit(value):
    if value is not None:
        print(value if isinstance(value, str) else json.dumps(value))

def fail(message="controlled API failure"):
    print(message, file=sys.stderr)
    raise SystemExit(1)

if args[:2] == ["pr", "view"]:
    counter_path = os.environ.get("GATE_COUNTER")
    view_count = 0
    if counter_path:
        try:
            view_count = int(open(counter_path).read())
        except (FileNotFoundError, ValueError):
            pass
        with open(counter_path, "w") as counter:
            counter.write(str(view_count + 1))
    selected_head = new_head if scenario == "head-moves" and view_count > 0 else head
    final_state = "UNKNOWN_VALUE" if scenario == "final-unrecognized" and view_count > 0 else ("BLOCKED" if scenario == "blocked-state" else ("DRAFT" if scenario == "draft-state" else "CLEAN"))
    title = "Safe merge gate control"
    if scenario == "metadata-title-id":
        title = "Customer " + "ABCDE" + "1234" + "F"
    body = (
        "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
        "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
        "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
    )
    if scenario == "body-loses-evidence" and view_count > 0:
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario == "body-loses-functional" and view_count > 0:
        body = (
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    if scenario == "checklist-foreign":
        body = "- [x] [Errors](https://github.com/other/repo/blob/HEAD/review-checklist.md#L10)"
    elif scenario == "checklist-unlinked":
        body = "- [x] review-checklist.md line 10"
    elif scenario == "checklist-template-continuation":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "- [x] One completed [`review-checklist.md`](../review-checklist.md) line is\n"
            "      linked here: https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10"
        )
    elif scenario == "missing-functional-summary":
        body = (
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario == "template-functional-prompt":
        body = (
            "## Outcome and reason\n\nWhat concrete user or maintainer workflow changes, and why now?\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario == "template-validation-prompt":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n- Exact candidate SHA:\n"
            "- Commands and results (`corepack pnpm ...`, `cargo ...`, or reproduction):\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario == "checklist-missing-anchor":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L999999)"
        )
    elif scenario == "missing-test-summary":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    one_file = scenario in {"files-empty", "formatted-phone", "formatted-phone-grouped", "path-id", "binary-delete", "metadata-only", "metadata-incomplete", "hunk-header-phone", "hunk-header-literals", "hunk-binary-literal", "separated-dates", "all-a-pan", "masked-pan", "quoted-path", "control-path"}
    selected_base = new_head if scenario == "base-oid-mismatch" else base
    emit({"headRefOid": selected_head, "baseRefOid": selected_base, "baseRefName": "master",
          "mergeable": "MERGEABLE", "mergeStateStatus": final_state,
          "isDraft": False, "state": "OPEN", "title": title,
          "body": body, "changedFiles": 1 if one_file else 2})
elif args[:2] == ["pr", "checks"]:
    if scenario == "checks-silent":
        raise SystemExit(0)
    if scenario == "cancel-check":
        emit([{"bucket": "cancel", "name": "Required checks"}])
    elif scenario == "required-skip":
        emit([{"bucket": "skipping", "name": "Required checks"}, {"bucket": "pass", "name": "Rust format"}])
    elif scenario == "missing-required":
        emit([{"bucket": "pass", "name": "Required checks"}])
    else:
        emit([{"bucket": "pass", "name": "Frontend build"},
              {"bucket": "pass", "name": "Rust format"},
              {"bucket": "pass", "name": "GitGuardian Security Checks"},
              {"bucket": "pass", "name": "Dependency security"},
              {"bucket": "pass", "name": "Required checks"},
              {"bucket": "skipping", "name": "Optional documentation"}])
elif args[:2] == ["pr", "diff"]:
    if scenario == "formatted-phone":
        phone = "+91 " + "98765" + "-43210"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+Call {phone}\n")
    elif scenario == "path-id":
        path_id = "ABCDE" + "1234" + "F"
        emit(f"diff --git a/docs/safe.md b/docs/{path_id}.md\n--- a/docs/safe.md\n+++ b/docs/{path_id}.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "binary-delete":
        emit("diff --git a/docs/old.png b/docs/old.png\nBinary files a/docs/old.png and /dev/null differ\n")
    elif scenario == "diff-omits-file":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "diff-truncated-payload":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +2 @@\n+first line\n")
    elif scenario == "formatted-phone-grouped":
        phone = "6" + "98 765-4321"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {phone}\n")
    elif scenario == "metadata-only":
        emit("diff --git a/docs/example.md b/docs/example.md\nsimilarity index 100%\nrename from docs/example.md\nrename to docs/example.md\n")
    elif scenario == "metadata-incomplete":
        emit("diff --git a/docs/example.md b/docs/example.md\nsimilarity index 100%\nrename from docs/example.md\nrename to docs/example.md\n")
    elif scenario == "hunk-header-phone":
        phone = "6" + "9876" + "54321"
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+++ b/synthetic {phone}\n")
    elif scenario == "hunk-header-literals":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +2 @@\n+++ b/safe\n+++ /dev/null\n")
    elif scenario == "hunk-binary-literal":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+GIT binary patch\n")
    elif scenario == "all-a-pan":
        identifier = "AAAAA" + "1234" + "A"
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{identifier}\n")
    elif scenario == "masked-pan":
        identifier = "XXXXX" + "1234" + "X"
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{identifier}\n")
    elif scenario == "quoted-path":
        emit('diff --git "a/docs/caf\\303\\251.md" "b/docs/caf\\303\\251.md"\n--- "a/docs/caf\\303\\251.md"\n+++ "b/docs/caf\\303\\251.md"\n@@ -0,0 +1 @@\n+safe text\n')
    elif scenario == "separated-dates":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+2026-09-12 2026-09-13\n")
    elif scenario == "surface-unpins":
        emit("diff --git a/src/example.rs b/src/example.rs\n--- a/src/example.rs\n+++ b/src/example.rs\n@@ -0,0 +1 @@\n+safe text\n"
             "diff --git a/docs/tally/compatibility/compatibility-surface.json b/docs/tally/compatibility/compatibility-surface.json\n"
             "--- a/docs/tally/compatibility/compatibility-surface.json\n+++ b/docs/tally/compatibility/compatibility-surface.json\n@@ -0,0 +1 @@\n+safe manifest\n")
    else:
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+safe text\ndiff --git a/docs/second.md b/docs/second.md\n--- a/docs/second.md\n+++ b/docs/second.md\n@@ -0,0 +1 @@\n+other text\n")
elif args and args[0] == "api":
    joined = " ".join(args)
    if "graphql" in args:
        def thread_nodes(start, count, unresolved=False):
            return [{"id": f"thread-{index}", "isResolved": not (unresolved and index == start)}
                    for index in range(start, start + count)]
        has_cursor = "C1" in joined
        if scenario == "threads-empty-more":
            nodes, page_info = [], {"hasNextPage": True, "endCursor": "C1"}
        elif scenario == "threads-short":
            nodes, page_info = thread_nodes(1, 1), {"hasNextPage": False, "endCursor": None}
        elif scenario == "threads-malformed-pagination":
            nodes, page_info = thread_nodes(1, 100), {"hasNextPage": True, "endCursor": None}
        elif has_cursor:
            if scenario == "threads-duplicate-id":
                nodes = [{"id": "thread-1", "isResolved": True}]
            else:
                nodes = thread_nodes(101, 1, scenario == "threads-unresolved-second")
            page_info = {"hasNextPage": False, "endCursor": None}
        else:
            nodes, page_info = thread_nodes(1, 100), {"hasNextPage": True, "endCursor": "C1"}
        emit({"data": {"repository": {"pullRequest": {"reviewThreads": {
            "totalCount": 101.5 if scenario == "threads-fractional" else (102 if scenario == "threads-total-drift" and has_cursor else 101),
            "pageInfo": page_info, "nodes": nodes
        }}}}})
    elif "/compare/" in joined:
        if scenario == "lineage-mismatch":
            emit({"status": "diverged", "behind_by": 1,
                  "merge_base_commit": {"sha": new_head}})
        else:
            emit({"status": "ahead", "behind_by": 0,
                  "merge_base_commit": {"sha": base}})
    elif "/commits/" in joined and "/check-runs" in joined:
        if scenario == "check-run-wrong-head":
            run_head = new_head
        else:
            run_head = head
        total_count = 3 if scenario == "check-run-count-mismatch" else 2
        emit([{"total_count": total_count, "check_runs": [
            {"name": "Required checks", "head_sha": run_head},
            {"name": "Rust format", "head_sha": run_head}]}])
    elif "/commits/" in joined and "/status" in joined:
        if scenario == "status-malformed":
            emit({"total_count": "0", "statuses": []})
        elif scenario == "status-failed-combined":
            emit({"state": "failure", "total_count": 0, "statuses": []})
        elif scenario == "status-failed-context":
            emit({"state": "failure", "total_count": 1,
                  "statuses": [{"context": "legacy optional", "state": "failure", "sha": head}]})
        else:
            emit({"state": "success", "total_count": 0, "statuses": []})
    elif "/pulls/321/commits" in joined:
        message = "safe commit metadata"
        if scenario == "metadata-commit-id":
            message = "Customer " + "ABCDE" + "1234" + "F"
        emit([[{"sha": head, "commit": {"message": message}}]])
    elif "branches/master/protection/required_status_checks" in joined:
        contexts = [
            "Frontend build", "Rust format", "GitGuardian Security Checks",
            "Dependency security", "Required checks"]
        if scenario == "protection-missing-gitguardian":
            contexts = [context for context in contexts if context != "GitGuardian Security Checks"]
        emit({"contexts": contexts, "checks": []})
    elif "branches/master" in joined:
        emit(base)
    elif "/pulls/321/reviews" in joined:
        if scenario in {"short-review", "summary-only", "manual-summary"}:
            emit([[]])
        else:
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"},
                     "state": "COMMENTED", "commit_id": head}]])
    elif "/issues/321/comments" in joined:
        if scenario == "short-review":
            summary = f"codex-pull-request-review-summary\n| 📝 | ✅ **Completed** | `{head[:7]}` |"
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "body": summary}]])
        elif scenario == "summary-failed":
            summary = f"codex-pull-request-review-summary\n| 📝 | ❌ **Failed** | `{head[:7]}` |"
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "body": summary}]])
        elif scenario in {"summary-only", "manual-summary"}:
            summary = f"codex-pull-request-review-summary\n| 📝 | ✅ **Completed** | `{head[:7]}` |"
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "body": summary}]])
        else:
            summary = f"codex-pull-request-review-summary\n| 📝 | ✅ **Completed** | `{head[:7]}` |"
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "body": summary}]])
    elif "/pulls/321/files" in joined:
        if scenario == "files-empty":
            emit([[]])
        elif scenario in {"formatted-phone", "formatted-phone-grouped"}:
            emit([[{"filename": "docs/contact.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario == "path-id":
            path_id = "ABCDE" + "1234" + "F"
            emit([[{"filename": f"docs/{path_id}.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario == "binary-delete":
            emit([[{"filename": "docs/old.png", "status": "removed", "additions": 0, "deletions": 0}]])
        elif scenario == "metadata-only":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 0, "deletions": 0}]])
        elif scenario == "metadata-incomplete":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "hunk-header-phone":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "hunk-header-literals":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 2, "deletions": 0}]])
        elif scenario == "hunk-binary-literal":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"all-a-pan", "masked-pan", "separated-dates"}:
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "quoted-path":
            emit([[{"filename": "docs/café.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario == "control-path":
            emit([[{"filename": "docs/unsafe\x1b.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario == "malformed-files":
            emit([[{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}], [{"filename": 3, "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "missing-file-status":
            emit([[{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}], [{"filename": "docs/second.md", "additions": 1, "deletions": 0}]])
        elif scenario == "surface-unpins":
            emit([[{"filename": "src/example.rs", "status": "modified", "additions": 1, "deletions": 0}],
                  [{"filename": "docs/tally/compatibility/compatibility-surface.json", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "files-count-mismatch":
            emit([[{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}]])
        else:
            additions = 2 if scenario == "diff-truncated-payload" else 1
            emit([[{"filename": "docs/example.md", "status": "added", "additions": additions, "deletions": 0}], [{"filename": "docs/second.md", "status": "modified", "additions": 1, "deletions": 0}]])
    elif "/contents/" in joined:
        if "review-checklist.md" in joined:
            checklist = "\n" * 9 + "- [ ] Errors are actionable without exposing sensitive values.\n"
            emit({"encoding": "base64", "content": base64.b64encode(checklist.encode()).decode()})
        elif scenario == "surface-fail":
            fail("controlled surface read failure")
        elif scenario == "surface-malformed":
            emit({"content": "not-base64"})
        else:
            digest = "a" * 64
            if scenario == "surface-unpins" and f"ref={head}" not in joined:
                surface_files = [
                    {"path": "src/example.rs", "sha256": digest},
                    {"path": "docs/tally/compatibility/compatibility-surface.json", "sha256": digest},
                ]
            else:
                surface_files = [{"path": "src/example.rs", "sha256": digest}]
            surface = {"schema_version": 1, "manifest_sha256": digest,
                       "files": surface_files}
            if scenario == "surface-schema-malformed":
                surface = {"files": [{"path": "src/example.rs"}]}
            emit({"encoding": "base64", "content": base64.b64encode(json.dumps(surface).encode()).decode()})
    else:
        fail("unknown API fixture")
else:
    fail("unknown command fixture")
'''


class MergeGateControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="merge-gate-controls-")
        cls.bin = Path(cls.tmp.name)
        gh = cls.bin / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(0o755)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_gate(self, scenario="pass", extra_args=()):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["GATE_SCENARIO"] = scenario
        counter = self.bin / f"{scenario}-counter-{os.getpid()}"
        counter.write_text("0")
        env["GATE_COUNTER"] = str(counter)
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

    def test_optional_skipped_check_does_not_block(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("optional check(s) are skipped", result.stdout)

    def test_required_skipped_check_blocks(self):
        self.assert_blocked("required-skip", "required check 'Required checks' is not passing")

    def test_missing_required_context_blocks(self):
        self.assert_blocked("missing-required", "required check 'Rust format' was not reported")

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

    def test_thread_total_drift_is_indeterminate(self):
        self.assert_indeterminate("threads-total-drift", "totalCount changed")

    def test_head_change_is_blocked(self):
        self.assert_blocked("head-moves", "PR head moved during preflight")

    def test_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone", "privacy scan found")

    def test_grouped_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone-grouped", "privacy scan found")

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

    def test_destination_path_is_scanned(self):
        self.assert_blocked("path-id", "privacy scan found")

    def test_binary_deletion_is_not_treated_as_added_content(self):
        result = self.run_gate("binary-delete")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

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

    def test_base_oid_mismatch_is_indeterminate(self):
        self.assert_indeterminate("base-oid-mismatch", "PR base OID")

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
        self.assert_indeterminate("status-failed-combined", "head-bound commit-status evidence")

    def test_failing_individual_commit_status_blocks(self):
        self.assert_indeterminate("status-failed-context", "head-bound commit-status evidence")

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

    def test_all_a_pan_is_not_exempted_as_a_placeholder(self):
        self.assert_blocked("all-a-pan", "privacy scan found")

    def test_explicit_masked_pan_remains_a_placeholder(self):
        result = self.run_gate("masked-pan")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_quoted_git_destination_path_is_covered(self):
        result = self.run_gate("quoted-path")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_control_character_in_destination_path_is_indeterminate(self):
        self.assert_indeterminate("control-path", "could not read the complete changed-file set")


if __name__ == "__main__":
    unittest.main(verbosity=2)
