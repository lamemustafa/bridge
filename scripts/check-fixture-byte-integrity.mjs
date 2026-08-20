// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync, realpathSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";

const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const fixtureDirectories = [
  "src-tauri/crates/bridge-tally-protocol/tests/fixtures",
  "src-tauri/crates/tally-protocol-simulator/fixtures",
  "docs/tally/compatibility/fixtures",
];
const discoveredFixtureDirectories = discoverDirectories(repositoryRoot)
  .filter((directory) => directory.endsWith("/fixture") || directory.endsWith("/fixtures"))
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

// Which fixtures already have a blob at HEAD is established by an explicit,
// batched `git ls-tree` lookup — never by interpreting a `git show` failure.
// That keeps "this path has no HEAD blob yet" (fine, it's a brand-new
// fixture) distinct from "git show failed for some other reason"
// (permissions, a corrupt object, git missing, a broken pipe, ...), which
// must fail the gate loudly instead of being read as "new file, skip".
const trackedFixtures = gitTrackedFixturePaths(fixtures);

const byteFailures = [];
let newFixtureCount = 0;
for (const fixture of fixtures) {
  if (!trackedFixtures.has(fixture)) {
    // A new fixture has no blob until its first commit. Attribute coverage still
    // protects its bytes during that window; comparison starts once evidence is
    // committed. Existing fixtures always have a HEAD blob and remain strict.
    newFixtureCount += 1;
    continue;
  }
  // In CI the checkout materialises this file from the same HEAD blob, so this
  // comparison cannot independently establish captured-byte provenance there.
  // The checked .gitattributes rule is the meaningful CI protection; this
  // comparison still catches local worktree conversion or mutation.
  //
  // `trackedFixtures` already proved this path has a HEAD blob, so any
  // failure here is a real problem, not a "new file" — runGit throws instead
  // of swallowing the failure.
  const committed = runGit(["show", "--no-textconv", `HEAD:${fixture}`]);
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

console.log(
  `Fixture byte integrity is sealed (${fixtures.length} files, ${newFixtureCount} new).`,
);

// Classifies a directory entry as "directory", "file", or "other", resolving
// one level of symlink indirection so a symlinked fixture directory (or
// fixture file) is never silently treated as absent. A symlink that cannot be
// resolved (broken target, permission denied, ...) fails loudly rather than
// being swallowed as "not present".
function classifyEntry(path, entry) {
  if (entry.isDirectory()) return "directory";
  if (entry.isFile()) return "file";
  if (entry.isSymbolicLink()) {
    let stat;
    try {
      stat = statSync(path);
    } catch (error) {
      throw new Error(`unable to resolve symlink while walking the repository tree: ${path}: ${error.message}`);
    }
    if (stat.isDirectory()) return "directory";
    if (stat.isFile()) return "file";
    return "other";
  }
  return "other";
}

// Walks a fixture directory for files, following symlinked subdirectories
// (and symlinked files) instead of silently dropping them. `visited` guards
// against symlink cycles by tracking canonical (realpath'd) directories.
function walkFiles(directory, visited = new Set()) {
  const canonical = realpathSync(directory);
  if (visited.has(canonical)) return [];
  visited.add(canonical);
  const paths = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    const kind = classifyEntry(path, entry);
    if (kind === "directory") paths.push(...walkFiles(path, visited));
    else if (kind === "file") paths.push(path);
  }
  return paths;
}

// Discovers every non-ignored directory under `root` (skipping .git,
// node_modules, and target unconditionally). Ignored trees such as
// .pnpm-store, coverage, and dist are filtered out level-by-level, BEFORE
// their contents are ever read — so an unreadable or huge directory inside an
// ignored tree cannot fail (or slow down) this walk. Symlinked directories
// are followed, with a realpath-based cycle guard.
function discoverDirectories(root) {
  const excludedDirectoryNames = new Set([".git", "node_modules", "target"]);
  const discovered = [];
  const visited = new Set();
  let level = [{ absolute: root, relative: "" }];
  while (level.length) {
    const candidates = [];
    for (const { absolute, relative: relativePath } of level) {
      const canonical = realpathSync(absolute);
      if (visited.has(canonical)) continue;
      visited.add(canonical);
      for (const entry of readdirSync(absolute, { withFileTypes: true })) {
        if (excludedDirectoryNames.has(entry.name)) continue;
        const entryAbsolute = join(absolute, entry.name);
        if (classifyEntry(entryAbsolute, entry) !== "directory") continue;
        const entryRelative = relativePath ? `${relativePath}/${entry.name}` : entry.name;
        candidates.push({ absolute: entryAbsolute, relative: entryRelative });
      }
    }
    if (!candidates.length) break;
    const ignored = gitIgnoredPaths(candidates.map((candidate) => candidate.relative));
    const survivors = candidates.filter((candidate) => !ignored.has(candidate.relative));
    for (const survivor of survivors) discovered.push(survivor.relative);
    level = survivors;
  }
  return discovered;
}

function gitIgnoredPaths(paths) {
  if (!paths.length) return new Set();
  const result = runGit(["check-ignore", "-z", "--stdin"], {
    allowFailure: true,
    input: `${paths.join("\0")}\0`,
  });
  if (result.status !== 0 && result.status !== 1) {
    throw new Error(`git check-ignore failed with status ${result.status}`);
  }
  return new Set(
    result.stdout
      .toString("utf8")
      .split("\0")
      .filter(Boolean)
      .map((path) => path.replaceAll("\\", "/")),
  );
}

// Returns the subset of `paths` that have a blob at HEAD, via a single
// batched `git ls-tree` lookup. A path missing from the result is simply
// absent from the HEAD tree (a brand-new fixture); a failure of the
// `ls-tree` invocation itself (bad HEAD, corrupt repo, git missing, ...)
// throws via runGit's default (non-allowFailure) behaviour instead of being
// mistaken for "every path is new".
function gitTrackedFixturePaths(paths) {
  if (!paths.length) return new Set();
  const result = runGit(["ls-tree", "-r", "-z", "--name-only", "HEAD", "--", ...paths]);
  return new Set(
    result.stdout
      .toString("utf8")
      .split("\0")
      .filter(Boolean)
      .map((path) => path.replaceAll("\\", "/")),
  );
}

function runGit(args, { allowFailure = false, input } = {}) {
  const result = spawnSync("git", args, {
    cwd: repositoryRoot,
    input,
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || (!allowFailure && result.status !== 0)) {
    const detail = result.error?.message ?? result.stderr?.toString("utf8") ?? "unknown error";
    throw new Error(`git ${args.join(" ")} failed: ${detail}`);
  }
  return result;
}
