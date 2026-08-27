// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("UX1 keeps client selection searchable, truthful about read readiness, and explicit about unavailable sections", async () => {
  const [app, switcher] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/ClientSwitcher.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(app, /const NON_TALLY_SECTIONS_ENABLED = false/);
  assert.match(app, /disabled=\{!NON_TALLY_SECTIONS_ENABLED\}/);
  assert.match(app, /Not yet available/);
  assert.match(app, /port: 9001/);
  assert.match(app, /liveCompanyKeys\.includes\(key\).*company\.canonical_endpoint === configuredTallyEndpoint\(config\)/s);
  assert.match(app, /const selectedCompanyReadable = selectedCompanyReady/);
  assert.doesNotMatch(app, /fetch_saved_tally_outstandings|detect_saved_tally_base_currency/);
  assert.match(switcher, /type="search"/);
  assert.match(switcher, /onKeyDown=[\s\S]*?event\.key === "Escape"/);
  assert.match(switcher, /Setup and verification are required before other clients can be read/);
});

test("UX1 nav sends unreadable outstandings and client requests to Manage Tally", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const nav = app.slice(app.indexOf('<nav aria-label="Bridge operations">'), app.indexOf("</nav>"));

  assert.match(nav, /Outstandings/);
  assert.match(nav, /Compare clients/);
  assert.match(nav, /Manage Tally/);
  assert.match(nav, /onClick=\{\(\) => setView\(selectedCompanyReadable \? "outstandings" : "companies"\)\}/);
  assert.match(nav, /onClick=\{\(\) => setView\(selectedCompanyReadable \? "clients" : "companies"\)\}/);
});

test("UX1 has reachable responsive rules and contains wide content", async () => {
  const [tauriConfig, styles, mirrorProof] = await Promise.all([
    readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src/MirrorProofScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(tauriConfig, /"minWidth": 520/);
  assert.match(styles, /@media \(max-width: 860px\)/);
  assert.match(styles, /@media \(max-width: 520px\)/);
  assert.match(styles, /\.table-wrap > table\s*\{\s*min-width:\s*820px;/s);
  assert.doesNotMatch(styles, /(?:^|\n)table\s*\{[\s\S]{0,100}min-width:\s*820px;/);
  assert.match(styles, /\.outstandings-heading-actions\s*\{[\s\S]*?flex-wrap:\s*wrap;/);
  assert.match(styles, /\.outstandings-screen\s*\{[\s\S]*?min-width:\s*0;/);
  assert.match(styles, /@media \(max-width: 860px\)\s*\{[\s\S]*?\.client-switcher-current\s*\{\s*flex:\s*0 0 auto;/);

  const tableCount = [...mirrorProof.matchAll(/<table\b[^>]*>/g)].length;
  const scrollableTableCount = [...mirrorProof.matchAll(/<div className="table-wrap"[^>]*>\s*<table\b[^>]*>/g)].length;
  assert.equal(scrollableTableCount, tableCount, "every Mirror & Proof table must have a preceding table-wrap container");
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Recent durable Core Accounting runs" tabIndex=\{0\}>/);
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Paged local mirror records" tabIndex=\{0\}>/);
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Hash-linked local proof ledger" tabIndex=\{0\}>/);
});
