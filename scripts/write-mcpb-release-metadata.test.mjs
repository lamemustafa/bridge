import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { writeMcpbReleaseMetadata } from "./write-mcpb-release-metadata.mjs";

test("unsigned preview metadata binds the release asset to its immutable source and checksum", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "bridge-mcpb-release-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const archive = join(directory, "bridge-tally-mcp-preview-0.2.1-windows-x64.mcpb");
  await writeFile(archive, "archive fixture");
  const result = await writeMcpbReleaseMetadata({
    archivePath: archive,
    channel: "preview-unsigned",
    platform: "windows-x64",
    releaseTag: "mcp-preview-0.2.1",
    sourceSha: "a".repeat(40),
  });
  assert.match(await readFile(result.checksumPath, "utf8"), /^8c175e7c95fd6996a5ac22aa0cf1504ad751f81aecaddf3f48a6c05ab904b84c  bridge-tally-mcp-preview-0\.2\.1-windows-x64\.mcpb\n$/);
  assert.deepEqual(JSON.parse(await readFile(result.provenancePath, "utf8")), {
    schema_version: 1,
    release_tag: "mcp-preview-0.2.1",
    channel: "preview-unsigned",
    platform: "windows-x64",
    asset_name: "bridge-tally-mcp-preview-0.2.1-windows-x64.mcpb",
    sha256: result.sha256,
    source_sha: "a".repeat(40),
    signing: "unsigned_preview",
  });
});

test("release metadata rejects a production claim or an ambiguous release identity", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "bridge-mcpb-release-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const archive = join(directory, "fixture.mcpb");
  await writeFile(archive, "fixture");
  const input = {
    archivePath: archive,
    channel: "preview-unsigned",
    platform: "windows-x64",
    releaseTag: "mcp-preview-0.2.1",
    sourceSha: "b".repeat(40),
  };
  await assert.rejects(() => writeMcpbReleaseMetadata({ ...input, channel: "production-signed" }), /only preview-unsigned/);
  await assert.rejects(() => writeMcpbReleaseMetadata({ ...input, releaseTag: "v0.2.1" }), /mcp-preview/);
});
