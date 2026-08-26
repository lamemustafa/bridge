// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("UX1 keeps client selection searchable, reversible, and explicit about unavailable sections", async () => {
  const [app, switcher, outstandings] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/ClientSwitcher.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(app, /const NON_TALLY_SECTIONS_ENABLED = false/);
  assert.match(app, /disabled=\{!NON_TALLY_SECTIONS_ENABLED\}/);
  assert.match(app, /Not yet available/);
  assert.match(app, /port: 9001/);
  assert.match(outstandings, /fetch_saved_tally_outstandings/);
  assert.match(outstandings, /detect_saved_tally_base_currency/);
  assert.match(switcher, /type="search"/);
  assert.match(switcher, /onKeyDown=[\s\S]*?event\.key === "Escape"/);
  assert.match(switcher, /Setup and verification are required before other clients can be read/);
});

test("UX1 has reachable responsive rules and contains wide content", async () => {
  const [tauriConfig, styles] = await Promise.all([
    readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ]);

  assert.match(tauriConfig, /"minWidth": 520/);
  assert.match(styles, /@media \(max-width: 860px\)/);
  assert.match(styles, /@media \(max-width: 520px\)/);
  assert.match(styles, /\.table-wrap > table\s*\{\s*min-width:\s*820px;/s);
  assert.doesNotMatch(styles, /(?:^|\n)table\s*\{[\s\S]{0,100}min-width:\s*820px;/);
  assert.match(styles, /\.outstandings-heading-actions\s*\{[\s\S]*?flex-wrap:\s*wrap;/);
  assert.match(styles, /\.outstandings-screen\s*\{[\s\S]*?min-width:\s*0;/);
});
