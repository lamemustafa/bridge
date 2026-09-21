import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";

import { mcpbHostTarget, packageMcpbArguments, pdfiumNotice, releaseMcpbBinaryPath, stageHostManifest, stagePdfium, verifyMcpbStage } from "./package-mcpb.mjs";

const resources = [
  "LICENSE",
  "NOTICE",
  "THIRD_PARTY_LICENSES.txt",
  "THIRD_PARTY_LICENSES_RUST.txt",
];

async function temporaryStage(t) {
  const stage = await mkdtemp(join(tmpdir(), "bridge-mcpb-stage-"));
  t.after(() => rm(stage, { recursive: true, force: true }));
  return stage;
}

async function writeBinary(stage, entryPoint) {
  await mkdir(dirname(join(stage, entryPoint)), { recursive: true });
  await writeFile(join(stage, entryPoint), "packaging fixture");
  const library = entryPoint.endsWith(".exe") ? "pdfium.dll" : "libpdfium.dylib";
  await writeFile(join(dirname(join(stage, entryPoint)), library), "packaging fixture");
  await writeFile(join(stage, pdfiumNotice), "packaging fixture");
}

test("MCPB stage verifier requires every license and inventory resource", async (t) => {
  const stage = await temporaryStage(t);
  for (const resource of resources.slice(0, -1)) {
    await writeFile(join(stage, resource), "packaging fixture");
  }
  await assert.rejects(() => verifyMcpbStage(stage), /THIRD_PARTY_LICENSES_RUST\.txt/);
  await writeFile(join(stage, resources.at(-1)), "packaging fixture");
  const entryPoint = await stageHostManifest(stage, undefined, mcpbHostTarget("darwin", "arm64"));
  await writeBinary(stage, entryPoint);
  await verifyMcpbStage(stage);
});

test("MCPB packaging reads the release binary", () => {
  assert.equal(releaseMcpbBinaryPath("/fixture", "bridge_mcp"), resolve("/fixture", "src-tauri", "target", "release", "bridge_mcp"));
});

test("MCPB packaging can stage a reviewed prebuilt binary without rebuilding Rust", () => {
  assert.deepEqual(packageMcpbArguments(["--pdfium", "/fixture/pdfium"]), {
    binaryPath: undefined,
    pdfiumDirectory: resolve("/fixture/pdfium"),
  });
  assert.deepEqual(packageMcpbArguments(["--binary", "/fixture/bridge_mcp", "--pdfium", "/fixture/pdfium"]), {
    binaryPath: resolve("/fixture/bridge_mcp"),
    pdfiumDirectory: resolve("/fixture/pdfium"),
  });
  assert.throws(() => packageMcpbArguments(["--binary"]), /usage:/);
  assert.throws(() => packageMcpbArguments(["--other", "/fixture/bridge_mcp"]), /usage:/);
  assert.throws(() => packageMcpbArguments(["--pdfium", "/a", "--pdfium", "/b"]), /usage:/);
});

test("MCPB packaging refuses a bundle without PDFium", () => {
  assert.throws(() => packageMcpbArguments([]), /--pdfium/);
  assert.throws(() => packageMcpbArguments(["--binary", "/fixture/bridge_mcp"]), /--pdfium/);
});

test("PDFium is staged beside the binary with its notice at the root, and the verifier requires both", async (t) => {
  for (const [platform, arch, library] of [["darwin", "arm64", "libpdfium.dylib"], ["win32", "x64", "pdfium.dll"]]) {
    const stage = await temporaryStage(t);
    const source = await temporaryStage(t);
    const host = mcpbHostTarget(platform, arch);
    await writeFile(join(source, library), "pdfium fixture");
    await writeFile(join(source, pdfiumNotice), "notice fixture");
    for (const resource of resources) await writeFile(join(stage, resource), "packaging fixture");
    const entryPoint = await stageHostManifest(stage, undefined, host);
    await mkdir(dirname(join(stage, entryPoint)), { recursive: true });
    await writeFile(join(stage, entryPoint), "packaging fixture");
    await assert.rejects(() => verifyMcpbStage(stage), /PDFium library/);
    const staged = await stagePdfium(stage, source, host);
    assert.equal(staged, `bin/${host.target}/${library}`);
    assert.equal(await readFile(join(stage, staged), "utf8"), "pdfium fixture");
    assert.equal(await readFile(join(stage, pdfiumNotice), "utf8"), "notice fixture");
    await verifyMcpbStage(stage);
    await rm(join(stage, pdfiumNotice));
    await assert.rejects(() => verifyMcpbStage(stage), /licence notice/);
  }
});

test("every host manifest launches its bundled binary and maps user settings to environment", async (t) => {
  for (const [platform, arch] of [["darwin", "arm64"], ["darwin", "x64"], ["win32", "x64"]]) {
    const stage = await temporaryStage(t);
    const host = mcpbHostTarget(platform, arch);
    const entryPoint = await stageHostManifest(stage, undefined, host);
    const manifest = JSON.parse(await readFile(join(stage, "manifest.json"), "utf8"));
    assert.equal(entryPoint, `bin/${host.target}/${host.binary}`);
    assert.equal(manifest.server.entry_point, entryPoint);
    assert.equal(manifest.server.mcp_config.command, `${"${__dirname}"}/${entryPoint}`);
    assert.deepEqual(manifest.compatibility.platforms, [platform]);
    // Posting is off by default until bridge#574 and bridge#575 are fixed;
    // preparation and bank-statement parsing stay on.
    assert.equal(manifest.user_config.enable_writes.default, false);
    assert.deepEqual(manifest.server.mcp_config.env, {
      BRIDGE_TALLY_HOST: "${user_config.host}",
      BRIDGE_TALLY_PORT: "${user_config.port}",
      BRIDGE_AGENT_REDACTION: "${user_config.redaction}",
      BRIDGE_AGENT_ENABLE_IMPORT: "true",
      BRIDGE_AGENT_ENABLE_WRITES: "${user_config.enable_writes}",
    });
    for (const resource of resources) await writeFile(join(stage, resource), "packaging fixture");
    await assert.rejects(() => verifyMcpbStage(stage), /missing binary/);
    await writeBinary(stage, entryPoint);
    await verifyMcpbStage(stage);
    manifest.server.mcp_config.command = "bridge_mcp";
    await writeFile(join(stage, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(() => verifyMcpbStage(stage), /launch command/);
  }
});

test("MCPB verifier rejects unsupported binary paths and mismatched platform claims", async (t) => {
  const stage = await temporaryStage(t);
  for (const resource of resources) await writeFile(join(stage, resource), "packaging fixture");
  await stageHostManifest(stage, undefined, mcpbHostTarget("darwin", "arm64"));
  const manifest = JSON.parse(await readFile(join(stage, "manifest.json"), "utf8"));
  manifest.compatibility.platforms.push("win32");
  await writeFile(join(stage, "manifest.json"), JSON.stringify(manifest));
  await assert.rejects(() => verifyMcpbStage(stage), /only its binary's operating system/);
  for (const entryPoint of [{ "darwin-arm64": "bin/bridge_mcp" }, "bin/../../outside"]) {
    manifest.server.entry_point = entryPoint;
    await writeFile(join(stage, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(() => verifyMcpbStage(stage), /no supported host binary entry point/);
  }
  assert.throws(() => mcpbHostTarget("linux", "x64"), /does not support/);
});
