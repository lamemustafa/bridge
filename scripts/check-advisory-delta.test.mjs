#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/check-advisory-delta.mjs.
//
// These deliberately run against REAL repository history rather than a
// synthetic fixture: commit 8b17f95c is the actual pre-fix state that had
// RUSTSEC-2026-0285 (rustls) as a live `cargo audit` error on 2026-09-15, and
// 9558a316 is the very next commit, which patched it (see that commit's own
// message for the incident). Using real history means these tests prove the
// script would have correctly classified the actual PR that fixed this, in
// both directions -- not just some hypothetical id string.
//
// A false negative here is the dangerous failure mode: a gate that silently
// never fires provides no protection while looking exactly like one that
// does, which is the whole risk category PROBLEM 1 exists to close. So this
// file tests the firing direction, not only the passing one.
//
// `corepack pnpm test` runs every scripts/*.test.mjs in the "Frontend build"
// CI job (.github/workflows/ci.yml), which has no Rust toolchain and does not
// install cargo-audit -- see .github/workflows/dependency-security.yml for
// the job that does. These tests need `cargo audit` (any lockfile-scanning
// invocation, not the pinned 1.96.0 toolchain reseal.sh needs) and skip
// themselves with a clear reason when it is unavailable, rather than failing
// a job that was never given the tool. See docs/proposed-dependency-policy.md
// for the proposed CI wiring that would let them actually run.
//
// Run: node scripts/check-advisory-delta.test.mjs

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join, dirname } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const SCRIPT = join(here, "check-advisory-delta.mjs");

const FIX_COMMIT = "9558a316a21bad1dce0b90ea83734e3dcc8557eb"; // fix(deps): patch rustls for RUSTSEC-2026-0285 (#381)
const PRE_FIX_COMMIT = "8b17f95cd0de6a0258f6765eda410bc10ca8fd1f"; // its parent

function cargoAuditAvailable() {
  const version = spawnSync("cargo", ["audit", "--version"], { encoding: "utf8" });
  if (version.error || version.status !== 0) {
    return { ok: false, reason: "`cargo audit` is not available (cargo-audit not installed)" };
  }
  return { ok: true, reason: null };
}

function commitsPresent() {
  for (const commit of [FIX_COMMIT, PRE_FIX_COMMIT]) {
    const result = spawnSync("git", ["cat-file", "-e", commit], { cwd: repoRoot, encoding: "utf8" });
    if (result.status !== 0) {
      return { ok: false, reason: `fixture commit ${commit} is not reachable (shallow clone?)` };
    }
  }
  return { ok: true, reason: null };
}

const audit = cargoAuditAvailable();
const commits = audit.ok ? commitsPresent() : { ok: false, reason: null };
const unavailable = !audit.ok ? audit.reason : !commits.ok ? commits.reason : null;
const skip = unavailable ? `prerequisite unavailable: ${unavailable}` : false;

function run(args) {
  return spawnSync(process.execPath, [SCRIPT, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

test(
  "fires on the exact known-vulnerable commit pair, in the direction that introduces the advisory",
  { skip },
  () => {
    // base = the FIXED state, head = the PRE-FIX state: head's Cargo.lock
    // genuinely carries an advisory (RUSTSEC-2026-0285) that base's does not,
    // so this must fail. (Backwards from a real PR's chronology on purpose --
    // see the file header for why real history is used at all -- the
    // direction of "which side lacks the finding" is what the script checks,
    // and this is the one real pair of commits on this repository's history
    // that differs by exactly one known, named advisory.)
    const result = run(["--base", FIX_COMMIT, "--head", PRE_FIX_COMMIT]);
    assert.notEqual(result.status, 0, `expected failure; stdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
    assert.match(result.stderr, /RUSTSEC-2026-0285/);
    assert.match(result.stderr, /introduces 1 new advisory finding/);
  },
);

test("passes in the real chronological direction, where the advisory is only removed", { skip }, () => {
  // base = PRE-FIX, head = FIXED: this is the actual PR, and it must not be
  // reported as introducing anything -- it removes a finding, which is
  // informational (logged) but not a failure.
  const result = run(["--base", PRE_FIX_COMMIT, "--head", FIX_COMMIT]);
  assert.equal(result.status, 0, `expected success; stdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
  assert.match(result.stdout, /no longer present at HEAD/);
  assert.match(result.stdout, /RUSTSEC-2026-0285/);
  assert.match(result.stdout, /no new advisory findings/);
});

test("passes with no findings to report when base and head share the same lockfile", { skip }, () => {
  const result = run(["--base", FIX_COMMIT, "--head", FIX_COMMIT]);
  assert.equal(result.status, 0, `expected success; stdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
  assert.match(result.stdout, /byte-identical/);
});

test("a bad --base ref fails with a clear error rather than a stack trace on `undefined`", { skip }, () => {
  const result = run(["--base", "not-a-real-ref-xyz", "--head", FIX_COMMIT]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not resolve to a commit/);
});
