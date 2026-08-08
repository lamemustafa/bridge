// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";

const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const fixturesDirectory = "src-tauri/crates/bridge-tally-protocol/tests/fixtures";
const fixtureRoot = join(repositoryRoot, fixturesDirectory);
const fixtures = walkFiles(fixtureRoot)
  .map((path) => relative(repositoryRoot, path).replaceAll("\\", "/"))
  .sort();

if (!fixtures.length) {
  throw new Error(`protocol fixture tree is empty: ${fixturesDirectory}`);
}

const attributes = runGit(["check-attr", "-z", "text", "--", ...fixtures]);
const attributeRecords = attributes.stdout.toString("utf8").split("\0");
const attributeFailures = [];
for (let index = 0; index < attributeRecords.length - 1; index += 3) {
  const [path, attribute, value] = attributeRecords.slice(index, index + 3);
  if (attribute !== "text" || value !== "unset") {
    attributeFailures.push(`${path}: ${attribute ?? "missing"}=${value ?? "missing"}`);
  }
}
if (attributeFailures.length) {
  throw new Error(
    `protocol fixtures must opt out of text normalization:\n${attributeFailures.join("\n")}`,
  );
}

const byteFailures = [];
for (const fixture of fixtures) {
  const committed = runGit(["show", "--no-textconv", `HEAD:${fixture}`], { allowFailure: true });
  if (committed.status !== 0) {
    byteFailures.push(`${fixture}: has no committed blob at HEAD`);
    continue;
  }
  const worktree = readFileSync(join(repositoryRoot, fixture));
  if (!worktree.equals(committed.stdout)) {
    byteFailures.push(
      `${fixture}: worktree=${worktree.length} bytes, HEAD=${committed.stdout.length} bytes`,
    );
  }
}
if (byteFailures.length) {
  throw new Error(
    `protocol fixture bytes differ from committed evidence:\n${byteFailures.join("\n")}`,
  );
}

console.log(`Protocol fixture byte integrity is sealed (${fixtures.length} files).`);

function walkFiles(directory) {
  const paths = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) paths.push(...walkFiles(path));
    else if (entry.isFile()) paths.push(path);
  }
  return paths;
}

function runGit(args, { allowFailure = false } = {}) {
  const result = spawnSync("git", args, {
    cwd: repositoryRoot,
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || (!allowFailure && result.status !== 0)) {
    const detail = result.error?.message ?? result.stderr?.toString("utf8") ?? "unknown error";
    throw new Error(`git ${args.join(" ")} failed: ${detail}`);
  }
  return result;
}
