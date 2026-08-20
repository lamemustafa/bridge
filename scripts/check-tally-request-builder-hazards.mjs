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
    // "tests" is skipped here (integration-test crates live entirely under a
    // top-level tests/ directory) as part of the test-quarantine strategy
    // described in scanRequestBuilderStrings() below.
    if (["target", ".git", "tests"].includes(entry.name)) continue;
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...rustFiles(path));
    else if (entry.isFile() && entry.name.endsWith(".rs")) files.push(path);
  }
  return files;
}

function scanRequestBuilderStrings(repositoryRoot, path, violations) {
  const file = relative(repositoryRoot, path).replaceAll("\\", "/");
  // Defense in depth: rustFiles() already prunes any directory literally
  // named "tests", but re-check the relative path so a future traversal
  // change can't silently start pulling integration-test files back in.
  if (file.split("/").includes("tests")) return;

  const source = readFileSync(path, "utf8");
  // Scan all three Rust string forms a request builder could use:
  // hash-delimited raw strings (r#"..."#), zero-hash raw strings (r"..."),
  // and ordinary escaped strings ("..."). Comments (//, /* */, nested) are
  // skipped so commented-out hazards don't trip the gate.
  //
  // Test-quarantine strategy: strings are skipped when they fall inside a
  // #[cfg(test)] *module* body (specifically `#[cfg(test)] mod name { ... }`,
  // tracked via brace-depth) or inside a file under a tests/ directory
  // (integration tests). This is deliberate, not incidental: unit tests such
  // as `exact_report_collection_is_shared_by_count_and_rows`, which live
  // inside `#[cfg(test)] mod tests { ... }`, assert against string literals
  // containing `<REPORT NAME="...">` or `$$NumItems:... With Spaces` as
  // *expectations*, not as a dispatched request. Those are not request
  // builders and must not be scanned.
  //
  // Deliberately narrower than "skip anything under #[cfg(test)]": a bare
  // #[cfg(test)] fn (not inside a mod) is still scanned, because this
  // repository pins exactly that shape as a real hazard --
  // `legacy_company_list_request` in tdl_engine.rs is a top-level
  // `#[cfg(test)] fn` whose body *is* the rendered request, kept only to
  // assert byte-parity with the production renderer. Excluding all
  // #[cfg(test)] items would silently drop it from the pinned set.
  //
  // What this still cannot catch:
  //  - A hazard assembled at runtime via string concatenation/format!
  //    across multiple literals (no single literal contains the full
  //    pattern).
  //  - A hazard placed in a bare #[cfg(test)] fn/const/impl (not a `mod`)
  //    that is purely a test fixture, not a request builder -- it will
  //    still be scanned and, if it happens to contain hazard-shaped text,
  //    flagged. That is a false-positive risk, not a missed-hazard one.
  //  - A hazard built from a `const`/`static` marked #[cfg(test)] without a
  //    brace-delimited body (e.g. `#[cfg(test)] const X: &str = "...";`) --
  //    scanned the same way, same false-positive-only risk.
  //  - A zero-hash raw string (r"...") can never itself contain a `"`
  //    character (Rust raw strings have no escape mechanism at all), so it
  //    can carry a function-argument-with-space hazard but never the
  //    quote-bearing custom-report `<REPORT NAME="...">` hazard -- that is a
  //    fact about Rust's grammar, not a gap in this scanner.
  for (const literal of scanStrings(source)) {
    if (literal.insideTest) continue;
    const identifier = enclosingFunction(source, literal.start);
    for (const match of literal.value.matchAll(/\$\$[A-Za-z_][A-Za-z0-9_]*:/g)) {
      const afterColon = match.index + match[0].length;
      // A TDL function argument that opens with a double quote is a quoted
      // span: the real argument is whatever sits between that quote and the
      // next one, and only whitespace *inside* the quotes is a hazard.
      // Text after the closing quote (e.g. the rest of a larger XML-escaped
      // expression like `AND $Date &lt;= $$Date:"..."`) is not part of this
      // argument at all, so it must not be swallowed into the check --
      // that was the false-positive source this quote-aware branch fixes.
      // An unquoted argument keeps the pre-existing behaviour exactly: it
      // runs to the next real `<` or newline, and any whitespace anywhere
      // in that span is a hazard.
      if (literal.value[afterColon] === '"') {
        const closeQuote = literal.value.indexOf('"', afterColon + 1);
        if (closeQuote !== -1) {
          const inner = literal.value.slice(afterColon + 1, closeQuote);
          if (/\s/.test(inner)) {
            const expression = `${match[0]}${literal.value.slice(afterColon, closeQuote + 1)}`.trim();
            violations.add(`function-argument-with-space|${file}::${identifier}|${expression}`);
          }
          continue;
        }
        // No closing quote found -- fall through to the unquoted scan below
        // so a malformed literal is still checked rather than silently
        // skipped.
      }
      const rest = literal.value.slice(afterColon);
      const unquoted = /^([^<\r\n]*)/.exec(rest)[1];
      if (/\s/.test(unquoted)) {
        const expression = `${match[0]}${unquoted}`.trim();
        violations.add(`function-argument-with-space|${file}::${identifier}|${expression}`);
      }
    }
    for (const match of literal.value.matchAll(/<REPORT\s+NAME="([^"]+)"/g)) {
      violations.add(`custom-report|${file}::${identifier}|${match[1]}`);
    }
  }
}

// Single forward pass over the source that recognises (and skips) line and
// block comments, char literals, and #[cfg(test)] *module* bodies, while
// collecting every raw (r#"..."#, r"...") and ordinary ("...", escape-aware)
// string literal found in "production" code.
function scanStrings(source) {
  const strings = [];
  const n = source.length;
  const cfgTestAttribute = /^#\[\s*cfg\s*\(\s*test\s*\)\s*\]/;
  const charLiteral = /^'(?:\\(?:['"\\nrt0]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\})|[^'\\\n])'/;
  // Only a #[cfg(test)] attribute immediately (modulo whitespace and other
  // attributes) followed by `mod name {` opens a quarantined test region --
  // see the "Deliberately narrower" note in scanRequestBuilderStrings().
  const cfgTestModuleAhead = /^(?:(?:pub(?:\([^)]*\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{)/;

  function nextItemIsTestModule(position) {
    let cursor = position;
    for (;;) {
      while (cursor < n && /\s/.test(source[cursor])) cursor += 1;
      if (source[cursor] === "#" && source[cursor + 1] === "[") {
        let depth = 0;
        let j = cursor + 1;
        while (j < n) {
          if (source[j] === "[") depth += 1;
          else if (source[j] === "]") {
            depth -= 1;
            j += 1;
            if (depth === 0) break;
            continue;
          }
          j += 1;
        }
        cursor = j;
        continue;
      }
      break;
    }
    return cfgTestModuleAhead.test(source.slice(cursor, cursor + 200));
  }

  let i = 0;
  let braceDepth = 0;
  let pendingCfgTest = false;
  const testStack = []; // brace depths at which a #[cfg(test)] item body opened

  while (i < n) {
    const two = source.slice(i, i + 2);

    if (two === "//") {
      const end = source.indexOf("\n", i);
      i = end === -1 ? n : end;
      continue;
    }

    if (two === "/*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        const pair = source.slice(i, i + 2);
        if (pair === "/*") {
          depth += 1;
          i += 2;
        } else if (pair === "*/") {
          depth -= 1;
          i += 2;
        } else {
          i += 1;
        }
      }
      continue;
    }

    const cfgMatch = cfgTestAttribute.exec(source.slice(i, i + 64));
    if (cfgMatch) {
      const afterAttribute = i + cfgMatch[0].length;
      if (nextItemIsTestModule(afterAttribute)) pendingCfgTest = true;
      i = afterAttribute;
      continue;
    }

    const ch = source[i];

    if (ch === "'") {
      const charMatch = charLiteral.exec(source.slice(i, i + 10));
      if (charMatch) {
        i += charMatch[0].length;
        continue;
      }
      // Not a char literal (e.g. a lifetime like 'a) -- fall through as a
      // plain character so lifetimes never trip the raw/ordinary parsers.
      i += 1;
      continue;
    }

    if (ch === "r" && (source[i + 1] === '"' || source[i + 1] === "#")) {
      let cursor = i + 1;
      let hashCount = 0;
      while (source[cursor] === "#") {
        hashCount += 1;
        cursor += 1;
      }
      if (source[cursor] === '"') {
        const hashes = "#".repeat(hashCount);
        const endMarker = `"${hashes}`;
        const valueStart = cursor + 1;
        const end = source.indexOf(endMarker, valueStart);
        if (end === -1) throw new Error(`unterminated Rust raw string at offset ${i}`);
        strings.push({
          start: i,
          value: source.slice(valueStart, end),
          insideTest: testStack.length > 0,
        });
        i = end + endMarker.length;
        continue;
      }
      // Looked like a raw-string prefix but wasn't (e.g. an identifier
      // starting with "r"); treat the "r" as an ordinary character.
    }

    if (ch === '"') {
      let cursor = i + 1;
      let value = "";
      let terminated = false;
      while (cursor < n) {
        const c = source[cursor];
        if (c === "\\" && cursor + 1 < n) {
          // Escape-aware: \" does not end the string, and \\ consumes only
          // the escaped backslash, so a following quote (as in \\") is a
          // real, unescaped terminator.
          const next = source[cursor + 1];
          if (next === '"') value += '"';
          else if (next === "\\") value += "\\";
          else value += next;
          cursor += 2;
          continue;
        }
        if (c === '"') {
          cursor += 1;
          terminated = true;
          break;
        }
        value += c;
        cursor += 1;
      }
      if (!terminated) throw new Error(`unterminated Rust string literal at offset ${i}`);
      strings.push({ start: i, value, insideTest: testStack.length > 0 });
      i = cursor;
      continue;
    }

    if (ch === "{") {
      braceDepth += 1;
      if (pendingCfgTest) {
        testStack.push(braceDepth);
        pendingCfgTest = false;
      }
      i += 1;
      continue;
    }

    if (ch === "}") {
      if (testStack.length && testStack[testStack.length - 1] === braceDepth) {
        testStack.pop();
      }
      braceDepth = Math.max(0, braceDepth - 1);
      i += 1;
      continue;
    }

    if (ch === ";" || ch === ",") {
      // The pending #[cfg(test)] attribute applied to an item with no
      // brace-delimited body (a `use`/`const`/struct field/...); there is
      // nothing to push onto testStack, so just stop tracking it rather
      // than letting it leak onto an unrelated later brace.
      pendingCfgTest = false;
      i += 1;
      continue;
    }

    i += 1;
  }

  return strings;
}

function enclosingFunction(source, position) {
  const prefix = source.slice(0, position);
  const functions = [...prefix.matchAll(/(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z0-9_]+)/g)];
  return functions.at(-1)?.[1] ?? "<module>";
}
