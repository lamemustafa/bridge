#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/reseal.sh.
//
// These run the real script against a fully sandboxed fixture -- not the
// repository's actual compatibility-surface.json -- built from a couple of
// throwaway files under a temp directory. That lets the tests mutate
// pinned-file content and force real drift without ever touching the
// committed compatibility artifacts.
//
// The surface stores only per-file hashes (bridge#760), so a reseal is one
// `rehash-surface`. These tests still run the real tool (compiled through the
// pinned toolchain), because what they prove is the tool's behaviour: real
// hashes written, a malformed or schema-1 surface refused and left as it was,
// and --verify never writing.
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

function surfaceFixture() {
  return (
    JSON.stringify(
      {
        schema_version: 2,
        files: [
          { path: "a.txt", sha256: ZERO_SHA256 },
          { path: "b/c.txt", sha256: ZERO_SHA256 },
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
  await writeFile(surface, surfaceFixture());
  return { root, surface };
}

function run(args) {
  return spawnSync(RESEAL, args, { encoding: "utf8" });
}

for (const [name, edit] of [
  [
    "a schema-1 surface",
    (value) => ({ ...value, schema_version: 1, manifest_sha256: "" }),
  ],
  [
    "a pin with a malformed hash",
    (value) => ({ ...value, files: [{ ...value.files[0], sha256: "zz" }, value.files[1]] }),
  ],
  ["an unsorted pin list", (value) => ({ ...value, files: [...value.files].reverse() })],
]) {
  test(`${name} is refused and left as it was`, { skip }, async () => {
    const { root, surface } = await makeSandbox();
    try {
      await writeFile(surface, `${JSON.stringify(edit(JSON.parse(surfaceFixture())), null, 2)}\n`);
      const before = await readFile(surface, "utf8");
      const result = run(["--root", root, "--surface", surface]);
      assert.notEqual(result.status, 0, `must refuse ${name}`);
      assert.equal(await readFile(surface, "utf8"), before, "a refused reseal must not overwrite the surface");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
}

for (const flags of [[], ["--pins-changed"]]) {
  test(`one reseal writes every pin's real hash${flags.length ? " (--pins-changed)" : ""}`, { skip }, async () => {
    const { root, surface } = await makeSandbox();
    try {
      const result = run([...flags, "--root", root, "--surface", surface]);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /rehash-surface changed 2 pinned file hash\(es\)/);
      const resealed = JSON.parse(await readFile(surface, "utf8"));
      assert.deepEqual(Object.keys(resealed), ["schema_version", "files"], "no aggregate digest is stored");
      assert.notEqual(
        resealed.files.find((f) => f.path === "a.txt").sha256,
        ZERO_SHA256,
        "a.txt must be rehashed from its real bytes, not left at the placeholder",
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
}

test("--verify passes on a freshly resealed fixture and fails once a pinned file drifts", { skip }, async () => {
  const { root, surface } = await makeSandbox();
  try {
    const bootstrap = run(["--root", root, "--surface", surface]);
    assert.equal(bootstrap.status, 0, bootstrap.stderr);

    const surfaceBefore = await readFile(surface, "utf8");

    const verifyClean = run(["--verify", "--root", root, "--surface", surface]);
    assert.equal(verifyClean.status, 0, verifyClean.stderr);
    assert.match(verifyClean.stdout, /compatibility surface is current/);
    // --verify must never mutate the file it checks.
    assert.equal(await readFile(surface, "utf8"), surfaceBefore);

    // Mutate a pinned file without resealing -- the exact scenario the gate
    // exists to catch (a stale hash over changed source content).
    await writeFile(join(root, "a.txt"), "one-mutated");

    const verifyDirty = run(["--verify", "--root", root, "--surface", surface]);
    assert.notEqual(verifyDirty.status, 0, "verify must fail once pinned content drifts");
    assert.match(verifyDirty.stderr, /surface\.json is stale/);
    // Still must not have touched the committed file.
    assert.equal(await readFile(surface, "utf8"), surfaceBefore);

    // A reseal must heal the drift and make --verify pass again.
    const heal = run(["--root", root, "--surface", surface]);
    assert.equal(heal.status, 0, heal.stderr);
    assert.match(heal.stderr, /rehash-surface changed 1 pinned file hash\(es\)/);

    const verifyHealed = run(["--verify", "--root", root, "--surface", surface]);
    assert.equal(verifyHealed.status, 0, verifyHealed.stderr);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
