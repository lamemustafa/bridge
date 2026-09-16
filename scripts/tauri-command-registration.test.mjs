// SPDX-License-Identifier: Apache-2.0

// Every `#[tauri::command]` must be registered in `generate_handler!` or listed below as
// deliberately unexposed. Nothing else checks this: the macro accepts whatever list it is
// given, the compiler does not warn about a command that is never registered, and a missing
// registration fails only when the frontend's `invoke` reaches the IPC boundary at runtime.

import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

// Declared on purpose but never registered. Kept unexposed as unqualified legacy reads by
// scripts/tally-setup-safety.test.mjs; see docs/rust-module-conventions.md for the open decision
// on whether to delete them. An entry here that becomes registered or disappears fails the test,
// so the list cannot go stale silently.
const DELIBERATELY_UNEXPOSED = new Set([
  "qualify_selected_tally_reads",
  "fetch_tally_ledgers",
  "fetch_standard_tally_ledger_catalog",
  "fetch_tally_vouchers",
]);

function stripComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

export function declaredCommands(source) {
  const names = [];
  const pattern =
    /#\[\s*tauri\s*::\s*command\b[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/g;
  for (const match of stripComments(source).matchAll(pattern)) names.push(match[1]);
  return names;
}

export function registeredCommands(source) {
  const lists = [...stripComments(source).matchAll(/generate_handler!\s*\[([\s\S]*?)\]/g)];
  assert.equal(lists.length, 1, "expected exactly one generate_handler! list");
  return lists[0][1]
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((path) => path.split("::").at(-1));
}

async function rustFiles(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory()) out.push(...(await rustFiles(path)));
    else if (entry.name.endsWith(".rs")) out.push(path);
  }
  return out;
}

test("the extractor finds commands through attributes, doc comments and visibility forms", () => {
  const source = `
    /// Doc comment.
    #[tauri::command]
    pub async fn alpha() {}
    #[tauri::command(rename_all = "snake_case")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn beta() {}
    // #[tauri::command] fn commented_out() {}
    fn not_a_command() {}
  `;
  assert.deepEqual(declaredCommands(source), ["alpha", "beta"]);
});

test("the registration reader ignores commented-out entries and keeps the last path segment", () => {
  const source = `builder.invoke_handler(tauri::generate_handler![
    commands::alpha,
    // commands::beta,
    commands::trial_balance::gamma
  ])`;
  assert.deepEqual(registeredCommands(source), ["alpha", "gamma"]);
});

test("every declared Tauri command is registered or deliberately unexposed", async () => {
  const root = new URL("../src-tauri/src", import.meta.url).pathname;
  const declared = new Map();
  for (const file of await rustFiles(root)) {
    for (const name of declaredCommands(await readFile(file, "utf8"))) {
      assert.ok(!declared.has(name), `command name ${name} is declared twice (${declared.get(name)}, ${file})`);
      declared.set(name, file);
    }
  }
  const registered = new Set(registeredCommands(await readFile(`${root}/lib.rs`, "utf8")));

  assert.ok(declared.size > 0, "no #[tauri::command] declarations found; the extractor is broken");
  const missing = [...declared.keys()].filter((name) => !registered.has(name) && !DELIBERATELY_UNEXPOSED.has(name));
  assert.deepEqual(missing, [], "declared commands missing from generate_handler!");
  for (const name of registered) assert.ok(declared.has(name), `registered command ${name} has no #[tauri::command] declaration`);
  for (const name of DELIBERATELY_UNEXPOSED) {
    assert.ok(declared.has(name), `${name} is allow-listed as unexposed but is no longer declared; remove it from the list`);
    assert.ok(!registered.has(name), `${name} is allow-listed as unexposed but is now registered; remove it from the list`);
  }
});
