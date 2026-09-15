// SPDX-License-Identifier: Apache-2.0
//
// Unvalidated input was 132 of the 880 findings this coverage is built from —
// by far the largest class. The convention gate named in that analysis is
// narrower than "test your parser": a parser module that gains a new
// accept-path test (this input parses, and parses to the expected value) in a
// diff with no new reject-path test (some other input is rejected) is exactly
// the shape that produces an unvalidated-input finding later — the happy path
// got proven, and nothing proves the parser is still a parser rather than
// something that now accepts everything.
//
// "Parser module" is identified structurally, not by path: any Rust file
// that currently defines a function whose name contains `parse` (`fn
// parse_x`, `fn x_parse`, ...). That catches
// bridge-tally-protocol/src/lib.rs, whose filename says nothing about
// parsing but which is the single largest parser in this repository.
//
// "New accept-path test" / "new reject-path test" are read off the *diff*
// (only `#[test]`-family functions newly added in this change), classified by
// splitting the function name on `_` and checking for these tokens:
//   accept: valid, accepts, accept, success, succeeds, ok, happy, wellformed, roundtrip
//   reject: invalid, rejects, reject, malformed, error, errors, fails, fail,
//            corrupt, missing, unexpected, bad, truncated, garbage, wrong, unparseable
// A name carrying both is counted as reject (it exercises the reject path,
// which is what this gate is actually trying to make sure exists at least
// once). A name carrying neither is not counted either way — this is
// deliberately conservative: an unclassifiable test name should not produce a
// false failure, and does not launder a real gap either, since it still is
// not a *reject*-classified test.
//
// Residual, stated rather than hidden: this cannot verify a reject-test
// actually asserts rejection (only that its name says so), and it is blind to
// a reject-test added to a *different* file (a shared `parser_test_helpers.rs`,
// say) that exercises the same parser. Both are human-review territory; this
// gate only makes the mechanical half — "did anything reject-shaped get
// added at all" — impossible to skip by accident.

import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

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

const PARSE_FN = /\bfn\s+([A-Za-z0-9_]*[Pp]arse[A-Za-z0-9_]*)\s*[(<]/;
const TEST_ATTRIBUTE = /^#\[(?:test|tokio::test|async_std::test)\]\s*$/;
const FN_SIGNATURE = /^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*\(/;

const ACCEPT_TOKENS = new Set([
  "valid", "accepts", "accept", "success", "succeeds", "ok", "happy", "wellformed", "roundtrip",
]);
const REJECT_TOKENS = new Set([
  "invalid", "rejects", "reject", "malformed", "error", "errors", "fails", "fail",
  "corrupt", "missing", "unexpected", "bad", "truncated", "garbage", "wrong", "unparseable",
]);

function classify(name) {
  const tokens = name.toLowerCase().split("_");
  const hasReject = tokens.some((token) => REJECT_TOKENS.has(token));
  if (hasReject) return "reject";
  const hasAccept = tokens.some((token) => ACCEPT_TOKENS.has(token));
  if (hasAccept) return "accept";
  return null;
}

function git(args) {
  return execFileSync("git", args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
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

// Newly-added `#[test]`-family function names, read off a -U0 diff: every
// line of a genuinely new test (attribute, signature, and body) is a `+`
// line, contiguous in the hunk, so scanning the added lines in order and
// pairing each test attribute with the next `fn` line finds them without
// needing to parse the whole file.
function newlyAddedTestNames(path, base) {
  const diff = git(["diff", "-U0", base, "--", path]);
  const addedLines = diff
    .split("\n")
    .filter((line) => line.startsWith("+") && !line.startsWith("+++"))
    .map((line) => line.slice(1));

  const names = [];
  let pendingTestAttribute = false;
  for (const line of addedLines) {
    const trimmed = line.trim();
    if (!trimmed.length) continue;
    if (TEST_ATTRIBUTE.test(trimmed)) {
      pendingTestAttribute = true;
      continue;
    }
    if (!pendingTestAttribute) continue;
    const signature = FN_SIGNATURE.exec(trimmed);
    if (signature) names.push(signature[1]);
    // Tolerate other attributes (`#[allow(...)]`, `#[should_panic]`, ...)
    // stacked between `#[test]` and the `fn` line by only clearing the
    // pending flag once we hit something that is not itself an attribute.
    if (signature || !trimmed.startsWith("#[")) pendingTestAttribute = false;
  }
  return names;
}

function isParserModule(path) {
  if (!path.endsWith(".rs")) return false;
  const absolute = resolve(repositoryRoot, path);
  if (!existsSync(absolute)) return false; // Deleted in this diff.
  const text = readFileSync(absolute, "utf8");
  return PARSE_FN.test(text);
}

const base = resolveBase();
if (!base) {
  console.warn(
    "note: no base revision reachable (shallow clone?) — parser accept/reject " +
      "test symmetry was NOT checked.",
  );
  process.exit(0);
}

const changedFiles = git(["diff", "--name-only", base, "--"]).split("\n").filter(Boolean);

const failures = [];
let parserFilesChecked = 0;

for (const path of changedFiles) {
  if (!isParserModule(path)) continue;
  parserFilesChecked += 1;

  const names = newlyAddedTestNames(path, base);
  const accepted = names.filter((name) => classify(name) === "accept");
  const rejected = names.filter((name) => classify(name) === "reject");
  if (!accepted.length || rejected.length) continue;

  failures.push(
    `${path}: ${accepted.length} new accept-path test(s) added ` +
      `(${accepted.join(", ")}) with no new reject-path test in the same diff — ` +
      "add a test asserting this parser rejects a malformed/invalid input too " +
      '(name it with "invalid"/"rejects"/"malformed"/... so this gate recognises it)',
  );
}

if (failures.length) {
  throw new Error(
    `parser accept/reject test symmetry (${failures.length} problem(s), base ${base}):\n` +
      failures.map((line) => `  - ${line}`).join("\n"),
  );
}

console.log(
  `Parser accept/reject test symmetry holds against ${base} ` +
    `(${parserFilesChecked} parser module(s) touched, checked).`,
);
