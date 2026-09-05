import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { mcpbHostTarget, releaseMcpbBinaryPath, verifyMcpbStage } from "./package-mcpb.mjs";

const resources = [
  "LICENSE",
  "NOTICE",
  "THIRD_PARTY_LICENSES.txt",
  "THIRD_PARTY_LICENSES_RUST.txt",
];

test("MCPB stage verifier requires every license and inventory resource", async () => {
  const stage = await mkdtemp(join(tmpdir(), "bridge-mcpb-stage-"));
  await mkdir(stage, { recursive: true });
  for (const resource of resources.slice(0, -1)) {
    await writeFile(join(stage, resource), "fixture");
  }
  await assert.rejects(
    () => verifyMcpbStage(stage),
    /THIRD_PARTY_LICENSES_RUST\.txt/,
  );
  await writeFile(join(stage, resources.at(-1)), "fixture");
  await mkdir(join(stage, "bin", "aarch64-apple-darwin"), { recursive: true });
  await writeFile(join(stage, "bin", "aarch64-apple-darwin", "bridge_mcp"), "fixture");
  await writeFile(join(stage, "manifest.json"), JSON.stringify({
    server: { entry_point: { "darwin-arm64": "bin/aarch64-apple-darwin/bridge_mcp" } },
  }));
  await verifyMcpbStage(stage);
});

test("MCPB packaging reads the release binary", () => {
  assert.match(releaseMcpbBinaryPath("/fixture", "bridge_mcp"), /src-tauri\/target\/release\/bridge_mcp$/);
});

test("MCPB verifier rejects a manifest platform without its staged binary", async () => {
  const stage = await mkdtemp(join(tmpdir(), "bridge-mcpb-stage-"));
  for (const resource of resources) await writeFile(join(stage, resource), "fixture");
  await mkdir(join(stage, "bin", "aarch64-apple-darwin"), { recursive: true });
  await writeFile(join(stage, "bin", "aarch64-apple-darwin", "bridge_mcp"), "fixture");
  await writeFile(join(stage, "manifest.json"), JSON.stringify({
    server: { entry_point: {
      "darwin-arm64": "bin/aarch64-apple-darwin/bridge_mcp",
      "win32-x64": "bin/x86_64-pc-windows-msvc/bridge_mcp.exe",
    } },
  }));
  await assert.rejects(() => verifyMcpbStage(stage), /win32-x64/);
  assert.deepEqual(mcpbHostTarget("darwin", "arm64"), {
    key: "darwin-arm64", target: "aarch64-apple-darwin", binary: "bridge_mcp",
  });
});
