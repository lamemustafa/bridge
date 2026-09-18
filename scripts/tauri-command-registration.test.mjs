// SPDX-License-Identifier: Apache-2.0

// Every `#[tauri::command]` must be registered in `generate_handler!`. Nothing else checks this:
// the macro accepts whatever list it is given, the compiler does not warn about a command that is
// never registered, and a missing registration fails only when the frontend's `invoke` reaches the
// IPC boundary at runtime.

import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

function stripComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

// Index just past the `]` that closes the attribute whose `#[` starts at `start`, counting nested
// brackets and skipping string literals, so neither `values = [1, 2]` nor `doc = "]"` ends the
// attribute early.
function attributeEnd(source, start) {
  let depth = 0;
  for (let i = start + 1; i < source.length; i += 1) {
    if (source[i] === '"') {
      for (i += 1; i < source.length && source[i] !== '"'; i += 1) if (source[i] === "\\") i += 1;
      continue;
    }
    if (source[i] === "[") depth += 1;
    else if (source[i] === "]") {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
  }
  return source.length;
}

const FN_HEAD =
  /^\s*(?:(?:pub(?:\s*\([^)]*\))?|async|unsafe|const|extern(?:\s+"[^"]*")?)\s+)*fn\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)/;

export function declaredCommands(source) {
  const text = stripComments(source);
  const names = [];
  const marker = /#\[\s*tauri\s*::\s*command\b/g;
  for (const match of text.matchAll(marker)) {
    let cursor = attributeEnd(text, match.index);
    for (;;) {
      const next = /^\s*#\[/.exec(text.slice(cursor));
      if (!next) break;
      cursor = attributeEnd(text, cursor + next[0].length - 2);
    }
    const head = FN_HEAD.exec(text.slice(cursor));
    assert.ok(head, `#[tauri::command] at offset ${match.index} is not followed by a fn the extractor can read`);
    names.push(head[1]);
  }
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

test("the extractor reads every fn form a command can take", () => {
  const source = `
    /// Doc comment.
    #[tauri::command]
    pub async fn alpha() {}
    #[tauri::command(rename_all = "snake_case")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn beta() {}
    #[tauri::command]
    pub unsafe fn gamma() {}
    #[tauri::command]
    const fn delta() {}
    #[tauri::command]
    #[cfg_attr(feature = "x", doc = "[bracketed]")]
    #[some_attr(values = [1, 2, 3])]
    pub async unsafe fn epsilon() {}
    #[tauri::command]
    pub fn r#type() {}
    #[tauri::command]
    #[doc = "a ] and a \\" quote"]
    pub fn zeta() {}
    // #[tauri::command] fn commented_out() {}
    fn not_a_command() {}
  `;
  assert.deepEqual(declaredCommands(source), ["alpha", "beta", "gamma", "delta", "epsilon", "type", "zeta"]);
});

test("a command attribute not followed by a readable fn fails instead of being skipped", () => {
  assert.throws(() => declaredCommands("#[tauri::command]\nstruct NotAFunction;"), /not followed by a fn/);
});

test("the registration reader ignores commented-out entries and keeps the last path segment", () => {
  const source = `builder.invoke_handler(tauri::generate_handler![
    commands::alpha,
    // commands::beta,
    commands::trial_balance::gamma
  ])`;
  assert.deepEqual(registeredCommands(source), ["alpha", "gamma"]);
});

// #474 deleted the last deliberately-unexposed commands (qualify_selected_tally_reads,
// fetch_tally_ledgers, fetch_standard_tally_ledger_catalog, fetch_tally_vouchers) and their
// commands.rs-exclusive helpers, so this no longer allow-lists anything: every declared
// `#[tauri::command]` must be registered in `generate_handler!`. A future command that is
// deliberately built but not yet wired up should get its own named exemption here, with a reason,
// rather than reviving a silent allow-list.
test("every declared Tauri command is registered in generate_handler!", async () => {
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
  const missing = [...declared.keys()].filter((name) => !registered.has(name));
  assert.deepEqual(missing, [], "declared commands missing from generate_handler!");
  for (const name of registered) assert.ok(declared.has(name), `registered command ${name} has no #[tauri::command] declaration`);
});
