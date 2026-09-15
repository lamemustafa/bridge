#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/reseal-merge-driver.mjs, run as an actual `git
// merge` against this repository -- not a synthetic fixture repo. Three real
// bugs in earlier drafts of this driver were only caught this way (a
// hardcoded `.git/MERGE_HEAD` silently never resolving in this repo's own
// worktree checkout; MERGE_HEAD not existing yet at driver-invocation time
// even when the path is right; and a stale-hash race from reading the
// working tree mid-merge) -- a mocked git environment would not have
// surfaced any of the three, since each was specific to how a real `git
// merge` behaves. So these tests build two disposable branches from the
// current commit, actually merge them, and delete both branches (and
// restore the original HEAD) whether they pass or fail.
//
// `corepack pnpm test` runs every scripts/*.test.mjs in the "Frontend build"
// CI job (.github/workflows/ci.yml), which has no Rust toolchain -- this
// needs it (the driver runs seal-surface / repoint-matrix through it) and
// skips itself with a clear reason when it is unavailable. See
// docs/proposed-dependency-policy.md for the proposed CI job that would give
// it somewhere to actually run.
//
// Run: node scripts/reseal-merge-driver.test.mjs
// (from a checkout with uncommitted changes only outside docs/adr/ and
// docs/tally/compatibility/ -- this test creates and deletes real branches
// and will refuse to run over uncommitted changes to the files it touches)

import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const SURFACE = "docs/tally/compatibility/compatibility-surface.json";
const MATRIX = "docs/tally/compatibility/compatibility-matrix.json";
const RESEAL = join(repoRoot, "scripts", "reseal.sh");
const DRIVER_COMMAND = "node scripts/reseal-merge-driver.mjs %O %A %B %P %S %X %Y";

function pinnedToolchainAvailable() {
  let tomlText;
  try {
    tomlText = readFileSync(join(repoRoot, "rust-toolchain.toml"), "utf8");
  } catch {
    return { ok: false, reason: "rust-toolchain.toml not found" };
  }
  const match = /^channel *= *"(.*)"/m.exec(tomlText);
  if (!match) return { ok: false, reason: "could not read [toolchain].channel" };
  const which = spawnSync("rustup", ["which", "--toolchain", match[1], "rustc"], { encoding: "utf8" });
  if (which.error || which.status !== 0) {
    return { ok: false, reason: `rustup toolchain ${match[1]} is not installed` };
  }
  return { ok: true, reason: null };
}

function git(args) {
  return spawnSync("git", args, { cwd: repoRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

function gitOk(args, label) {
  const result = git(args);
  assert.equal(result.status, 0, `${label ?? args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout;
}

function reseal() {
  const result = spawnSync("bash", [RESEAL], { cwd: repoRoot, encoding: "utf8" });
  assert.equal(result.status, 0, `scripts/reseal.sh failed:\n${result.stderr}`);
}

const toolchain = pinnedToolchainAvailable();
const skip = toolchain.ok ? false : `pinned Rust toolchain unavailable: ${toolchain.reason}`;

// A single suite (not per-test isolation) because every case needs the same
// disposable-branch setup and, more importantly, must never run concurrently
// with another case that's also checking out branches in this same worktree.
test("git merge driver: reconciles disjoint pinned-file changes, refuses genuine ones", { skip }, async (t) => {
  const suffix = randomBytes(4).toString("hex");
  const base = `test/reseal-driver-base-${suffix}`;
  const branchA = `test/reseal-driver-a-${suffix}`;
  const branchB = `test/reseal-driver-b-${suffix}`;
  const mergeBranch = `test/reseal-driver-merge-${suffix}`;
  const conflictA = `test/reseal-driver-conflict-a-${suffix}`;
  const conflictB = `test/reseal-driver-conflict-b-${suffix}`;
  const conflictMerge = `test/reseal-driver-conflict-merge-${suffix}`;

  const originalBranch = gitOk(["rev-parse", "--abbrev-ref", "HEAD"], "capture current branch").trim();
  assert.notEqual(originalBranch, "HEAD", "refusing to run from a detached HEAD (can't safely return to it by name)");
  const statusBefore = git(["status", "--porcelain", "--", "docs/adr/", "docs/tally/compatibility/"]).stdout;
  assert.equal(statusBefore.trim(), "", "refusing to run with uncommitted changes under docs/adr/ or docs/tally/compatibility/");

  // Scoped to THIS repo's local config only (never --global); this mirrors
  // exactly the one-time setup step documented in docs/release-process.md,
  // done here instead of trusting ambient config so the test is
  // self-contained. Any pre-existing value is restored afterward.
  const priorDriver = git(["config", "--get", "merge.bridge-compat-reseal.driver"]);
  const priorName = git(["config", "--get", "merge.bridge-compat-reseal.name"]);

  const cleanup = () => {
    git(["merge", "--abort"]);
    git(["checkout", originalBranch]);
    for (const branch of [base, branchA, branchB, mergeBranch, conflictA, conflictB, conflictMerge]) {
      git(["branch", "-D", branch]);
    }
    if (priorDriver.status === 0) git(["config", "merge.bridge-compat-reseal.driver", priorDriver.stdout.trim()]);
    else git(["config", "--unset", "merge.bridge-compat-reseal.driver"]);
    if (priorName.status === 0) git(["config", "merge.bridge-compat-reseal.name", priorName.stdout.trim()]);
    else git(["config", "--unset", "merge.bridge-compat-reseal.name"]);
  };
  t.after(cleanup);

  gitOk(["config", "merge.bridge-compat-reseal.driver", DRIVER_COMMAND]);
  gitOk(["config", "merge.bridge-compat-reseal.name", "test run"]);
  gitOk(["checkout", "-b", base]);

  await t.test("disjoint pinned files merge cleanly with byte-correct hashes", () => {
    gitOk(["checkout", "-b", branchA, base]);
    appendLine("docs/adr/0014-tally-native-outstandings-probe-authority.md", `test ${suffix} branch A`);
    reseal();
    gitOk(["add", "docs/adr/0014-tally-native-outstandings-probe-authority.md", SURFACE, MATRIX]);
    gitOk(["commit", "-m", `test: branch A ${suffix}`]);

    gitOk(["checkout", "-b", branchB, base]);
    appendLine("docs/adr/0015-tally-selected-read-qualification-authority.md", `test ${suffix} branch B`);
    reseal();
    gitOk(["add", "docs/adr/0015-tally-selected-read-qualification-authority.md", SURFACE, MATRIX]);
    gitOk(["commit", "-m", `test: branch B ${suffix}`]);

    gitOk(["checkout", "-b", mergeBranch, branchA]);
    const merge = git(["merge", branchB, "--no-edit"]);
    assert.equal(merge.status, 0, `expected a clean merge; stdout:\n${merge.stdout}\nstderr:\n${merge.stderr}`);

    const conflictMarkers = git(["grep", "-l", "-e", "<<<<<<<", "--", SURFACE, MATRIX]);
    assert.equal(conflictMarkers.status, 1, "expected no conflict markers in either resealed file");

    const mismatches = hashMismatches();
    assert.deepEqual(mismatches, [], `every pinned file's recorded hash must match its real bytes: ${JSON.stringify(mismatches)}`);

    const verify = spawnSync("bash", [RESEAL, "--verify"], { cwd: repoRoot, encoding: "utf8" });
    assert.equal(verify.status, 0, `--verify must pass on the driver's own output:\n${verify.stderr}`);
  });

  await t.test("both sides changing the same pinned file's content is refused, not guessed at", () => {
    gitOk(["checkout", "-b", conflictA, base]);
    appendLine("docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict A`);
    reseal();
    gitOk(["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    gitOk(["commit", "-m", `test: conflict A ${suffix}`]);

    gitOk(["checkout", "-b", conflictB, base]);
    appendLine("docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict B (different)`);
    reseal();
    gitOk(["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    gitOk(["commit", "-m", `test: conflict B ${suffix}`]);

    gitOk(["checkout", "-b", conflictMerge, conflictA]);
    const merge = git(["merge", conflictB, "--no-edit"]);
    assert.notEqual(merge.status, 0, "expected the merge to stop with conflicts");
    assert.match(merge.stderr ?? "", /reseal-merge-driver: refusing to auto-resolve/);

    for (const file of [SURFACE, MATRIX]) {
      const content = readFileSync(join(repoRoot, file), "utf8");
      assert.match(content, /^<<<<<<< /m, `${file} must be left with ordinary conflict markers for manual resolution`);
    }
    git(["merge", "--abort"]);
  });
});

function appendLine(relativePath, text) {
  const path = join(repoRoot, relativePath);
  appendFileSync(path, `\n<!-- ${text} -->\n`);
}

// Deliberately Node's own crypto rather than shelling out to shasum/
// sha256sum: which of those exists differs by platform (macOS ships
// `shasum`, most Linux distros `sha256sum`), and this needs to run wherever
// the pinned toolchain does.
function hashMismatches() {
  const surface = JSON.parse(readFileSync(join(repoRoot, SURFACE), "utf8"));
  const mismatches = [];
  for (const file of surface.files) {
    let actual;
    try {
      actual = createHash("sha256").update(readFileSync(join(repoRoot, file.path))).digest("hex");
    } catch {
      mismatches.push({ path: file.path, error: "unreadable" });
      continue;
    }
    if (actual !== file.sha256) mismatches.push({ path: file.path, expected: file.sha256, actual });
  }
  return mismatches;
}
