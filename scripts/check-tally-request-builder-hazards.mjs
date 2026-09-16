// SPDX-License-Identifier: Apache-2.0

import { readFileSync, readdirSync, realpathSync } from "node:fs";
import { basename, dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// These are legacy, deliberately quarantined request profiles. The match is
// exact: removing one does not create capacity for another, and adding either
// hazard anywhere else fails this gate. New exceptions require reviewed edits
// to this list and a distinct request-profile decision.
export const expected = new Set([
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

// The module files the scan skips as test code, pinned exactly for the same
// reason the violations are. The quarantine below decides which files are test
// modules, but it lexes Rust and cannot see every way a file can be loaded; a
// mistake there must surface as a named diff here, not as a quieter gate. When
// you extract a test module, confirm the new file is loaded only under
// #[cfg(test)] and add it; nothing else should ever be added.
export const EXPECTED_TEST_MODULE_FILES = new Set([
  "src-tauri/crates/bridge-tally-core/src/book_presence_tests.rs",
  "src-tauri/crates/bridge-tally-core/src/master_binding_tests.rs",
  "src-tauri/crates/bridge-tally-protocol/src/native_outstandings/wire_currency_tests.rs",
  "src-tauri/crates/bridge-tally-protocol/src/native_outstandings/wire_group_tests.rs",
  "src-tauri/crates/bridge-tally-protocol/src/native_trial_balance/tests.rs",
  "src-tauri/src/agent_admission_tests.rs",
  "src-tauri/src/agent_company_identity_tests.rs",
  "src-tauri/src/agent_company_tuple_tests.rs",
  "src-tauri/src/agent_delivery_tests.rs",
  "src-tauri/src/agent_desktop_journal_tests.rs",
  "src-tauri/src/agent_egress_tests.rs",
  "src-tauri/src/agent_failure_tests.rs",
  "src-tauri/src/agent_financial_profile_tests.rs",
  "src-tauri/src/agent_import_amend_tests.rs",
  "src-tauri/src/agent_import_bank_tests.rs",
  "src-tauri/src/agent_import_boundary_tests.rs",
  "src-tauri/src/agent_import_file_tests.rs",
  "src-tauri/src/agent_import_identity_tests.rs",
  "src-tauri/src/agent_import_index_tests.rs",
  "src-tauri/src/agent_import_ledger_stream_tests.rs",
  "src-tauri/src/agent_import_ledger_tests.rs",
  "src-tauri/src/agent_import_mode_tests.rs",
  "src-tauri/src/agent_import_multiplicity_tests.rs",
  "src-tauri/src/agent_import_persistence_tests.rs",
  "src-tauri/src/agent_import_post_tests.rs",
  "src-tauri/src/agent_import_preflight_tests.rs",
  "src-tauri/src/agent_import_qualification_tests.rs",
  "src-tauri/src/agent_import_source_tests.rs",
  "src-tauri/src/agent_import_tests.rs",
  "src-tauri/src/agent_import_text_tests.rs",
  "src-tauri/src/agent_import_verify_mode_tests.rs",
  "src-tauri/src/agent_lab_import_tests.rs",
  "src-tauri/src/agent_movement_snapshot_tests.rs",
  "src-tauri/src/agent_movement_tests.rs",
  "src-tauri/src/agent_outstandings_tests.rs",
  "src-tauri/src/agent_post_cancellation_tests.rs",
  "src-tauri/src/agent_post_recovery_tests.rs",
  "src-tauri/src/agent_presence_tests.rs",
  "src-tauri/src/agent_protocol_cap_tests.rs",
  "src-tauri/src/agent_protocol_evidence_tests.rs",
  "src-tauri/src/agent_protocol_redaction_tests.rs",
  "src-tauri/src/agent_protocol_tests.rs",
  "src-tauri/src/agent_receipt_fields_tests.rs",
  "src-tauri/src/agent_recovery_tests.rs",
  "src-tauri/src/agent_response_tests.rs",
  "src-tauri/src/agent_status_identity_tests.rs",
  "src-tauri/src/agent_status_tests.rs",
  "src-tauri/src/agent_tests.rs",
  "src-tauri/src/agent_voucher_parse_tests.rs",
  "src-tauri/src/agent_voucher_selection_tests.rs",
  "src-tauri/src/agent_wire_evidence_tests.rs",
  "src-tauri/src/commands_native_ledger_tests.rs",
  "src-tauri/src/commands_party_statement_export_tests.rs",
  "src-tauri/src/commands_statement_export_tests.rs",
  "src-tauri/src/commands_tests.rs",
  "src-tauri/src/db/tally_capability_license_tests.rs",
  "src-tauri/src/db/tally_mirror_tests.rs",
  "src-tauri/src/db/tally_write_store_tests.rs",
  "src-tauri/src/endpoint_coordination_macos_tests.rs",
  "src-tauri/src/endpoint_coordination_tests.rs",
  "src-tauri/src/local_files/paths_tests.rs",
  "src-tauri/src/sync/coordinator_lease_tests.rs",
  "src-tauri/src/sync/reconciliation_tests.rs",
  "src-tauri/src/sync/snapshot_tests.rs",
  "src-tauri/src/tally/connection_tests.rs",
  "src-tauri/src/tally/runtime_agent_read_evidence_tests.rs",
  "src-tauri/src/tally/runtime_failure_evidence_inventory_tests.rs",
  "src-tauri/src/tally/runtime_financial_mode_tests.rs",
  "src-tauri/src/tally/runtime_import_admission_tests.rs",
  "src-tauri/src/tally/runtime_ledger_opening_tests.rs",
  "src-tauri/src/tally/runtime_outstandings_currency_tests.rs",
  "src-tauri/src/tally/runtime_party_evidence_tests.rs",
  "src-tauri/src/tally/runtime_tests.rs",
  "src-tauri/src/tally/runtime_trial_balance_tests.rs",
]);

export function collect(repositoryRoot) {
  const testModules = testOnlyModuleFiles(repositoryRoot);
  const violations = new Set();
  const skipped = new Set();
  for (const sourceRoot of ["src-tauri", "tools"]) {
    for (const path of rustFiles(resolve(repositoryRoot, sourceRoot))) {
      const file = relativePath(repositoryRoot, path);
      if (testModules.has(file)) {
        skipped.add(file);
        continue;
      }
      scanRequestBuilderStrings(repositoryRoot, path, violations);
    }
  }
  return { violations, skipped };
}

function main() {
  const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const rootArgument = process.argv.indexOf("--root");
  if (rootArgument !== -1 && !process.argv[rootArgument + 1]) {
    throw new Error("--root requires a repository path");
  }
  const repositoryRoot = rootArgument === -1 ? scriptRoot : resolve(process.argv[rootArgument + 1]);
  const { violations: actual, skipped } = collect(repositoryRoot);

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
  if (rootArgument === -1) {
    const newlySkipped = [...skipped].filter((file) => !EXPECTED_TEST_MODULE_FILES.has(file)).sort();
    const noLongerSkipped = [...EXPECTED_TEST_MODULE_FILES].filter((file) => !skipped.has(file)).sort();
    if (newlySkipped.length || noLongerSkipped.length) {
      throw new Error(
        "test-module quarantine changed:\n" +
          (newlySkipped.length ? `newly skipped:\n${newlySkipped.map((value) => `- ${value}`).join("\n")}\n` : "") +
          (noLongerSkipped.length ? `no longer skipped:\n${noLongerSkipped.map((value) => `- ${value}`).join("\n")}\n` : "") +
          "Add a file only after confirming it is loaded solely under #[cfg(test)].",
      );
    }
  }
  console.log(
    `Tally request-builder hazards match the pinned set (${actual.size} violations; ` +
      `${skipped.size} test-only module files skipped).`,
  );
}

function relativePath(repositoryRoot, path) {
  return relative(repositoryRoot, path).replaceAll("\\", "/");
}

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
  // (integration tests). The same module moved to its own file and loaded by
  // `#[cfg(test)] #[path = "..."] mod name;` is skipped whole by collect();
  // see testOnlyModuleFiles() at the end of this file for how that is decided. This is deliberate, not incidental: unit tests such
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

// ---------------------------------------------------------------------------
// Test-module quarantine for extracted test files.
//
// An inline `#[cfg(test)] mod tests { ... }` body is skipped by scanStrings().
// The same module moved to its own file -- `#[cfg(test)] #[path = "x_tests.rs"]
// mod tests;` -- would otherwise be scanned as production code. This finds the
// files that are loaded only as test modules and lets collect() skip them.
//
// Every rule errs toward scanning: a wrong answer here may raise a false alarm,
// but must never skip a production file. File names are never trusted.
//
// A file is test-only when a trusted edge reaches it and no veto names it:
//
//  Edges (only from files whose brace depth never underflows and ends at 0):
//   - a brace-depth-0 out-of-line `mod name;` whose attribute group has a
//     `cfg` implying `test` (`cfg(test)`, or `test` as an argument of
//     `cfg(all(...))`), with `#[path]`, or bare in a crate root or mod.rs;
//   - any brace-depth-0 out-of-line `mod name;` declared *by* a test-only file
//     (it cannot compile unless its parent does), iterated to a fixed point.
//   `#[path]` resolves beside the declaring file (Rust Reference, "The path
//   attribute", for modules not inside an inline module); a bare `mod name;`
//   resolves beside a file that owns its directory and under `<stem>/` otherwise.
//
//  Vetoes, collected from every .rs file in the repository except those already
//  test-only, and matched by basename case-insensitively so they over-veto:
//   - any `mod name;` at any depth that is not itself an edge (`name.rs`,
//     `name/mod.rs`), including inside inline modules and macro bodies;
//   - any string literal written as `path = "..."` or passed to `include!`,
//     `include_str!` or `include_bytes!` that is not part of an edge, which
//     covers `#[cfg_attr(..., path = "...")]`;
//   - every Cargo crate root, by exact path.
//
// Not handled, all of which leave a file scanned: `cfg(any(...))` and other
// cfg shapes, `#[path]` inside an inline module, `mod r#name;`.
// ---------------------------------------------------------------------------

export function testOnlyModuleFiles(repositoryRoot) {
  const files = allFiles(repositoryRoot);
  const rust = files.filter((file) => file.endsWith(".rs"));
  const exists = new Set(files);
  const sources = new Map(rust.map((file) => [file, readFileSync(resolve(repositoryRoot, file), "utf8")]));
  for (const [file, source] of sources) refuseUnrecognisedLoaders(file, source);
  const lexed = new Map([...sources].map(([file, source]) => [file, lexModules(source)]));
  const roots = crateRoots(repositoryRoot, files);

  let testOnly = new Map(); // file -> owns its directory
  for (let round = 0; round < 64; round += 1) {
    const vetoBase = new Set();
    const vetoDirModule = new Set();
    const candidates = new Map();
    for (const [file, lex] of lexed) {
      const parentIsTest = testOnly.has(file);
      for (const declaration of lex.declarations) {
        const edge =
          lex.healthy &&
          declaration.depth === 0 &&
          (parentIsTest || declaration.attributes.some(impliesTest));
        const target = edge ? resolveDeclaration(file, declaration, parentIsTest ? testOnly.get(file) : null, exists) : null;
        if (target) {
          if (!candidates.has(target.path)) candidates.set(target.path, target.ownsDirectory);
          continue;
        }
        if (parentIsTest) continue;
        vetoBase.add(`${declaration.name}.rs`.toLowerCase());
        vetoDirModule.add(`${declaration.name}/mod.rs`.toLowerCase());
        for (const value of declaration.pathValues) vetoBase.add(basename(value).toLowerCase());
      }
      if (parentIsTest) continue;
      for (const reference of lex.looseReferences) vetoBase.add(basename(reference).toLowerCase());
    }
    const next = new Map();
    for (const [path, ownsDirectory] of candidates) {
      const base = basename(path).toLowerCase();
      const dirModule = `${basename(dirname(path))}/${base}`.toLowerCase();
      if (roots.has(path) || vetoBase.has(base) || (base === "mod.rs" && vetoDirModule.has(dirModule))) continue;
      next.set(path, ownsDirectory);
    }
    const stable = next.size === testOnly.size && [...next.keys()].every((path) => testOnly.has(path));
    testOnly = next;
    if (stable) return new Set(testOnly.keys());
  }
  throw new Error("test-module quarantine did not converge");
}

// Ways to load a module file that the lexer below does not model. Matched on raw
// source, so text in comments or strings can trip it too: that fails the gate
// loudly, which is the intended direction. Every pattern here is absent from the
// repository today; if one is ever needed, teach the lexer about it first.
const UNRECOGNISED_LOADERS = [
  [/\bmod\s*\$/, "a macro declaring a module through a metavariable"],
  [/\bmod\s+r#/, "a raw-identifier module name"],
  [/\bmod\b\s*\/[*/]|\bmod\s+[A-Za-z_][A-Za-z0-9_]*\b\s*\/[*/]/, "a comment inside a module declaration"],
  [/\bpath\s*=\s*\/[*/]/, "a comment inside a path attribute"],
  [/\bpath\s*=\s*r?#*"[^"\n]*\\/, "a path attribute containing a backslash"],
  [/\binclude\s+!|\binclude!\s*[[{]|\binclude!\s*\(\s*(?!r?#*")/, "include! without a literal argument"],
  [/\binclude!\s*\(\s*r?#*"(?![^"]*\.rs")/, "include! of a file that is not .rs"],
];

function refuseUnrecognisedLoaders(file, source) {
  for (const [pattern, description] of UNRECOGNISED_LOADERS) {
    const match = pattern.exec(source);
    if (match) {
      const line = source.slice(0, match.index).split("\n").length;
      throw new Error(
        `test-module quarantine refuses ${file}:${line}: ${description}. ` +
          "It cannot tell whether that loads a file as production code.",
      );
    }
  }
}

function resolveDeclaration(file, declaration, parentOwnsDirectory, exists) {
  const directory = dirname(file);
  const explicit = declaration.attributes.map(pathAttribute).find((value) => value !== null);
  if (explicit !== undefined) {
    const path = normalise(`${directory}/${explicit}`);
    return exists.has(path) ? { path, ownsDirectory: true } : null;
  }
  const ownsDirectory =
    parentOwnsDirectory ?? ["lib.rs", "main.rs", "mod.rs"].includes(basename(file));
  const base = ownsDirectory || basename(file) === "mod.rs" ? directory : `${directory}/${basename(file, ".rs")}`;
  for (const candidate of [`${base}/${declaration.name}.rs`, `${base}/${declaration.name}/mod.rs`]) {
    const path = normalise(candidate);
    if (exists.has(path)) return { path, ownsDirectory: basename(path) === "mod.rs" };
  }
  return null;
}

function normalise(path) {
  const parts = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function impliesTest(attribute) {
  // A comment or raw string inside the predicate is not parsed; treat it as not
  // implying test, which keeps the module scanned.
  if (/\/\*|\/\/|\br#*"/.test(attribute)) return false;
  const cfg = /^\s*cfg\s*\(([\s\S]*)\)\s*$/.exec(attribute);
  return cfg !== null && predicateImpliesTest(cfg[1].trim());
}

function predicateImpliesTest(predicate) {
  if (predicate === "test") return true;
  const all = /^all\s*\(([\s\S]*)\)$/.exec(predicate);
  return all !== null && topLevelArguments(all[1]).some(predicateImpliesTest);
}

function topLevelArguments(text) {
  const argumentsFound = [];
  let depth = 0;
  let quoted = false;
  let current = "";
  for (let index = 0; index < text.length; index += 1) {
    const c = text[index];
    if (quoted) {
      current += c;
      if (c === "\\") {
        current += text[index + 1] ?? "";
        index += 1;
      } else if (c === '"') quoted = false;
      continue;
    }
    if (c === '"') quoted = true;
    else if (c === "(") depth += 1;
    else if (c === ")") depth -= 1;
    else if (c === "," && depth === 0) {
      argumentsFound.push(current.trim());
      current = "";
      continue;
    }
    current += c;
  }
  if (current.trim()) argumentsFound.push(current.trim());
  return argumentsFound;
}

function pathAttribute(attribute) {
  const path = /^\s*path\s*=\s*(?:r(#*)"([\s\S]*)"\1|"((?:\\.|[^"\\])*)")\s*$/.exec(attribute);
  if (path === null) return null;
  return path[2] ?? path[3];
}

// One forward pass that skips comments, string and char literals, and records:
// out-of-line `mod name;` declarations with their full outer attribute group and
// brace depth; string literals used as `path = "..."` or `include*!("...")`
// outside such a group; and whether the file's braces balance.
export function lexModules(source) {
  const declarations = [];
  const looseReferences = [];
  const n = source.length;
  let i = 0;
  let depth = 0;
  let healthy = true;
  let group = null; // { start, attributes, pathValues } for the attribute group being read

  const skipTrivia = (position) => {
    for (;;) {
      while (position < n && /\s/.test(source[position])) position += 1;
      if (source.startsWith("//", position)) {
        const end = source.indexOf("\n", position);
        position = end === -1 ? n : end;
      } else if (source.startsWith("/*", position)) {
        position = skipBlockComment(source, position);
      } else return position;
    }
  };

  while (i < n) {
    if (source.startsWith("//", i)) {
      const end = source.indexOf("\n", i);
      i = end === -1 ? n : end;
      continue;
    }
    if (source.startsWith("/*", i)) {
      i = skipBlockComment(source, i);
      continue;
    }
    const literal = readLiteral(source, i);
    if (literal) {
      const before = source.slice(Math.max(0, i - 256), i).replace(/\s+$/, "");
      if (/(?:^|[^A-Za-z0-9_])path\s*=$/.test(before) || /include(?:_str|_bytes)?!\s*\($/.test(before)) {
        looseReferences.push(literal.value);
      }
      i = literal.end;
      continue;
    }
    if (source.startsWith("#[", i)) {
      // Read the whole outer attribute group, then decide whether it attaches
      // to an out-of-line module declaration.
      const attributes = [];
      let cursor = i;
      while (source.startsWith("#[", cursor)) {
        const end = readBracket(source, cursor + 1);
        attributes.push(source.slice(cursor + 2, end - 1));
        cursor = skipTrivia(end);
      }
      const item = /^(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/.exec(source.slice(cursor, cursor + 160));
      if (item) {
        const pathValues = attributes.flatMap((attribute) => attributeStrings(attribute, /(?:^|[^A-Za-z0-9_])path\s*=$/));
        declarations.push({ name: item[1], attributes, pathValues, depth });
        i = cursor + item[0].length;
        continue;
      }
      // Not a module declaration: record any path/include strings inside the
      // attributes as loose references, and carry on after the group.
      for (const attribute of attributes) {
        looseReferences.push(...attributeStrings(attribute, /(?:^|[^A-Za-z0-9_])path\s*=$|include(?:_str|_bytes)?!\s*\($/));
      }
      i = cursor;
      continue;
    }
    const bare = /^(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/.exec(source.slice(i, i + 160));
    if (bare && (i === 0 || !/[A-Za-z0-9_]/.test(source[i - 1]))) {
      declarations.push({ name: bare[1], attributes: [], pathValues: [], depth });
      i += bare[0].length;
      continue;
    }
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      if (depth === 0) healthy = false;
      else depth -= 1;
    }
    const word = /^[A-Za-z_][A-Za-z0-9_]*/.exec(source.slice(i, i + 64));
    i += word ? word[0].length : 1;
  }
  return { declarations, looseReferences, healthy: healthy && depth === 0 };
}

function attributeStrings(attribute, prefix) {
  const values = [];
  let i = 0;
  while (i < attribute.length) {
    const literal = readLiteral(attribute, i);
    if (literal) {
      if (prefix.test(attribute.slice(Math.max(0, i - 256), i).replace(/\s+$/, ""))) values.push(literal.value);
      i = literal.end;
    } else i += 1;
  }
  return values;
}

function skipBlockComment(source, start) {
  let depth = 1;
  let i = start + 2;
  while (i < source.length && depth > 0) {
    if (source.startsWith("/*", i)) {
      depth += 1;
      i += 2;
    } else if (source.startsWith("*/", i)) {
      depth -= 1;
      i += 2;
    } else i += 1;
  }
  return i;
}

function readBracket(source, start) {
  let depth = 0;
  let i = start;
  while (i < source.length) {
    const literal = readLiteral(source, i);
    if (literal) {
      i = literal.end;
      continue;
    }
    if (source[i] === "[") depth += 1;
    else if (source[i] === "]") {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
    i += 1;
  }
  return source.length;
}

// A string, raw string, byte string or char literal starting at `i`, if any.
// Lifetimes (`'a`) are not literals.
function readLiteral(source, i) {
  if (i > 0 && /[A-Za-z0-9_]/.test(source[i - 1]) && source[i] !== '"' && source[i] !== "'") return null;
  const raw = /^[bc]?r(#*)"/.exec(source.slice(i, i + 40));
  if (raw) {
    const close = `"${raw[1]}`;
    const valueStart = i + raw[0].length;
    const end = source.indexOf(close, valueStart);
    const stop = end === -1 ? source.length : end;
    return { value: source.slice(valueStart, stop), end: end === -1 ? source.length : end + close.length };
  }
  const quote = source[i] === '"' ? i : (source[i] === "b" || source[i] === "c") && source[i + 1] === '"' ? i + 1 : -1;
  if (quote !== -1) {
    let j = quote + 1;
    let value = "";
    while (j < source.length && source[j] !== '"') {
      if (source[j] === "\\" && j + 1 < source.length) {
        value += source[j + 1];
        j += 2;
      } else {
        value += source[j];
        j += 1;
      }
    }
    return { value, end: Math.min(source.length, j + 1) };
  }
  const char = /^b?'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]{1,6}\}|.)|[^\\'\n])'/u.exec(source.slice(i, i + 16));
  if (char) return { value: "", end: i + char[0].length };
  return null;
}

function allFiles(repositoryRoot) {
  const files = [];
  const walk = (directory) => {
    const entries = readdirSync(directory, { withFileTypes: true });
    const isCrate = entries.some((entry) => entry.isFile() && entry.name === "Cargo.toml");
    for (const entry of entries) {
      if ([".git", "node_modules"].includes(entry.name)) continue;
      // Only a crate's (or the repository's) build directory is pruned; a source
      // directory that happens to be named `target` is still read.
      if (entry.name === "target" && (isCrate || directory === repositoryRoot)) continue;
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) files.push(relativePath(repositoryRoot, path));
    }
  };
  walk(repositoryRoot);
  return files;
}

function crateRoots(repositoryRoot, files) {
  const roots = new Set();
  for (const manifest of files.filter((file) => basename(file) === "Cargo.toml")) {
    const crate = dirname(manifest);
    const prefix = crate === "." ? "" : `${crate}/`;
    for (const file of files) {
      if (!file.startsWith(prefix) || !file.endsWith(".rs")) continue;
      const rest = file.slice(prefix.length).split("/");
      if (
        ["src/lib.rs", "src/main.rs", "build.rs"].includes(rest.join("/")) ||
        (rest[0] === "src" && rest[1] === "bin") ||
        (["tests", "benches", "examples"].includes(rest[0]) && (rest.length === 2 || rest.at(-1) === "main.rs"))
      ) {
        roots.add(file);
      }
    }
    for (const match of readFileSync(resolve(repositoryRoot, manifest), "utf8").matchAll(/\b(?:path|build)\s*=\s*["']([^"']+\.rs)["']/g)) {
      roots.add(normalise(`${crate}/${match[1]}`));
    }
  }
  return roots;
}

// `import.meta.main` needs Node 24.2+. The job that runs this gate does not pin
// Node, so fall back to comparing real paths rather than silently doing nothing.
const invokedDirectly =
  import.meta.main ??
  (process.argv[1] !== undefined &&
    realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url)));
if (invokedDirectly) main();
