// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const modes = new Set(process.argv.slice(2));
const checkFrontend = modes.size === 0 || modes.has("--frontend");
const checkRust = modes.size === 0 || modes.has("--rust");
const firstPartyRustPackages = new Set([
  "bridge",
  "bridge-tally-canonical",
  "bridge-tally-compatibility",
  "bridge-tally-core",
  "bridge-tally-incremental",
  "bridge-tally-live-read",
  "bridge-tally-observability",
  "bridge-tally-protocol",
  "bridge-tally-qualification",
  "bridge-tally-read-transport",
  "bridge-tally-runtime",
  "bridge-tally-transport",
  "bridge-tally-write",
  "tally-protocol-simulator",
]);

if ([...modes].some((mode) => !["--frontend", "--rust"].includes(mode))) {
  throw new Error("Usage: check-dependency-inventory.mjs [--frontend] [--rust]");
}

const runJson = (command, args, label) => {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${label} inventory command failed`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${label} inventory command returned invalid JSON`);
  }
};

const runText = (command, args, label) => {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${label} inventory command failed`);
  }
  return result.stdout;
};

const componentPattern = /(@?[A-Za-z0-9_.+-]+(?:\/[A-Za-z0-9_.+-]+)?) (\d+\.\d+\.\d+(?:[+-][^,\s]+)?)/g;
const reportComponents = (report, endMarker) => {
  const components = new Set();
  const inventory = endMarker ? report.split(endMarker, 1)[0] : report;
  for (const line of inventory.split(/\r?\n/)) {
    for (const match of line.matchAll(componentPattern)) {
      components.add(`${match[1]} ${match[2]}`);
    }
  }
  return components;
};

const compare = (label, expected, reported, { allowStale = false } = {}) => {
  const missing = [...expected].filter((component) => !reported.has(component)).sort();
  const stale = [...reported].filter((component) => !expected.has(component)).sort();
  if (missing.length || (!allowStale && stale.length)) {
    const details = [
      missing.length ? `missing: ${missing.join(", ")}` : "",
      stale.length ? `stale: ${stale.join(", ")}` : "",
    ].filter(Boolean).join("; ");
    throw new Error(`${label} third-party inventory drift (${details})`);
  }
  return stale;
};

if (checkFrontend) {
  const packageManager = process.env.npm_execpath;
  if (!packageManager) {
    throw new Error("Run the frontend inventory through the pinned pnpm script");
  }
  const licenses = runJson(
    process.execPath,
    [packageManager, "licenses", "list", "--prod", "--json"],
    "frontend",
  );
  const expected = new Set();
  for (const packages of Object.values(licenses)) {
    for (const dependency of packages) {
      for (const version of dependency.versions) {
        expected.add(`${dependency.name} ${version}`);
      }
    }
  }
  const report = await readFile(new URL("../THIRD_PARTY_LICENSES.txt", import.meta.url), "utf8");
  compare("frontend", expected, reportComponents(report));
  console.log(`Frontend license inventory matches ${expected.size} locked components.`);
}

if (checkRust) {
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  const targets = [
    "x86_64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
  ];
  const expected = new Set();
  for (const target of targets) {
    const tree = runText(
      cargo,
      [
        "tree", "--locked",
        "--manifest-path", "src-tauri/Cargo.toml",
        "--target", target,
        "--edges", "normal,build",
        "--prefix", "none",
        "--format", "{p}",
      ],
      `Rust ${target}`,
    );
    for (const line of tree.split(/\r?\n/)) {
      const match = line.match(/^([A-Za-z0-9_.+-]+) v(\d+\.\d+\.\d+(?:[+-][^\s]+)?)/);
      if (match && !firstPartyRustPackages.has(match[1])) {
        expected.add(`${match[1]} ${match[2]}`);
      }
    }
  }
  const report = await readFile(
    new URL("../THIRD_PARTY_LICENSES_RUST.txt", import.meta.url),
    "utf8",
  );
  const overincluded = compare(
    "Rust",
    expected,
    reportComponents(report, "License texts and notices"),
    { allowStale: true },
  );
  console.log(`Rust license inventory matches ${expected.size} locked components.`);
  if (overincluded.length) {
    console.warn(
      `Rust notice conservatively includes ${overincluded.length} additional ` +
        `multi-target components: ${overincluded.join(", ")}`,
    );
  }
}
