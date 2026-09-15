// SPDX-License-Identifier: Apache-2.0
//
// A hand-authored fixture standing in for a captured one was 14 of the 880
// findings this coverage is built from, and 86% of those were P1: a parser
// test that looks like it proves something about real Tally/bank output can
// silently be exercising only what its author imagined that output looks
// like.
//
// `src-tauri/crates/bridge-tally-protocol/tests/fixtures/{encoding,native}/PROVENANCE.md`
// already carry the pattern this gate generalises: a per-fixture line naming
// where the bytes came from, and — for a fixture asserted as byte-exact
// captured evidence — its size and SHA-256 so a later hand-edit or
// regeneration is a hash mismatch, not a silent swap. This script does not
// invent a new convention; it enforces that every fixture in every directory
// already covered by the byte-integrity gate
// (check-fixture-byte-integrity.mjs) carries that same paper trail, and that
// a captured-fixture's declared hash still matches its actual bytes.
//
// Two independent failures, so a fixture cannot pass by accident:
//   1. undocumented  — the fixture's filename appears in no Markdown file
//      anywhere under its fixture root. Nothing attests to what it is.
//   2. hash mismatch — a Markdown table declares this fixture "captured"
//      with a specific byte count and SHA-256, and the file on disk no
//      longer matches. This is the actual swap-detection: provenance text
//      alone is a comment nobody re-reads, but a hash is checked by machine.
//
// This does NOT require a table entry for every fixture — the native/
// PROVENANCE.md documents fixtures whose byte-level fidelity is explicitly
// NOT established (Git-normalised on first commit) by naming them in prose
// instead. That is a legitimate, weaker attestation this gate accepts: it
// only escalates to a hash check where the document itself claims one.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync, realpathSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { extname, join, relative, resolve } from "node:path";

// --root lets the contract tests point this at a synthetic tree instead of
// the real repository, the same convention check-tally-request-builder-hazards.mjs
// uses, so those tests can exercise every branch (undocumented fixture,
// hash mismatch, prose-only mention, a clean pass) without touching real
// tracked fixtures.
const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

// Mirrors check-fixture-byte-integrity.mjs's registered directories exactly.
// A fixture directory only ever becomes byte-integrity-covered by being added
// there, so reusing the same list (rather than rediscovering fixture
// directories independently) means this gate can never drift to cover a
// directory that gate does not, or vice versa.
const fixtureDirectories = [
  "src-tauri/crates/bridge-tally-protocol/tests/fixtures",
  "src-tauri/crates/tally-protocol-simulator/fixtures",
  "docs/tally/compatibility/fixtures",
  "scripts/fixtures",
];

// Diagnostics are bounded for the same reason every other gate in this repo
// bounds them: a tree with many undocumented fixtures must still produce
// output a reviewer can read.
const MAX_REPORTED = 25;

function classifyEntry(path, entry) {
  if (entry.isDirectory()) return "directory";
  if (entry.isFile()) return "file";
  if (entry.isSymbolicLink()) {
    let stat;
    try {
      stat = statSync(path);
    } catch (error) {
      throw new Error(`unable to resolve symlink while walking ${path}: ${error.message}`);
    }
    if (stat.isDirectory()) return "directory";
    if (stat.isFile()) return "file";
    return "other";
  }
  return "other";
}

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

// A table row of the shape documented in PROVENANCE.md:
//   | `name.ext` | 1,170 | `<64 lowercase-or-uppercase hex chars>` |
// Byte counts may carry thousands separators (`7,114`) or not (`1170`); both
// forms appear in the existing files.
const TABLE_ROW = /\|\s*`([^`]+)`\s*\|\s*([\d,]+)\s*\|\s*`([0-9a-fA-F]{64})`\s*\|/g;

function short(value, max = 200) {
  const text = String(value);
  return text.length <= max ? text : `${text.slice(0, max)}…`;
}

const failures = [];
let undocumented = 0;
let checkedHashes = 0;

for (const fixtureDirectory of fixtureDirectories) {
  const absoluteDirectory = join(repositoryRoot, fixtureDirectory);
  let allPaths;
  try {
    allPaths = walkFiles(absoluteDirectory);
  } catch (error) {
    throw new Error(`unable to walk fixture directory ${fixtureDirectory}: ${error.message}`);
  }

  const markdownFiles = allPaths.filter((path) => extname(path).toLowerCase() === ".md");
  const fixtureFiles = allPaths.filter((path) => extname(path).toLowerCase() !== ".md");
  if (!fixtureFiles.length) continue;

  // Every filename this directory's own documentation names, plus every
  // captured-fixture table row found in it. Concatenating every Markdown
  // file under the directory (not just a file named PROVENANCE.md) matches
  // what the convention actually does today: scripts/fixtures/README.md
  // names its fixtures in prose rather than in a file called PROVENANCE.md,
  // and that is a legitimate paper trail this gate accepts.
  let documentationText = "";
  const declaredHashes = new Map(); // basename -> [{ bytes, sha256, sourceFile }]
  for (const markdownPath of markdownFiles) {
    const text = readFileSync(markdownPath, "utf8");
    documentationText += `\n${text}`;
    for (const match of text.matchAll(TABLE_ROW)) {
      const [, name, bytesText, sha256] = match;
      const bytes = Number(bytesText.replaceAll(",", ""));
      if (!declaredHashes.has(name)) declaredHashes.set(name, []);
      declaredHashes.get(name).push({
        bytes,
        sha256: sha256.toLowerCase(),
        sourceFile: relative(repositoryRoot, markdownPath),
      });
    }
  }

  for (const fixturePath of fixtureFiles) {
    const relativePath = relative(repositoryRoot, fixturePath);
    const basename = relativePath.split("/").pop();

    // A plain substring match with word-boundary-ish punctuation on both
    // sides. Fixture names contain `.`, `-`, and `_`, none of which are `\w`
    // word-boundary-safe, so this checks the character immediately outside
    // the match is not itself part of a longer filename instead of using
    // `\b`, which a dot or hyphen would defeat silently.
    const escaped = basename.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const mentioned = new RegExp(`(?<![A-Za-z0-9_.-])${escaped}(?![A-Za-z0-9_.-])`).test(
      documentationText,
    );
    if (!mentioned) {
      undocumented += 1;
      if (failures.length < MAX_REPORTED) {
        failures.push(
          `${relativePath}: not named in any Markdown file under ` +
            `${fixtureDirectory} — add a provenance line saying where its bytes ` +
            "came from (see tests/fixtures/*/PROVENANCE.md for the pattern)",
        );
      }
      continue;
    }

    const declarations = declaredHashes.get(basename);
    if (!declarations) continue; // Named in prose only — accepted, see file banner.

    const actualBytes = readFileSync(fixturePath);
    const actualSha256 = createHash("sha256").update(actualBytes).digest("hex");
    checkedHashes += 1;
    for (const declaration of declarations) {
      const bytesMatch = declaration.bytes === actualBytes.length;
      const hashMatch = declaration.sha256 === actualSha256;
      if (bytesMatch && hashMatch) continue;
      if (failures.length < MAX_REPORTED) {
        failures.push(
          `${relativePath}: declared in ${declaration.sourceFile} as ` +
            `${declaration.bytes.toLocaleString("en-US")} bytes / ` +
            `sha256:${short(declaration.sha256, 16)}…, but the file on disk is ` +
            `${actualBytes.length.toLocaleString("en-US")} bytes / ` +
            `sha256:${short(actualSha256, 16)}… — this is exactly the hand-authored-` +
            "substitute pattern this gate exists to catch: either the capture " +
            "was genuinely re-taken (update the provenance table) or something " +
            "replaced it (restore the captured bytes)",
        );
      }
    }
  }
}

if (failures.length) {
  const shown = failures.slice(0, MAX_REPORTED);
  throw new Error(
    `fixture provenance (${shown.length} problem(s) shown` +
      (failures.length > shown.length ? `, ${failures.length - shown.length} more omitted` : "") +
      "):\n" +
      shown.map((line) => `  - ${line}`).join("\n"),
  );
}

console.log(
  `Fixture provenance is intact across ${fixtureDirectories.length} directories ` +
    `(${checkedHashes} captured-fixture hash(es) verified, 0 undocumented).`,
);
