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
  assert.match(frontend, /const savedCompanySelectionLocked = snapshotActive\s*\|\| snapshotStartOutcomeUnknown\s*\|\| tallyAction !== null\s*\|\| childTallyReadCount > 0;/s);
  assert.match(frontend, /function selectSavedCompany\(key: string\) \{\s*if \(key === selectedCompany \|\| savedCompanySelectionLocked\) return;\s*clearSelectedCompanyScope\(\);\s*setSelectedCompany\(key\);\s*\}/s);
  assert.match(frontend, /selectSavedCompany\(""\)\} disabled=\{savedCompanySelectionLocked\}/);
  assert.match(frontend, /selectSavedCompany\(key\)\} disabled=\{savedCompanySelectionLocked\}/);
});

test("changing a saved client retains endpoint probe evidence but clears the old client review", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const clearSelection = frontend.slice(
    frontend.indexOf("function clearSelectedCompanyScope"),
    frontend.indexOf("function selectSavedCompany"),
  );

  assert.match(clearSelection, /if \(!preserveCurrentProbeReview\) \{\s*setReviewId\(null\);\s*setReviewCommitmentSha256\(null\);\s*setSelectedReadScope\(null\);/s);
  assert.doesNotMatch(clearSelection, /setPassport\(null\)|setProfileSha256\(null\)/);
  assert.match(frontend, /function selectSavedCompany[\s\S]*?clearSelectedCompanyScope\(\);/);
});

test("choosing a current unsaved client from the shell retains its unused probe review", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const shellSelection = frontend.slice(
    frontend.indexOf("function selectClientFromShell"),
    frontend.indexOf("function updateTallyHost"),
  );

  assert.match(shellSelection, /liveCompanyKeys\.includes\(key\)\s*&& company\.canonical_endpoint === currentProbeCanonicalOrigin/s);
  assert.match(shellSelection, /canReuseCurrentProbeReview\(\{\s*reviewAvailable: Boolean\(reviewId && reviewCommitmentSha256\),\s*setupSaved: Boolean\(passportSnapshotId\),\s*\}\)/s);
  assert.match(shellSelection, /clearSelectedCompanyScope\(\{ preserveCurrentProbeReview \}\);/);
});

test("choosing an already-selected setup-required client opens Manage Tally", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const shellSelection = frontend.slice(
    frontend.indexOf("function selectClientFromShell"),
    frontend.indexOf("function updateTallyHost"),
  );

  assert.match(shellSelection, /if \(key === selectedCompany\) \{\s*setView\("companies"\);\s*return;\s*\}/s);
});

test("saved-profile shell selections stay in local Mirror and Proof review", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const shellSelection = frontend.slice(
    frontend.indexOf("function selectClientFromShell"),
    frontend.indexOf("function updateTallyHost"),
  );

  assert.match(shellSelection, /if \(company\.mirror_company_id\) \{[\s\S]*?selectSavedCompany\(key\);\s*if \(view === "mirror"\) return;/);
  assert.match(shellSelection, /if \(currentAtProbedEndpoint\) \{\s*setView\("outstandings"\);/s);
});

test("structured Tally errors retain their backend remediation", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(frontend, /Next step: \{message\.remediation\}/);
});

test("persisted-company load failures remain visible before a Tally connection is established", async () => {
  const frontend = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const profileLoad = frontend.slice(
    frontend.indexOf("const refreshPersistedCompanyProfiles"),
    frontend.indexOf("// Both of these are backed by the encrypted mirror"),
  );

  assert.match(profileLoad, /setPersistedCompanyProfileError\(operatorError\);/);
  assert.match(profileLoad, /setPersistedCompanyProfilesTruncated\(page\.truncated\);\s*setPersistedCompanyProfileError\(null\);/s);
  assert.doesNotMatch(profileLoad, /setCompanyError\(/);
  assert.match(frontend, /\{persistedCompanyProfileError && !setupConnectionComplete && <TallyErrorNotice message=\{persistedCompanyProfileError\} \/>\}/);
  assert.match(frontend, /\{companyError && !setupConnectionComplete && <TallyErrorNotice message=\{companyError\} \/>\}/);
  const mirror = frontend.slice(frontend.indexOf('{view === "mirror" && ('), frontend.indexOf("companyError={companyError}"));
  assert.match(mirror, /\{persistedCompanyProfileError && <TallyErrorNotice message=\{persistedCompanyProfileError\} \/>\}/);
});

test("child Tally reads keep client selection locked until every invocation settles", async () => {
  const [frontend, allClients, outstandings, readiness] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/TallyReadinessFlow.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(frontend, /const changeChildTallyReadActivity = React\.useCallback\(\(delta: 1 \| -1\) => \{\s*setChildTallyReadCount\(\(current\) => Math\.max\(0, current \+ delta\)\);/s);
  assert.match(frontend, /onTallyReadActivityChange=\{changeChildTallyReadActivity\}/);
  for (const screen of [allClients, outstandings]) {
    assert.match(screen, /onTallyReadActivityChange\(1\);/);
    assert.match(screen, /onTallyReadActivityChange\(-1\);/);
  }
  const companySetup = frontend.slice(frontend.indexOf("{setupConnectionComplete && ("), frontend.indexOf("{selectedCompanyReady && ("));
  assert.match(companySetup, /disabled=\{!current \|\| savedCompanySelectionLocked\}/);
  assert.match(companySetup, /bootstrapDirectCompany\(company\.name\)\} disabled=\{savedCompanySelectionLocked\}/);
  assert.match(companySetup, /discoverUntrustedCompanies\(\)\} disabled=\{savedCompanySelectionLocked\}/);
  assert.match(companySetup, /saveReviewedTallySetup\(\)\} disabled=\{savedCompanySelectionLocked \|\| !passport/);
  assert.match(frontend, /onClick=\{checkTally\} disabled=\{tallyAction !== null \|\| childTallyReadCount > 0\}/);
  assert.match(frontend, /const endpointSettingsLockMessage = snapshotActive[\s\S]*?childTallyReadCount > 0[\s\S]*?Tally read is in progress/);
  assert.match(frontend, /settingsLocked=\{endpointSettingsLockMessage !== null\}\s*settingsLockMessage=\{endpointSettingsLockMessage\}/);
  assert.match(readiness, /settingsLockMessage: string \| null;/);
  assert.match(readiness, /\{settingsLockMessage && <p role="status">\{settingsLockMessage\}<\/p>\}/);
  assert.match(readiness, /onClick=\{onCheck\} disabled=\{busy \|\| settingsLocked\}/);
});

test("the shell never treats a truncated profile page or stale unsaved identity as exhaustive", async () => {
  const [frontend, switcher] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/ClientSwitcher.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(frontend, /const selected = companies\.find\(\(company\) => tallyCompanyKey\(company\) === selectedCompany\);\s*if \(selected && !selected\.mirror_company_id\) setSelectedCompany\(""\);/s);
  assert.match(frontend, /const clientSwitcherCompanies = companies\.filter\(\s*\(company\) => Boolean\(company\.mirror_company_id\) \|\| company\.canonical_endpoint === currentProbeCanonicalOrigin,/s);
  assert.match(frontend, /\.\.\.clientSwitcherCompanies\.map\(\(company\) =>/);
  assert.match(frontend, /profilesTruncated=\{persistedCompanyProfilesTruncated\}/);
  assert.match(frontend, /loadedProfileCount=\{persistedCompanyProfilesLoaded\}/);
  assert.match(switcher, /profilesTruncated: boolean;/);
  assert.match(switcher, /The fetched saved-profile page contains only the newest \$\{loadedProfileCount\} records; an older saved client may still exist\./);
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
