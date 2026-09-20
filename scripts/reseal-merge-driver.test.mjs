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
// merge` behaves. So these tests initialize an empty-template repository,
// shallow-fetch the source checkout's captured committed HEAD, start from that
// exact commit and tree detached, and perform every config/branch/merge/reseal
// mutation in the fixture. The source checkout is read only and may itself be
// detached or dirty; uncommitted source bytes are deliberately not exercised.
//
// `corepack pnpm test` runs every scripts/*.test.mjs in the "Frontend build"
// CI job, where the real Git suite may skip when pinned Rust is absent. The
// separate required "Workflow consistency" job installs pinned Rust, requires
// it to resolve, and runs this file directly, so that job must execute the real
// suite without skips.
//
// If the host's normal temporary filesystem is mounted noexec, point
// BRIDGE_RESEAL_TEST_EXEC_ROOT at a private executable directory. The fixture,
// Cargo target and generated helper binaries then stay below that root. This
// configuration support does not by itself qualify a real noexec host.
//
// Run: node scripts/reseal-merge-driver.test.mjs

import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { devNull, tmpdir } from "node:os";
import { dirname, isAbsolute, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = realpathSync(join(dirname(fileURLToPath(import.meta.url)), ".."));
const SURFACE = "docs/tally/compatibility/compatibility-surface.json";
const MATRIX = "docs/tally/compatibility/compatibility-matrix.json";
const DRIVER_COMMAND = "node scripts/reseal-merge-driver.mjs %O %A %B %P %S %X %Y";
const EXEC_ROOT_ENV = "BRIDGE_RESEAL_TEST_EXEC_ROOT";

function scratchRoot(environment = process.env) {
  const configured = environment[EXEC_ROOT_ENV];
  if (!configured) return tmpdir();
  mkdirSync(configured, { recursive: true });
  return realpathSync(configured);
}

function makeSandbox(prefix, environment = process.env) {
  return mkdtempSync(join(scratchRoot(environment), prefix));
}

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
function sourceGitEnvironment(sourceRoot, environment = gitEnv) {
  return {
    ...environment,
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: "safe.directory",
    GIT_CONFIG_VALUE_0: realpathSync(sourceRoot),
  };
}

const sourceGitEnv = sourceGitEnvironment(repoRoot);

function pinnedToolchainAvailable(root, revision, env = gitEnv) {
  // A failed source read is a test failure, never evidence that Rust is unavailable.
  const tomlText = gitOk(root, ["show", `${revision}:rust-toolchain.toml`], "read committed toolchain", env);
  const match = /^channel *= *"(.*)"/m.exec(tomlText);
  assert.ok(match, "committed rust-toolchain.toml has no parseable channel");
  const channel = match[1];
  // `rustup which --toolchain <channel>` may synchronize a missing named
  // channel. Check the local inventory first and only resolve a toolchain that
  // is already installed. The required CI job separately installs the real
  // pinned channel before invoking this suite.
  const listed = spawnSync("rustup", ["toolchain", "list"], { encoding: "utf8", env });
  if (listed.error || listed.status !== 0) {
    return { ok: false, reason: "rustup could not list installed toolchains" };
  }
  const installed = listed.stdout
    .split("\n")
    .map((line) => line.trim().split(/\s+/, 1)[0])
    .filter(Boolean);
  const officialChannel = /^(?:stable|beta|nightly|[0-9]+\.[0-9]+(?:\.[0-9]+)?)(?:-[0-9]{4}-[0-9]{2}-[0-9]{2})?$/.test(channel);
  const installedName = installed.find(
    (name) => name === channel || (officialChannel && name.startsWith(`${channel}-`)),
  );
  if (!installedName) {
    return { ok: false, reason: `rustup toolchain ${channel} is not installed` };
  }
  // Resolve the exact installed inventory entry, never the uninstalled channel
  // spelling. This keeps the lookup local even for host-qualified toolchains.
  const which = spawnSync("rustup", ["which", "--toolchain", installedName, "rustc"], { encoding: "utf8", env });
  if (which.error || which.status !== 0) {
    return { ok: false, reason: `rustup toolchain ${channel} is not installed` };
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

function resealInEnvironment(root, env, ...args) {
  const result = spawnSync("bash", [join(root, "scripts", "reseal.sh"), ...args], {
    cwd: root,
    encoding: "utf8",
    env,
  });
  assert.equal(result.status, 0, `scripts/reseal.sh failed:\n${result.stderr}`);
}

function sourceIndexPath(root, env) {
  const path = gitOk(root, ["rev-parse", "--git-path", "index"], "resolve source index", env).trim();
  return isAbsolute(path) ? path : join(root, path);
}

function workingFilesSha256(root, env) {
  const listed = git(root, ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    encoding: null,
    env,
  });
  assert.equal(listed.status, 0, `list source files failed:\n${listed.stderr.toString("utf8")}`);
  const paths = listed.stdout.toString("utf8").split("\0").filter(Boolean).sort();
  const hash = createHash("sha256");
  for (const path of paths) {
    const absolute = join(root, path);
    hash.update(path);
    hash.update("\0");
    let stat;
    try {
      stat = lstatSync(absolute);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      hash.update("missing\0");
      continue;
    }
    hash.update(String(stat.mode));
    hash.update("\0");
    if (stat.isSymbolicLink()) hash.update(readlinkSync(absolute));
    else if (stat.isFile()) hash.update(readFileSync(absolute));
    else hash.update("non-file");
    hash.update("\0");
  }
  return hash.digest("hex");
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
    tree: gitOk(root, ["rev-parse", "HEAD^{tree}"], "capture tree", env).trim(),
    symbolicHeadStatus: symbolicHead.status,
    symbolicHead: symbolicHead.stdout,
    status: gitOk(root, ["--no-optional-locks", "status", "--porcelain=v1", "-z", "--untracked-files=all"], "capture status", env),
    indexSha256: createHash("sha256").update(readFileSync(sourceIndexPath(root, env))).digest("hex"),
    workingFilesSha256: workingFilesSha256(root, env),
    localConfigSha256: createHash("sha256").update(localConfig.stdout).digest("hex"),
  };
}

function checkoutCapturedSource(
  sourceRoot,
  fixtureRoot,
  captured,
  emptyTemplate,
  env = sourceGitEnvironment(sourceRoot),
) {
  gitOk(dirname(fixtureRoot), ["init", "--quiet", `--template=${emptyTemplate}`, fixtureRoot], "initialize fixture", env);
  gitOk(
    fixtureRoot,
    [
      "-c",
      `safe.directory=${realpathSync(fixtureRoot)}`,
      "fetch",
      "--quiet",
      "--no-tags",
      "--depth=1",
      sourceRoot,
      captured.head,
    ],
    "fetch captured source",
    env,
  );
  gitOk(fixtureRoot, ["checkout", "--quiet", "--detach", captured.head], "detach fixture at source HEAD");
  assert.equal(gitOk(fixtureRoot, ["rev-parse", "HEAD"], "verify captured commit").trim(), captured.head);
  assert.equal(gitOk(fixtureRoot, ["rev-parse", "HEAD^{tree}"], "verify captured tree").trim(), captured.tree);
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
  const sandbox = makeSandbox("bridge-reseal-driver-");
  const fixtureRoot = join(sandbox, "repo");
  const emptyTemplate = join(sandbox, "empty-template");
  const fixtureEnv = {
    ...gitEnv,
    CARGO_TARGET_DIR: join(sandbox, "cargo-target"),
  };
  const fixtureSourceEnv = sourceGitEnvironment(repoRoot, fixtureEnv);
  mkdirSync(emptyTemplate);

  t.after(() => {
    rmSync(sandbox, { recursive: true, force: true });
    assert.deepEqual(
      repositoryState(repoRoot, sourceGitEnv),
      sourceState,
      "merge-driver test must preserve source HEAD/tree, index, files, status, and local config",
    );
  });

  checkoutCapturedSource(repoRoot, fixtureRoot, sourceState, emptyTemplate, fixtureSourceEnv);
  assert.equal(
    gitOk(fixtureRoot, ["rev-parse", "--abbrev-ref", "HEAD"], "verify detached fixture").trim(),
    "HEAD",
    "fixture must exercise setup from a detached source commit",
  );

  const emptyHooks = join(sandbox, "empty-hooks");
  mkdirSync(emptyHooks);
  gitOk(fixtureRoot, ["config", "user.name", "Bridge merge-driver test"], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "user.email", "bridge-merge-driver-test@example.invalid"], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "commit.gpgSign", "false"], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "tag.gpgSign", "false"], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "core.hooksPath", emptyHooks], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "merge.bridge-compat-reseal.driver", DRIVER_COMMAND], null, fixtureEnv);
  gitOk(fixtureRoot, ["config", "merge.bridge-compat-reseal.name", "test run"], null, fixtureEnv);

  const testRoot = fixtureRoot;
  const fixtureGitOk = (args, label) => gitOk(testRoot, args, label, fixtureEnv);
  const fixtureGit = (args, options = {}) => git(testRoot, args, { ...options, env: fixtureEnv });
  const fixtureReseal = (...args) => resealInEnvironment(testRoot, fixtureEnv, ...args);
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
  fixtureGitOk(["checkout", "-b", base]);

  await t.test("disjoint pinned files merge cleanly with byte-correct hashes", () => {
    fixtureGitOk(["checkout", "-b", branchA, base]);
    appendLine(testRoot, "docs/adr/0014-tally-native-outstandings-probe-authority.md", `test ${suffix} branch A`);
    fixtureReseal();
    fixtureGitOk(["add", "docs/adr/0014-tally-native-outstandings-probe-authority.md", SURFACE, MATRIX]);
    fixtureGitOk(["commit", "-m", `test: branch A ${suffix}`]);

    fixtureGitOk(["checkout", "-b", branchB, base]);
    appendLine(testRoot, "docs/adr/0015-tally-selected-read-qualification-authority.md", `test ${suffix} branch B`);
    fixtureReseal();
    fixtureGitOk(["add", "docs/adr/0015-tally-selected-read-qualification-authority.md", SURFACE, MATRIX]);
    fixtureGitOk(["commit", "-m", `test: branch B ${suffix}`]);

    fixtureGitOk(["checkout", "-b", mergeBranch, branchA]);
    const merge = fixtureGit(["merge", branchB, "--no-edit"]);
    assert.equal(merge.status, 0, `expected a clean merge; stdout:\n${merge.stdout}\nstderr:\n${merge.stderr}`);

    const conflictMarkers = fixtureGit(["grep", "-l", "-e", "<<<<<<<", "--", SURFACE, MATRIX]);
    assert.equal(conflictMarkers.status, 1, "expected no conflict markers in either resealed file");

    const mismatches = hashMismatches(testRoot);
    assert.deepEqual(mismatches, [], `every pinned file's recorded hash must match its real bytes: ${JSON.stringify(mismatches)}`);

    fixtureReseal("--verify");
  });

  await t.test("both sides changing the same pinned file's content is refused, not guessed at", () => {
    fixtureGitOk(["checkout", "-b", conflictA, base]);
    appendLine(testRoot, "docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict A`);
    fixtureReseal();
    fixtureGitOk(["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    fixtureGitOk(["commit", "-m", `test: conflict A ${suffix}`]);

    fixtureGitOk(["checkout", "-b", conflictB, base]);
    appendLine(testRoot, "docs/adr/0004-tally-write-safety.md", `test ${suffix} conflict B (different)`);
    fixtureReseal();
    fixtureGitOk(["add", "docs/adr/0004-tally-write-safety.md", SURFACE, MATRIX]);
    fixtureGitOk(["commit", "-m", `test: conflict B ${suffix}`]);

    fixtureGitOk(["checkout", "-b", conflictMerge, conflictA]);
    const merge = fixtureGit(["merge", conflictB, "--no-edit"]);
    assert.notEqual(merge.status, 0, "expected the merge to stop with conflicts");
    assert.match(merge.stderr ?? "", /reseal-merge-driver: refusing to auto-resolve/);

    for (const file of [SURFACE, MATRIX]) {
      const content = readFileSync(join(testRoot, file), "utf8");
      assert.match(content, /^<<<<<<< /m, `${file} must be left with ordinary conflict markers for manual resolution`);
    }
    fixtureGit(["merge", "--abort"]);
  });

  assert.equal(
    existsSync(fixtureEnv.CARGO_TARGET_DIR),
    true,
    "real Cargo outputs must remain below the selected executable scratch root",
  );
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


function withGitConfig(environment, key, value) {
  const count = Number(environment.GIT_CONFIG_COUNT ?? "0");
  return {
    ...environment,
    GIT_CONFIG_COUNT: String(count + 1),
    [`GIT_CONFIG_KEY_${count}`]: key,
    [`GIT_CONFIG_VALUE_${count}`]: value,
  };
}

function localRustupFixture(t, installedToolchains = []) {
  const sandbox = makeSandbox("bridge-local-rustup-");
  const bin = join(sandbox, "bin");
  const log = join(sandbox, "rustup.log");
  mkdirSync(bin);
  writeFileSync(
    join(bin, "rustup"),
    `#!/bin/sh
printf '%s\\n' "$*" >> "$BRIDGE_RUSTUP_LOG"
if [ "$1 $2" = "toolchain list" ]; then
  printf '%s\\n' "$BRIDGE_RUSTUP_TOOLCHAINS"
  exit 0
fi
if [ "$1" = "which" ]; then
  printf '%s\\n' "/synthetic/local/toolchain/$4"
  exit 0
fi
exit 97
`,
  );
  chmodSync(join(bin, "rustup"), 0o755);
  writeFileSync(log, "");
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  return {
    env: {
      ...gitEnv,
      PATH: `${bin}:${process.env.PATH ?? ""}`,
      BRIDGE_RUSTUP_LOG: log,
      BRIDGE_RUSTUP_TOOLCHAINS: installedToolchains.join("\n"),
    },
    log,
  };
}

// Small real Git repositories exercise source reads even without Rust installed.
function sourceReadFixture(t, channel = "bridge-unavailable-regression-toolchain") {
  const sandbox = makeSandbox("bridge-reseal-source-");
  const root = join(sandbox, "repo");
  const emptyTemplate = join(sandbox, "empty-template");
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  mkdirSync(emptyTemplate);
  gitOk(sandbox, ["init", "--quiet", `--template=${emptyTemplate}`, root]);
  gitOk(root, ["config", "user.name", "Source read test"]);
  gitOk(root, ["config", "user.email", "source-read@example.invalid"]);
  writeFileSync(join(root, "rust-toolchain.toml"), `[toolchain]\nchannel = "${channel}"\n`);
  gitOk(root, ["add", "rust-toolchain.toml"]);
  gitOk(root, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "source fixture"]);
  return root;
}

test("an explicit execution root owns executable scratch and build files", (t) => {
  const parent = makeSandbox("bridge-exec-root-control-");
  const configured = join(parent, "configured");
  mkdirSync(configured);
  const sandbox = makeSandbox("bridge-explicit-exec-", {
    ...process.env,
    [EXEC_ROOT_ENV]: configured,
  });
  t.after(() => rmSync(parent, { recursive: true, force: true }));
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  assert.equal(dirname(sandbox), realpathSync(configured));
  const executable = join(sandbox, "control.sh");
  writeFileSync(executable, "#!/bin/sh\nexit 0\n");
  chmodSync(executable, 0o755);
  assert.equal(spawnSync(executable).status, 0, "configured execution root must run a real helper on this host");
});

test("captured checkout ignores an ambient Git template from repository creation onward", (t) => {
  const source = sourceReadFixture(t);
  const captured = repositoryState(source);
  const sandbox = makeSandbox("bridge-template-control-");
  const hostileTemplate = join(sandbox, "hostile-template");
  const emptyTemplate = join(sandbox, "empty-template");
  const marker = join(sandbox, "checkout-hook-ran");
  const fixture = join(sandbox, "fixture");
  mkdirSync(join(hostileTemplate, "hooks"), { recursive: true });
  mkdirSync(emptyTemplate);
  writeFileSync(join(hostileTemplate, "hooks", "post-checkout"), `#!/bin/sh\nprintf ran > "${marker}"\n`);
  chmodSync(join(hostileTemplate, "hooks", "post-checkout"), 0o755);
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));

  const hostileEnv = withGitConfig(sourceGitEnvironment(source), "init.templateDir", hostileTemplate);
  checkoutCapturedSource(source, fixture, captured, emptyTemplate, hostileEnv);
  assert.equal(existsSync(marker), false, "ambient checkout hook must never be installed or run");
  assert.equal(existsSync(join(fixture, ".git", "hooks", "post-checkout")), false);
});

test("captured checkout does not require an unrelated missing historical blob", (t) => {
  const source = sourceReadFixture(t);
  const historical = join(source, "historical.txt");
  writeFileSync(historical, "unrelated historical blob\n");
  gitOk(source, ["add", "historical.txt"]);
  gitOk(source, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "historical blob"]);
  const historicalBlob = gitOk(source, ["rev-parse", "HEAD:historical.txt"]).trim();
  rmSync(historical);
  gitOk(source, ["add", "-u"]);
  gitOk(source, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "current tree"]);
  const captured = repositoryState(source);
  rmSync(join(source, ".git", "objects", historicalBlob.slice(0, 2), historicalBlob.slice(2)));

  const sandbox = makeSandbox("bridge-partial-control-");
  const emptyTemplate = join(sandbox, "empty-template");
  mkdirSync(emptyTemplate);
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  checkoutCapturedSource(source, join(sandbox, "fixture"), captured, emptyTemplate);
});

test("an unavailable committed toolchain is checked locally without resolving it", (t) => {
  const root = sourceReadFixture(t);
  const revision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const localRustup = localRustupFixture(t);
  const result = pinnedToolchainAvailable(root, revision, localRustup.env);
  assert.equal(result.ok, false);
  assert.match(result.reason, /bridge-unavailable-regression-toolchain/);
  assert.deepEqual(readFileSync(localRustup.log, "utf8").trim().split("\n"), ["toolchain list"]);
});

test("a locally installed committed toolchain resolves only after the inventory check", (t) => {
  const channel = "bridge-installed-regression-toolchain";
  const root = sourceReadFixture(t, channel);
  const revision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const localRustup = localRustupFixture(t, [channel]);
  assert.deepEqual(pinnedToolchainAvailable(root, revision, localRustup.env), { ok: true, reason: null });
  assert.deepEqual(readFileSync(localRustup.log, "utf8").trim().split("\n"), [
    "toolchain list",
    `which --toolchain ${channel} rustc`,
  ]);
});

test("captured checkout remains bound after the source branch advances", (t) => {
  const source = sourceReadFixture(t);
  const captured = repositoryState(source);
  writeFileSync(join(source, "later.txt"), "later branch state\n");
  gitOk(source, ["add", "later.txt"]);
  gitOk(source, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "advance source branch"]);

  const sandbox = makeSandbox("bridge-captured-revision-");
  const emptyTemplate = join(sandbox, "empty-template");
  const fixture = join(sandbox, "fixture");
  mkdirSync(emptyTemplate);
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  checkoutCapturedSource(source, fixture, captured, emptyTemplate);
  assert.equal(existsSync(join(fixture, "later.txt")), false);
});

test("captured checkout trusts only its differently-owned source path", (t) => {
  const source = sourceReadFixture(t);
  const ownerSimulation = { ...gitEnv, GIT_TEST_ASSUME_DIFFERENT_OWNER: "1" };
  const trusted = sourceGitEnvironment(source, ownerSimulation);
  const before = repositoryState(source, trusted);
  const sandbox = makeSandbox("bridge-owner-control-");
  const emptyTemplate = join(sandbox, "empty-template");
  mkdirSync(emptyTemplate);
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  checkoutCapturedSource(source, join(sandbox, "fixture"), before, emptyTemplate, trusted);
  assert.deepEqual(repositoryState(source, trusted), before);

  const wrongTrust = sourceGitEnvironment(sandbox, ownerSimulation);
  const refused = git(source, ["rev-parse", "HEAD"], { env: wrongTrust });
  assert.notEqual(refused.status, 0);
  assert.match(refused.stderr, /dubious ownership/);
});

test("Git environment sanitization removes inherited overrides case-insensitively", () => {
  const sanitized = sanitizedGitEnvironment({
    ...process.env,
    gIt_DiR: "/synthetic/hostile/git-dir",
    Git_Work_Tree: "/synthetic/hostile/work-tree",
    git_index_file: "/synthetic/hostile/index",
    Git_Config_Count: "7",
    BRIDGE_UNRELATED_CONTROL: "preserved",
  });
  for (const key of ["gIt_DiR", "Git_Work_Tree", "git_index_file", "Git_Config_Count"]) {
    assert.equal(sanitized[key], undefined, `${key} must not reach scratch Git commands`);
  }
  assert.equal(sanitized.BRIDGE_UNRELATED_CONTROL, "preserved");
});

test("captured checkout preserves dirty, deleted, and untracked source bytes", (t) => {
  const source = sourceReadFixture(t);
  const clean = repositoryState(source);
  rmSync(join(source, "rust-toolchain.toml"));
  writeFileSync(join(source, "untracked.txt"), "untracked source bytes\n");
  const dirty = repositoryState(source);
  assert.notEqual(dirty.workingFilesSha256, clean.workingFilesSha256);
  assert.match(dirty.status, /rust-toolchain\.toml/);
  assert.match(dirty.status, /untracked\.txt/);

  const sandbox = makeSandbox("bridge-dirty-source-");
  const emptyTemplate = join(sandbox, "empty-template");
  mkdirSync(emptyTemplate);
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  checkoutCapturedSource(source, join(sandbox, "fixture"), dirty, emptyTemplate);
  assert.deepEqual(repositoryState(source), dirty);
});

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
  const originalChannel = "bridge-unavailable-regression-toolchain";
  const alternateChannel = "bridge-installed-regression-toolchain";
  const root = sourceReadFixture(t, originalChannel);
  const revision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const localRustup = localRustupFixture(t, [alternateChannel]);
  const file = join(root, "rust-toolchain.toml");
  const committed = pinnedToolchainAvailable(root, revision, localRustup.env);
  assert.equal(committed.ok, false);
  assert.match(committed.reason, new RegExp(originalChannel));
  writeFileSync(file, `[toolchain]\nchannel = "${alternateChannel}"\n`);
  assert.deepEqual(pinnedToolchainAvailable(root, revision, localRustup.env), committed);
  rmSync(file);
  assert.deepEqual(pinnedToolchainAvailable(root, revision, localRustup.env), committed);

  writeFileSync(file, `[toolchain]\nchannel = "${alternateChannel}"\n`);
  gitOk(root, ["add", "rust-toolchain.toml"]);
  gitOk(root, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "alternate toolchain"]);
  const alternateRevision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  const alternate = pinnedToolchainAvailable(root, alternateRevision, localRustup.env);
  assert.deepEqual(alternate, { ok: true, reason: null });
  writeFileSync(file, `[toolchain]\nchannel = "${originalChannel}"\n`);
  assert.deepEqual(pinnedToolchainAvailable(root, alternateRevision, localRustup.env), alternate);
  assert.deepEqual(
    pinnedToolchainAvailable(root, revision, localRustup.env),
    committed,
    "captured revision remains authoritative after HEAD moves",
  );
});


test("a failed committed-toolchain read fails instead of skipping the suite", (t) => {
  const root = sourceReadFixture(t);
  assert.throws(
    () => pinnedToolchainAvailable(root, "missing-revision"),
    /read committed toolchain failed/,
  );
});


test("a malformed committed toolchain fails instead of skipping the suite", (t) => {
  const root = sourceReadFixture(t);
  writeFileSync(join(root, "rust-toolchain.toml"), "[toolchain]\n");
  gitOk(root, ["add", "rust-toolchain.toml"]);
  gitOk(root, ["-c", "commit.gpgSign=false", "commit", "--quiet", "-m", "malformed toolchain"]);
  const revision = gitOk(root, ["rev-parse", "HEAD"]).trim();
  assert.throws(() => pinnedToolchainAvailable(root, revision), /no parseable channel/);
});
