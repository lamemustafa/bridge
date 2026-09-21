import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  SEAM_MARKER,
  assertNoTestSeam,
  holdsMarker,
  markedFiles,
  tauriBuildExecutables,
} from "./check-no-test-seam.mjs";

function scratch() {
  return mkdtempSync(join(tmpdir(), "bridge-seam-scan-"));
}

function binary(path, withMarker) {
  const bytes = Buffer.concat([
    Buffer.from([0x7f, 0x45, 0x4c, 0x46, 0x00, 0xff]),
    Buffer.from(withMarker ? `..${SEAM_MARKER}..` : "..no seam here..", "utf8"),
    Buffer.from([0x00, 0x01]),
  ]);
  writeFileSync(path, bytes);
  return path;
}

test("the marker is the one the Rust seam carries", () => {
  const source = readFileSync(
    new URL("../src-tauri/src/tally/approved_import.rs", import.meta.url),
    "utf8",
  );
  assert.ok(source.includes(`const SEAM_MARKER: &str = "${SEAM_MARKER}";`));
});

test("a binary holding the marker is found, and one without it is not", () => {
  const directory = scratch();
  assert.equal(holdsMarker(binary(join(directory, "marked"), true)), true);
  assert.equal(holdsMarker(binary(join(directory, "clean"), false)), false);
  assert.throws(() => assertNoTestSeam([join(directory, "marked")]), /approval seam compiled into/);
  assert.doesNotThrow(() => assertNoTestSeam([join(directory, "clean")]));
});

test("a directory such as an .app bundle is scanned file by file", () => {
  const directory = scratch();
  const executables = join(directory, "Bridge.app", "Contents", "MacOS");
  mkdirSync(executables, { recursive: true });
  binary(join(executables, "bridge"), false);
  binary(join(executables, "bridge_mcp"), true);
  assert.deepEqual(markedFiles([join(directory, "Bridge.app")]), [join(executables, "bridge_mcp")]);
});

test("compressed artefacts, missing paths and empty scans are refused, never passed", () => {
  const directory = scratch();
  for (const name of ["Bridge.dmg", "Bridge.msi", "bridge-tally.mcpb", "Bridge_0.2.0_x64-setup.exe"]) {
    binary(join(directory, name), false);
    assert.throws(() => markedFiles([join(directory, name)]), /compressed/);
  }
  assert.throws(() => markedFiles([join(directory, "absent")]), /does not exist/);
  mkdirSync(join(directory, "empty"));
  assert.throws(() => markedFiles([join(directory, "empty")]), /no regular files/);
  assert.throws(() => markedFiles([]), /no files given/);
});

test("the bundle hook scans the executables of the profile tauri built", () => {
  const root = scratch();
  const release = join(root, "src-tauri", "target", "release");
  const debug = join(root, "src-tauri", "target", "debug");
  const targeted = join(root, "src-tauri", "target", "aarch64-apple-darwin", "release");
  for (const directory of [release, debug, targeted]) mkdirSync(directory, { recursive: true });
  binary(join(release, "bridge"), false);
  binary(join(release, "bridge_mcp"), false);
  binary(join(debug, "bridge.exe"), false);
  binary(join(targeted, "bridge"), false);
  assert.deepEqual(tauriBuildExecutables({}, root).sort(), [
    join(release, "bridge"),
    join(release, "bridge_mcp"),
    join(targeted, "bridge"),
  ].sort());
  assert.deepEqual(tauriBuildExecutables({ TAURI_ENV_DEBUG: "true" }, root), [join(debug, "bridge.exe")]);
  const custom = join(root, "elsewhere", "release");
  mkdirSync(custom, { recursive: true });
  binary(join(custom, "bridge_mcp"), false);
  assert.deepEqual(tauriBuildExecutables({ CARGO_TARGET_DIR: "elsewhere" }, root), [join(custom, "bridge_mcp")]);
});

test("a bundle hook that finds no executable fails rather than passing", () => {
  const root = scratch();
  assert.throws(() => tauriBuildExecutables({}, root), /no bridge or bridge_mcp executable/);
});
