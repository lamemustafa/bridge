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
  assert.match(frontend, /async function discoverUntrustedCompanies\(\) \{\s*if \(currentProbeCompanyList\.length > 0\) return;/s);
});

test("Tally nav routes unreadable client views to connection management", async () => {
  const [frontend, outstandings] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);
  const nav = frontend.slice(frontend.indexOf('<nav aria-label="Bridge operations">'), frontend.indexOf("</nav>"));

  assert.match(nav, /<Cable size=\{18\} \/> Outstandings/);
  assert.match(nav, /<Building2 size=\{18\} \/> Compare clients/);
  assert.match(nav, /<Cable size=\{18\} \/> Manage Tally/);
  assert.match(frontend, /selectedCompanyReadable \? "outstandings" : "companies"/);
  assert.match(frontend, /selectedCompanyReadable \? "clients" : "companies"/);
  assert.match(frontend, /selectedCompanyRecord\?\.guid && selectedCompanyRecord\.company_number && selectedCompanyRecord\.books_from_yyyymmdd/);
  assert.match(outstandings, /Manage Tally/);
});

test("saved pins remain selectable for local proof review without a Tally read", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(frontend, /savedCompanyList\.length > 0/);
  assert.match(frontend, /Review local Mirror &amp; Proof evidence without contacting Tally\./);
  assert.match(frontend, /Change saved company/);
  assert.match(frontend, /const savedCompanyMutationPending = tallyAction === "save"\s*\|\| tallyAction === "fixture_enroll"\s*\|\| tallyAction === "fixture_revoke";/s);
  assert.match(frontend, /const savedCompanySelectionLocked = snapshotActive\s*\|\| snapshotStartOutcomeUnknown\s*\|\| savedCompanyMutationPending\s*\|\| tallyAction === "start"\s*\|\| tallyAction === "resume";/s);
  assert.match(frontend, /function selectSavedCompany\(key: string\) \{\s*if \(key === selectedCompany \|\| savedCompanySelectionLocked\) return;\s*clearSelectedCompanyScope\(\);\s*setSelectedCompany\(key\);\s*\}/s);
  assert.match(frontend, /selectSavedCompany\(""\)\} disabled=\{savedCompanySelectionLocked\}/);
  assert.match(frontend, /selectSavedCompany\(key\)\} disabled=\{savedCompanySelectionLocked\}/);
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
  const companySetup = frontend.slice(frontend.indexOf("{setupConnectionComplete && ("), frontend.indexOf("{selectedCompanyReady && ("));
  const currentProbePicker = companySetup.slice(
    companySetup.indexOf("{currentProbeCompanyList.length > 0 ? ("),
    companySetup.indexOf(") : untrustedDiscoveredCompanies.length > 0 ? ("),
  );

  assert.match(frontend, /tallyReadinessState\(\{[\s\S]*?companySaved:\s*Boolean\(selectedCompanyRecord\?\.mirror_company_id\),[\s\S]*?\}\)\.companyReady/);
  assert.match(currentProbePicker, /if \(key === selectedCompany\) return;[\s\S]*?clearSelectedCompanyScope\(/);
  assert.match(companySetup, /\{currentProbeCompanyList\.length > 0 \? \([\s\S]*?\) : untrustedDiscoveredCompanies\.length > 0 \? \(/);
  assert.doesNotMatch(currentProbePicker, /bootstrapDirectCompany/);
  assert.match(frontend, /Synthetic write fixture \(advanced\)/);
  assert.match(frontend, /onClick=\{\(\) => void enrollWriteFixture\(\)\}/);
  assert.match(frontend, /onClick=\{\(\) => void revokeWriteFixture\(\)\}/);
  assert.match(frontend, /Retry local fixture status/);
});

test("direct fallback replaces untrusted candidates only after verified identity is selected", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const bootstrap = frontend.slice(
    frontend.indexOf("async function bootstrapDirectCompany"),
    frontend.indexOf("async function saveReviewedTallySetup"),
  );

  assert.match(bootstrap, /const verifiedCompany = liveCompanies\.length === 1 && liveCompanies\[0\]\.guid[\s\S]*?const verifiedCompanyKey = verifiedCompany[\s\S]*?setSelectedCompany\(verifiedCompanyKey\);/);
  assert.match(bootstrap, /if \(verifiedCompany\) \{\s*setUntrustedDiscoveredCompanies\(\[\]\);\s*setUntrustedDiscoveryError\(null\);\s*\}/);
});

test("desktop scrolling remains inside the content pane", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /\.shell\s*\{[^}]*height:\s*100dvh;[^}]*overflow:\s*hidden;/s);
  assert.match(styles, /\.sidebar\s*\{[^}]*overflow-y:\s*auto;/s);
  assert.match(styles, /\.content\s*\{[^}]*min-height:\s*0;[^}]*overflow-y:\s*auto;/s);
});
