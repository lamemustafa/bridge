// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";

const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const fixtureDirectories = [
  "src-tauri/crates/bridge-tally-protocol/tests/fixtures",
  "src-tauri/crates/tally-protocol-simulator/fixtures",
  "docs/tally/compatibility/fixtures",
];
const discoveredFixtureDirectories = walkDirectories(repositoryRoot)
  .filter((directory) => directory.endsWith("/fixture") || directory.endsWith("/fixtures"))
  .map((directory) => relative(repositoryRoot, directory).replaceAll("\\", "/"))
  .sort();
const unexpectedFixtureDirectories = discoveredFixtureDirectories.filter(
  (directory) => !fixtureDirectories.includes(directory),
);
if (unexpectedFixtureDirectories.length) {
  throw new Error(
    "unexpected fixture directories are not covered by byte-integrity policy:\n" +
      unexpectedFixtureDirectories.map((directory) => `- ${directory}`).join("\n") +
      "\nRegister each directory in fixtureDirectories and .gitattributes before adding fixtures.",
  );
}
const fixtures = fixtureDirectories
  .flatMap((directory) => {
    const paths = walkFiles(join(repositoryRoot, directory));
    if (!paths.length) throw new Error(`fixture tree is empty: ${directory}`);
    return paths.map((path) => relative(repositoryRoot, path).replaceAll("\\", "/"));
  })
  .sort();

if (!fixtures.length) {
  throw new Error("fixture trees are empty");
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
    `fixtures must opt out of text normalization:\n${attributeFailures.join("\n")}`,
  );
}

const byteFailures = [];
for (const fixture of fixtures) {
  // In CI the checkout materialises this file from the same HEAD blob, so this
  // comparison cannot independently establish captured-byte provenance there.
  // The checked .gitattributes rule is the meaningful CI protection; this
  // comparison still catches local worktree conversion or mutation.
  const committed = runGit(["show", "--no-textconv", `HEAD:${fixture}`], { allowFailure: true });
  if (committed.status !== 0) {
    // A new fixture has no blob until its first commit. Attribute coverage still
    // protects its bytes during that window; comparison starts once evidence is
    // committed. Existing fixtures always have a HEAD blob and remain strict.
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

console.log(`Fixture byte integrity is sealed (${fixtures.length} files).`);

function walkFiles(directory) {
  const paths = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) paths.push(...walkFiles(path));
    else if (entry.isFile()) paths.push(path);
  }
  return paths;
}

function walkDirectories(directory) {
  const directories = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isDirectory() || [".git", "node_modules", "target"].includes(entry.name)) continue;
    const path = join(directory, entry.name);
    directories.push(path, ...walkDirectories(path));
  }
  return directories;
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
