// SPDX-License-Identifier: Apache-2.0
//
// "Missing bound/cap on unbounded output or resource" was 49 of the 880
// findings this coverage is built from. The single most common concrete shape
// of that in a Rust codebase is `Read::read_to_end`/`Read::read_to_string`
// called on a reader that can hand back an attacker- or environment-sized
// amount of data (stdin, a subprocess pipe, a network response, a file whose
// size this process does not control) with nothing capping how much of it
// gets pulled into memory.
//
// This repo already has the fix pattern in wide use — `reader.take(N +
// 1).read_to_end(&mut buf)`, reading one byte past a declared limit so an
// over-limit input is detectable rather than silently truncated (see
// src-tauri/src/agent_bank_statement.rs, agent_desktop_journal.rs, source_draft/files.rs,
// tools/bridge-tally-qualification). This gate checks that every such call
// site actually uses it, so a new call site that forgets the `.take(...)` is
// caught mechanically instead of depending on a reviewer noticing.
//
// Two things a naive `grep` gets wrong, both handled below:
//
//   1. `quick_xml::Reader::read_to_end(end: QName)` is a same-named, wholly
//      different method — "skip forward to this closing tag", bounded by the
//      document's own structure, not a byte sink. `std::io::Read`'s two
//      methods both take `&mut <buffer>` as their argument; quick_xml's does
//      not. Requiring the argument to start with `&mut` is what tells them
//      apart, and bridge-tally-protocol/src/lib.rs has six of exactly this
//      quick_xml call that a bare substring match would misreport.
//   2. A test fixture is allowed to read a small, test-authored buffer
//      unbounded — the hazard is untrusted or environment-sized input, and a
//      `#[test]` function's input is neither. Calls inside a `#[cfg(test)]`
//      module or a `#[test]`/`#[tokio::test]`-attributed function are
//      excluded, tracked by indentation rather than brace-counting: this
//      codebase is rustfmt-clean (see rustfmt.toml and the reported
//      `cargo fmt --check` count), so indentation reliably marks scope, and
//      unlike brace-counting it cannot be thrown off by a `{`/`}` inside a
//      string or XML/JSON literal — of which this codebase's parser tests
//      have many.
//
// Residual, stated rather than hidden: this only recognises the *direct*
// `.take(...)` -> `.read_to_end`/`.read_to_string` chain, matched by scanning
// backward from the read call to the start of its statement (a line ending
// `;`, `{`, or `}` is a boundary). A reader that was capped further back —
// wrapped in a limiting adapter and bound to a variable on an earlier
// statement, then read from later — is not recognised and reports a false
// positive; ALLOWED_UNBOUNDED below is the reviewed escape hatch for that.

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { relative, resolve } from "node:path";

const scriptRoot = fileURLToPath(new URL("../", import.meta.url));
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

const SOURCE_ROOTS = ["src-tauri/src", "src-tauri/crates", "tools"];
const READ_CALL = /\.read_to_(?:end|string)\(\s*&mut\b/;
const TAKE_CALL = /\.take\(/;
const STATEMENT_BOUNDARY = /[;{}]\s*$/;
const TEST_ATTRIBUTE = /^#\[(?:test|tokio::test|async_std::test|wasm_bindgen_test)\]\s*$/;
const CFG_TEST = /^#\[cfg\(test\)\]\s*$/;
const FN_LINE = /^(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s/;
const MOD_LINE = /^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{?\s*$/;
const MAX_LOOKBACK = 12;

// Reviewed, named exceptions — never a bare count. Each entry documents why
// the direct-chain heuristic above cannot see that this call site is already
// bounded, so the exception is legible on its own without re-deriving it.
const ALLOWED_UNBOUNDED = new Set([
  // Zip entries read from a template XLSX/PDF bundled into the binary at
  // build time (via `include_bytes!` / the packaged app resources), not from
  // an untrusted or attacker-sized source. Tracked as a known gap rather than
  // silently accepted: docs/proposed-ci-gates.md's REPORTING entry for this
  // gate lists these as the concrete class-1 findings this scan actually
  // surfaced.
  "src-tauri/src/reports/outstandings_working_paper_xlsx.rs",
  "src-tauri/src/reports/trial_balance_xlsx.rs",
  "src-tauri/src/reports/party_statement_pdf.rs",
]);

function listSourceFiles() {
  const files = [];
  for (const sourceRoot of SOURCE_ROOTS) {
    const result = execFileSync(
      "git",
      ["ls-files", "-z", "--", sourceRoot],
      { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    );
    for (const path of result.split("\0")) {
      if (path && path.endsWith(".rs")) files.push(path);
    }
  }
  return files;
}

function isExcludedByPath(path) {
  return path.includes("/tests/") || /_tests?\.rs$/.test(path);
}

// Returns, for each line index, whether that line is inside a `#[cfg(test)]`
// module or a `#[test]`-family-attributed function — tracked by indentation,
// not braces (see file banner for why).
function testScopeMask(lines) {
  const inside = new Array(lines.length).fill(false);
  const stack = []; // { indent }
  let pendingCfgTest = false;
  let pendingTestAttribute = false;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmed = line.trim();

    // Pop any scope whose owning line we have now dedented past or to.
    if (trimmed.length) {
      const indent = line.length - line.trimStart().length;
      while (stack.length && indent <= stack[stack.length - 1].indent) stack.pop();
    }

    inside[index] = stack.length > 0;

    if (!trimmed.length) continue; // Blank lines do not reset a pending attribute.

    if (CFG_TEST.test(trimmed)) {
      pendingCfgTest = true;
      continue;
    }
    if (TEST_ATTRIBUTE.test(trimmed)) {
      pendingTestAttribute = true;
      continue;
    }

    const indent = line.length - line.trimStart().length;
    if (pendingCfgTest) {
      const mod = MOD_LINE.exec(line);
      if (mod) stack.push({ indent });
      pendingCfgTest = false;
    } else if (pendingTestAttribute) {
      const fn = FN_LINE.exec(line);
      if (fn) stack.push({ indent });
      pendingTestAttribute = false;
    } else {
      // Any other attribute line (`#[derive(...)]`, `#[allow(...)]`, ...)
      // between the marker and its target is tolerated by simply not
      // clearing pending* here — but only for attribute lines, so a real
      // statement in between correctly drops a stale pending marker.
      if (!/^#\[/.test(trimmed)) {
        pendingCfgTest = false;
        pendingTestAttribute = false;
      }
    }
  }
  return inside;
}

function isBounded(lines, index) {
  if (TAKE_CALL.test(lines[index])) return true;
  let steps = 0;
  for (let cursor = index - 1; cursor >= 0 && steps < MAX_LOOKBACK; cursor -= 1, steps += 1) {
    const line = lines[cursor];
    if (STATEMENT_BOUNDARY.test(line.trimEnd())) return false;
    if (TAKE_CALL.test(line)) return true;
  }
  return false;
}

const failures = [];
let scanned = 0;
let boundedCount = 0;

for (const path of listSourceFiles()) {
  if (isExcludedByPath(path)) continue;
  const absolute = resolve(repositoryRoot, path);
  const text = readFileSync(absolute, "utf8");
  const lines = text.split("\n");
  const inTestScope = testScopeMask(lines);

  lines.forEach((line, index) => {
    if (!READ_CALL.test(line)) return;
    scanned += 1;
    if (inTestScope[index]) return;
    if (isBounded(lines, index)) {
      boundedCount += 1;
      return;
    }
    if (ALLOWED_UNBOUNDED.has(path)) return;
    failures.push(`${path}:${index + 1}: ${line.trim()}`);
  });
}

if (failures.length) {
  throw new Error(
    `${failures.length} unbounded Read::read_to_end/read_to_string call(s) found — each ` +
      "reads an unbounded amount of external data into memory with no `.take(N)` cap " +
      "in the same statement. Wrap the reader in `.take(limit + 1)` first (see " +
      "src-tauri/src/agent_bank_statement.rs for the pattern — the `+ 1` lets an " +
      "over-limit input be detected rather than silently truncated), or add a " +
      "reviewed entry to ALLOWED_UNBOUNDED in this script with the reason:\n" +
      failures.map((line) => `  - ${line}`).join("\n"),
  );
}

console.log(
  `Read bound coverage holds: ${scanned} read_to_end/read_to_string call site(s) scanned, ` +
    `${boundedCount} bounded, 0 unbounded (${ALLOWED_UNBOUNDED.size} reviewed exception(s)).`,
);
