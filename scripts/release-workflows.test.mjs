import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("MCPB release workflow publishes only an explicit unsigned preview with both qualified host archives", async () => {
  const workflow = await readFile(new URL("../.github/workflows/release-mcpb-preview.yml", import.meta.url), "utf8");
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /\n  push:/);
  assert.doesNotMatch(workflow, /production-signed/);
  assert.match(workflow, /windows-x64/);
  assert.match(workflow, /macos-arm64/);
  assert.match(workflow, /refusing to replace existing release assets/);
  assert.match(workflow, /scripts\/check-mcpb-bundle\.py/);
  assert.match(workflow, /--prerelease/);
  assert.match(workflow, /GH_REPO: \$\{\{ github\.repository \}\}/);
  assert.match(workflow, /packaging\/mcpb\/UNSIGNED_PREVIEW_RELEASE\.md/);
});

test("install page deployment remains a reviewed manual action", async () => {
  const workflow = await readFile(new URL("../.github/workflows/deploy-install-page.yml", import.meta.url), "utf8");
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /\n  push:/);
  assert.match(workflow, /path: site/);
  assert.match(workflow, /actions\/deploy-pages@/);
});
