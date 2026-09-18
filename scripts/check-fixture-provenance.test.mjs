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
  "src-tauri/crates/bridge-bank-statement/tests/fixtures",
  "src-tauri/crates/bridge-tax-audit/tests/fixtures",
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

test("a fixture named in no provenance record fails the gate", async () => {
  const root = await makeTree();
  try {
    await writeFile(join(root, "scripts/fixtures/mystery-capture.xml"), "<x/>\n");
    const output = runGateExpectingFailure(root);
    assert.match(output, /mystery-capture\.xml/);
    assert.match(output, /named in no provenance record/);
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

// --- provenance that is not Markdown ------------------------------------
//
// The agent fixtures record provenance as a JSON sidecar per capture, and one
// Markdown file documents its fixture by filename pairing rather than by
// naming it in the text. Reading only Markdown text reported 82 undocumented
// fixtures where 51 are, and left 13 declared hashes unchecked -- a gate
// understating its own strength and overstating its own backlog.

const SIDECAR_DIR = join(
  "src-tauri/crates/bridge-tally-protocol/tests/fixtures",
  "agent",
);

test("a JSON sidecar carrying `source` documents the fixture sharing its stem", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), "captured bytes");
  await writeFile(
    join(root, SIDECAR_DIR, "cap.json"),
    JSON.stringify({ source: "Live licensed TallyPrime synthetic-company read" }),
  );
  const output = runGate(root);
  assert.match(output, /Fixture provenance is intact/);
});

test("a sidecar declaring a fixture hash escalates to the same check a table row does", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  const bytes = "captured bytes";
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), bytes);
  await writeFile(
    join(root, SIDECAR_DIR, "cap.json"),
    JSON.stringify({
      source: "Live licensed TallyPrime synthetic-company read",
      fixture_bytes: Buffer.byteLength(bytes),
      fixture_sha256: createHash("sha256").update(bytes).digest("hex"),
    }),
  );
  assert.match(runGate(root), /1 captured-fixture hash\(es\) verified|hash\(es\) verified/);

  // And the whole point: a swapped fixture under a stale sidecar must fail.
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), "hand-authored substitute");
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /cap\.utf16le\.xml/);
  assert.match(failure, /hand-authored-substitute pattern/);
});

test("a JSON file without a `source` string is a fixture, not a record", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  // An ordinary expected-output fixture. It must still need its own paper
  // trail -- otherwise any .json in the tree could excuse itself.
  await writeFile(join(root, SIDECAR_DIR, "expected.json"), JSON.stringify({ rows: [] }));
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /expected\.json/);
});

test("a sidecar cannot document a fixture that merely shares a prefix", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  await writeFile(join(root, SIDECAR_DIR, "cap.json"), JSON.stringify({ source: "a live read" }));
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), "documented");
  // `cap-extra` shares the prefix `cap` but not the stem `cap.`; documenting
  // it would be the gate excusing a neighbouring fixture by accident.
  await writeFile(join(root, SIDECAR_DIR, "cap-extra.utf16le.xml"), "undocumented");
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /cap-extra\.utf16le\.xml/);
  assert.doesNotMatch(failure, /(?<!-extra)(?<!-)\bcap\.utf16le\.xml/);
});

test("`<stem>.PROVENANCE.md` documents `<stem>.*` without naming it in the text", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  await writeFile(join(root, SIDECAR_DIR, "cap.utf8.xml"), "captured bytes");
  // Deliberately never writes the fixture's filename -- this is how
  // native-company-book-extents-with-number.PROVENANCE.md is written.
  await writeFile(
    join(root, SIDECAR_DIR, "cap.PROVENANCE.md"),
    "# A field observation\n\nExact decoded XML captured on 2026-09-09.\n",
  );
  assert.match(runGate(root), /Fixture provenance is intact/);
});

test("a bare PROVENANCE.md still documents only what it names", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  await writeFile(join(root, SIDECAR_DIR, "cap.utf8.xml"), "captured bytes");
  // No stem, so no filename pairing: the directory-level file has to say which
  // fixture it means, exactly as before this change.
  await writeFile(join(root, SIDECAR_DIR, "PROVENANCE.md"), "# Notes\n\nNothing named here.\n");
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /cap\.utf8\.xml/);
});

test("the failure names the real backlog, not just the sample it prints", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  for (let index = 0; index < 30; index += 1) {
    await writeFile(join(root, SIDECAR_DIR, `undocumented-${index}.xml`), "bytes");
  }
  const failure = runGateExpectingFailure(root);
  // Capped output, uncapped count: quoting the sample as the total is how a
  // headline number ends up wrong.
  assert.match(failure, /shown of 30 undocumented fixture\(s\)/);
});

// The real sidecars are not schema-consistent, and reading only the canonical
// key name exempted two of them from swap detection while their own records
// held the correct hash. These cases use the key names actually present in
// the repository rather than only the one the convention prefers.

test("a sidecar declaring its hash as `sha256` is checked, not treated as prose", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  const bytes = "captured bytes";
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), bytes);
  await writeFile(
    join(root, SIDECAR_DIR, "cap.json"),
    // `sha256`, not `fixture_sha256` -- the spelling in
    // native-three-vouchers.json and native-empty-collection.json.
    JSON.stringify({
      source: "Live licensed TallyPrime synthetic-company read",
      sha256: createHash("sha256").update(bytes).digest("hex"),
    }),
  );
  assert.match(runGate(root), /hash\(es\) verified/);

  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), "hand-authored substitute");
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /cap\.utf16le\.xml/);
  assert.match(failure, /hand-authored-substitute pattern/);
});

test("a sidecar with no byte count is checked on its hash and says so", async () => {
  const root = await makeTree();
  await mkdir(join(root, SIDECAR_DIR), { recursive: true });
  const bytes = "captured bytes";
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), bytes);
  await writeFile(
    join(root, SIDECAR_DIR, "cap.json"),
    // `fixture_sha256` without `fixture_bytes` -- native-namespaced-journal.json.
    JSON.stringify({
      source: "a live read",
      fixture_sha256: createHash("sha256").update(bytes).digest("hex"),
    }),
  );
  assert.match(runGate(root), /hash\(es\) verified/);

  // The hash must still be able to fail. Inferring the byte count from the
  // file would compare it against itself and never fail on size.
  await writeFile(join(root, SIDECAR_DIR, "cap.utf16le.xml"), "swapped");
  const failure = runGateExpectingFailure(root);
  assert.match(failure, /\(no byte count\)/);
  assert.match(failure, /cap\.utf16le\.xml/);
});
