// SPDX-License-Identifier: Apache-2.0
//
// Privacy/redaction pattern gaps were 44 of the 880 findings this coverage is
// built from, and 68% of those were P1. The rule this gate enforces is the
// one named in that analysis: a diff that edits a PII-matching regular
// expression inside a scanner or sanitizer must add a regression test case
// for that edit in the *same* diff. A pattern that narrows silently starts
// leaking the thing it used to catch; a pattern that widens silently starts
// mangling text it used to leave alone — and both are invisible at review
// time unless the diff itself carries evidence the new behaviour was
// exercised.
//
// This does not try to judge whether the added test is any good — that is a
// human reviewer's job. It only checks that *a* test file changed alongside
// the regex, which is the same bar check-fixture-byte-integrity.mjs and
// check-protocol-section-numbers.mjs hold other silent-drift classes to:
// mechanical presence, not judgement.
//
// Scope: "scanner or sanitizer" is identified by filename
// (`sanit*`/`redact*`/`scrub*`/`*pii*`, case-insensitive) rather than an
// explicit registry, because scripts/sanitise-bbox-capture.py is exactly this
// shape and the convention should cover the next one by naming alone, not by
// a list someone has to remember to update. "PII regex" is identified by the
// language-appropriate call that defines one — `re.compile(`/`re.match(`/
// `re.search(`/`re.sub(`/`re.fullmatch(` in Python, `new RegExp(` in
// JS/TS, `Regex::new(` in Rust — checked against the diff's own added/removed
// lines (not the whole file), so an unrelated formatting or comment change to
// the same file does not demand a new test.

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { extname, resolve } from "node:path";

const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

const baseArgument = process.argv.indexOf("--base");
if (baseArgument !== -1 && !process.argv[baseArgument + 1]) {
  throw new Error("--base requires a git ref");
}
const explicitBase = baseArgument === -1 ? null : process.argv[baseArgument + 1];

const SCANNER_NAME = /(?:^|[\\/])(?:sanit\w*|redact\w*|scrub\w*|\w*pii\w*)[^\\/]*$/i;
const TEST_SUFFIX = /\.test\.[^.]+$|\.spec\.[^.]+$/i;

const REGEX_DEFINITION_BY_EXTENSION = {
  ".py": /\bre\.(?:compile|match|search|sub|fullmatch)\(/,
  ".mjs": /\bnew RegExp\(/,
  ".js": /\bnew RegExp\(/,
  ".ts": /\bnew RegExp\(/,
  ".tsx": /\bnew RegExp\(/,
  ".rs": /\bRegex::new\(/,
};

function git(args) {
  const result = execFileSync("git", args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return result;
}

function resolveBase() {
  if (explicitBase) return explicitBase;
  const candidates = [
    process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : null,
    "origin/master",
    "master",
  ].filter(Boolean);
  for (const ref of candidates) {
    try {
      git(["rev-parse", "--verify", ref]);
      return ref;
    } catch {
      continue;
    }
  }
  return null;
}

const base = resolveBase();
if (!base) {
  console.warn(
    "note: no base revision reachable (shallow clone?) — PII regex regression " +
      "coverage was NOT checked.",
  );
  process.exit(0);
}

// Working-tree comparison (one ref, not base...HEAD): this must catch an
// uncommitted regex edit before it is ever pushed, the same way a contributor
// running check-fixture-byte-integrity.mjs locally expects an uncommitted
// mutation to be visible.
const changedFiles = git(["diff", "--name-only", base, "--"])
  .split("\n")
  .filter(Boolean);
const changedFileSet = new Set(changedFiles);

function testSiblingFor(path) {
  const extension = extname(path);
  const withoutExtension = path.slice(0, path.length - extension.length);
  return `${withoutExtension}.test${extension}`;
}

function diffHunkLines(path) {
  // -U0: zero context lines, so every `+`/`-` line is an actual change, not
  // surrounding context that would falsely look like an edit.
  const diff = git(["diff", "-U0", base, "--", path]);
  return diff
    .split("\n")
    .filter((line) => (line.startsWith("+") || line.startsWith("-")) && !line.startsWith("+++") && !line.startsWith("---"));
}

const failures = [];
let regexEditsChecked = 0;

for (const path of changedFiles) {
  if (!SCANNER_NAME.test(path) || TEST_SUFFIX.test(path)) continue;
  const extension = extname(path);
  const pattern = REGEX_DEFINITION_BY_EXTENSION[extension];
  if (!pattern) continue; // Not a language this gate knows how to scan.

  const hunkLines = diffHunkLines(path);
  const touchesRegexDefinition = hunkLines.some((line) => pattern.test(line));
  if (!touchesRegexDefinition) continue;

  regexEditsChecked += 1;
  const sibling = testSiblingFor(path);
  if (!existsSync(resolve(repositoryRoot, sibling))) {
    failures.push(
      `${path}: a PII-pattern definition changed, but its expected regression-test ` +
        `sibling ${sibling} does not exist at all — add it`,
    );
    continue;
  }
  if (!changedFileSet.has(sibling)) {
    failures.push(
      `${path}: a PII-pattern definition changed but ${sibling} was not touched in the ` +
        "same diff — add a regression-test case covering the change (see " +
        "scripts/sanitise-bbox-capture.test.py for the pattern: assert the input " +
        "character does NOT survive, rather than asserting one expected output string)",
    );
  }
}

if (failures.length) {
  throw new Error(
    `PII regex regression coverage (${failures.length} problem(s), base ${base}):\n` +
      failures.map((line) => `  - ${line}`).join("\n"),
  );
}

console.log(
  `PII regex regression coverage holds against ${base} ` +
    `(${regexEditsChecked} scanner/sanitizer regex edit(s) checked, all paired with a test).`,
);
