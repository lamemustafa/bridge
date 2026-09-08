// SPDX-License-Identifier: Apache-2.0

// Exercise the real gate against a copy of the built app, never the shipped bytes.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { appendFileSync, mkdtempSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const bundle = process.argv[2]
  ? resolve(process.argv[2])
  : fileURLToPath(new URL("../src-tauri/target/release/bundle/", import.meta.url));
const checker = fileURLToPath(new URL("./check-macos-bundle-resources.mjs", import.meta.url));
const apps = readdirSync(join(bundle, "macos"), { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && entry.name.endsWith(".app"));
assert.equal(apps.length, 1, "expected one built app for the mutation check");
const temporary = mkdtempSync(join(tmpdir(), "bridge-signature-check-"));
try {
  mkdirSync(join(temporary, "macos"));
  const copied = join(temporary, "macos", apps[0].name);
  execFileSync("ditto", [join(bundle, "macos", apps[0].name), copied]);
  execFileSync(process.execPath, [checker, temporary, "--app-only"]);
  appendFileSync(join(copied, "Contents", "Resources", "NOTICE"), "\nSynthetic integrity mutation\n");
  const rejected = spawnSync(process.execPath, [checker, temporary, "--app-only"], {
    encoding: "utf8",
    timeout: 40_000,
  });
  assert.ifError(rejected.error);
  assert.equal(rejected.status, 1, "modified bundle must fail the resource gate");
  assert.match(rejected.stderr, /a sealed resource is missing or invalid/);
  console.log("macOS bundle gate rejects modified sealed legal resources.");
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
