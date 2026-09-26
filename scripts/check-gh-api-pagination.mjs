// SPDX-License-Identifier: Apache-2.0
//
// A `first:100` GraphQL page hid 19 open Codex review threads on PR 228; a plain
// `gh api repos/.../pulls` without `--paginate` silently returns only the
// first page the same way. Both look identical to a correct call until the
// list they are reading grows past one page, at which point they are wrong
// in a way nothing in CI notices.
//
// This scans for `gh api` invocations — not `gh pr list`/`gh issue list`/etc,
// whose own `--limit`/pagination defaults are a different, gh-owned concern —
// across workflow YAML, shell/Python/JS scripts, and Markdown runbooks, and
// flags a REST list call missing `--paginate` or a GraphQL call whose query
// text (anywhere in the same file — queries are typically built as a
// separate string/heredoc, not inlined into the invocation) mentions a
// connection's `nodes`/`edges` selection without `hasNextPage` appearing
// anywhere in the same file (a hand-rolled cursor loop is usually not on the
// same line as the query).
//
// Mechanical, not semantic: a REST call is judged "a list call" by whether
// its last path segment is a known collection name (`pulls`, `issues`,
// `comments`, ...) not immediately pinned to a specific numeric ID — the
// exact shape of "list every X" for GitHub's REST API. A mutating call
// (`--method POST/PUT/PATCH/DELETE`) is never a list call and is skipped
// outright, which is why the one real `gh api` call in this repository today
// (release-mcpb-preview.yml's `--method POST .../git/refs`, creating a tag
// ref) does not fire this gate.

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

const SCAN_GLOBS = [
  ".github/workflows",
  ".github/actions",
  "scripts",
  "docs",
];
const SCAN_EXTENSIONS = new Set([".yml", ".yaml", ".sh", ".py", ".mjs", ".js", ".md"]);

const LIST_COLLECTION_SEGMENT =
  /^(?:pulls|issues|comments|commits|branches|collaborators|deployments|releases|tags|reviews|events|runs|artifacts|workflows|labels|assignees|reactions|check-runs|statuses|members|teams|repos|forks|stargazers|subscribers|hooks|contents|refs|reactions)$/i;

function listFiles() {
  const result = execFileSync(
    "git",
    ["ls-files", "-z", "--", ...SCAN_GLOBS],
    { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  return result.split("\0").filter((path) => path && SCAN_EXTENSIONS.has(extname(path)));
}

function extname(path) {
  const dot = path.lastIndexOf(".");
  return dot === -1 ? "" : path.slice(dot);
}

// Joins a `gh api ...` invocation's shell line-continuations (`\` at EOL)
// into one logical command string, since the repository's own example spans
// three lines this way.
function logicalCommands(text) {
  const rawLines = text.split("\n");
  const commands = [];
  let index = 0;
  while (index < rawLines.length) {
    const line = rawLines[index];
    if (!/\bgh\s+api\b/.test(line)) {
      index += 1;
      continue;
    }
    const parts = [line];
    let cursor = index;
    while (parts[parts.length - 1].trimEnd().endsWith("\\") && cursor + 1 < rawLines.length) {
      cursor += 1;
      parts.push(rawLines[cursor]);
    }
    commands.push({ text: parts.join("\n"), startLine: index + 1 });
    index = cursor + 1;
  }
  return commands;
}

function isMutation(command) {
  return /--method[\s=]+["']?(POST|PUT|PATCH|DELETE)/i.test(command);
}

function isGraphQL(command) {
  return /\bgh\s+api\s+graphql\b/.test(command);
}

// The first non-flag token after `gh api` (and, for GraphQL, unused — that
// branch is judged by query shape instead).
function restPath(command) {
  const withoutContinuations = command.replace(/\\\r?\n/g, " ");
  const afterApi = withoutContinuations.split(/\bgh\s+api\b/)[1] ?? "";
  const tokens = afterApi.trim().split(/\s+/);
  for (const token of tokens) {
    if (token.startsWith("-")) continue;
    return token.replace(/^["']|["']$/g, "");
  }
  return null;
}

function isListPath(path) {
  if (!path) return false;
  const withoutQuery = path.split("?")[0];
  const segments = withoutQuery.split("/").filter(Boolean);
  if (!segments.length) return false;
  const last = segments[segments.length - 1];
  // `.../pulls/123` targets one specific PR, not the collection — only the
  // bare collection segment (or one ending in a template placeholder such as
  // `$PR_NUMBER`/`{id}`, which is not a concrete numeric ID) is a list call.
  if (/^\d+$/.test(last)) return false;
  return LIST_COLLECTION_SEGMENT.test(last);
}

const failures = [];
let checked = 0;

for (const path of listFiles()) {
  const absolute = resolve(repositoryRoot, path);
  const text = readFileSync(absolute, "utf8");
  const commands = logicalCommands(text);
  if (!commands.length) continue;

  for (const command of commands) {
    checked += 1;
    if (isMutation(command.text)) continue;

    if (isGraphQL(command.text)) {
      // Searched across the whole file, not just this command line: the
      // query text is typically built as a separate string/heredoc earlier
      // in the file and interpolated into the invocation, the same reason
      // hasNextPage is searched file-wide just below.
      const requestsConnection = /\b(?:nodes|edges)\b/.test(text);
      if (!requestsConnection) continue;
      if (/hasNextPage/.test(text)) continue; // Anywhere in the file: see file banner.
      failures.push(
        `${path}:${command.startLine}: gh api graphql query requests a connection ` +
          "(nodes/edges) but this file never checks hasNextPage — a page beyond the " +
          "first is silently dropped (see docs note: a first:100 page hid 19 open " +
          "review threads on PR 228)",
      );
      continue;
    }

    if (/--paginate\b/.test(command.text)) continue;
    const apiPath = restPath(command.text);
    if (!isListPath(apiPath)) continue;
    failures.push(
      `${path}:${command.startLine}: gh api ${apiPath} looks like a collection listing ` +
        "call with no --paginate — only the first page is fetched",
    );
  }
}

if (failures.length) {
  throw new Error(
    `gh api pagination (${failures.length} problem(s)):\n` +
      failures.map((line) => `  - ${line}`).join("\n"),
  );
}

console.log(
  `gh api pagination coverage holds (${checked} gh api invocation(s) scanned, 0 unpaginated list calls).`,
);
