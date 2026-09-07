import assert from "node:assert/strict";
import test from "node:test";

import { assetName, isInstallablePreview, releaseAssets, releaseLabel, selectRelease } from "../site/release-catalog.mjs";

const preview = {
  draft: false,
  prerelease: true,
  tag_name: "mcp-preview-0.2.0",
  assets: [
    { name: "bridge-tally-mcp-preview-0.2.0-windows-x64.mcpb", browser_download_url: "https://example.invalid/windows" },
    { name: "bridge-tally-mcp-preview-0.2.0-windows-x64.mcpb.sha256", browser_download_url: "https://example.invalid/windows.sha256" },
    { name: "bridge-tally-mcp-preview-0.2.0-macos-arm64.mcpb", browser_download_url: "https://example.invalid/macos" },
    { name: "bridge-tally-mcp-preview-0.2.0-macos-arm64.mcpb.sha256", browser_download_url: "https://example.invalid/macos.sha256" },
  ],
};

test("release catalogue selects the newest usable unsigned preview, not an unrelated release", () => {
  const unrelatedNewest = { ...preview, prerelease: false, tag_name: "v9.0.0", assets: [] };
  const incompleteNewestPreview = { ...preview, tag_name: "mcp-preview-0.3.0", assets: preview.assets.slice(0, 2) };
  assert.equal(isInstallablePreview(unrelatedNewest), false);
  assert.equal(isInstallablePreview(incompleteNewestPreview), false);
  assert.equal(selectRelease([unrelatedNewest, incompleteNewestPreview, preview]), preview);
  assert.match(releaseLabel(preview), /unsigned preview/);
});

test("release catalogue requires immutable package and checksum names together", () => {
  assert.equal(assetName("mcp-preview-0.2.0", "windows-x64"), "bridge-tally-mcp-preview-0.2.0-windows-x64.mcpb");
  assert.deepEqual(releaseAssets(preview, "windows-x64"), {
    bundle: preview.assets[0],
    checksum: preview.assets[1],
  });
  assert.equal(releaseAssets({ ...preview, assets: preview.assets.slice(0, 2) }, "macos-arm64"), undefined);
});
