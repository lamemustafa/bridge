// SPDX-License-Identifier: Apache-2.0

import { readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const rootArgument = process.argv.indexOf("--root");
if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
  throw new Error("--root requires a repository path");
}
const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);

// These are legacy, deliberately quarantined request profiles. The match is
// exact: removing one does not create capacity for another, and adding either
// hazard anywhere else fails this gate. New exceptions require reviewed edits
// to this list and a distinct request-profile decision.
const expected = new Set([
  "custom-report|src-tauri/crates/bridge-tally-protocol/src/xml_read_profiles.rs::render_company_list|Company Report",
  "custom-report|src-tauri/crates/bridge-tally-protocol/src/xml_read_profiles.rs::render_ledgers|BRIDGE Ledger Export V1",
  "custom-report|src-tauri/crates/bridge-tally-protocol/src/xml_read_profiles.rs::render_vouchers|BRIDGE Voucher Export V2",
  "custom-report|src-tauri/src/tally/tdl_engine.rs::groups_request|BRIDGE Group Export V1",
  "custom-report|src-tauri/src/tally/tdl_engine.rs::ledger_period_balances_request|BRIDGE Ledger Period Balances V1",
  "custom-report|src-tauri/src/tally/tdl_engine.rs::legacy_company_list_request|Company Report",
  "function-argument-with-space|src-tauri/crates/bridge-tally-protocol/src/xml_read_profiles.rs::render_ledgers|$$NumItems:BRIDGE Ledger Collection V1",
  "function-argument-with-space|src-tauri/crates/bridge-tally-protocol/src/xml_read_profiles.rs::render_vouchers|$$NumItems:BRIDGE Voucher Collection V1",
  "function-argument-with-space|src-tauri/src/tally/tdl_engine.rs::groups_request|$$NumItems:BRIDGE Group Collection V1",
  "function-argument-with-space|src-tauri/src/tally/tdl_engine.rs::ledger_period_balances_request|$$NumItems:BRIDGE Ledger Period Collection V1",
]);

const actual = new Set();
for (const sourceRoot of ["src-tauri", "tools"]) {
  for (const path of rustFiles(resolve(repositoryRoot, sourceRoot))) {
    scanRequestBuilderStrings(repositoryRoot, path, actual);
  }
}

const unexpected = [...actual].filter((violation) => !expected.has(violation)).sort();
const missing = [...expected].filter((violation) => !actual.has(violation)).sort();
if (unexpected.length || missing.length) {
  throw new Error(
    "Tally request-builder hazard allowlist changed:\n" +
      (unexpected.length ? `unexpected:\n${unexpected.map((value) => `- ${value}`).join("\n")}\n` : "") +
      (missing.length ? `missing:\n${missing.map((value) => `- ${value}`).join("\n")}\n` : "") +
      "Use a native Collection export by default; a new exception requires a reviewed exact-set update.",
  );
}

console.log(`Tally request-builder hazards match the pinned set (${actual.size} violations).`);

function rustFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (["target", ".git"].includes(entry.name)) continue;
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...rustFiles(path));
    else if (entry.isFile() && entry.name.endsWith(".rs")) files.push(path);
  }
  return files;
}

function scanRequestBuilderStrings(repositoryRoot, path, violations) {
  const source = readFileSync(path, "utf8");
  // Deliberately scan only hash-delimited raw strings. Extending this to
  // ordinary strings finds test-helper expectations such as
  // `exact_report_collection_is_shared_by_count_and_rows` as if they were
  // request builders, creating false positives without a Rust AST capable of
  // distinguishing a dispatched profile from test data. The `tools/` tree is
  // still covered so a native Rust builder cannot hide outside `src-tauri`.
  for (const literal of rawStrings(source)) {
    const identifier = enclosingFunction(source, literal.start);
    const file = relative(repositoryRoot, path).replaceAll("\\", "/");
    for (const match of literal.value.matchAll(/\$\$[A-Za-z_][A-Za-z0-9_]*:([^<\r\n]*)/g)) {
      const expression = match[0].trim();
      if (/\s/.test(match[1])) {
        violations.add(`function-argument-with-space|${file}::${identifier}|${expression}`);
      }
    }
    for (const match of literal.value.matchAll(/<REPORT\s+NAME="([^"]+)"/g)) {
      violations.add(`custom-report|${file}::${identifier}|${match[1]}`);
    }
  }
}

function rawStrings(source) {
  const strings = [];
  for (let start = source.indexOf("r"); start !== -1; start = source.indexOf("r", start + 1)) {
    let cursor = start + 1;
    while (source[cursor] === "#") cursor += 1;
    if (source[cursor] !== '"') continue;
    const hashes = source.slice(start + 1, cursor);
    // The repository's XML builders use hash-delimited raw strings. Requiring
    // that delimiter avoids treating ordinary source text ending in `r` and a
    // following quoted string as a Rust raw literal.
    if (!hashes.length) continue;
    const endMarker = `"${hashes}`;
    const valueStart = cursor + 1;
    const end = source.indexOf(endMarker, valueStart);
    if (end === -1) throw new Error(`unterminated Rust raw string in ${start}`);
    strings.push({ start, value: source.slice(valueStart, end) });
    start = end + endMarker.length - 1;
  }
  return strings;
}

function enclosingFunction(source, position) {
  const prefix = source.slice(0, position);
  const functions = [...prefix.matchAll(/(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z0-9_]+)/g)];
  return functions.at(-1)?.[1] ?? "<module>";
}
