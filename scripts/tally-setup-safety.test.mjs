// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("Tally setup does not expose unqualified legacy reads", async () => {
  const [frontend, commands] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8"),
  ]);

  for (const command of [
    "qualify_selected_tally_reads",
    "fetch_tally_ledgers",
    "fetch_standard_tally_ledger_catalog",
    "fetch_tally_vouchers",
  ]) {
    assert.doesNotMatch(frontend, new RegExp(`\\b${command}\\b`));
    assert.doesNotMatch(commands, new RegExp(`\\bcommands::${command}\\b`));
  }

  assert.match(frontend, /discoveredCompanyPrompt && view !== "companies"/);
  assert.match(frontend, /Find all companies/);
});

test("Tally has one top-level navigation entry and keeps connection management in its workspace", async () => {
  const [frontend, outstandings] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);
  const nav = frontend.slice(frontend.indexOf('<nav aria-label="Bridge operations">'), frontend.indexOf("</nav>"));

  assert.match(nav, /<Cable size=\{18\} \/> Tally/);
  assert.doesNotMatch(nav, /Outstandings|Tally Setup/);
  assert.match(frontend, /selectedCompanyRecord\?\.mirror_company_id \? "outstandings" : "companies"/);
  assert.match(outstandings, /Manage Tally/);
});

test("saved pins remain selectable for local proof review without a Tally read", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(frontend, /savedCompanyList\.length > 0/);
  assert.match(frontend, /Review local Mirror &amp; Proof evidence without contacting Tally\./);
  assert.match(frontend, /Change saved company/);
  assert.match(frontend, /function selectSavedCompany\(key: string\) \{\s*if \(key === selectedCompany\) return;\s*clearSelectedCompanyScope\(\);\s*setSelectedCompany\(key\);\s*\}/s);
});

test("structured Tally errors retain their backend remediation", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(frontend, /Next step: \{message\.remediation\}/);
});

test("persisted-company load failures remain visible before a Tally connection is established", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(frontend, /refreshPersistedCompanyProfiles[\s\S]*?setCompanyError\(toOperatorError\(error\)\)/);
  assert.match(frontend, /\{companyError && !setupConnectionComplete && <TallyErrorNotice message=\{companyError\} \/>\}/);
});

test("Tally setup uses the durable pin for readiness and keeps fixture control local to proof work", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const currentProbePicker = frontend.slice(frontend.indexOf("currentProbeCompanyList.map"), frontend.indexOf("Find all companies"));

  assert.match(frontend, /tallyReadinessState\(\{[\s\S]*?companySaved:\s*Boolean\(selectedCompanyRecord\?\.mirror_company_id\),[\s\S]*?\}\)\.companyReady/);
  assert.match(currentProbePicker, /if \(key === selectedCompany\) return;[\s\S]*?clearSelectedCompanyScope\(/);
  assert.match(frontend, /Synthetic write fixture \(advanced\)/);
  assert.match(frontend, /onClick=\{\(\) => void enrollWriteFixture\(\)\}/);
  assert.match(frontend, /onClick=\{\(\) => void revokeWriteFixture\(\)\}/);
  assert.match(frontend, /Retry local fixture status/);
});

test("desktop scrolling remains inside the content pane", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /\.shell\s*\{[^}]*height:\s*100dvh;[^}]*overflow:\s*hidden;/s);
  assert.match(styles, /\.sidebar\s*\{[^}]*overflow-y:\s*auto;/s);
  assert.match(styles, /\.content\s*\{[^}]*min-height:\s*0;[^}]*overflow-y:\s*auto;/s);
});
