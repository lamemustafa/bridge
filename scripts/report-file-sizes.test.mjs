#!/usr/bin/env node
// Contract tests for scripts/report-file-sizes.mjs.
//
// This script is a REPORTING tool with no pass/fail contract — it always
// exits 0 — so "verify it fails on bad input" does not apply the way it does
// to a gate. What is verified instead: the reported numbers are exactly
// right against a fixture whose line counts are known in advance (a chart
// that silently miscounts is worse than no chart), and that it does not
// crash or misreport on the inputs its own banner comment says it
// deliberately excludes — a binary/NUL-containing file and a non-text
// extension.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "report-file-sizes.mjs");

function git(cwd, ...args) {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
}

async function makeRepo(files) {
  const root = await mkdtemp(join(tmpdir(), ".file-sizes-"));
  for (const [path, content] of Object.entries(files)) {
    const directory = path.split("/").slice(0, -1).join("/");
    if (directory) await mkdir(join(root, directory), { recursive: true });
    if (Buffer.isBuffer(content)) await writeFile(join(root, path), content);
    else await writeFile(join(root, path), content);
  }
  git(root, "init", "-q", ".");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "test");
  git(root, "add", "-A");
  git(root, "commit", "-qm", "seed");
  return root;
}

function runReport(root, extraArgs = []) {
  return execFileSync("node", [GATE, "--root", root, ...extraArgs], { encoding: "utf8", stdio: "pipe" });
}

test("line counts are exact: 5 newline-terminated lines counts as 5", async () => {
  const root = await makeRepo({ "src-tauri/src/five.rs": "a\nb\nc\nd\ne\n" });
  try {
    const output = runReport(root);
    assert.match(output, /1 text file\(s\) counted/);
    assert.match(output, /Total lines: 5/);
    assert.match(output, /5\s+src-tauri\/src\/five\.rs/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a final line with no trailing newline still counts", async () => {
  const root = await makeRepo({ "src-tauri/src/three.rs": "a\nb\nc" });
  try {
    const output = runReport(root);
    assert.match(output, /Total lines: 3/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an empty file counts as 0 lines, not crashing on an empty match", async () => {
  const root = await makeRepo({ "src-tauri/src/empty.rs": "" });
  try {
    const output = runReport(root);
    assert.match(output, /Total lines: 0/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a NUL-containing file is skipped as binary, not miscounted", async () => {
  const root = await makeRepo({
    "src-tauri/src/binary.rs": Buffer.from([0x00, 0x01, 0x0a, 0x00, 0x0a]),
    "src-tauri/src/text.rs": "a\nb\n",
  });
  try {
    const output = runReport(root);
    assert.match(output, /1 text file\(s\) counted \(0 skipped by extension, 1 skipped as binary/);
    assert.match(output, /Total lines: 2/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a non-text extension is skipped by extension, not by content", async () => {
  const root = await makeRepo({
    "src-tauri/icons/app.icns": "not actually valid icns but has no NUL byte\nline2\n",
    "src-tauri/src/text.rs": "a\n",
  });
  try {
    const output = runReport(root);
    assert.match(output, /1 text file\(s\) counted \(1 skipped by extension, 0 skipped as binary/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("--top limits the printed list without affecting totals", async () => {
  const root = await makeRepo({
    "src-tauri/src/a.rs": "1\n2\n3\n",
    "src-tauri/src/b.rs": "1\n2\n",
    "src-tauri/src/c.rs": "1\n",
  });
  try {
    const output = runReport(root, ["--top", "1"]);
    assert.match(output, /Total lines: 6/);
    assert.match(output, /Largest 1 file\(s\)/);
    assert.doesNotMatch(output, /src-tauri\/src\/b\.rs/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("this script never fails, even on a repository with nothing to report", async () => {
  const root = await makeRepo({ "README.md": "" });
  try {
    // README.md is one .md file, so this is really "nothing BUT a trivial
    // file" — the point is the process still exits 0 and prints a report.
    assert.doesNotThrow(() => runReport(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("largest files are sorted descending by line count", async () => {
  const root = await makeRepo({
    "src-tauri/src/small.rs": "1\n",
    "src-tauri/src/big.rs": "1\n2\n3\n4\n5\n",
    "src-tauri/src/medium.rs": "1\n2\n3\n",
  });
  try {
    const output = runReport(root);
    const bigIndex = output.indexOf("big.rs");
    const mediumIndex = output.indexOf("medium.rs");
    const smallIndex = output.indexOf("small.rs");
    assert.ok(bigIndex > 0 && bigIndex < mediumIndex && mediumIndex < smallIndex);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

console.log("\nrunning report-file-sizes contract tests via node:test above");
