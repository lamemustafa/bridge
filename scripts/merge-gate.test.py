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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "merge-gate.sh"
HEAD = "0123456789abcdef0123456789abcdef01234567"
FAKE_GH = r'''#!/usr/bin/env python3
import base64, json, os, sys
args = sys.argv[1:]
scenario = os.environ.get("GATE_SCENARIO", "pass")
security_case = scenario.startswith("security-review-")
sync_case = scenario.startswith("sync-")
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
    final_state = "UNKNOWN_VALUE" if scenario == "final-unrecognized" and view_count > 0 else ("BLOCKED" if scenario == "blocked-state" else ("DRAFT" if scenario in {"draft-state", "draft-surface-fail"} else "CLEAN"))
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
    elif scenario in {"template-validation-prompt", "template-validation-sha-only"}:
        sha = "deadbeef" if scenario == "template-validation-sha-only" else ""
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            f"## Validation and evidence\n\n- Exact candidate SHA: {sha}\n"
            "- Commands and results (`corepack pnpm ...`, `cargo ...`, or reproduction):\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario == "template-validation-empty":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\nCommands and results are recorded elsewhere.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario in {"workflow-notes-present", "workflow-delete-notes", "workflow-rename-out-notes"}:
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "## Rollback notes\n\nRevert the workflow commit.\n\n"
            "## Migration compatibility\n\nNo persisted data changes.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario in {"security-notes-present", "surface-unpins"}:
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "## Security impact\n\nNo credential material is added; the Tally path change is reviewed.\n\n"
            "## Migration compatibility\n\nExisting callers retain their paths and formats.\n\n"
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
    if security_case or sync_case:
        body += "\n## Security impact\n\nCredential admission remains enforced.\n"
        if not scenario.endswith("migration-missing"):
            body += "\n## Migration compatibility\n\nNo persisted format change.\n"
    if scenario == "validation-command-outside":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "Tests not run") + "\n## Rollback\n`cargo test`\n"
    if scenario == "validation-tool-prose":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "Tests not run; cargo is available")
    if scenario == "validation-placeholder-command":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "`cargo ...`")
    if scenario == "checklist-heading":
        body = body.replace("#L10", "#L1")
    if scenario in {"implementation-p4-present", "platform-evidence-present", "platform-checkbox-evidence", "platform-checkbox-comment", "migration-rollback-present", "migration-template-wrapped", "security-notes-present", "security-review-valid"}:
        body += (
            "\n## Scope, reuse, and impact\n\n"
            "- Existing component reused: the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified): no duplicate path remains.\n"
            "- What breaks if this is not built: unsafe evidence could reach a merge.\n"
        )
    if security_case or scenario in {"surface-unpins", "migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"}:
        body += (
            "\n## Scope, reuse, and impact\n\n"
            "- Existing component reused: the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified): no duplicate path remains.\n"
            "- What breaks if this is not built: unsafe evidence could reach a merge.\n"
            "\n## Security impact\n\nNo credential material is added.\n"
            "\n## Migration compatibility\n\nExisting callers retain their paths and formats.\n"
            "\n- Windows validation evidence: Windows CI ran `python3 scripts/merge-gate.test.py`.\n"
            "- macOS validation evidence: macOS CI ran `python3 scripts/merge-gate.test.py`.\n"
        )
    if scenario in {"platform-evidence-present", "migration-template-wrapped", "security-notes-present", "security-review-valid", "sync-migration-present"}:
        body += (
            "\n- Windows validation evidence: Windows CI ran `python3 scripts/merge-gate.test.py`.\n"
            "- macOS validation evidence: macOS CI ran `python3 scripts/merge-gate.test.py`.\n"
        )
    if scenario == "platform-checkbox-evidence":
        body += (
            "\n- [x] Native Windows validation completed: `python3 scripts/merge-gate.test.py` passed on Windows CI.\n"
            "- [x] Native macOS validation completed: `python3 scripts/merge-gate.test.py` passed on macOS CI.\n"
        )
    if scenario == "platform-checkbox-comment":
        body += (
            "\n- [x] Native Windows validation completed: <!-- paste evidence -->\n"
            "- [x] Native macOS validation completed: <!-- paste evidence -->\n"
        )
    if scenario in {"platform-evidence-present", "platform-checkbox-evidence"}:
        body += (
            "\n## Security impact\n\nNo credential material is added.\n"
            "\n## Migration compatibility\n\nExisting callers retain their paths and formats.\n"
        )
    if scenario == "migration-rollback-present":
        body += "\n## Rollback notes\n\nRevert the migration commit before deployment.\n"
    if scenario == "migration-template-wrapped":
        body += (
            "\n- Migration/sync compatibility and rollback procedure (required when an\n"
            "  existing workflow changes): Existing readers retain the old format; revert this commit before deployment.\n"
        )
    if scenario == "migration-template-other-field":
        body += (
            "\n- Migration/sync compatibility and rollback procedure (required when an\n"
            "  existing workflow changes):\n"
            "- Destructive database migration: No\n"
        )
    body = body.replace("blob/HEAD", f"blob/{head}")
    if scenario == "checklist-stale-ref":
        body = body.replace(f"blob/{head}", "blob/" + "f" * 40)
    one_file = scenario in {"files-empty", "formatted-phone", "formatted-phone-grouped", "repeated-phone", "grouped-identifier-12", "grouped-identifier-16", "phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized", "path-id", "binary-delete", "metadata-only", "metadata-incomplete", "hunk-header-phone", "hunk-header-literals", "hunk-binary-literal", "separated-dates", "separated-dates-new-year", "separated-dates-year-month", "adr-identifier", "all-a-pan", "masked-pan", "quoted-path", "control-path", "workflow-notes-missing", "workflow-notes-present", "workflow-delete", "workflow-delete-notes", "workflow-rename-out", "workflow-rename-out-notes", "renamed-previous-missing", "renamed-previous-null", "renamed-previous-false", "security-notes-missing", "security-notes-present", "security-rename-out", "security-crate", "security-agent-import", "security-dsc", "home-macos", "home-unix", "home-windows", "crlf-diff", "ambiguous-unquoted-path", "ambiguous-rename-path", "gitlink", "implementation-p4-missing", "implementation-p4-present", "platform-evidence-missing", "platform-evidence-present", "platform-checkbox-evidence", "platform-checkbox-comment", "migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"} or scenario.startswith("home-") or security_case or sync_case
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
    if security_case:
        emit("diff --git a/src-tauri/src/dsc.rs b/src-tauri/src/dsc.rs\n--- a/src-tauri/src/dsc.rs\n+++ b/src-tauri/src/dsc.rs\n@@ -0,0 +1 @@\n+safe check\n")
    elif sync_case:
        emit("diff --git a/src-tauri/src/sync.rs b/docs/retired.rs\nsimilarity index 100%\nrename from src-tauri/src/sync.rs\nrename to docs/retired.rs\n")
    elif scenario == "formatted-phone":
        phone = "+91 " + "98765" + "-43210"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+Call {phone}\n")
    elif scenario == "path-id":
        path_id = "ABCDE" + "1234" + "F"
        emit(f"diff --git a/docs/safe.md b/docs/{path_id}.md\n--- a/docs/safe.md\n+++ b/docs/{path_id}.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"security-notes-missing", "security-notes-present"}:
        emit("diff --git a/src-tauri/src/tally/runtime.rs b/src-tauri/src/tally/runtime.rs\n--- a/src-tauri/src/tally/runtime.rs\n+++ b/src-tauri/src/tally/runtime.rs\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"security-crate", "security-agent-import", "security-dsc"}:
        paths = {"security-crate": "src-tauri/crates/bridge-tally-core/src/master_binding.rs", "security-agent-import": "src-tauri/src/agent_import.rs", "security-dsc": "src-tauri/src/dsc.rs"}
        emit(f"diff --git a/{paths[scenario]} b/{paths[scenario]}\n--- a/{paths[scenario]}\n+++ /dev/null\n@@ -1 +0,0 @@\n-safe text\n")
    elif scenario == "security-rename-out":
        emit("diff --git a/src-tauri/src/tally/runtime.rs b/src/runtime.rs\nsimilarity index 100%\nrename from src-tauri/src/tally/runtime.rs\nrename to src/runtime.rs\n")
    elif scenario.startswith("home-"):
        homes = {"home-macos": "/" + "Users" + "/" + "tester" + "/work", "home-unix": "/" + "home" + "/" + "tester" + "/work", "home-windows": "C:" + "\\" + "Users" + "\\" + "tester" + "\\work", "home-macos-root": "/" + "Users" + "/" + "tester", "home-unix-root": "/" + "home" + "/" + "tester", "home-windows-forward": "C:" + "/" + "Users" + "/" + "tester" + "/work", "home-windows-escaped": "C:" + "\\\\" + "Users" + "\\\\" + "tester" + "\\\\work", "home-regex-source": "mac_home=" + "'/'" + '"Users"' + "'/[A-Za-z0-9._-]+'"}
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{homes[scenario]}\n")
    elif scenario == "binary-delete":
        emit("diff --git a/docs/old.png b/docs/old.png\nBinary files a/docs/old.png and /dev/null differ\n")
    elif scenario == "diff-omits-file":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "diff-truncated-payload":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +2 @@\n+first line\n")
    elif scenario == "formatted-phone-grouped":
        phone = "6" + "98 765-4321"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {phone}\n")
    elif scenario == "repeated-phone":
        phone = "6" + "6" * 9
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {phone}\n")
    elif scenario in {"grouped-identifier-12", "grouped-identifier-16"}:
        identifier = "8421 " + "7654 9012" if scenario.endswith("12") else "8421-" + "7654-9012-3456"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {identifier}\n")
    elif scenario in {"workflow-notes-missing", "workflow-notes-present"}:
        emit("diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -0,0 +1 @@\n+safe workflow text\n")
    elif scenario in {"workflow-delete", "workflow-delete-notes"}:
        emit("diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ /dev/null\n@@ -1 +0,0 @@\n-safe workflow text\n")
    elif scenario in {"workflow-rename-out", "workflow-rename-out-notes"}:
        emit("diff --git a/.github/workflows/ci.yml b/docs/retired-ci.yml\nsimilarity index 100%\nrename from .github/workflows/ci.yml\nrename to docs/retired-ci.yml\n")
    elif scenario in {"phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized"}:
        phones = {
            "phone-space": "6" + "9876 54321",
            "phone-dot": "6" + "987.654.321",
            "phone-plus": "6" + "9876+54321",
            "phone-underscore": "6" + "987_654_321",
            "phone-parenthesized": "(" + "69876" + ") 54321",
        }
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {phones[scenario]}\n")
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
    elif scenario == "crlf-diff":
        emit("diff --git a/docs/example.md b/docs/example.md\r\n--- a/docs/example.md\r\n+++ b/docs/example.md\r\n@@ -0,0 +1 @@\r\n+safe text\r\n")
    elif scenario == "ambiguous-unquoted-path":
        emit("diff --git a/docs/a b/example.md b/docs/a b/example.md\n--- a/docs/a b/example.md\n+++ b/docs/a b/example.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "ambiguous-rename-path":
        emit("diff --git a/docs/a b/example.md b/docs/a b/example.md\nsimilarity index 100%\nrename from docs/a b/example.md\nrename to docs/a b/example.md\n")
    elif scenario == "gitlink":
        emit("diff --git a/vendor/module b/vendor/module\nnew file mode 160000\nindex 0000000..2222222\n--- /dev/null\n+++ b/vendor/module\n@@ -0,0 +1 @@\n+Subproject commit 2222222\n")
    elif scenario in {"implementation-p4-missing", "implementation-p4-present"}:
        emit("diff --git a/scripts/example.py b/scripts/example.py\n--- a/scripts/example.py\n+++ b/scripts/example.py\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"platform-evidence-missing", "platform-evidence-present", "platform-checkbox-evidence", "platform-checkbox-comment"}:
        emit("diff --git a/src-tauri/src/local_files/paths.rs b/src-tauri/src/local_files/paths.rs\n--- a/src-tauri/src/local_files/paths.rs\n+++ b/src-tauri/src/local_files/paths.rs\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"}:
        emit("diff --git a/src-tauri/migrations/001.sql b/src-tauri/migrations/001.sql\n--- a/src-tauri/migrations/001.sql\n+++ b/src-tauri/migrations/001.sql\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "separated-dates":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+2026-09-12 2026-09-13\n")
    elif scenario == "separated-dates-new-year":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+0101-2026 0201-2026\n")
    elif scenario == "separated-dates-year-month":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+2025-09-11 2025-09-12\n")
    elif scenario == "adr-identifier":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+CE_ADR_0016_E\n")
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
        second_id = 1 if scenario == "check-run-duplicate-id" else 2
        run_status = "queued" if scenario == "check-run-incomplete" else "completed"
        conclusion = "failure" if scenario == "check-run-failed" else ("skipped" if scenario in {"check-run-required-skip", "check-run-optional-skip"} else "success")
        run_name = "Optional changed after rollup" if scenario in {"check-run-failed", "check-run-optional-skip"} else "Required checks"
        emit([{"total_count": total_count, "check_runs": [
            {"id": 1, "name": run_name, "head_sha": run_head, "status": run_status, "conclusion": conclusion},
            {"id": second_id, "name": "Rust format", "head_sha": run_head, "status": "completed", "conclusion": "success"}]}])
    elif "/commits/" in joined and "/status?" in joined:
        def status_page(rows, total_count, state="success", page_head=head):
            return {"sha": page_head, "state": state, "total_count": total_count, "statuses": rows}
        if scenario == "status-two-page":
            emit([status_page([{"id": 1, "context": "legacy one", "state": "success"}], 2),
                  status_page([{"id": 2, "context": "legacy two", "state": "success"}], 2)])
        elif scenario == "status-truncated":
            emit([{"sha": head, "state": "success", "total_count": 1, "statuses": []}])
        elif scenario == "status-duplicate":
            row = {"id": 1, "context": "legacy one", "state": "success"}
            emit([{"sha": head, "state": "success", "total_count": 2, "statuses": [row]},
                  {"sha": head, "state": "success", "total_count": 2, "statuses": [row]}])
        elif scenario == "status-mixed-head":
            emit([{"sha": head, "state": "success", "total_count": 2, "statuses": [{"id": 1, "context": "legacy one", "state": "success"}]},
                  {"sha": new_head, "state": "success", "total_count": 2, "statuses": [{"id": 2, "context": "legacy two", "state": "success"}]}])
        elif scenario == "status-failed-context":
            emit([{"sha": head, "state": "failure", "total_count": 1, "statuses": [{"id": 1, "context": "legacy optional", "state": "failure"}]}])
        elif scenario == "status-nonempty-pending":
            emit([{"sha": head, "state": "pending", "total_count": 1, "statuses": [{"id": 1, "context": "queued", "state": "pending"}]}])
        elif scenario == "status-empty-pending":
            emit([{"sha": head, "state": "pending", "total_count": 0, "statuses": []}])
        elif scenario == "status-malformed":
            emit([{"sha": head, "state": "success", "total_count": "0", "statuses": []}])
        elif scenario == "status-failed-combined":
            emit([{"sha": head, "state": "failure", "total_count": 0, "statuses": []}])
        else:
            emit([{"sha": head, "state": "success", "total_count": 0, "statuses": []}])
    elif "/pulls/321/commits" in joined:
        message = "safe commit metadata"
        if scenario == "metadata-commit-id":
            message = "Customer " + "ABCDE" + "1234" + "F"
        identity = {"name": "Maintainer", "email": "maintainer@example.invalid"}
        if scenario == "metadata-author-id":
            identity = {"name": "ABCDE" + "1234" + "F", "email": "maintainer@example.invalid"}
        linked_author = None if scenario == "metadata-unlinked-identities" else {"login": "author"}
        linked_committer = None if scenario == "metadata-unlinked-identities" else {"login": "committer"}
        commits = [{"sha": head, "commit": {"message": message, "author": identity, "committer": identity},
                    "author": linked_author, "committer": linked_committer}]
        if scenario == "metadata-duplicate":
            commits.append({"sha": head, "commit": {"message": message}})
        emit([commits])
    elif any(arg.endswith("/pulls/321") for arg in args):
        commits = 1
        metadata_head = head
        if scenario == "metadata-capped":
            commits = 251
        elif scenario in {"metadata-truncated", "metadata-duplicate"}:
            commits = 2
        elif scenario == "metadata-head-mismatch":
            metadata_head = new_head
        emit({"commits": commits, "head": {"sha": metadata_head}, "user": {"login": "author"}})
    elif "branches/master/protection/required_status_checks" in joined:
        contexts = [
            "Frontend build", "Rust format", "GitGuardian Security Checks",
            "Dependency security", "Required checks"]
        if scenario == "protection-missing-gitguardian":
            contexts = [context for context in contexts if context != "GitGuardian Security Checks"]
        if scenario == "required-context-output-bound":
            contexts += ["control-" + str(i) + "-" + "z" * 10000 for i in range(50)]
        emit({"strict": scenario != "protection-nonstrict", "contexts": contexts, "checks": []})
    elif "branches/master" in joined:
        emit(base)
    elif "/pulls/321/reviews" in joined:
        if scenario in {"short-review", "summary-only", "manual-summary"}:
            emit([[]])
        else:
            records = [{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "state": "COMMENTED", "commit_id": head}]
            if security_case and scenario != "security-review-missing":
                record = {"user": {"login": "reviewer", "type": "User"}, "author_association": "COLLABORATOR", "state": "COMMENTED", "commit_id": head, "body": f"Security review: {head}\nResult: accepted\nReviewed credential handling and error redaction."}
                if scenario == "security-review-stale": record["commit_id"] = new_head
                if scenario == "security-review-author": record["user"]["login"] = "author"
                if scenario == "security-review-unrelated": record["body"] = "Looks good"
                if scenario == "security-review-outsider": record["author_association"] = "NONE"
                if scenario == "security-review-dismissed": record["state"] = "DISMISSED"
                records.append(record)
            emit([records])
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
        if security_case:
            emit([[{"filename": "src-tauri/src/dsc.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif sync_case:
            emit([[{"filename": "docs/retired.rs", "previous_filename": "src-tauri/src/sync.rs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "files-empty":
            emit([[]])
        elif scenario in {"formatted-phone", "formatted-phone-grouped", "grouped-identifier-12", "grouped-identifier-16", "phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized"}:
            emit([[{"filename": "docs/contact.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario in {"workflow-notes-missing", "workflow-notes-present"}:
            emit([[{"filename": ".github/workflows/ci.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"workflow-delete", "workflow-delete-notes"}:
            emit([[{"filename": ".github/workflows/ci.yml", "status": "removed", "additions": 0, "deletions": 1}]])
        elif scenario in {"workflow-rename-out", "workflow-rename-out-notes"}:
            emit([[{"filename": "docs/retired-ci.yml", "previous_filename": ".github/workflows/ci.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "renamed-previous-missing":
            emit([[{"filename": "docs/retired-ci.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "renamed-previous-null":
            emit([[{"filename": "docs/retired-ci.yml", "previous_filename": None, "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "renamed-previous-false":
            emit([[{"filename": "docs/retired-ci.yml", "previous_filename": False, "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "path-id":
            path_id = "ABCDE" + "1234" + "F"
            emit([[{"filename": f"docs/{path_id}.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario in {"security-notes-missing", "security-notes-present"}:
            emit([[{"filename": "src-tauri/src/tally/runtime.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-rename-out":
            emit([[{"filename": "src/runtime.rs", "previous_filename": "src-tauri/src/tally/runtime.rs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"security-crate", "security-agent-import", "security-dsc"}:
            paths = {"security-crate": "src-tauri/crates/bridge-tally-core/src/master_binding.rs", "security-agent-import": "src-tauri/src/agent_import.rs", "security-dsc": "src-tauri/src/dsc.rs"}
            emit([[{"filename": paths[scenario], "status": "removed", "additions": 0, "deletions": 1}]])
        elif scenario.startswith("home-"):
            emit([[{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}]])
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
        elif scenario in {"all-a-pan", "masked-pan", "separated-dates", "separated-dates-new-year", "separated-dates-year-month", "adr-identifier"}:
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "quoted-path":
            emit([[{"filename": "docs/café.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario in {"crlf-diff", "ambiguous-unquoted-path", "ambiguous-rename-path"}:
            status = "renamed" if scenario == "ambiguous-rename-path" else "added"
            record = {"filename": "docs/a b/example.md" if scenario != "crlf-diff" else "docs/example.md", "status": status, "additions": 0 if status == "renamed" else 1, "deletions": 0}
            if status == "renamed": record["previous_filename"] = "docs/a b/example.md"
            emit([[record]])
        elif scenario == "gitlink":
            emit([[{"filename": "vendor/module", "status": "modified", "additions": 1, "deletions": 1}]])
        elif scenario in {"implementation-p4-missing", "implementation-p4-present"}:
            emit([[{"filename": "scripts/example.py", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"platform-evidence-missing", "platform-evidence-present", "platform-checkbox-evidence", "platform-checkbox-comment"}:
            emit([[{"filename": "src-tauri/src/local_files/paths.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"}:
            emit([[{"filename": "src-tauri/migrations/001.sql", "status": "modified", "additions": 1, "deletions": 0}]])
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
            checklist = "# Review checklist\n" + "\n" * 8 + "- [ ] Errors are actionable without exposing sensitive values.\n"
            emit({"encoding": "base64", "content": base64.b64encode(checklist.encode()).decode()})
        elif scenario in {"surface-fail", "draft-surface-fail"}:
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

    def test_incomplete_check_run_is_indeterminate(self):
        self.assert_indeterminate("check-run-incomplete", "head-bound check-run evidence")

    def test_empty_pending_legacy_status_does_not_block(self):
        result = self.run_gate("status-empty-pending")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

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

    def test_repeated_digit_phone_is_scanned(self):
        self.assert_blocked("repeated-phone", "privacy scan found")

    def test_grouped_formatted_phone_is_scanned(self):
        self.assert_blocked("formatted-phone-grouped", "privacy scan found")

    def test_separated_indian_mobile_styles_are_scanned(self):
        for scenario in ("phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")

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

    def test_checklist_permalink_must_bind_the_full_current_head(self):
        self.assert_blocked("checklist-stale-ref", "same-repository line-specific")

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
        self.assert_indeterminate("gitlink", "gitlink change(s) require explicit provenance, license, and NOTICE review")

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
        for scenario in ("grouped-identifier-12", "grouped-identifier-16"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "privacy scan found")

    def test_workflow_change_requires_rollback_and_migration_notes(self):
        self.assert_blocked("workflow-notes-missing", "workflow change lacks non-empty rollback notes")

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

    def test_sensitive_paths_without_security_attestation_are_indeterminate(self):
        self.assert_indeterminate("security-review-missing", "security-focused reviewer comment")

    def test_refreshed_failed_or_required_skipped_runs_block(self):
        for scenario in ("check-run-failed", "check-run-required-skip"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "refreshed check run(s)")
        result = self.run_gate("check-run-optional-skip")
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
        for scenario in ("validation-command-outside", "validation-tool-prose", "validation-placeholder-command"):
            with self.subTest(scenario=scenario):
                self.assert_blocked(scenario, "actual test or reproduction command")

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
        for scenario in ("security-review-stale", "security-review-author", "security-review-unrelated", "security-review-outsider", "security-review-dismissed"):
            with self.subTest(scenario=scenario):
                self.assert_indeterminate(scenario, "security-focused reviewer comment")
        result = self.run_gate("security-review-valid")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_sync_rename_out_requires_migration_notes(self):
        self.assert_blocked("sync-migration-missing", "migration compatibility")
        result = self.run_gate("sync-migration-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_implementation_additions_need_all_three_p4_answers(self):
        self.assert_blocked("implementation-p4-missing", "all three substantive P4")
        result = self.run_gate("implementation-p4-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_platform_sensitive_paths_need_substantive_both_host_evidence(self):
        self.assert_blocked("platform-evidence-missing", "substantive Windows validation")
        result = self.run_gate("platform-evidence-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.run_gate("platform-checkbox-evidence")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_blocked("platform-checkbox-comment", "substantive Windows validation")

    def test_database_migration_paths_need_rollback_notes(self):
        self.assert_blocked("migration-rollback-missing", "database migration path lacks non-empty rollback notes")
        result = self.run_gate("migration-rollback-present")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_wrapped_canonical_migration_template_field_is_recognized(self):
        result = self.run_gate("migration-template-wrapped")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_blocked("migration-template-other-field", "database migration path lacks non-empty rollback notes")

    def test_developer_home_path_shapes_are_scanned_without_echoing_values(self):
        for scenario in ("home-macos", "home-unix", "home-windows", "home-macos-root", "home-unix-root", "home-windows-forward", "home-windows-escaped"):
            with self.subTest(scenario=scenario):
                result = self.run_gate(scenario)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("developer-home path shape", result.stdout)
                self.assertNotIn("tester", result.stdout)
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
