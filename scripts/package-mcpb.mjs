#!/usr/bin/env node
import { access, cp, mkdir, rm } from "node:fs/promises";
import { platform } from "node:os";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = resolve(import.meta.dirname, "..");
const requiredResources = [
  "LICENSE",
  "NOTICE",
  "THIRD_PARTY_LICENSES.txt",
  "THIRD_PARTY_LICENSES_RUST.txt",
];

export async function verifyMcpbStage(stageDirectory) {
  const missing = [];
  for (const resource of requiredResources) {
    try {
      await access(resolve(stageDirectory, resource));
    } catch {
      missing.push(resource);
    }
  }
  if (missing.length > 0) {
    throw new Error(`MCPB stage missing required license resources: ${missing.join(", ")}`);
  }
}

export async function stageMcpbResources(stageDirectory, sourceRoot = root) {
  for (const resource of requiredResources) {
    await cp(resolve(sourceRoot, resource), resolve(stageDirectory, resource));
  }
  await verifyMcpbStage(stageDirectory);
}

export function releaseMcpbBinaryPath(sourceRoot = root, binary) {
  return resolve(sourceRoot, "src-tauri", "target", "release", binary);
}

async function main() {
  const manifest = resolve(root, "src-tauri", "Cargo.toml");
  const binary = platform() === "win32" ? "bridge_mcp.exe" : "bridge_mcp";
  const build = spawnSync("cargo", ["build", "--release", "--manifest-path", manifest, "--bin", "bridge_mcp"], {
    cwd: root,
    stdio: "inherit",
  });
  if (build.status !== 0) process.exit(build.status ?? 1);

  const stageDirectory = resolve(root, "packaging", "mcpb");
  const destination = resolve(stageDirectory, "bin", binary);
  await mkdir(resolve(stageDirectory, "bin"), { recursive: true });
  await rm(destination, { force: true });
  await cp(releaseMcpbBinaryPath(root, binary), destination);
  await stageMcpbResources(stageDirectory);
  console.log(`Prepared ${destination} and verified required license resources for local .mcpb assembly; do not commit this host binary.`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await main();
}
