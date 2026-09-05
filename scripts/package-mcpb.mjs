#!/usr/bin/env node
import { access, cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { arch, platform } from "node:os";
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
const hostTargets = {
  "darwin-arm64": { target: "aarch64-apple-darwin", binary: "bridge_mcp" },
  "darwin-x64": { target: "x86_64-apple-darwin", binary: "bridge_mcp" },
  "win32-x64": { target: "x86_64-pc-windows-msvc", binary: "bridge_mcp.exe" },
};

export function mcpbHostTarget(hostPlatform = platform(), hostArch = arch()) {
  const key = `${hostPlatform}-${hostArch}`;
  const target = hostTargets[key];
  if (!target) throw new Error(`MCPB staging does not support host ${key}`);
  return { key, ...target };
}

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
  let manifest;
  try {
    manifest = JSON.parse(await readFile(resolve(stageDirectory, "manifest.json"), "utf8"));
  } catch {
    throw new Error("MCPB stage manifest is missing or invalid");
  }
  const entries = manifest.server?.entry_point;
  if (!entries || typeof entries !== "object" || Array.isArray(entries)) {
    throw new Error("MCPB stage manifest has no binary entry points");
  }
  for (const [target, binary] of Object.entries(entries)) {
    if (typeof binary !== "string" || !binary.startsWith("bin/")) {
      throw new Error(`MCPB stage has invalid binary entry for ${target}`);
    }
    try {
      await access(resolve(stageDirectory, binary));
    } catch {
      throw new Error(`MCPB stage advertises ${target} without binary ${binary}`);
    }
  }
}

export async function stageMcpbResources(stageDirectory, sourceRoot = root) {
  for (const resource of requiredResources) {
    await cp(resolve(sourceRoot, resource), resolve(stageDirectory, resource));
  }
}

export async function stageHostManifest(stageDirectory, sourceRoot = root, host = mcpbHostTarget()) {
  const template = JSON.parse(
    await readFile(resolve(sourceRoot, "packaging", "mcpb", "manifest.json"), "utf8"),
  );
  const entryPoint = `bin/${host.target}/${host.binary}`;
  template.server.entry_point = { [host.key]: entryPoint };
  await mkdir(stageDirectory, { recursive: true });
  await writeFile(resolve(stageDirectory, "manifest.json"), `${JSON.stringify(template, null, 2)}\n`);
  return entryPoint;
}

export function releaseMcpbBinaryPath(sourceRoot = root, binary) {
  return resolve(sourceRoot, "src-tauri", "target", "release", binary);
}

async function main() {
  const manifest = resolve(root, "src-tauri", "Cargo.toml");
  const host = mcpbHostTarget();
  const build = spawnSync("cargo", ["build", "--release", "--manifest-path", manifest, "--bin", "bridge_mcp"], {
    cwd: root,
    stdio: "inherit",
  });
  if (build.status !== 0) process.exit(build.status ?? 1);

  const stageDirectory = resolve(root, "packaging", "mcpb", "stage");
  const entryPoint = await stageHostManifest(stageDirectory, root, host);
  const destination = resolve(stageDirectory, entryPoint);
  await mkdir(resolve(stageDirectory, "bin", host.target), { recursive: true });
  await rm(destination, { force: true });
  await cp(releaseMcpbBinaryPath(root, host.binary), destination);
  await stageMcpbResources(stageDirectory);
  await verifyMcpbStage(stageDirectory);
  console.log(`Prepared ${destination} with a ${host.key}-only manifest and verified its staged resources; do not commit host artifacts.`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await main();
}
