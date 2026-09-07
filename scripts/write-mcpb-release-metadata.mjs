#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";

export async function writeMcpbReleaseMetadata({ archivePath, channel, platform, releaseTag, sourceSha }) {
  if (!/^mcp-preview-[0-9]+\.[0-9]+\.[0-9]+(?:[-.][0-9A-Za-z]+)*$/.test(releaseTag)) {
    throw new Error("release tag must be an immutable mcp-preview semantic version");
  }
  if (channel !== "preview-unsigned") {
    throw new Error("only preview-unsigned assets may be published by this workflow");
  }
  if (!new Set(["windows-x64", "macos-arm64"]).has(platform)) {
    throw new Error("release platform is unsupported");
  }
  if (!/^[0-9a-f]{40}$/i.test(sourceSha)) {
    throw new Error("source SHA must be a full commit SHA");
  }

  const archive = resolve(archivePath);
  const bytes = await readFile(archive);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const directory = dirname(archive);
  const name = basename(archive);
  const checksumPath = resolve(directory, `${name}.sha256`);
  const provenancePath = resolve(directory, `${name}.provenance.json`);
  await mkdir(directory, { recursive: true });
  await writeFile(checksumPath, `${sha256}  ${name}\n`);
  await writeFile(provenancePath, `${JSON.stringify({
    schema_version: 1,
    release_tag: releaseTag,
    channel,
    platform,
    asset_name: name,
    sha256,
    source_sha: sourceSha,
    signing: "unsigned_preview",
  }, null, 2)}\n`);
  return { checksumPath, provenancePath, sha256 };
}

async function main() {
  const [archivePath, releaseTag, platform, sourceSha] = process.argv.slice(2);
  if (![archivePath, releaseTag, platform, sourceSha].every(Boolean)) {
    throw new Error("usage: node scripts/write-mcpb-release-metadata.mjs ARCHIVE TAG PLATFORM SOURCE_SHA");
  }
  const result = await writeMcpbReleaseMetadata({
    archivePath,
    releaseTag,
    platform,
    sourceSha,
    channel: "preview-unsigned",
  });
  console.log(`Wrote ${result.checksumPath} and ${result.provenancePath} for ${result.sha256}`);
}

if (import.meta.main) await main();
