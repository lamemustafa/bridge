// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);
const workflowPath = resolve(repositoryRoot, ".github/workflows/ci.yml");
const workflow = readFileSync(workflowPath, "utf8");
const failures = [];
const metadataByWorkspace = new Map();

// These suites may skip in the frontend job, which intentionally has no Rust.
// Bind their real execution to an existing required job with the pinned toolchain.
const workflowConsistency = jobBlock(workflow, "workflow-consistency");
const toolchain = readFileSync(resolve(repositoryRoot, "rust-toolchain.toml"), "utf8").match(/^channel *= *"([^"]+)"/m)?.[1];
if (!toolchain) throw new Error("could not read [toolchain].channel from rust-toolchain.toml");
const pinnedRustPreflight = `rustup which --toolchain ${toolchain} rustc`;
const resealRegressions = "node --test scripts/reseal.test.mjs scripts/reseal-merge-driver.test.mjs";
const pinnedRustSetup = new RegExp(
  `^      - uses: dtolnay/rust-toolchain@[^\\n]+\\n        with:\\n          toolchain: ${escapeRegex(toolchain)}$`,
  "m",
);
if (!pinnedRustSetup.test(workflowConsistency)) {
  failures.push("workflow-consistency must install the repository's pinned Rust toolchain");
}
const preflightOffset = exactRunStepOffset(workflowConsistency, "Require pinned Rust for reseal regressions", pinnedRustPreflight);
const regressionOffset = exactRunStepOffset(workflowConsistency, "Test reseal and merge-driver regressions", resealRegressions);
if (preflightOffset === -1) failures.push("workflow-consistency must fail when pinned Rust is unavailable");
if (regressionOffset === -1) failures.push("workflow-consistency must run the reseal regression suites");
if (preflightOffset !== -1 && regressionOffset !== -1 && preflightOffset > regressionOffset) {
  failures.push("workflow-consistency must require pinned Rust before running reseal regressions");
}

const requiredChecks = jobBlock(workflow, "required-checks");
if (!/^    needs: \[[^\n]*\bworkflow-consistency\b[^\n]*\]$/m.test(requiredChecks)) {
  failures.push("required-checks must propagate workflow-consistency failures");
}

for (const step of parseWorkflowSteps(workflow)) {
  for (const command of cargoCommands(step.run)) {
    const packages = [...command.matchAll(/(?:^|\s)-p\s+([A-Za-z0-9_.-]+)/g)].map((match) => match[1]);
    if (!packages.length) continue;

    const manifestPath = manifestForStep(step, command);
    const metadata = workspaceMetadata(manifestPath);
    for (const packageName of packages) {
      const packageManifest = metadata.packages.find((candidate) => candidate.name === packageName);
      if (!packageManifest) {
        failures.push(`${step.name}: package ${packageName} is not in ${relativePath(manifestPath)}`);
        continue;
      }

      for (const features of featureLists(command)) {
        for (const feature of features) {
          if (!Object.hasOwn(packageManifest.features, feature)) {
            failures.push(`${step.name}: feature ${feature} is absent from ${packageName}`);
          }
        }
      }
    }
  }
}

for (const stalePath of staleToolPaths()) {
  failures.push(`stale tools-workspace path: ${stalePath.file} references ${stalePath.path}`);
}

if (failures.length) {
  throw new Error(`CI workflow references do not resolve:\n${failures.join("\n")}`);
}

console.log("CI workflow references and required execution contracts resolve.");

function parseWorkflowSteps(source) {
  const steps = [];
  const lines = source.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const start = lines[index].match(/^ {6}-\s+(.*)$/);
    if (!start) continue;

    const name = start[1].match(/^name:\s*(.*)$/)?.[1] ?? start[1];
    const step = { name, workingDirectory: ".", run: "" };
    for (index += 1; index < lines.length && !/^ {6}-\s/.test(lines[index]); index += 1) {
      const workingDirectory = lines[index].match(/^ {8}working-directory:\s*(.+?)\s*$/);
      if (workingDirectory) step.workingDirectory = workingDirectory[1];

      const run = lines[index].match(/^ {8}run:\s*(.*)$/);
      if (!run) continue;
      if (run[1] && run[1] !== ">-" && run[1] !== "|") {
        step.run = run[1];
        continue;
      }

      const commandLines = [];
      for (index += 1; index < lines.length && /^ {10}/.test(lines[index]); index += 1) {
        commandLines.push(lines[index].slice(10));
      }
      step.run = commandLines.join("\n");
      index -= 1;
    }
    index -= 1;
    if (step.run) steps.push(step);
  }
  return steps;
}

function jobBlock(source, jobName) {
  const lines = source.split(/\r?\n/);
  const start = lines.findIndex((line) => line === `  ${jobName}:`);
  if (start === -1) {
    failures.push(`CI workflow is missing job ${jobName}`);
    return "";
  }
  let end = start + 1;
  while (end < lines.length && !/^  [A-Za-z0-9_-]+:\s*$/.test(lines[end])) end += 1;
  return lines.slice(start, end).join("\n");
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function exactRunStepOffset(job, name, command) {
  const expected = `      - name: ${name}\n        run: ${command}`;
  const offset = job.indexOf(expected);
  if (offset === -1) return -1;
  const suffix = job.slice(offset + expected.length);
  return suffix === "" || suffix.startsWith("\n      - ") ? offset : -1;
}

function cargoCommands(run) {
  return [...run.matchAll(/(?:^|\n)\s*cargo\s+([\s\S]*?)(?=(?:\n\s*cargo\s)|$)/g)].map((match) => match[1]);
}

function featureLists(command) {
  return [...command.matchAll(/--features\s+([A-Za-z0-9_.,-]+)/g)].map((match) => match[1].split(","));
}

function manifestForStep(step, command) {
  const explicitManifest = command.match(/--manifest-path\s+([^\s]+)/)?.[1];
  return resolve(repositoryRoot, explicitManifest ?? step.workingDirectory, explicitManifest ? "" : "Cargo.toml");
}

function workspaceMetadata(manifestPath) {
  if (metadataByWorkspace.has(manifestPath)) return metadataByWorkspace.get(manifestPath);
  const result = spawnSync(
    "cargo",
    ["metadata", "--locked", "--no-deps", "--format-version", "1", "--manifest-path", manifestPath],
    { cwd: repositoryRoot, encoding: "utf8", windowsHide: true },
  );
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? result.stderr.trim() ?? "unknown error";
    throw new Error(`cargo metadata failed for ${relativePath(manifestPath)}: ${detail}`);
  }
  const metadata = JSON.parse(result.stdout);
  metadataByWorkspace.set(manifestPath, metadata);
  return metadata;
}

function staleToolPaths() {
  const toolsMetadata = workspaceMetadata(resolve(repositoryRoot, "tools", "Cargo.toml"));
  const legacyPaths = toolsMetadata.packages.map((candidate) => `src-tauri/crates/${candidate.name}`);
  const tracked = spawnSync("git", ["-C", repositoryRoot, "ls-files", "-z"], {
    encoding: "utf8",
    windowsHide: true,
  });
  if (tracked.error || tracked.status !== 0) {
    const detail = tracked.error?.message ?? tracked.stderr.trim() ?? "unknown error";
    throw new Error(`git ls-files failed: ${detail}`);
  }

  const stale = [];
  for (const file of tracked.stdout.split("\0").filter(Boolean)) {
    const contents = readFileSync(resolve(repositoryRoot, file), "utf8");
    for (const path of legacyPaths) {
      if (contents.includes(path)) stale.push({ file, path });
    }
  }
  return stale;
}

function relativePath(path) {
  return path.slice(repositoryRoot.length + 1).replaceAll("\\", "/");
}
