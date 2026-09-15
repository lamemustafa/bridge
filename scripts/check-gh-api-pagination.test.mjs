#!/usr/bin/env node
// Contract tests for scripts/check-gh-api-pagination.mjs.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "check-gh-api-pagination.mjs");

function git(cwd, ...args) {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
}

async function makeRepo(files) {
  const root = await mkdtemp(join(tmpdir(), ".gh-pagination-"));
  for (const [path, content] of Object.entries(files)) {
    const directory = path.split("/").slice(0, -1).join("/");
    if (directory) await mkdir(join(root, directory), { recursive: true });
    await writeFile(join(root, path), content);
  }
  git(root, "init", "-q", ".");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "test");
  git(root, "add", "-A");
  git(root, "commit", "-qm", "seed");
  return root;
}

function runGate(root) {
  return execFileSync("node", [GATE, "--root", root], { encoding: "utf8", stdio: "pipe" });
}

function runGateExpectingFailure(root) {
  try {
    runGate(root);
  } catch (error) {
    return `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  throw new Error("expected the gate to fail, but it passed");
}

test("an unpaginated REST list call fails", async () => {
  const root = await makeRepo({
    ".github/workflows/example.yml": `jobs:\n  x:\n    steps:\n      - run: gh api repos/acme/widgets/pulls\n`,
  });
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /pulls looks like a collection listing/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the same call with --paginate passes", async () => {
  const root = await makeRepo({
    ".github/workflows/example.yml": `jobs:\n  x:\n    steps:\n      - run: gh api --paginate repos/acme/widgets/pulls\n`,
  });
  try {
    const output = runGate(root);
    assert.match(output, /0 unpaginated list calls/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a call pinned to one specific PR number is not a list call", async () => {
  const root = await makeRepo({
    ".github/workflows/example.yml": `jobs:\n  x:\n    steps:\n      - run: gh api repos/acme/widgets/pulls/228\n`,
  });
  try {
    const output = runGate(root);
    assert.match(output, /0 unpaginated list calls/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a mutating POST call is never treated as a list call", async () => {
  const root = await makeRepo({
    ".github/workflows/example.yml":
      `jobs:\n  x:\n    steps:\n      - run: |\n          gh api --method POST "repos/acme/widgets/git/refs" \\\n            -f ref="refs/tags/v1"\n`,
  });
  try {
    const output = runGate(root);
    assert.match(output, /0 unpaginated list calls/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a multi-line continued command is joined and still caught", async () => {
  const root = await makeRepo({
    "scripts/example.sh":
      `#!/bin/sh\ngh api \\\n  repos/acme/widgets/issues \\\n  --jq '.[].title'\n`,
  });
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /issues looks like a collection listing/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a GraphQL query requesting a connection with no hasNextPage anywhere fails", async () => {
  const root = await makeRepo({
    "scripts/review-threads.py":
      `import subprocess\n\nQUERY = """\nquery {\n  repository(owner: "a", name: "b") {\n    pullRequest(number: 228) {\n      reviewThreads(first: 100) {\n        nodes { id isResolved }\n      }\n    }\n  }\n}\n"""\n\nsubprocess.run(f"gh api graphql -f query='{QUERY}'", shell=True)\n`,
  });
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /requests a connection/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the same GraphQL query with a hasNextPage cursor loop elsewhere in the file passes", async () => {
  const root = await makeRepo({
    "scripts/review-threads.py":
      `import subprocess\n\nQUERY = """\nquery($cursor: String) {\n  repository(owner: "a", name: "b") {\n    pullRequest(number: 228) {\n      reviewThreads(first: 100, after: $cursor) {\n        nodes { id isResolved }\n        pageInfo { hasNextPage endCursor }\n      }\n    }\n  }\n}\n"""\n\nsubprocess.run(f"gh api graphql -f query='{QUERY}'", shell=True)\n\n# elsewhere: while page["pageInfo"]["hasNextPage"]: ...\n`,
  });
  try {
    const output = runGate(root);
    assert.match(output, /0 unpaginated list calls/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a GraphQL query with no connection fields at all is not flagged", async () => {
  const root = await makeRepo({
    "scripts/example.py": `import subprocess\nQUERY = "query { viewer { login } }"\nsubprocess.run(["gh", "api", "graphql", "-f", f"query={QUERY}"])\n`,
  });
  try {
    const output = runGate(root);
    assert.match(output, /0 unpaginated list calls/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a Markdown runbook code fence is scanned too", async () => {
  const root = await makeRepo({
    "docs/runbook.md": "```bash\ngh api repos/acme/widgets/comments\n```\n",
  });
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /comments looks like a collection listing/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

console.log("\nrunning check-gh-api-pagination contract tests via node:test above");
