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
FAKE_GH = r'''#!/usr/bin/env python3
import base64, json, os, sys
args = sys.argv[1:]
scenario = os.environ.get("GATE_SCENARIO", "pass")
head = "0123456789abcdef0123456789abcdef01234567"
new_head = "fedcba9876543210fedcba9876543210fedcba98"

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
    emit({"headRefOid": selected_head, "baseRefName": "master",
          "mergeable": "MERGEABLE", "mergeStateStatus":
          "BLOCKED" if scenario == "blocked-state" else "CLEAN",
          "isDraft": False, "state": "OPEN",
          "body": "[review-checklist.md](../blob/master/review-checklist.md)\n- [x] evidence"})
elif args[:2] == ["pr", "checks"]:
    if scenario == "checks-silent":
        raise SystemExit(0)
    if scenario == "cancel-check":
        emit([{"bucket": "cancel", "name": "Required checks"}])
    elif scenario == "missing-required":
        emit([{"bucket": "pass", "name": "Required checks"}])
    else:
        emit([{"bucket": "pass", "name": "Frontend build"},
              {"bucket": "pass", "name": "Rust format"},
              {"bucket": "pass", "name": "GitGuardian Security Checks"},
              {"bucket": "pass", "name": "Dependency security"},
              {"bucket": "pass", "name": "Required checks"}])
elif args[:2] == ["pr", "diff"]:
    if scenario == "formatted-phone":
        emit("diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+Call +91 98765-43210\n")
    elif scenario == "path-id":
        emit("diff --git a/docs/safe.md b/docs/ABCDE1234F.md\n--- a/docs/safe.md\n+++ b/docs/ABCDE1234F.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "binary-delete":
        emit("diff --git a/docs/old.png b/docs/old.png\nBinary files a/docs/old.png and /dev/null differ\n")
    else:
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+safe text\n")
elif args and args[0] == "api":
    joined = " ".join(args)
    if "graphql" in args:
        has_cursor = "C1" in joined
        emit({"data": {"repository": {"pullRequest": {"reviewThreads": {
            "totalCount": 101 if not has_cursor else 101,
            "pageInfo": {"hasNextPage": False, "endCursor": None},
            "nodes": ([{"isResolved": True}] if not has_cursor else [{"isResolved": True}])
        }}}}})
    elif "branches/master/protection/required_status_checks" in joined:
        contexts = ["Required checks", "Rust format"] if scenario == "missing-required" else [
            "Frontend build", "Rust format", "GitGuardian Security Checks",
            "Dependency security", "Required checks"]
        emit({"contexts": contexts, "checks": []})
    elif "branches/master" in joined:
        emit(head)
    elif "/pulls/321/reviews" in joined:
        if scenario == "short-review":
            emit([[]])
        else:
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"},
                     "state": "COMMENTED", "commit_id": head}]])
    elif "/issues/321/comments" in joined:
        if scenario == "short-review":
            emit([[{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"},
                     "body": "codex-pull-request-review-summary\n| 📝 | ✅ **Completed** | `0123456` |"}]])
        else:
            emit([[]])
    elif "/pulls/321/files" in joined:
        emit([[{"filename": "docs/example.md"}], [{"filename": "docs/second.md"}]])
    elif "/contents/" in joined:
        if scenario == "surface-fail":
            fail("controlled surface read failure")
        if scenario == "surface-malformed":
            emit({"content": "not-base64"})
        else:
            surface = {"files": [{"path": "src/example.rs"}]}
            emit({"content": base64.b64encode(json.dumps(surface).encode()).decode()})
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

    def run_gate(self, scenario="pass"):
        env = os.environ.copy()
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["GATE_SCENARIO"] = scenario
        counter = self.bin / f"{scenario}-counter-{os.getpid()}"
        counter.write_text("0")
        env["GATE_COUNTER"] = str(counter)
        return subprocess.run(
            [str(SCRIPT), "321", "--repo", "example/repo"],
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

    def test_missing_required_context_blocks(self):
        self.assert_blocked("missing-required", "required check 'Rust format' was not reported")

    def test_cancelled_check_blocks(self):
        self.assert_blocked("cancel-check", "cancelled, pending, or skipped")

    def test_surface_transport_failure_is_indeterminate(self):
        self.assert_indeterminate("surface-fail", "could not read compatibility surface")

    def test_silent_checks_response_is_indeterminate(self):
        self.assert_indeterminate("checks-silent", "checks query returned no JSON")

    def test_summary_only_short_sha_is_indeterminate(self):
        self.assert_indeterminate("short-review", "full-SHA provider evidence")

    def test_blocked_merge_state_cannot_pass(self):
        self.assert_blocked("blocked-state", "merge state BLOCKED")

    def test_head_change_is_blocked(self):
        self.assert_blocked("head-moves", "PR head moved during preflight")

    def test_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone", "privacy scan found")

    def test_destination_path_is_scanned(self):
        self.assert_blocked("path-id", "privacy scan found")

    def test_binary_deletion_is_not_treated_as_added_content(self):
        result = self.run_gate("binary-delete")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_malformed_surface_is_indeterminate(self):
        self.assert_indeterminate("surface-malformed", "compatibility surface could not be decoded")


if __name__ == "__main__":
    unittest.main(verbosity=2)
