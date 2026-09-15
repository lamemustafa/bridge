#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/reseal.sh.
//
// These run the real script against a fully sandboxed fixture -- not the
// repository's actual compatibility-surface.json / compatibility-matrix.json
// -- built from a couple of throwaway files under a temp directory. That lets
// the tests mutate pinned-file content and force real drift without ever
// touching the committed compatibility artifacts.
//
// The wrapper's entire reason to exist is enforcing an order dependency
// (rehash-surface before seal-surface) that a maintainer running the three
// commands by hand can get wrong silently -- so these tests exercise the
// tool for real (compiling and running `bridge-tally-compatibility` through
// the pinned toolchain), not just the shell scripting around it. A test that
// stubs the tool out would prove the wrapper calls *something* in the right
// order, not that reordering the calls actually changes the result.
//
// Run: node scripts/reseal.test.mjs
// (needs the pinned Rust toolchain from rust-toolchain.toml available via
// rustup, same as scripts/reseal.sh itself)
//
// `corepack pnpm test` runs every scripts/*.test.mjs from Node's built-in
// test runner in the "Frontend build" CI job (.github/workflows/ci.yml),
// which has no Rust toolchain at all -- no rustup, no cargo. These tests
// compile and run the real bridge-tally-compatibility tool (see the header
// above: a stubbed tool would not prove the ordering actually matters), so
// they cannot run there. Rather than fail that job outright over tooling it
// was never given, they skip themselves with an explicit reason when the
// pinned toolchain is not resolvable -- see docs/proposed-dependency-policy.md
// for the proposed CI job that would give them somewhere to actually run.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const RESEAL = join(here, "reseal.sh");

function pinnedToolchainAvailable() {
  let tomlText;
  try {
    tomlText = readFileSync(join(repoRoot, "rust-toolchain.toml"), "utf8");
  } catch {
    return { ok: false, reason: "rust-toolchain.toml not found" };
  }
  const match = /^channel *= *"(.*)"/m.exec(tomlText);
  if (!match) return { ok: false, reason: "could not read [toolchain].channel from rust-toolchain.toml" };
  const channel = match[1];
  const which = spawnSync("rustup", ["which", "--toolchain", channel, "rustc"], {
    encoding: "utf8",
  });
  if (which.error || which.status !== 0) {
    return { ok: false, reason: `rustup toolchain ${channel} is not installed (rustup which failed)` };
  }
  return { ok: true, reason: null };
}

const toolchain = pinnedToolchainAvailable();
const skip = toolchain.ok ? false : `pinned Rust toolchain unavailable: ${toolchain.reason}`;

const ZERO_SHA256 = "0".repeat(64);
const ZERO_COMMIT = "0".repeat(40);

function surfaceFixture() {
  return (
    JSON.stringify(
      {
        schema_version: 1,
        files: [
          { path: "a.txt", sha256: ZERO_SHA256 },
          { path: "b/c.txt", sha256: ZERO_SHA256 },
        ],
        manifest_sha256: "",
      },
      null,
      2,
    ) + "\n"
  );
}

function matrixFixture() {
  return (
    JSON.stringify(
      {
        schema_version: 1,
        bridge_commit_sha: ZERO_COMMIT,
        compatibility_surface_sha256: ZERO_SHA256,
        claims: [
          {
            claim_id: "test-claim-one",
            level: "unknown",
            promotion_eligible: true,
            product: "tally_erp9",
            release: "unknown",
            mode: "education",
            platform: "windows",
            architecture: "x86_64",
            transport: "xml_http",
            endpoint_family: "ipv4",
            odbc_state: "disabled",
            company_state: "one",
            locale: "english_india",
            encoding: "utf8",
            dataset_tier: "synthetic_small",
            fixture_manifest_sha256: null,
            required_profiles: [],
            max_evidence_age_days: 180,
            evidence_id: null,
          },
        ],
      },
      null,
      2,
    ) + "\n"
  );
}

async function makeSandbox() {
  const root = await mkdtemp(join(tmpdir(), "bridge-reseal-test-"));
  await mkdir(join(root, "b"), { recursive: true });
  await writeFile(join(root, "a.txt"), "one");
  await writeFile(join(root, "b", "c.txt"), "two");
  const surface = join(root, "surface.json");
  const matrix = join(root, "matrix.json");
  await writeFile(surface, surfaceFixture());
  await writeFile(matrix, matrixFixture());
  return { root, surface, matrix };
}

function run(args) {
  return spawnSync(RESEAL, args, { encoding: "utf8" });
}

test("running the ordinary order against an unsealed pin list fails closed and writes nothing", { skip }, async () => {
  const { root, surface, matrix } = await makeSandbox();
  try {
    const before = await readFile(surface, "utf8");
    const result = run(["--root", root, "--surface", surface, "--matrix", matrix]);
    assert.notEqual(result.status, 0, "must fail when the pin list was never sealed");
    const after = await readFile(surface, "utf8");
    assert.equal(after, before, "a failed step must not partially overwrite the destination");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("--pins-changed bootstraps a fresh pin list and rehashes every entry", { skip }, async () => {
  const { root, surface, matrix } = await makeSandbox();
  try {
    const result = run(["--pins-changed", "--root", root, "--surface", surface, "--matrix", matrix]);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /rehash-surface changed 2 pinned file hash\(es\)/);

    const sealed = JSON.parse(await readFile(surface, "utf8"));
    assert.notEqual(sealed.manifest_sha256, "");
    assert.notEqual(
      sealed.files.find((f) => f.path === "a.txt").sha256,
      ZERO_SHA256,
      "a.txt must be rehashed from its real bytes, not left at the placeholder",
    );

    const repointed = JSON.parse(await readFile(matrix, "utf8"));
    assert.equal(repointed.compatibility_surface_sha256, sealed.manifest_sha256);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("--verify passes on a freshly resealed fixture and fails once a pinned file drifts", { skip }, async () => {
  const { root, surface, matrix } = await makeSandbox();
  try {
    const bootstrap = run(["--pins-changed", "--root", root, "--surface", surface, "--matrix", matrix]);
    assert.equal(bootstrap.status, 0, bootstrap.stderr);

    const surfaceBefore = await readFile(surface, "utf8");
    const matrixBefore = await readFile(matrix, "utf8");

    const verifyClean = run(["--verify", "--root", root, "--surface", surface, "--matrix", matrix]);
    assert.equal(verifyClean.status, 0, verifyClean.stderr);
    assert.match(verifyClean.stdout, /compatibility surface and matrix are current/);
    // --verify must never mutate the files it checks.
    assert.equal(await readFile(surface, "utf8"), surfaceBefore);
    assert.equal(await readFile(matrix, "utf8"), matrixBefore);

    // Mutate a pinned file without resealing -- the exact scenario the gate
    // exists to catch (stale digest over changed source content).
    await writeFile(join(root, "a.txt"), "one-mutated");

    const verifyDirty = run(["--verify", "--root", root, "--surface", surface, "--matrix", matrix]);
    assert.notEqual(verifyDirty.status, 0, "verify must fail once pinned content drifts");
    assert.match(verifyDirty.stderr, /surface\.json is stale/);
    assert.match(verifyDirty.stderr, /matrix\.json is stale/);
    // Still must not have touched the committed files.
    assert.equal(await readFile(surface, "utf8"), surfaceBefore);
    assert.equal(await readFile(matrix, "utf8"), matrixBefore);

    // An ordinary reseal (no flag -- the pin list itself did not change)
    // must heal the drift and make --verify pass again.
    const heal = run(["--root", root, "--surface", surface, "--matrix", matrix]);
    assert.equal(heal.status, 0, heal.stderr);
    assert.match(heal.stderr, /rehash-surface changed 1 pinned file hash\(es\)/);

    const verifyHealed = run(["--verify", "--root", root, "--surface", surface, "--matrix", matrix]);
    assert.equal(verifyHealed.status, 0, verifyHealed.stderr);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
