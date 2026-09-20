#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/reseal-merge-driver.mjs, run as an actual `git
// merge` in a disposable clone of this repository -- not a synthetic fixture
// repo. Three real
// bugs in earlier drafts of this driver were only caught this way (a
// hardcoded `.git/MERGE_HEAD` silently never resolving in this repo's own
// worktree checkout; MERGE_HEAD not existing yet at driver-invocation time
// even when the path is right; and a stale-hash race from reading the
// working tree mid-merge) -- a mocked git environment would not have
// surfaced any of the three, since each was specific to how a real `git
// merge` behaves. So these tests clone the source checkout's committed HEAD,
// start from that commit detached, and perform every config/branch/merge/reseal
// mutation in the clone. The source checkout is read only and may itself be
// detached or dirty; uncommitted source bytes are deliberately not exercised.
//
// `corepack pnpm test` runs every scripts/*.test.mjs in the "Frontend build"
// CI job (.github/workflows/ci.yml), which does not explicitly install Rust.
// This needs the pinned toolchain (the driver runs seal-surface /
// repoint-matrix through it) and skips itself with a clear reason when that
// toolchain is unavailable on the runner. See
// docs/proposed-dependency-policy.md for the proposed CI job that would give
// it somewhere to actually run.
//
// Run: node scripts/reseal-merge-driver.test.mjs

import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { devNull, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = realpathSync(join(dirname(fileURLToPath(import.meta.url)), ".."));
const SURFACE = "docs/tally/compatibility/compatibility-surface.json";
const MATRIX = "docs/tally/compatibility/compatibility-matrix.json";
const DRIVER_COMMAND = "node scripts/reseal-merge-driver.mjs %O %A %B %P %S %X %Y";

// Git honours repository/config/index overrides from the environment even
// when cwd points at the disposable clone. Drop every inherited GIT_* value
// case-insensitively, then disable system/global config so ambient hooks,
// signing, and include directives cannot redirect a scratch operation back
// into the source tree.
function sanitizedGitEnvironment(environment) {
  const sanitized = { ...environment };
  for (const key of Object.keys(sanitized)) {
    if (key.toUpperCase().startsWith("GIT_")) delete sanitized[key];
  }
  sanitized.GIT_CONFIG_NOSYSTEM = "1";
  sanitized.GIT_CONFIG_GLOBAL = devNull;
  return sanitized;
}

const gitEnv = sanitizedGitEnvironment(process.env);
// A differently owned checkout is still a valid read-only source in CI and
// containers. Trust only its canonical path, and only for deliberate source
// reads and the clone command whose upload-pack child reads that source.
const sourceGitEnv = {
  ...gitEnv,
  GIT_CONFIG_COUNT: "1",
  GIT_CONFIG_KEY_0: "safe.directory",
  GIT_CONFIG_VALUE_0: repoRoot,
};

function pinnedToolchainAvailable(root, revision, env = gitEnv) {
  // A failed source read is a test failure, never evidence that Rust is unavailable.
  const tomlText = gitOk(root, ["show", `${revision}:rust-toolchain.toml`], "read committed toolchain", env);
  const match = /^channel *= *"(.*)"/m.exec(tomlText);
  if (!match) return { ok: false, reason: "could not read [toolchain].channel" };
  const which = spawnSync("rustup", ["which", "--toolchain", match[1], "rustc"], { encoding: "utf8" });
  if (which.error || which.status !== 0) {
    return { ok: false, reason: `rustup toolchain ${match[1]} is not installed` };
  }
  return { ok: true, reason: null };
}

function git(root, args, { encoding = "utf8", env = gitEnv } = {}) {
  const options = {
    cwd: root,
    env,
    maxBuffer: 64 * 1024 * 1024,
  };
  if (encoding !== null) options.encoding = encoding;
  return spawnSync("git", args, options);
}

function gitOk(root, args, label, env = gitEnv) {
  const result = git(root, args, { env });
  assert.equal(result.status, 0, `${label ?? args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout;
}

function reseal(root, ...args) {
  const result = spawnSync("bash", [join(root, "scripts", "reseal.sh"), ...args], {
    cwd: root,
    encoding: "utf8",
    env: gitEnv,
  });
  assert.equal(result.status, 0, `scripts/reseal.sh failed:\n${result.stderr}`);
}

function repositoryState(root, env = gitEnv) {
  const symbolicHead = git(root, ["symbolic-ref", "--quiet", "HEAD"], { env });
  assert.ok(
    symbolicHead.status === 0 || symbolicHead.status === 1,
    `capture symbolic HEAD failed:\n${symbolicHead.stderr}`,
  );
  const localConfig = git(root, ["config", "--local", "--null", "--list"], { encoding: null, env });
  assert.equal(localConfig.status, 0, `capture local config failed:\n${localConfig.stderr.toString("utf8")}`);
  return {
    head: gitOk(root, ["rev-parse", "--verify", "HEAD"], "capture HEAD", env).trim(),
    symbolicHeadStatus: symbolicHead.status,
    symbolicHead: symbolicHead.stdout,
    status: gitOk(root, ["--no-optional-locks", "status", "--porcelain=v1", "-z", "--untracked-files=all"], "capture status", env),
    localConfigSha256: createHash("sha256").update(localConfig.stdout).digest("hex"),
  };
}


// A single suite shares one unique clone. Different scripts/*.test.mjs workers
// get different temp directories, so their Git refs, index, config, and working
// files cannot collide.
test("git merge driver: reconciles disjoint pinned-file changes, refuses genuine ones", async (t) => {
  const sourceState = repositoryState(repoRoot, sourceGitEnv);
  const toolchain = pinnedToolchainAvailable(repoRoot, sourceState.head, sourceGitEnv);
  if (!toolchain.ok) {
    t.skip(`pinned Rust toolchain unavailable: ${toolchain.reason}`);
    return;
  }
  const sandbox = mkdtempSync(join(tmpdir(), "bridge-reseal-driver-"));
  const fixtureRoot = join(sandbox, "repo");

  t.after(() => {
    rmSync(sandbox, { recursive: true, force: true });
    assert.deepEqual(
      repositoryState(repoRoot, sourceGitEnv),
      sourceState,
      "merge-driver test must preserve the source checkout's HEAD, status, and local config",
    );
  });

  gitOk(
    sandbox,
    ["clone", "--quiet", "--no-local", "--no-checkout", repoRoot, fixtureRoot],
    "clone committed source",
    sourceGitEnv,
  );
  gitOk(fixtureRoot, ["checkout", "--quiet", "--detach", sourceState.head], "detach fixture at source HEAD");
  assert.equal(
    gitOk(fixtureRoot, ["rev-parse", "--abbrev-ref", "HEAD"], "verify detached fixture").trim(),
    "HEAD",
    "fixture must exercise setup from a detached source commit",
  );

  const emptyHooks = join(sandbox, "empty-hooks");
  mkdirSync(emptyHooks);
  gitOk(fixtureRoot, ["config", "user.name", "Bridge merge-driver test"]);
  gitOk(fixtureRoot, ["config", "user.email", "bridge-merge-driver-test@example.invalid"]);
  gitOk(fixtureRoot, ["config", "commit.gpgSign", "false"]);
  gitOk(fixtureRoot, ["config", "tag.gpgSign", "false"]);
  gitOk(fixtureRoot, ["config", "core.hooksPath", emptyHooks]);
  gitOk(fixtureRoot, ["config", "merge.bridge-compat-reseal.driver", DRIVER_COMMAND]);
  gitOk(fixtureRoot, ["config", "merge.bridge-compat-reseal.name", "test run"]);

  const testRoot = fixtureRoot;
  const suffix = randomBytes(4).toString("hex");
  const base = `test/reseal-driver-base-${suffix}`;
  const branchA = `test/reseal-driver-a-${suffix}`;
  const branchB = `test/reseal-driver-b-${suffix}`;
  const mergeBranch = `test/reseal-driver-merge-${suffix}`;
  const conflictA = `test/reseal-driver-conflict-a-${suffix}`;
  const conflictB = `test/reseal-driver-conflict-b-${suffix}`;
  const conflictMerge = `test/reseal-driver-conflict-merge-${suffix}`;

  // Scoped to the disposable clone's local config only (never --global); this mirrors
  // exactly the one-time setup step documented in docs/release-process.md,
  // done here instead of trusting ambient config so the test is
  // self-contained.
  gitOk(testRoot, ["checkout", "-b", base]);

  await t.test("disjoint pinned files merge cleanly with byte-correct hashes", () => {
    gitOk(testRoot, ["checkout", "-b", branchA, base]);
    appendLine(testRoot, "docs/adr/0014-tally-native-outstandings-probe-authority.md", `test ${suffix} branch A`);
    reseal(testRoot);
    gitOk(testRoot, ["add", "docs/adr/0014-tally-native-outstandings-probe-authority.md", SURFACE, MATRIX]);
    gitOk(testRoot, ["commit", "-m", `test: branch A ${suffix}`]);

    gitOk(testRoot, ["checkout", "-b", branchB, base]);
    appendLine(testRoot, "docs/adr/0015-tally-selected-read-qualification-authority.md", `test ${suffix} branch B`);
    reseal(testRoot);
    gitOk(testRoot, ["add", "docs/adr/0015-tally-selected-read-qualification-authority.md", SURFACE, MATRIX]);
    gitOk(testRoot, ["commit", "-m", `test: branch B ${suffix}`]);

    gitOk(testRoot, ["checkout", "-b", mergeBranch, branchA]);
    const merge = git(testRoot, ["merge", branchB, "--no-edit"]);
    assert.equal(merge.status, 0, `expected a clean merge; stdout:\n${merge.stdout}\nstderr:\n${merge.stderr}`);

    const conflictMarkers = git(testRoot, ["grep", "-l", "-e", "<<<<<<<", "--", SURFACE, MATRIX]);
    assert.equal(conflictMarkers.status, 1, "expected no conflict markers in either resealed file");

    const mismatches = hashMismatches(testRoot);
    assert.deepEqual(mismatches, [], `every pinned file's recorded hash must match its real bytes: ${JSON.stringify(mismatches)}`);

    reseal(testRoot, "--verify");
  });

  await t.test("both sides changing the same pinned file's content is refused, not guessed at", () => {
    gitOk(testRoot, ["checkout", "-b", conflictA, base]);
    appendLine(testRoot, "docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict A`);
    reseal(testRoot);
    gitOk(testRoot, ["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    gitOk(testRoot, ["commit", "-m", `test: conflict A ${suffix}`]);

    gitOk(testRoot, ["checkout", "-b", conflictB, base]);
    appendLine(testRoot, "docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict B (different)`);
    reseal(testRoot);
    gitOk(testRoot, ["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    gitOk(testRoot, ["commit", "-m", `test: conflict B ${suffix}`]);

    gitOk(testRoot, ["checkout", "-b", conflictMerge, conflictA]);
    const merge = git(testRoot, ["merge", conflictB, "--no-edit"]);
    assert.notEqual(merge.status, 0, "expected the merge to stop with conflicts");
    assert.match(merge.stderr ?? "", /reseal-merge-driver: refusing to auto-resolve/);

    for (const file of [SURFACE, MATRIX]) {
      const content = readFileSync(join(testRoot, file), "utf8");
      assert.match(content, /^<<<<<<< /m, `${file} must be left with ordinary conflict markers for manual resolution`);
    }
    git(testRoot, ["merge", "--abort"]);
  });
});

function appendLine(root, relativePath, text) {
  const path = join(root, relativePath);
  appendFileSync(path, `\n<!-- ${text} -->\n`);
}

// Deliberately Node's own crypto rather than shelling out to shasum/
// sha256sum: which of those exists differs by platform (macOS ships
// `shasum`, most Linux distros `sha256sum`), and this needs to run wherever
// the pinned toolchain does.
function hashMismatches(root) {
  const surface = JSON.parse(readFileSync(join(root, SURFACE), "utf8"));
  const mismatches = [];
  for (const file of surface.files) {
    let actual;
    try {
      actual = createHash("sha256").update(readFileSync(join(root, file.path))).digest("hex");
    } catch {
      mismatches.push({ path: file.path, error: "unreadable" });
      continue;
    }
    if (actual !== file.sha256) mismatches.push({ path: file.path, expected: file.sha256, actual });
  }
  return mismatches;
}


// Small real Git repositories exercise source reads even without Rust installed.
function sourceReadFixture(t) {
  const root = mkdtempSync(join(tmpdir(), "bridge-reseal-source-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  gitOk(root, ["init", "--quiet"]);
  gitOk(root, ["config", "user.name", "Source read test"]);
  gitOk(root, ["config", "user.email", "source-read@example.invalid"]);
  writeFileSync(join(root, "rust-toolchain.toml"), '[toolchain]\nchannel = "bridge-unavailable-regression-toolchain"\n');
  gitOk(root, ["add", "rust-toolchain.toml"]);
  gitOk(root, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "source fixture"]);
  return root;
}

test("source state leaves index bytes unchanged after a stat-only file change", (t) => {
  const root = sourceReadFixture(t);
  const file = join(root, "rust-toolchain.toml");
  const index = join(root, ".git", "index");
  const before = readFileSync(index);
  utimesSync(file, new Date("2001-09-09T01:46:40Z"), new Date("2001-09-09T01:46:40Z"));
  const state = repositoryState(root);
  assert.equal(state.status, "");
  assert.deepEqual(readFileSync(index), before, "source status must not refresh index bytes");
});

test("toolchain preflight reads the captured revision despite dirty or missing source file", (t) => {
  const root = sourceReadFixture(t);
  const revision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const file = join(root, "rust-toolchain.toml");
  const committed = pinnedToolchainAvailable(root, revision);
  assert.equal(committed.ok, false);
  assert.match(committed.reason, /bridge-unavailable-regression-toolchain/);
  writeFileSync(file, '[toolchain]\nchannel = "1.96.0"\n');
  assert.deepEqual(pinnedToolchainAvailable(root, revision), committed);
  rmSync(file);
  assert.deepEqual(pinnedToolchainAvailable(root, revision), committed);

  writeFileSync(file, '[toolchain]\nchannel = "1.96.0"\n');
  gitOk(root, ["add", "rust-toolchain.toml"]);
  gitOk(root, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "alternate toolchain"]);
  const alternateRevision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const alternate = pinnedToolchainAvailable(root, alternateRevision);
  writeFileSync(file, '[toolchain]\nchannel = "bridge-unavailable-regression-toolchain"\n');
  assert.deepEqual(pinnedToolchainAvailable(root, alternateRevision), alternate);
  assert.deepEqual(pinnedToolchainAvailable(root, revision), committed, "captured revision remains authoritative after HEAD moves");
});


test("a failed committed-toolchain read fails instead of skipping the suite", (t) => {
  const root = sourceReadFixture(t);
  assert.throws(
    () => pinnedToolchainAvailable(root, "missing-revision"),
    /read committed toolchain failed/,
  );
});
