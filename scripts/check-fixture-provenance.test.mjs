#!/usr/bin/env node
// Contract tests for scripts/check-fixture-provenance.mjs.
//
// Every case builds a small synthetic tree with the four real fixture-root
// paths (via --root) rather than touching the repository's own fixtures:
// this gate's directory list is a fixed mirror of
// check-fixture-byte-integrity.mjs's, not something it discovers, so there is
// no way to exercise "a fresh directory with no coverage at all" the way that
// gate's own test does — every case here instead varies what is inside one of
// the four already-covered directories.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "check-fixture-provenance.mjs");

const FIXTURE_DIRS = [
  "src-tauri/crates/bridge-tally-protocol/tests/fixtures",
  "src-tauri/crates/tally-protocol-simulator/fixtures",
  "docs/tally/compatibility/fixtures",
  "scripts/fixtures",
];

async function makeTree() {
  const root = await mkdtemp(join(tmpdir(), ".fixture-provenance-"));
  for (const dir of FIXTURE_DIRS) await mkdir(join(root, dir), { recursive: true });
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

test("an empty covered tree passes (nothing to document)", async () => {
  const root = await makeTree();
  try {
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a fixture named nowhere in any Markdown file fails the gate", async () => {
  const root = await makeTree();
  try {
    await writeFile(join(root, "scripts/fixtures/mystery-capture.xml"), "<x/>\n");
    const output = runGateExpectingFailure(root);
    assert.match(output, /mystery-capture\.xml/);
    assert.match(output, /not named in any Markdown file/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a fixture named only in prose (no table row) passes without a hash check", async () => {
  const root = await makeTree();
  try {
    await writeFile(join(root, "scripts/fixtures/prose-only.xml"), "<x/>\n");
    await writeFile(
      join(root, "scripts/fixtures/README.md"),
      "`prose-only.xml` was normalised by Git on first commit; byte fidelity is not established.\n",
    );
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a captured fixture whose table entry matches its real bytes passes", async () => {
  const root = await makeTree();
  try {
    const bytes = "captured, byte-exact\n";
    const dir = join(root, "src-tauri/crates/bridge-tally-protocol/tests/fixtures/encoding");
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "sample.bin"), bytes);
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    await writeFile(
      join(dir, "PROVENANCE.md"),
      `| \`sample.bin\` | ${bytes.length} | \`${sha256}\` |\n`,
    );
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// The headline case: this is the "hand-authored fixture standing in for a
// captured one" finding class made mechanical. A table declares specific
// bytes as captured evidence; the file on disk is something else entirely.
test("a captured fixture whose bytes no longer match its declared hash fails the gate", async () => {
  const root = await makeTree();
  try {
    const declaredBytes = "the real captured response\n";
    const actualBytes = "a hand-typed stand-in someone swapped in\n";
    const dir = join(root, "src-tauri/crates/bridge-tally-protocol/tests/fixtures/encoding");
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "swapped.bin"), actualBytes);
    const declaredSha256 = createHash("sha256").update(declaredBytes).digest("hex");
    await writeFile(
      join(dir, "PROVENANCE.md"),
      `| \`swapped.bin\` | ${declaredBytes.length} | \`${declaredSha256}\` |\n`,
    );
    const output = runGateExpectingFailure(root);
    assert.match(output, /swapped\.bin/);
    assert.match(output, /hand-authored-substitute pattern/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a byte-count-only mismatch (same filename, wrong declared size) still fails", async () => {
  const root = await makeTree();
  try {
    const actualBytes = "0123456789\n";
    const dir = join(root, "docs/tally/compatibility/fixtures");
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "size-mismatch.json"), actualBytes);
    const realSha256 = createHash("sha256").update(actualBytes).digest("hex");
    await writeFile(
      join(dir, "PROVENANCE.md"),
      // Right hash, wrong declared byte count.
      `| \`size-mismatch.json\` | 999999 | \`${realSha256}\` |\n`,
    );
    const output = runGateExpectingFailure(root);
    assert.match(output, /size-mismatch\.json/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// The real repository is not asserted to pass here — it does not yet (see
// docs/proposed-ci-gates.md: 82 of 125 fixtures across the four covered
// directories currently have no provenance mention). That is exactly why
// this gate is proposed as REPORTING rather than BLOCKING; this suite
// verifies behaviour on synthetic trees, not repository readiness.
console.log("\nrunning check-fixture-provenance contract tests via node:test above");

test("the failure names the true total, not the display cap", async () => {
  // Thirty undocumented fixtures against a display cap of twenty-five. The gate
  // used to cap COLLECTION at the same number, which made `failures.length`
  // unable to exceed the cap and the "omitted" branch unreachable: it reported
  // "25 problem(s)" whether the real figure was 25 or 250, so nobody could tell
  // whether the backlog was shrinking. Thirty is deliberately just past the cap
  // — the smallest tree that can tell a total from a ceiling.
  const root = await makeTree();
  const dir = join(root, FIXTURE_DIRS[0]);
  for (let index = 0; index < 30; index += 1) {
    await writeFile(join(dir, `undocumented-${index}.xml`), `<X>${index}</X>`);
  }
  const output = runGateExpectingFailure(root);

  assert.match(
    output,
    /fixture provenance: 30 problem\(s\), 30 of 30 fixtures with no provenance record/,
    "the count must be the real number of problems and carry its denominator",
  );
  assert.match(
    output,
    /showing the first 25, 5 omitted/,
    "and must disclose how many it is not showing",
  );
  assert.equal(
    (output.match(/undocumented-\d+\.xml: not named/g) ?? []).length,
    25,
    "while still printing only the capped number of lines",
  );
});
