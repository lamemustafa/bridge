#!/usr/bin/env python3
"""Offline GitHub CLI fixture for the shrunk ``merge-gate.test.py``.

Replaces ``merge_gate_fake_gh.py`` (deleted: it existed only to test the
identity-binding/PR-body/skipped-CI concerns that were cut). This double
covers only what the shrunk ``merge-gate.sh`` still calls: PR metadata, the
diff, the changed-files list, the compatibility surface, reviews, and issue
comments.
"""
import base64
import json
import os
import sys

args = sys.argv[1:]
scenario = os.environ.get("GATE_SCENARIO", "pass")

HEAD = "0123456789abcdef0123456789abcdef01234567"
SHORT = HEAD[:7]
BASE_TIP = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
DIGEST = "a" * 64


def emit(value):
    print(value if isinstance(value, str) else json.dumps(value))


def fail(message="controlled API failure"):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def b64(obj_or_text):
    text = obj_or_text if isinstance(obj_or_text, str) else json.dumps(obj_or_text)
    return base64.b64encode(text.encode()).decode()


# ---------------------------------------------------------------------------
# Per-scenario fixture state. Every scenario starts from DEFAULT and overrides
# only what it needs to exercise.
# ---------------------------------------------------------------------------
DEFAULT_FILES = [{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}]
DEFAULT_DIFF = (
    "diff --git a/docs/example.md b/docs/example.md\n"
    "new file mode 100644\n"
    "index 0000000..1111111\n"
    "--- /dev/null\n"
    "+++ b/docs/example.md\n"
    "@@ -0,0 +1 @@\n"
    "+Safe content line.\n"
)
DEFAULT_SURFACE = {"schema_version": 2,
                    "files": [{"path": "src/example.rs", "sha256": DIGEST}]}
DEFAULT_REVIEWS = [{"user": {"login": "reviewer", "type": "User"}, "commit_id": HEAD, "state": "COMMENTED"}]
DEFAULT_COMMENTS = []
DEFAULT_BODY = "## Outcome and reason\n\nA bounded merge preflight.\n"
DEFAULT_COMMIT = {
    "sha": HEAD,
    "commit": {
        "message": "safe commit metadata",
        "author": {"name": "Maintainer", "email": "maintainer@example.invalid"},
        "committer": {"name": "Maintainer", "email": "maintainer@example.invalid"},
    },
    "author": {"login": "author"},
    "committer": {"login": "author"},
}
PUBLIC_AGENT_ADDRESS = "noreply" + "@" + "anthropic.com"
CUSTOMER_ADDRESS = "customer" + "@" + "company.test"


def state():
    s = {
        "title": "Safe merge gate control",
        "body": DEFAULT_BODY,
        "changed_files_expected": 1,
        "files": list(DEFAULT_FILES),
        "diff": DEFAULT_DIFF,
        "surface_head": dict(DEFAULT_SURFACE),
        "surface_base": dict(DEFAULT_SURFACE),
        "surface_head_fail": False,
        "surface_head_malformed": False,
        "reviews": list(DEFAULT_REVIEWS),
        "comments": list(DEFAULT_COMMENTS),
        "commits": [dict(DEFAULT_COMMIT)],
    }

    if scenario == "surface-fetch-fail":
        s["surface_head_fail"] = True

    elif scenario == "surface-malformed":
        s["surface_head_malformed"] = True

    elif scenario == "surface-head-schema1":
        # Schema 1 is read only at the base tip (bridge#760); a head still on it is refused.
        s["surface_head"] = {"schema_version": 1, "manifest_sha256": DIGEST,
                             "files": [{"path": "src/example.rs", "sha256": DIGEST}]}

    elif scenario == "surface-unpins":
        # A schema-1 base (the tip before bridge#760) is still read.
        s["surface_base"] = {"schema_version": 1, "manifest_sha256": DIGEST, "files": [
            {"path": "src/example.rs", "sha256": DIGEST},
            {"path": "docs/tally/compatibility/compatibility-surface.json", "sha256": DIGEST},
        ]}

    elif scenario == "surface-reseal-missing":
        s["files"] = [{"filename": "src/example.rs", "status": "modified", "additions": 1, "deletions": 1}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/src/example.rs b/src/example.rs\n"
            "index 1111111..2222222 100644\n"
            "--- a/src/example.rs\n"
            "+++ b/src/example.rs\n"
            "@@ -1 +1 @@\n"
            "-old\n"
            "+new\n"
        )

    elif scenario == "surface-reseal-present":
        s["files"] = [
            {"filename": "src/example.rs", "status": "modified", "additions": 1, "deletions": 1},
            {"filename": "docs/tally/compatibility/compatibility-surface.json", "status": "modified", "additions": 1, "deletions": 1},
        ]
        s["changed_files_expected"] = 2
        s["diff"] = (
            "diff --git a/src/example.rs b/src/example.rs\n"
            "index 1111111..2222222 100644\n"
            "--- a/src/example.rs\n"
            "+++ b/src/example.rs\n"
            "@@ -1 +1 @@\n"
            "-old\n"
            "+new\n"
            "diff --git a/docs/tally/compatibility/compatibility-surface.json b/docs/tally/compatibility/compatibility-surface.json\n"
            "index 1111111..2222222 100644\n"
            "--- a/docs/tally/compatibility/compatibility-surface.json\n"
            "+++ b/docs/tally/compatibility/compatibility-surface.json\n"
            "@@ -1 +1 @@\n"
            "-old manifest\n"
            "+new manifest\n"
        )

    elif scenario == "privacy-email-blocker":
        s["title"] = "Customer contact: customer@company.test"

    elif scenario == "public-agent-coauthor-trailer":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-coauthor-trailer-crlf":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\r\n\r\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\r\n"
        )
        s["commits"] = [commit]

    elif scenario == "customer-coauthor-trailer":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Customer Contributor <" + CUSTOMER_ADDRESS + ">\n"
        )
        s["commits"] = [commit]

    elif scenario == "customer-coauthor-trailer-crlf":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\r\n\r\n"
            "Co-Authored-By: Customer Contributor <" + CUSTOMER_ADDRESS + ">\r\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-email-in-pr-body":
        s["body"] = DEFAULT_BODY + "Public agent contact: " + PUBLIC_AGENT_ADDRESS + "\n"

    elif scenario == "public-agent-email-in-source-payload":
        s["diff"] = (
            "diff --git a/docs/example.md b/docs/example.md\n"
            "new file mode 100644\n"
            "index 0000000..1111111\n"
            "--- /dev/null\n"
            "+++ b/docs/example.md\n"
            "@@ -0,0 +1 @@\n"
            "+Public agent contact: " + PUBLIC_AGENT_ADDRESS + "\n"
        )

    elif scenario == "public-agent-spoof-header":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "X-Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-nonfooter":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\n\n"
            "This is ordinary commit body text, not a trailer footer.\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-nonfooter-crlf":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\r\n\r\n"
            "This is ordinary commit body text, not a trailer footer.\r\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-mixed-line-endings":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + ">\r\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-malformed-trailer":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + "\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-malformed-trailer-crlf":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\r\n\r\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + "\r\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-trailer-extra-payload":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Claude Opus 5 <" + PUBLIC_AGENT_ADDRESS + "> extra\n"
        )
        s["commits"] = [commit]

    elif scenario == "public-agent-email-in-author-name":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: " + PUBLIC_AGENT_ADDRESS + " <dev@example.invalid>\n"
        )
        s["commits"] = [commit]

    elif scenario == "customer-email-in-author-name-with-public-agent-trailer":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = dict(DEFAULT_COMMIT["commit"])
        commit["commit"]["message"] = (
            "Keep the ledger-tag refusal typed.\n\n"
            "Co-Authored-By: Customer " + CUSTOMER_ADDRESS + " <" + PUBLIC_AGENT_ADDRESS + ">\n"
        )
        s["commits"] = [commit]

    elif scenario == "privacy-credential-blocker":
        s["diff"] = (
            "diff --git a/config/settings.py b/config/settings.py\n"
            "index 1111111..2222222 100644\n"
            "--- a/config/settings.py\n"
            "+++ b/config/settings.py\n"
            "@@ -1 +1 @@\n"
            "-api_key = \"placeholder\"\n"
            "+api_key = \"sk_live_abcdef1234567890abcdef\"\n"
        )
        s["files"] = [{"filename": "config/settings.py", "status": "modified", "additions": 1, "deletions": 1}]

    elif scenario == "privacy-uuid-indeterminate":
        s["diff"] = (
            "diff --git a/docs/example.md b/docs/example.md\n"
            "new file mode 100644\n"
            "index 0000000..1111111\n"
            "--- /dev/null\n"
            "+++ b/docs/example.md\n"
            "@@ -0,0 +1 @@\n"
            "+fixture reference 11111111-2222-3333-4444-555555555555\n"
        )

    elif scenario in {"binary-missing-attestation", "binary-with-attestation"}:
        s["files"] = [{"filename": "docs/new.png", "status": "added", "additions": 0, "deletions": 0}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/docs/new.png b/docs/new.png\n"
            "new file mode 100644\n"
            "index 0000000..1111111\n"
            "GIT binary patch\n"
            "literal 3\nQwerty\n"
        )

    elif scenario == "gitlink-change":
        s["files"] = [{"filename": "vendor/module", "status": "modified", "additions": 1, "deletions": 1}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/vendor/module b/vendor/module\n"
            "index 1111111..2222222 160000\n"
            "--- a/vendor/module\n"
            "+++ b/vendor/module\n"
            "@@ -1 +1 @@\n"
            "-Subproject commit 1111111111111111111111111111111111111111\n"
            "+Subproject commit 2222222222222222222222222222222222222222\n"
        )

    elif scenario == "review-names-head-via-review":
        s["reviews"] = [{"user": {"login": "reviewer", "type": "User"}, "commit_id": HEAD, "state": "APPROVED"}]

    elif scenario == "review-names-head-via-comment":
        s["reviews"] = []
        s["comments"] = [{"user": {"login": "bot", "type": "Bot"},
                           "body": f"codex-pull-request-review-summary\n| 📝 | ✅ **Completed** | `{SHORT}` |"}]

    elif scenario == "review-missing":
        s["reviews"] = []
        s["comments"] = []

    elif scenario == "date-range-parens":
        s["body"] = DEFAULT_BODY + "\nSprint window (0101-2026 0201-2026) is locked.\n"

    elif scenario == "scan-input-write-failure":
        pass  # handled entirely by the fake mktemp wrapper; fixtures stay default

    elif scenario == "lfs-pointer-binary":
        s["files"] = [{"filename": "assets/logo.psd", "status": "added", "additions": 3, "deletions": 0}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/assets/logo.psd b/assets/logo.psd\n"
            "new file mode 100644\n"
            "index 0000000..1111111\n"
            "--- /dev/null\n"
            "+++ b/assets/logo.psd\n"
            "@@ -0,0 +1,3 @@\n"
            "+version https://git-lfs.github.com/spec/v1\n"
            "+oid sha256:" + "b" * 64 + "\n"
            "+size 12345\n"
        )

    elif scenario == "removed-file-mismatch":
        s["files"] = [{"filename": "docs/retired.md", "status": "removed", "additions": 0, "deletions": 5}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/docs/retired.md b/docs/retired.md\n"
            "deleted file mode 100644\n"
            "index 1111111..0000000\n"
            "--- a/docs/retired.md\n"
            "+++ /dev/null\n"
            "@@ -1,2 +0,0 @@\n"
            "-line one\n"
            "-line two\n"
        )

    elif scenario == "diff-extra-destination":
        s["files"] = [{"filename": "docs/example.md", "status": "added", "additions": 1, "deletions": 0}]
        s["changed_files_expected"] = 1
        s["diff"] = DEFAULT_DIFF + (
            "diff --git a/docs/smuggled.md b/docs/smuggled.md\n"
            "new file mode 100644\n"
            "index 0000000..3333333\n"
            "--- /dev/null\n"
            "+++ b/docs/smuggled.md\n"
            "@@ -0,0 +1 @@\n"
            "+smuggled content\n"
        )

    elif scenario == "coverage-example-redaction":
        # The diff has no section at all for this REST filename, so it is
        # reported as a coverage issue; the filename itself carries an
        # identifier shape and must be redacted before it reaches gate output.
        s["files"] = [{"filename": "docs/ABCDE1234F.md", "status": "added", "additions": 1, "deletions": 0}]
        s["changed_files_expected"] = 1
        s["diff"] = (
            "diff --git a/unrelated.md b/unrelated.md\n"
            "new file mode 100644\n"
            "index 0000000..1111111\n"
            "--- /dev/null\n"
            "+++ b/unrelated.md\n"
            "@@ -0,0 +1 @@\n"
            "+content\n"
        )

    elif scenario == "author-email-localhost":
        commit = dict(DEFAULT_COMMIT)
        commit["commit"] = {
            "message": "safe commit metadata",
            "author": {"name": "Developer", "email": "dev@localhost"},
            "committer": {"name": "Developer", "email": "dev@localhost"},
        }
        s["commits"] = [commit]

    return s


S = state()


def has(*needles):
    joined = " ".join(args)
    return all(n in joined for n in needles)


if args[:2] == ["pr", "view"]:
    emit({"headRefOid": HEAD, "baseRefName": "master", "title": S["title"], "body": S["body"],
          "changedFiles": S["changed_files_expected"]})

elif args[:2] == ["pr", "diff"]:
    emit(S["diff"])

elif args and args[0] == "api":
    joined = " ".join(args)
    if "/contents/" in joined:
        if "docs/tally/compatibility/compatibility-surface.json" in joined:
            if f"ref={HEAD}" in joined:
                if S["surface_head_fail"]:
                    fail("controlled surface read failure")
                if S["surface_head_malformed"]:
                    emit({"content": "not-base64"})
                else:
                    emit({"encoding": "base64", "content": b64(S["surface_head"])})
            else:
                emit({"encoding": "base64", "content": b64(S["surface_base"])})
        else:
            fail("unknown contents fixture")
    elif "/pulls/321/commits" in joined:
        emit([S["commits"]])
    elif any(a.endswith("/pulls/321") for a in args):
        emit({"commits": len(S["commits"]), "head": {"sha": HEAD}})
    elif "/pulls/321/files" in joined:
        emit([S["files"]])
    elif "/pulls/321/reviews" in joined:
        emit([S["reviews"]])
    elif "/issues/321/comments" in joined:
        emit([S["comments"]])
    elif "branches/master" in joined:
        emit(BASE_TIP)
    else:
        fail("unknown API fixture")
else:
    fail("unknown command fixture")
