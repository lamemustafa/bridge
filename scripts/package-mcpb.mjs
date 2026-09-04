#!/usr/bin/env node
import { cp, mkdir, rm } from "node:fs/promises";
import { platform } from "node:os";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dirname, "..");
const manifest = resolve(root, "src-tauri", "Cargo.toml");
const binary = platform() === "win32" ? "bridge_mcp.exe" : "bridge_mcp";
const build = spawnSync("cargo", ["build", "--manifest-path", manifest, "--bin", "bridge_mcp"], {
  cwd: root,
  stdio: "inherit",
});
if (build.status !== 0) process.exit(build.status ?? 1);

const destination = resolve(root, "packaging", "mcpb", "bin", binary);
await mkdir(resolve(root, "packaging", "mcpb", "bin"), { recursive: true });
await rm(destination, { force: true });
await cp(resolve(root, "src-tauri", "target", "debug", binary), destination);
console.log(`Prepared ${destination} for local .mcpb assembly; do not commit this host binary.`);
