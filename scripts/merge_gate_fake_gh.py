#!/usr/bin/env python3
"""Offline GitHub CLI fixture for ``merge-gate.test.py``."""

import base64, json, os, sys
args = sys.argv[1:]
scenario = os.environ.get("GATE_SCENARIO", "pass")
security_case = scenario.startswith("security-review-") or scenario in {"security-camel-dsc", "security-camel-credential", "security-axal-frontend", "security-axal-native", "security-encrypted-keystore", "security-documents-consumer", "security-documents-consumer-rename-out", "security-documents-screen", "security-documents-screen-rename-out", "security-documents-screen-valid", "security-tauri-cargo", "security-tauri-cargo-rename-out", "security-tauri-lib", "security-tauri-lib-rename-out", "security-commands-facade", "security-bank-statement-import", "security-prune-package-compiler-cache", "security-prune-package-compiler-cache-rename-out", "security-ci-workflow", "security-ci-workflow-rename-out", "security-ci-workflow-valid", "security-release-preview", "security-release-preview-rename-out", "security-deploy-install-page", "security-deploy-install-page-rename-out", "security-deploy-install-page-valid", "workflow-notes-present", "workflow-delete-notes", "workflow-rename-out-notes"}
security_workflow_case = scenario in {"security-ci-workflow", "security-ci-workflow-rename-out", "security-ci-workflow-valid", "security-release-preview", "security-release-preview-rename-out", "security-deploy-install-page", "security-deploy-install-page-rename-out", "security-deploy-install-page-valid"}
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

def next_counter(name):
    path = os.environ.get(name)
    if not path:
        return 0
    try:
        count = int(open(path).read())
    except (FileNotFoundError, ValueError):
        count = 0
    with open(path, "w") as counter:
        counter.write(str(count + 1))
    return count

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
    selected_head = new_head if scenario in {"head-moves", "binary-review-head-moves"} and view_count > 0 else head
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
    elif scenario in {"workflow-notes-present", "workflow-delete-notes", "workflow-rename-out-notes", "platform-ci-workflow", "platform-ci-workflow-rename-out", "platform-release-mcpb-preview"}:
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "## Rollback notes\n\nRevert the workflow commit.\n\n"
            "## Migration compatibility\n\nNo persisted data changes.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    if scenario in {"workflow-notes-present", "workflow-delete-notes", "workflow-rename-out-notes"}:
        body += (
            "\n- Windows validation evidence: Windows CI passed `python3 scripts/merge-gate.test.py`.\n"
            "- macOS validation evidence: macOS CI passed `python3 scripts/merge-gate.test.py`.\n"
        )
    elif scenario == "workflow-sibling-migration":
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            "## Rollback notes\n\n- Migration compatibility: No persisted data changes.\n\n"
            "- [x] [Errors](https://github.com/lamemustafa/bridge/blob/HEAD/review-checklist.md#L10)"
        )
    elif scenario in {"security-notes-present", "security-none", "security-pending", "surface-unpins"}:
        security_impact = "None" if scenario == "security-none" else ("pending" if scenario == "security-pending" else "No credential material is added; the Tally path change is reviewed.")
        body = (
            "## Outcome and reason\n\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n\n"
            "## Validation and evidence\n\n`python3 scripts/merge-gate.test.py`\n\n"
            f"## Security impact\n\n{security_impact}\n\n"
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
    if scenario == "validation-node-command":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "`node --test scripts/prune-package-compiler-cache.test.mjs`")
    if scenario == "validation-tilde-command":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "~~~bash\npython3 scripts/merge-gate.test.py\n~~~")
    structural_summaries = {
        "empty-fenced-summary": "```\n```",
        "empty-tilde-summary": "~~~\n~~~",
        "empty-typed-tilde-summary": "~~~text example\n~~~",
        "empty-rule-summary": "---\n* * *\n___",
        "inline-fence-summary": "~~~text",
    }
    if scenario in structural_summaries:
        body = body.replace("A bounded merge preflight keeps incomplete evidence from becoming a merge.", structural_summaries[scenario])
        if scenario == "inline-fence-summary":
            body = body.replace("## Functional summary\n\n", "## Functional summary: ")
    if scenario in {"nonempty-fenced-summary", "nonempty-tilde-summary"}:
        fence = "~~~text" if scenario == "nonempty-tilde-summary" else "```"
        body = body.replace("A bounded merge preflight keeps incomplete evidence from becoming a merge.", fence + "\nA bounded merge preflight keeps incomplete evidence from becoming a merge.\n" + fence[:3])
    if scenario == "policy-multiline-comment":
        body = body.replace("A bounded merge preflight keeps incomplete evidence from becoming a merge.", "<!--\nA hidden summary cannot establish the change.\n-->")
    if scenario == "validation-html-comment":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "Tests not run <!-- `cargo test` -->")
    if scenario == "unterminated-html-comment":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "<!-- hidden through EOF\n`cargo test`")
    if scenario in {"hidden-comment-identifier", "binary-review-private"}:
        body += "\n<!-- " + "ABCDE" + "1234" + "F -->"
    if scenario == "body-comment-drift" and view_count > 0:
        body += "\n<!-- changed metadata -->"
    if scenario == "validation-tool-prose":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "Tests not run; cargo is available")
    if scenario == "validation-placeholder-command":
        body = body.replace("`python3 scripts/merge-gate.test.py`", "`cargo ...`")
    if scenario == "dependency-manifest-present":
        body += "\n## Dependency justification\n\nThe parser library is required for the sealed response format.\n"
    if scenario == "checklist-heading":
        body = body.replace("#L10", "#L1")
    if scenario == "checklist-anchor-suffix":
        body = body.replace("#L10", "#L10junk")
    if scenario in {"implementation-p4-present", "implementation-p4-continuation", "platform-evidence-present", "platform-evidence-heading", "platform-powershell-missing", "platform-powershell-evidence", "platform-checkbox-evidence", "platform-checkbox-comment", "platform-inline-prose", "platform-negative-outcome", "platform-unaffected-bare", "platform-unaffected-rationale", "platform-evidence-sibling-list", "platform-evidence-empty-fence", "platform-evidence-punctuated-placeholder", "platform-evidence-fenced-continuation", "migration-rollback-present", "migration-template-wrapped", "security-notes-present", "security-none", "security-pending", "security-review-valid"}:
        body += (
            "\n## Scope, reuse, and impact\n\n"
            "- Existing component reused: the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified): no duplicate path remains.\n"
            "- What breaks if this is not built: unsafe evidence could reach a merge.\n"
        )
    if scenario == "implementation-p4-continuation":
        body = body.replace(
            "- Existing component reused: the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified): no duplicate path remains.\n"
            "- What breaks if this is not built: unsafe evidence could reach a merge.\n",
            "- Existing component reused:\n  the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified):\n  no duplicate path remains.\n"
            "- What breaks if this is not built:\n  unsafe evidence could reach a merge.\n",
        )
    if security_case or scenario in {"surface-unpins", "migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"}:
        body += (
            "\n## Scope, reuse, and impact\n\n"
            "- Existing component reused: the existing gate parser and file inventory.\n"
            "- What is deleted (or why no deletion is justified): no duplicate path remains.\n"
            "- What breaks if this is not built: unsafe evidence could reach a merge.\n"
            "\n## Security impact\n\nNo credential material is added.\n"
            "\n## Migration compatibility\n\nExisting callers retain their paths and formats.\n"
            "\n- Windows validation evidence: Windows CI passed `python3 scripts/merge-gate.test.py`.\n"
            "- macOS validation evidence: macOS CI passed `python3 scripts/merge-gate.test.py`.\n"
        )
    if security_workflow_case:
        body += "\n## Rollback notes\n\nRevert the workflow change before the next release.\n"
    if scenario in {"security-tauri-cargo", "security-tauri-cargo-rename-out"}:
        body += "\n## Dependency rationale\n\nThe manifest boundary remains under independent credential-focused review.\n"
    if scenario == "security-encrypted-keystore":
        body += "\n## Rollback notes\n\nRevert the encrypted-store change before deployment.\n"
    if scenario in {"platform-evidence-present", "platform-powershell-evidence", "migration-template-wrapped", "security-notes-present", "security-none", "security-pending", "security-review-valid", "sync-migration-present"}:
        body += (
            "\n- Windows validation evidence: Windows CI passed `python3 scripts/merge-gate.test.py`.\n"
            "- macOS validation evidence: macOS CI passed `python3 scripts/merge-gate.test.py`.\n"
        )
    if scenario == "platform-evidence-heading":
        body += (
            "\n### Windows validation evidence\n\n"
            "`python3 scripts/merge-gate.test.py` passed on Windows CI.\n"
            "\n### macOS validation evidence\n\n"
            "`python3 scripts/merge-gate.test.py` passed on macOS CI.\n"
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
    if scenario == "platform-inline-prose":
        body += (
            "\n- Windows validation evidence: reviewed by the release team.\n"
            "- macOS validation evidence: reviewed by the release team.\n"
        )
    if scenario == "platform-negative-outcome":
        body += "\n- Windows validation evidence: Windows CI did not pass.\n- macOS validation evidence: macOS CI did not pass.\n"
    if scenario == "platform-unaffected-bare":
        body += "\n- Windows validation evidence: unaffected\n- macOS validation evidence: unaffected\n"
    if scenario == "platform-unaffected-rationale":
        body += "\n- Windows validation evidence: unaffected because this changes shared documentation only.\n- macOS validation evidence: unaffected because this changes shared documentation only.\n"
    if scenario in {"platform-evidence-present", "platform-evidence-heading", "platform-powershell-missing", "platform-powershell-evidence", "platform-checkbox-evidence", "platform-unaffected-bare", "platform-unaffected-rationale", "platform-evidence-sibling-list", "platform-evidence-empty-fence", "platform-evidence-punctuated-placeholder", "platform-evidence-fenced-continuation", "platform-evidence-package-manager"}:
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
    if scenario == "p4-placeholders":
        body += (
            "\n## Scope, reuse, and impact\n\n"
            "- Existing component reused: N/A\n"
            "- What is deleted (or why no deletion is justified): TODO\n"
            "- What breaks if this is not built: pending\n"
        )
    if scenario == "workflow-placeholders":
        body += "\n## Rollback notes\n\nN/A\n\n## Migration compatibility\n\nTBD\n"
    if scenario == "workflow-punctuated-placeholders":
        body += "\n## Rollback notes\n\nN/A.\n\n## Migration compatibility\n\nTBD.\n"
    if scenario == "platform-evidence-bare-label":
        body += "\n- Windows validation evidence\n- macOS validation evidence\n"
    if scenario == "platform-evidence-sibling-list":
        body += "\n- Windows validation evidence\n- Notes: tracked separately\n- macOS validation evidence\n- Notes: tracked separately\n"
    if scenario == "platform-evidence-package-manager":
        body += "\n- Windows validation evidence\n- Package manager: pnpm\n- macOS validation evidence\n- Package manager: pnpm\n"
    if scenario == "platform-evidence-empty-fence":
        body += "\n### Windows validation evidence\n~~~bash\n~~~\n### macOS validation evidence\n~~~bash\n~~~\n"
    if scenario == "platform-evidence-punctuated-placeholder":
        body += "\n- Windows validation evidence: N/A.\n- macOS validation evidence: TBD.\n"
    if scenario == "platform-evidence-fenced-continuation":
        body += "\n### Windows validation evidence\n~~~text\nWindows CI passed `python3 scripts/merge-gate.test.py`.\n~~~\n### macOS validation evidence\n~~~text\nmacOS CI passed `python3 scripts/merge-gate.test.py`.\n~~~\n"
    body = body.replace("blob/HEAD", f"blob/{head}")
    if scenario == "checklist-stale-ref":
        body = body.replace(f"blob/{head}", "blob/" + "f" * 40)
    one_file = scenario in {"metadata-private", "files-empty", "formatted-phone", "formatted-phone-grouped", "unicode-phone", "unicode-phone-tab", "unicode-phone-two-lines", "repeated-phone", "grouped-identifier-12", "grouped-identifier-16", "grouped-identifier-mixed", "phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized", "landline-grouped", "landline-standard-hyphen", "landline-standard-space", "landline-standard-underscore", "pem-certificate-envelope", "path-id", "binary-delete", "binary-review", "binary-review-private", "binary-review-head-moves", "metadata-only", "metadata-incomplete", "hunk-header-phone", "hunk-header-literals", "hunk-binary-literal", "separated-dates", "separated-dates-new-year", "separated-dates-year-month", "adr-identifier", "all-a-pan", "masked-pan", "grouped-pan-space", "grouped-pan-hyphen", "grouped-masked-pan-space", "quoted-path", "control-path", "workflow-notes-missing", "workflow-notes-present", "workflow-sibling-migration", "workflow-delete", "workflow-delete-notes", "workflow-rename-out", "workflow-rename-out-notes", "workflow-placeholders", "workflow-punctuated-placeholders", "renamed-previous-missing", "renamed-previous-null", "renamed-previous-false", "security-notes-missing", "security-notes-present", "security-none", "security-pending", "security-rename-out", "security-crate", "security-agent-import", "security-dsc", "dependency-manifest-missing", "dependency-manifest-present", "home-macos", "home-unix", "home-windows", "crlf-diff", "ambiguous-unquoted-path", "ambiguous-rename-path", "gitlink", "gitlink-existing", "implementation-p4-missing", "implementation-p4-present", "implementation-p4-continuation", "implementation-p4-shell", "implementation-p4-powershell", "implementation-p4-sql", "p4-placeholders", "platform-evidence-missing", "platform-evidence-present", "platform-evidence-heading", "platform-powershell-missing", "platform-powershell-evidence", "platform-checkbox-evidence", "platform-checkbox-comment", "platform-inline-prose", "platform-negative-outcome", "platform-unaffected-bare", "platform-unaffected-rationale", "platform-evidence-bare-label", "platform-evidence-sibling-list", "platform-evidence-empty-fence", "platform-evidence-punctuated-placeholder", "platform-evidence-fenced-continuation", "platform-evidence-package-manager", "platform-windows-native-action", "platform-windows-native-action-rename-out", "platform-ci-workflow", "platform-ci-workflow-rename-out", "platform-release-mcpb-preview", "migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"} or scenario.startswith("home-") or security_case or sync_case
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
    if scenario == "security-documents-consumer-rename-out":
        emit("diff --git a/src-tauri/src/documents.rs b/docs/retired-documents.rs\nsimilarity index 100%\nrename from src-tauri/src/documents.rs\nrename to docs/retired-documents.rs\n")
    elif scenario == "security-prune-package-compiler-cache-rename-out":
        emit("diff --git a/scripts/prune-package-compiler-cache.mjs b/docs/retired-cache-pruner.mjs\nsimilarity index 100%\nrename from scripts/prune-package-compiler-cache.mjs\nrename to docs/retired-cache-pruner.mjs\n")
    elif scenario == "security-ci-workflow-rename-out":
        emit("diff --git a/.github/workflows/ci.yml b/docs/retired-ci.yml\nsimilarity index 100%\nrename from .github/workflows/ci.yml\nrename to docs/retired-ci.yml\n")
    elif scenario == "security-release-preview-rename-out":
        emit("diff --git a/.github/workflows/release-mcpb-preview.yml b/docs/retired-preview.yml\nsimilarity index 100%\nrename from .github/workflows/release-mcpb-preview.yml\nrename to docs/retired-preview.yml\n")
    elif scenario == "security-deploy-install-page-rename-out":
        emit("diff --git a/.github/workflows/deploy-install-page.yml b/docs/retired-install-page.yml\nsimilarity index 100%\nrename from .github/workflows/deploy-install-page.yml\nrename to docs/retired-install-page.yml\n")
    elif scenario == "security-documents-screen-rename-out":
        emit("diff --git a/src/DocumentsScreen.tsx b/docs/retired-documents-screen.tsx\nsimilarity index 100%\nrename from src/DocumentsScreen.tsx\nrename to docs/retired-documents-screen.tsx\n")
    elif scenario == "security-tauri-cargo-rename-out":
        emit("diff --git a/src-tauri/Cargo.toml b/docs/retired-tauri-cargo.toml\nsimilarity index 100%\nrename from src-tauri/Cargo.toml\nrename to docs/retired-tauri-cargo.toml\n")
    elif scenario == "security-tauri-lib-rename-out":
        emit("diff --git a/src-tauri/src/lib.rs b/docs/retired-tauri-lib.rs\nsimilarity index 100%\nrename from src-tauri/src/lib.rs\nrename to docs/retired-tauri-lib.rs\n")
    elif security_case:
        paths = {"security-axal-frontend": "src/AxalScreen.tsx", "security-axal-native": "src-tauri/src/axal.rs", "security-encrypted-keystore": "src-tauri/src/db/encrypted.rs", "security-documents-consumer": "src-tauri/src/documents.rs", "security-documents-screen": "src/DocumentsScreen.tsx", "security-documents-screen-valid": "src/DocumentsScreen.tsx", "security-tauri-cargo": "src-tauri/Cargo.toml", "security-tauri-lib": "src-tauri/src/lib.rs", "security-commands-facade": "src-tauri/src/commands.rs", "security-bank-statement-import": "scripts/bank_statement_import.py", "security-prune-package-compiler-cache": "scripts/prune-package-compiler-cache.mjs", "security-ci-workflow": ".github/workflows/ci.yml", "security-ci-workflow-valid": ".github/workflows/ci.yml", "security-release-preview": ".github/workflows/release-mcpb-preview.yml", "security-deploy-install-page": ".github/workflows/deploy-install-page.yml", "security-deploy-install-page-valid": ".github/workflows/deploy-install-page.yml"}
        path = paths.get(scenario, "src-tauri/src/dsc.rs")
        emit(f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1 @@\n+safe check\n")
    elif sync_case:
        emit("diff --git a/src-tauri/src/sync.rs b/docs/retired.rs\nsimilarity index 100%\nrename from src-tauri/src/sync.rs\nrename to docs/retired.rs\n")
    elif scenario == "formatted-phone":
        phone = "+91 " + "98765" + "-43210"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+Call {phone}\n")
    elif scenario == "path-id":
        path_id = "ABCDE" + "1234" + "F"
        emit(f"diff --git a/docs/safe.md b/docs/{path_id}.md\n--- a/docs/safe.md\n+++ b/docs/{path_id}.md\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"security-notes-missing", "security-notes-present", "security-none", "security-pending"}:
        emit("diff --git a/src-tauri/src/tally/runtime.rs b/src-tauri/src/tally/runtime.rs\n--- a/src-tauri/src/tally/runtime.rs\n+++ b/src-tauri/src/tally/runtime.rs\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"security-crate", "security-agent-import", "security-dsc"}:
        paths = {"security-crate": "src-tauri/crates/bridge-tally-core/src/master_binding.rs", "security-agent-import": "src-tauri/src/agent_import.rs", "security-dsc": "src-tauri/src/dsc.rs"}
        emit(f"diff --git a/{paths[scenario]} b/{paths[scenario]}\n--- a/{paths[scenario]}\n+++ /dev/null\n@@ -1 +0,0 @@\n-safe text\n")
    elif scenario == "security-rename-out":
        emit("diff --git a/src-tauri/src/tally/runtime.rs b/src/runtime.rs\nsimilarity index 100%\nrename from src-tauri/src/tally/runtime.rs\nrename to src/runtime.rs\n")
    elif scenario.startswith("home-"):
        root_home = bytes((47, 114, 111, 111, 116)).decode("ascii")
        unicode_user = chr(0x03BB) + chr(0x00E9)
        homes = {"home-macos": "/" + "Users" + "/" + "tester" + "/work", "home-unix": "/" + "home" + "/" + "tester" + "/work", "home-root": root_home + "/work/customer.pem", "home-root-home": root_home, "home-windows": "C:" + "\\" + "Users" + "\\" + "tester" + "\\work", "home-macos-root": "/" + "Users" + "/" + "tester", "home-unix-root": "/" + "home" + "/" + "tester", "home-windows-forward": "C:" + "/" + "Users" + "/" + "tester" + "/work", "home-windows-escaped": "C:" + "\\\\" + "Users" + "\\\\" + "tester" + "\\\\work", "home-macos-unicode": "/" + "Users" + "/" + unicode_user + "/work", "home-unix-unicode": "/" + "home" + "/" + unicode_user + "/work", "home-windows-unicode": "C:" + "\\" + "Users" + "\\" + unicode_user + "\\work", "home-regex-source": "mac_home=" + "'/'" + '"Users"' + "'/[A-Za-z0-9._-]+'"}
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{homes[scenario]}\n")
    elif scenario == "binary-delete":
        emit("diff --git a/docs/old.png b/docs/old.png\nBinary files a/docs/old.png and /dev/null differ\n")
    elif scenario in {"binary-review", "binary-review-private", "binary-review-head-moves"}:
        emit("diff --git a/docs/new.png b/docs/new.png\nBinary files /dev/null and b/docs/new.png differ\n")
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
    elif scenario in {"grouped-identifier-12", "grouped-identifier-16", "grouped-identifier-mixed"}:
        groups = ("8421", "7654", "9012", "3456")
        identifier = (" ".join(groups[:3]) if scenario.endswith("12")
                      else (" ".join(groups[:2]) + "-" + " ".join(groups[2:])
                            if scenario == "grouped-identifier-mixed" else "-".join(groups)))
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {identifier}\n")
    elif scenario == "platform-ci-workflow-rename-out":
        emit("diff --git a/.github/workflows/ci.yml b/docs/retired-ci.yml\nsimilarity index 100%\nrename from .github/workflows/ci.yml\nrename to docs/retired-ci.yml\n")
    elif scenario == "platform-release-mcpb-preview":
        emit("diff --git a/.github/workflows/release-mcpb-preview.yml b/.github/workflows/release-mcpb-preview.yml\n--- a/.github/workflows/release-mcpb-preview.yml\n+++ b/.github/workflows/release-mcpb-preview.yml\n@@ -0,0 +1 @@\n+safe workflow text\n")
    elif scenario in {"workflow-notes-missing", "workflow-notes-present", "workflow-sibling-migration", "platform-ci-workflow"}:
        emit("diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -0,0 +1 @@\n+safe workflow text\n")
    elif scenario in {"dependency-manifest-missing", "dependency-manifest-present"}:
        emit("diff --git a/package.json b/package.json\n--- a/package.json\n+++ b/package.json\n@@ -0,0 +1 @@\n+safe dependency metadata\n")
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
    elif scenario == "landline-grouped":
        landline = "0" + "11" + "-" + "2345" + "-" + "6789"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {landline}\n")
    elif scenario in {"landline-standard-hyphen", "landline-standard-space", "landline-standard-underscore"}:
        separator = {"landline-standard-hyphen": "-", "landline-standard-space": " ", "landline-standard-underscore": "_"}[scenario]
        landline = "0" + "11" + separator + "23456789"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {landline}\n")
    elif scenario == "landline-standard-four-digit":
        landline = "0" + "120" + "-" + "2345678"
        emit(f"diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic {landline}\n")
    elif scenario == "pem-certificate-envelope":
        begin = "-" * 5 + "BEGIN CERTIFICATE" + "-" * 5
        end = "-" * 5 + "END CERTIFICATE" + "-" * 5
        body = "MII" + "A" * 48
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +3 @@\n+{begin}\n+{body}\n+{end}\n")
    elif scenario == "trusted-pem-certificate-envelope":
        begin = "-" * 5 + "BEGIN TRUSTED CERTIFICATE" + "-" * 5
        end = "-" * 5 + "END TRUSTED CERTIFICATE" + "-" * 5
        body = "MII" + "B" * 48
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +3 @@\n+{begin}\n+{body}\n+{end}\n")
    elif scenario == "unicode-phone":
        emit("diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic 69876\u00a054321\n")
    elif scenario == "unicode-phone-tab":
        emit("diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +1 @@\n+synthetic 69876\t54321\n")
    elif scenario == "unicode-phone-two-lines":
        emit("diff --git a/docs/contact.md b/docs/contact.md\n--- a/docs/contact.md\n+++ b/docs/contact.md\n@@ -0,0 +2 @@\n+69876\n+54321\n")
    elif scenario == "metadata-only":
        emit("diff --git a/docs/example.md b/docs/example.md\nsimilarity index 100%\nrename from docs/example.md\nrename to docs/example.md\n")
    elif scenario == "metadata-private":
        private_path = "docs/" + "/" + "Users" + "/tester/" + "x" * 300
        emit(f"diff --git a/{private_path} b/{private_path}\nsimilarity index 100%\nrename from {private_path}\nrename to {private_path}\n")
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
    elif scenario in {"grouped-pan-space", "grouped-pan-hyphen", "grouped-masked-pan-space"}:
        separator = "-" if scenario == "grouped-pan-hyphen" else " "
        prefix = "XXXXX" if scenario == "grouped-masked-pan-space" else "ABCDE"
        suffix = "X" if scenario == "grouped-masked-pan-space" else "A"
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{prefix}{separator}1234{separator}{suffix}\n")
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
    elif scenario == "gitlink-existing":
        emit("diff --git a/vendor/module b/vendor/module\nindex 1111111..2222222 160000\n--- a/vendor/module\n+++ b/vendor/module\n@@ -1 +1 @@\n-Subproject commit 1111111\n+Subproject commit 2222222\n")
    elif scenario in {"implementation-p4-missing", "implementation-p4-present", "implementation-p4-continuation", "implementation-p4-shell", "implementation-p4-powershell", "implementation-p4-sql", "p4-placeholders"}:
        suffix = {"implementation-p4-shell": "sh", "implementation-p4-powershell": "ps1", "implementation-p4-sql": "sql"}.get(scenario, "py")
        emit(f"diff --git a/scripts/example.{suffix} b/scripts/example.{suffix}\n--- a/scripts/example.{suffix}\n+++ b/scripts/example.{suffix}\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"platform-evidence-missing", "platform-evidence-present", "platform-evidence-heading", "platform-checkbox-evidence", "platform-checkbox-comment", "platform-inline-prose", "platform-negative-outcome", "platform-unaffected-bare", "platform-unaffected-rationale", "platform-evidence-bare-label", "platform-evidence-sibling-list", "platform-evidence-empty-fence", "platform-evidence-punctuated-placeholder", "platform-evidence-fenced-continuation", "platform-evidence-package-manager"}:
        emit("diff --git a/src-tauri/src/local_files/paths.rs b/src-tauri/src/local_files/paths.rs\n--- a/src-tauri/src/local_files/paths.rs\n+++ b/src-tauri/src/local_files/paths.rs\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "platform-windows-native-action":
        emit("diff --git a/.github/actions/setup-windows-native/action.yml b/.github/actions/setup-windows-native/action.yml\n--- a/.github/actions/setup-windows-native/action.yml\n+++ b/.github/actions/setup-windows-native/action.yml\n@@ -0,0 +1 @@\n+safe action text\n")
    elif scenario == "platform-windows-native-action-rename-out":
        emit("diff --git a/.github/actions/setup-windows-native/action.yml b/docs/retired-windows-native-action.yml\nsimilarity index 100%\nrename from .github/actions/setup-windows-native/action.yml\nrename to docs/retired-windows-native-action.yml\n")
    elif scenario in {"platform-powershell-missing", "platform-powershell-evidence"}:
        emit("diff --git a/scripts/signing.ps1 b/scripts/signing.ps1\n--- a/scripts/signing.ps1\n+++ b/scripts/signing.ps1\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario in {"migration-rollback-missing", "migration-rollback-present", "migration-template-wrapped", "migration-template-other-field"}:
        emit("diff --git a/src-tauri/migrations/001.sql b/src-tauri/migrations/001.sql\n--- a/src-tauri/migrations/001.sql\n+++ b/src-tauri/migrations/001.sql\n@@ -0,0 +1 @@\n+safe text\n")
    elif scenario == "separated-dates":
        emit("diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+2026-09-12 2026-09-13\n")
    elif scenario == "separated-dates-new-year":
        first_date = "-".join(("0101", "2026"))
        second_date = "-".join(("0201", "2026"))
        emit(f"diff --git a/docs/example.md b/docs/example.md\n--- a/docs/example.md\n+++ b/docs/example.md\n@@ -0,0 +1 @@\n+{first_date} {second_date}\n")
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
        if scenario == "threads-first-empty-cursor-rejected" and "cursor=" in args:
            fail("the first review-thread request must omit cursor")
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
        check_call = next_counter("GATE_CHECK_COUNTER")
        if scenario == "check-run-wrong-head":
            run_head = new_head
        else:
            run_head = head
        total_count = 3 if scenario == "check-run-count-mismatch" else 2
        second_id = 1 if scenario == "check-run-duplicate-id" else 2
        run_status = "queued" if scenario == "check-run-incomplete" else "completed"
        conclusion = "failure" if scenario in {"check-run-failed", "late-check-run-failure"} and (scenario != "late-check-run-failure" or check_call > 0) else ("skipped" if scenario in {"check-run-required-skip", "check-run-optional-skip"} else "success")
        run_name = "Optional changed after rollup" if scenario in {"check-run-failed", "check-run-optional-skip"} else "Required checks"
        emit([{"total_count": total_count, "check_runs": [
            {"id": 1, "name": run_name, "head_sha": run_head, "status": run_status, "conclusion": conclusion},
            {"id": second_id, "name": "Rust format", "head_sha": run_head, "status": "completed", "conclusion": "success"}]}])
    elif "/commits/" in joined and "/status?" in joined:
        status_call = next_counter("GATE_STATUS_COUNTER")
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
        elif scenario == "status-failed-context" or (scenario == "late-status-failure" and status_call > 0):
            emit([{"sha": head, "state": "failure", "total_count": 1, "statuses": [{"id": 1, "context": "legacy optional", "state": "failure"}]}])
        elif scenario == "status-nonempty-pending":
            emit([{"sha": head, "state": "pending", "total_count": 1, "statuses": [{"id": 1, "context": "queued", "state": "pending"}]}])
        elif scenario == "status-empty-pending":
            emit([{"sha": head, "state": "pending", "total_count": 0, "statuses": []}])
        elif scenario == "status-malformed":
            emit([{"sha": head, "state": "success", "total_count": "0", "statuses": []}])
        elif scenario == "status-failed-combined":
            emit([{"sha": head, "state": "failure", "total_count": 1, "statuses": [{"id": 1, "context": "legacy failed", "state": "failure"}]}])
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
        if scenario == "required-context-unexpected":
            contexts += ["Unexpected required context"]
        emit({"strict": scenario != "protection-nonstrict", "contexts": contexts, "checks": []})
    elif "branches/master" in joined:
        base_tip_call = next_counter("GATE_BASE_TIP_COUNTER")
        if scenario == "final-base-tip-failure" and base_tip_call >= 2:
            fail("controlled final base-tip read failure")
        emit(new_head if scenario == "final-base-tip-moves" and base_tip_call >= 2 else base)
    elif "/pulls/321/reviews" in joined:
        if scenario in {"short-review", "summary-only", "manual-summary"}:
            emit([[]])
        else:
            records = [{"user": {"login": "chatgpt-codex-connector[bot]", "type": "Bot"}, "state": "COMMENTED", "commit_id": head}]
            if security_case and scenario not in {"security-review-missing", "security-camel-dsc", "security-camel-credential", "security-axal-frontend", "security-axal-native", "security-encrypted-keystore", "security-documents-consumer", "security-documents-consumer-rename-out", "security-documents-screen", "security-documents-screen-rename-out", "security-tauri-cargo", "security-tauri-cargo-rename-out", "security-tauri-lib", "security-tauri-lib-rename-out", "security-commands-facade", "security-bank-statement-import", "security-prune-package-compiler-cache", "security-prune-package-compiler-cache-rename-out", "security-ci-workflow", "security-ci-workflow-rename-out", "security-release-preview", "security-release-preview-rename-out", "security-deploy-install-page", "security-deploy-install-page-rename-out"}:
                record = {"user": {"login": "reviewer", "type": "User"}, "author_association": "COLLABORATOR", "state": "COMMENTED", "commit_id": head, "body": f"Security review: {head}\nResult: accepted\nReviewed credential handling: token diagnostics remain redacted.\nSecurity rationale: the current access boundary prevents a cache token from reaching logs."}
                if scenario == "security-review-stale": record["commit_id"] = new_head
                if scenario == "security-review-author": record["user"]["login"] = "author"
                if scenario == "security-review-unrelated": record["body"] = "Looks good"
                if scenario == "security-review-no-scope": record["body"] = f"Security review: {head}\nResult: accepted"
                if scenario == "security-review-bare-scope": record["body"] = f"Security review: {head}\nResult: accepted\nReviewed credential:\nSecurity rationale: the current access boundary prevents a cache token from reaching logs."
                if scenario == "security-review-placeholder-rationale": record["body"] = f"Security review: {head}\nResult: accepted\nReviewed credential handling: token diagnostics remain redacted.\nSecurity rationale: TBD"
                if scenario == "security-review-hidden": record["body"] = f"<!--\nSecurity review: {head}\nResult: accepted\nReviewed credential handling and error redaction.\n-->"
                if scenario == "security-review-hidden-unterminated": record["body"] = f"<!--\nSecurity review: {head}\nResult: accepted\nReviewed credential handling and error redaction."
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
        if scenario == "security-encrypted-keystore":
            emit([[{"filename": "src-tauri/src/db/encrypted.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-documents-consumer":
            emit([[{"filename": "src-tauri/src/documents.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-commands-facade":
            emit([[{"filename": "src-tauri/src/commands.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-bank-statement-import":
            emit([[{"filename": "scripts/bank_statement_import.py", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-prune-package-compiler-cache":
            emit([[{"filename": "scripts/prune-package-compiler-cache.mjs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-prune-package-compiler-cache-rename-out":
            emit([[{"filename": "docs/retired-cache-pruner.mjs", "previous_filename": "scripts/prune-package-compiler-cache.mjs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"security-ci-workflow", "security-ci-workflow-valid"}:
            emit([[{"filename": ".github/workflows/ci.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-ci-workflow-rename-out":
            emit([[{"filename": "docs/retired-ci.yml", "previous_filename": ".github/workflows/ci.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "security-release-preview":
            emit([[{"filename": ".github/workflows/release-mcpb-preview.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-release-preview-rename-out":
            emit([[{"filename": "docs/retired-preview.yml", "previous_filename": ".github/workflows/release-mcpb-preview.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"security-deploy-install-page", "security-deploy-install-page-valid"}:
            emit([[{"filename": ".github/workflows/deploy-install-page.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-deploy-install-page-rename-out":
            emit([[{"filename": "docs/retired-install-page.yml", "previous_filename": ".github/workflows/deploy-install-page.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"security-documents-screen", "security-documents-screen-valid"}:
            emit([[{"filename": "src/DocumentsScreen.tsx", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-documents-screen-rename-out":
            emit([[{"filename": "docs/retired-documents-screen.tsx", "previous_filename": "src/DocumentsScreen.tsx", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "security-tauri-cargo":
            emit([[{"filename": "src-tauri/Cargo.toml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-tauri-cargo-rename-out":
            emit([[{"filename": "docs/retired-tauri-cargo.toml", "previous_filename": "src-tauri/Cargo.toml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "security-tauri-lib":
            emit([[{"filename": "src-tauri/src/lib.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "security-tauri-lib-rename-out":
            emit([[{"filename": "docs/retired-tauri-lib.rs", "previous_filename": "src-tauri/src/lib.rs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "security-documents-consumer-rename-out":
            emit([[{"filename": "docs/retired-documents.rs", "previous_filename": "src-tauri/src/documents.rs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"security-camel-dsc", "security-camel-credential"}:
            path = "src/DscScreen.tsx" if scenario == "security-camel-dsc" else "src/CredentialScreen.tsx"
            emit([[{"filename": path, "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"security-axal-frontend", "security-axal-native"}:
            path = "src/AxalScreen.tsx" if scenario == "security-axal-frontend" else "src-tauri/src/axal.rs"
            emit([[{"filename": path, "status": "modified", "additions": 1, "deletions": 0}]])
        elif security_case:
            emit([[{"filename": "src-tauri/src/dsc.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif sync_case:
            emit([[{"filename": "docs/retired.rs", "previous_filename": "src-tauri/src/sync.rs", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario == "files-empty":
            emit([[]])
        elif scenario in {"formatted-phone", "formatted-phone-grouped", "unicode-phone", "unicode-phone-tab", "unicode-phone-two-lines", "grouped-identifier-12", "grouped-identifier-16", "grouped-identifier-mixed", "phone-space", "phone-dot", "phone-plus", "phone-underscore", "phone-parenthesized", "landline-grouped", "landline-standard-hyphen", "landline-standard-space", "landline-standard-underscore", "landline-standard-four-digit"}:
            emit([[{"filename": "docs/contact.md", "status": "added", "additions": 2 if scenario == "unicode-phone-two-lines" else 1, "deletions": 0}]])
        elif scenario in {"pem-certificate-envelope", "trusted-pem-certificate-envelope"}:
            emit([[{"filename": "docs/example.md", "status": "added", "additions": 3, "deletions": 0}]])
        elif scenario == "platform-release-mcpb-preview":
            emit([[{"filename": ".github/workflows/release-mcpb-preview.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"workflow-notes-missing", "workflow-notes-present", "workflow-sibling-migration", "workflow-placeholders", "workflow-punctuated-placeholders", "platform-ci-workflow"}:
            emit([[{"filename": ".github/workflows/ci.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"dependency-manifest-missing", "dependency-manifest-present"}:
            emit([[{"filename": "package.json", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"workflow-delete", "workflow-delete-notes"}:
            emit([[{"filename": ".github/workflows/ci.yml", "status": "removed", "additions": 0, "deletions": 1}]])
        elif scenario == "platform-ci-workflow-rename-out":
            emit([[{"filename": "docs/retired-ci.yml", "previous_filename": ".github/workflows/ci.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
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
        elif scenario in {"security-notes-missing", "security-notes-present", "security-none", "security-pending"}:
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
        elif scenario in {"binary-review", "binary-review-private", "binary-review-head-moves"}:
            emit([[{"filename": "docs/new.png", "status": "added", "additions": 0, "deletions": 0}]])
        elif scenario == "metadata-only":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 0, "deletions": 0}]])
        elif scenario == "metadata-private":
            emit([[{"filename": "docs/" + "/" + "Users" + "/tester/" + "x" * 300, "status": "modified", "additions": 0, "deletions": 0}]])
        elif scenario == "metadata-incomplete":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "hunk-header-phone":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "hunk-header-literals":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 2, "deletions": 0}]])
        elif scenario == "hunk-binary-literal":
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"all-a-pan", "masked-pan", "grouped-pan-space", "grouped-pan-hyphen", "grouped-masked-pan-space", "separated-dates", "separated-dates-new-year", "separated-dates-year-month", "adr-identifier"}:
            emit([[{"filename": "docs/example.md", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "quoted-path":
            emit([[{"filename": "docs/café.md", "status": "added", "additions": 1, "deletions": 0}]])
        elif scenario in {"crlf-diff", "ambiguous-unquoted-path", "ambiguous-rename-path"}:
            status = "renamed" if scenario == "ambiguous-rename-path" else "added"
            record = {"filename": "docs/a b/example.md" if scenario != "crlf-diff" else "docs/example.md", "status": status, "additions": 0 if status == "renamed" else 1, "deletions": 0}
            if status == "renamed": record["previous_filename"] = "docs/a b/example.md"
            emit([[record]])
        elif scenario in {"gitlink", "gitlink-existing"}:
            emit([[{"filename": "vendor/module", "status": "modified", "additions": 1, "deletions": 1}]])
        elif scenario in {"implementation-p4-missing", "implementation-p4-present", "implementation-p4-continuation", "implementation-p4-shell", "implementation-p4-powershell", "implementation-p4-sql", "p4-placeholders"}:
            suffix = {"implementation-p4-shell": "sh", "implementation-p4-powershell": "ps1", "implementation-p4-sql": "sql"}.get(scenario, "py")
            emit([[{"filename": f"scripts/example.{suffix}", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario in {"platform-evidence-missing", "platform-evidence-present", "platform-evidence-heading", "platform-checkbox-evidence", "platform-checkbox-comment", "platform-inline-prose", "platform-negative-outcome", "platform-unaffected-bare", "platform-unaffected-rationale", "platform-evidence-bare-label", "platform-evidence-sibling-list", "platform-evidence-empty-fence", "platform-evidence-punctuated-placeholder", "platform-evidence-fenced-continuation", "platform-evidence-package-manager"}:
            emit([[{"filename": "src-tauri/src/local_files/paths.rs", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "platform-windows-native-action":
            emit([[{"filename": ".github/actions/setup-windows-native/action.yml", "status": "modified", "additions": 1, "deletions": 0}]])
        elif scenario == "platform-windows-native-action-rename-out":
            emit([[{"filename": "docs/retired-windows-native-action.yml", "previous_filename": ".github/actions/setup-windows-native/action.yml", "status": "renamed", "additions": 0, "deletions": 0}]])
        elif scenario in {"platform-powershell-missing", "platform-powershell-evidence"}:
            emit([[{"filename": "scripts/signing.ps1", "status": "modified", "additions": 1, "deletions": 0}]])
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
            if scenario == "checklist-fetch-fail":
                fail("controlled checklist read failure")
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
