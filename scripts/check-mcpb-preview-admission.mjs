#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(import.meta.dirname, "..");
const previewTag = /^mcp-preview-([0-9]+\.[0-9]+\.[0-9]+(?:[-.][0-9A-Za-z]+)*)$/;

export function previewVersion(releaseTag) {
  const match = previewTag.exec(releaseTag);
  if (!match) throw new Error("release tag must be an immutable mcp-preview semantic version");
  return match[1];
}

export function assertPreviewAdmission({ releaseTag, sourceRef, defaultBranch, applicationVersion, mcpbVersion }) {
  if (sourceRef !== `refs/heads/${defaultBranch}`) {
    throw new Error(`preview releases must be dispatched from the default branch ${defaultBranch}`);
  }
  const version = previewVersion(releaseTag);
  if (applicationVersion !== version || mcpbVersion !== version) {
    throw new Error(`release tag version ${version} must match application and MCPB manifest versions`);
  }
}

async function main() {
  const [releaseTag, sourceRef, defaultBranch] = process.argv.slice(2);
  if (![releaseTag, sourceRef, defaultBranch].every(Boolean)) {
    throw new Error("usage: node scripts/check-mcpb-preview-admission.mjs RELEASE_TAG SOURCE_REF DEFAULT_BRANCH");
  }
  const [application, mcpbManifest] = await Promise.all([
    readFile(resolve(root, "package.json"), "utf8").then(JSON.parse),
    readFile(resolve(root, "packaging", "mcpb", "manifest.json"), "utf8").then(JSON.parse),
  ]);
  assertPreviewAdmission({
    releaseTag,
    sourceRef,
    defaultBranch,
    applicationVersion: application.version,
    mcpbVersion: mcpbManifest.version,
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();
