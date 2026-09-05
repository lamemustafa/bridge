import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { releaseMcpbBinaryPath, verifyMcpbStage } from "./package-mcpb.mjs";

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
  await verifyMcpbStage(stage);
});

test("MCPB packaging reads the release binary", () => {
  assert.match(releaseMcpbBinaryPath("/fixture", "bridge_mcp"), /src-tauri\/target\/release\/bridge_mcp$/);
});
