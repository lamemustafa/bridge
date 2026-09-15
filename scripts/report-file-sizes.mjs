// SPDX-License-Identifier: Apache-2.0
//
// A file-size REPORTING script, deliberately not a gate. There is no
// defensible industry line-count standard to enforce — "files over N lines
// are bad" is a real code smell heuristic, but N is a taste, not a fact, and
// picking one and calling it a gate would fail this repository's own large,
// legitimately load-bearing files (a protocol parser handling every Tally XML
// shape in one place is not obviously wrong at any particular line count).
// So this never throws and always exits 0: it prints the distribution and the
// largest files so a human can decide what, if anything, is worth splitting,
// the same way `cargo clippy`'s `too_many_lines` warning is a *prompt* to
// look, not a verdict. Wire it into CI as an always-green step that appends
// to $GITHUB_STEP_SUMMARY (see docs/proposed-ci-gates.md) if you want the
// table visible on every run without gating anything on it.

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { extname, resolve } from "node:path";

const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

const topArgument = process.argv.indexOf("--top");
if (topArgument !== -1 && !process.argv[topArgument + 1]) {
  throw new Error("--top requires a count");
}
const topCount = topArgument === -1 ? 25 : Number(process.argv[topArgument + 1]);
if (!Number.isInteger(topCount) || topCount <= 0) {
  throw new Error("--top must be a positive integer");
}

// Extensions this report treats as line-oriented source text. Everything
// else (images, archives, the byte-integrity-pinned fixtures, compiled
// artifacts) is excluded rather than guessed at — a binary file's "line
// count" from counting `\n` bytes is not a line count, it is noise, and
// reporting it as one would be worse than not reporting it.
const TEXT_EXTENSIONS = new Set([
  ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".sh", ".ps1",
  ".md", ".mdx", ".json", ".yml", ".yaml", ".toml", ".css", ".html", ".sql",
]);

function listTrackedFiles() {
  const result = execFileSync("git", ["ls-files", "-z"], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return result.split("\0").filter(Boolean);
}

function countLines(path) {
  const bytes = readFileSync(resolve(repositoryRoot, path));
  // A NUL byte anywhere means this is not text regardless of extension — a
  // corrupt or mislabelled file must not silently produce a line count.
  if (bytes.includes(0)) return null;
  const text = bytes.toString("utf8");
  if (!text.length) return 0;
  const newlines = (text.match(/\n/g) ?? []).length;
  // A final line with no trailing newline still counts as a line.
  return text.endsWith("\n") ? newlines : newlines + 1;
}

function percentile(sortedAscending, fraction) {
  if (!sortedAscending.length) return 0;
  const index = Math.min(
    sortedAscending.length - 1,
    Math.floor(fraction * (sortedAscending.length - 1)),
  );
  return sortedAscending[index];
}

const entries = [];
let skippedBinary = 0;
let skippedExtension = 0;

for (const path of listTrackedFiles()) {
  const extension = extname(path).toLowerCase();
  if (!TEXT_EXTENSIONS.has(extension)) {
    skippedExtension += 1;
    continue;
  }
  const lines = countLines(path);
  if (lines === null) {
    skippedBinary += 1;
    continue;
  }
  entries.push({ path, lines, extension });
}

entries.sort((a, b) => b.lines - a.lines);
const lineCounts = entries.map((entry) => entry.lines).sort((a, b) => a - b);
const totalLines = lineCounts.reduce((sum, value) => sum + value, 0);

const byExtension = new Map();
for (const entry of entries) {
  const bucket = byExtension.get(entry.extension) ?? { files: 0, lines: 0 };
  bucket.files += 1;
  bucket.lines += entry.lines;
  byExtension.set(entry.extension, bucket);
}

console.log(`File size report — ${entries.length} text file(s) counted ` +
  `(${skippedExtension} skipped by extension, ${skippedBinary} skipped as binary/NUL-containing).`);
console.log("");
console.log(`Total lines: ${totalLines.toLocaleString("en-US")}`);
console.log(`Median: ${percentile(lineCounts, 0.5)}  p90: ${percentile(lineCounts, 0.9)}  ` +
  `p99: ${percentile(lineCounts, 0.99)}  max: ${lineCounts.length ? lineCounts[lineCounts.length - 1] : 0}`);
console.log("");

console.log(`By extension (files, total lines):`);
for (const [extension, bucket] of [...byExtension.entries()].sort((a, b) => b[1].lines - a[1].lines)) {
  console.log(`  ${extension.padEnd(8)} ${String(bucket.files).padStart(5)} files  ${bucket.lines.toLocaleString("en-US").padStart(10)} lines`);
}
console.log("");

console.log(`Largest ${Math.min(topCount, entries.length)} file(s):`);
for (const entry of entries.slice(0, topCount)) {
  console.log(`  ${String(entry.lines).padStart(6)}  ${entry.path}`);
}

console.log("");
console.log("This is a REPORTING script only — it never fails the build. See this file's " +
  "banner comment for why no line-count threshold is enforced.");
