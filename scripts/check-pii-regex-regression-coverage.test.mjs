#!/usr/bin/env node
// Contract tests for scripts/check-pii-regex-regression-coverage.mjs.
//
// Each case builds a small synthetic repository: a `base` commit (what
// master already has) and then working-tree edits on top (uncommitted, the
// same as a contributor mid-change), and runs the gate with `--base` pointing
// at the base commit's SHA so no `origin/master` needs to exist.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "check-pii-regex-regression-coverage.mjs");

function git(cwd, ...args) {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
}

async function makeRepo(files) {
  const root = await mkdtemp(join(tmpdir(), ".pii-regex-"));
  for (const [path, content] of Object.entries(files)) {
    const directory = path.split("/").slice(0, -1).join("/");
    if (directory) await mkdir(join(root, directory), { recursive: true });
    await writeFile(join(root, path), content);
  }
  git(root, "init", "-q", ".");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "test");
  git(root, "add", "-A");
  git(root, "commit", "-qm", "base");
  const base = git(root, "rev-parse", "HEAD").trim();
  return { root, base };
}

function runGate(root, base) {
  return execFileSync("node", [GATE, "--root", root, "--base", base], {
    encoding: "utf8",
    stdio: "pipe",
  });
}

function runGateExpectingFailure(root, base) {
  try {
    runGate(root, base);
  } catch (error) {
    return `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  throw new Error("expected the gate to fail, but it passed");
}

const SANITISER_V1 = `import re

PHONE = re.compile(r"\\d{10}")


def scrub(text):
    return PHONE.sub("<phone>", text)
`;

test("no changes at all passes", async () => {
  const { root, base } = await makeRepo({
    "scripts/sanitise-example.py": SANITISER_V1,
    "scripts/sanitise-example.test.py": "# placeholder\n",
  });
  try {
    const output = runGate(root, base);
    assert.match(output, /0 scanner\/sanitizer regex edit/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("editing the regex without touching the test sibling fails", async () => {
  const { root, base } = await makeRepo({
    "scripts/sanitise-example.py": SANITISER_V1,
    "scripts/sanitise-example.test.py": "# placeholder\n",
  });
  try {
    await writeFile(
      join(root, "scripts/sanitise-example.py"),
      SANITISER_V1.replace('r"\\d{10}"', 'r"\\d{6,10}"'),
    );
    const output = runGateExpectingFailure(root, base);
    assert.match(output, /sanitise-example\.py/);
    assert.match(output, /was not touched in the same diff/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("editing the regex AND the test sibling in the same diff passes", async () => {
  const { root, base } = await makeRepo({
    "scripts/sanitise-example.py": SANITISER_V1,
    "scripts/sanitise-example.test.py": "# placeholder\n",
  });
  try {
    await writeFile(
      join(root, "scripts/sanitise-example.py"),
      SANITISER_V1.replace('r"\\d{10}"', 'r"\\d{6,10}"'),
    );
    await writeFile(
      join(root, "scripts/sanitise-example.test.py"),
      "# placeholder\n# regression case for the widened 6-10 digit phone pattern\n",
    );
    const output = runGate(root, base);
    assert.match(output, /1 scanner\/sanitizer regex edit/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an unrelated edit to the same sanitizer file (no regex line touched) passes", async () => {
  const { root, base } = await makeRepo({
    "scripts/sanitise-example.py": SANITISER_V1,
    "scripts/sanitise-example.test.py": "# placeholder\n",
  });
  try {
    await writeFile(
      join(root, "scripts/sanitise-example.py"),
      SANITISER_V1.replace("def scrub(text):", "def scrub(text):  # entry point"),
    );
    const output = runGate(root, base);
    assert.match(output, /0 scanner\/sanitizer regex edit/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a regex edit in a scanner file with no test sibling at all fails distinctly", async () => {
  const { root, base } = await makeRepo({
    "scripts/redact-example.py": SANITISER_V1,
  });
  try {
    await writeFile(
      join(root, "scripts/redact-example.py"),
      SANITISER_V1.replace('r"\\d{10}"', 'r"\\d{6,10}"'),
    );
    const output = runGateExpectingFailure(root, base);
    assert.match(output, /does not exist at all/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("editing a regex in a file that is not a scanner by name is ignored", async () => {
  const { root, base } = await makeRepo({
    "scripts/format-example.py": SANITISER_V1,
  });
  try {
    await writeFile(
      join(root, "scripts/format-example.py"),
      SANITISER_V1.replace('r"\\d{10}"', 'r"\\d{6,10}"'),
    );
    const output = runGate(root, base);
    assert.match(output, /0 scanner\/sanitizer regex edit/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

console.log("\nrunning check-pii-regex-regression-coverage contract tests via node:test above");
