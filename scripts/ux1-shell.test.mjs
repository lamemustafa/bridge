// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("UX1 keeps client selection searchable, truthful about read readiness, and explicit about unavailable sections", async () => {
  const [app, switcher, outstandings, allClients] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/ClientSwitcher.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(app, /const NON_TALLY_SECTIONS_ENABLED = false/);
  assert.match(app, /disabled=\{!NON_TALLY_SECTIONS_ENABLED\}/);
  assert.match(app, /onClick=\{\(\) => setView\("gst"\)\}/);
  assert.match(app, /onClick=\{\(\) => setView\("dsc"\)\}/);
  assert.match(app, /onClick=\{\(\) => setView\("documents"\)\}/);
  assert.match(app, /onClick=\{\(\) => setView\("axal"\)\}/);
  assert.match(app, /\{!NON_TALLY_SECTIONS_ENABLED && <small>Not yet available<\/small>\}/);
  assert.match(app, /Not yet available/);
  assert.match(app, /if \(!NON_TALLY_SECTIONS_ENABLED\) \{\s*setDashboardError\("GST availability is unavailable until end-to-end workflow evidence is complete\."\);\s*return;/s);
  const dashboard = app.slice(app.indexOf('{view === "dashboard" && ('), app.indexOf('{view === "gst" && ('));
  assert.match(dashboard, /\{NON_TALLY_SECTIONS_ENABLED \? \([\s\S]*?Check GST Availability[\s\S]*?: \(\s*<p className="future-sections-note" role="status">GST availability is unavailable until end-to-end workflow evidence is complete\.<\/p>/);
  assert.match(app, /\{!NON_TALLY_SECTIONS_ENABLED && \(\s*<p className="future-sections-note" id="future-sections-note">Unavailable until their workflow evidence is complete\.<\/p>\s*\)\}/);
  assert.match(app, /port: 9000/);
  assert.match(app, /currentProbeCanonicalOrigin/);
  assert.match(app, /company\.canonical_endpoint === currentProbeCanonicalOrigin/);
  assert.doesNotMatch(app, /function configuredTallyEndpoint/);
  assert.match(app, /const selectedCompanyReadable = selectedCompanyReady/);
  assert.match(app, /const \[view, setView\] = React\.useState<View>\("dashboard"\)/);
  assert.match(app, /if \(view !== "companies" && view !== "outstandings" && view !== "clients"\) return;/);
  assert.match(app, /<OutstandingsScreen\s+key=\{selectedCompany \|\| "unselected"\}/);
  assert.match(app, /setOpenCompanyNames\(\[\]\);\s*setUntrustedDiscoveredCompanies\(\[\]\);/);
  assert.match(app, /correlation_key: company\.correlation_key/);
  assert.match(app, /onOpen=\{\(\) => void refreshPersistedCompanyProfiles\(\)\}/);
  assert.match(app, /const \[persistedCompanyProfilesLoading, setPersistedCompanyProfilesLoading\] = React\.useState\(false\);/);
  assert.match(app, /const persistedCompanyProfileLoadVersion = React\.useRef\(0\);/);
  assert.match(app, /setPersistedCompanyProfilesLoading\(true\);[\s\S]*?if \(loadVersion !== persistedCompanyProfileLoadVersion\.current\) return;/);
  assert.match(app, /profilesLoading=\{persistedCompanyProfilesLoading\}/);
  assert.match(app, /activeView=\{view\}/);
  assert.match(app, /loadError=\{persistedCompanyProfileError \? toErrorMessage\(persistedCompanyProfileError\) : null\}/);
  assert.match(app, /profilesTruncated=\{persistedCompanyProfilesTruncated\}/);
  assert.match(app, /loadedProfileCount=\{persistedCompanyProfilesLoaded\}/);
  assert.doesNotMatch(app, /fetch_saved_tally_outstandings|detect_saved_tally_base_currency/);
  assert.match(switcher, /type="search"/);
  assert.match(switcher, /profilesLoading: boolean;/);
  assert.match(switcher, /aria-busy=\{profilesLoading\}/);
  assert.match(switcher, /\{profilesLoading && <p className="client-switcher-empty" role="status">Loading saved client profiles…<\/p>\}/);
  assert.match(switcher, /disabled=\{selectionLocked\}/);
  assert.match(switcher, /if \(!current\) onOpen\(\);/);
  assert.match(switcher, /activeView: string;/);
  assert.match(switcher, /React\.useEffect\(\(\) => \{\s*setOpen\(false\);\s*setQuery\(""\);\s*\}, \[activeView\]\);/s);
  assert.match(switcher, /client\.searchText\.toLocaleLowerCase\(\)\.includes/);
  assert.match(switcher, /client\.identityDiscriminator/);
  assert.match(switcher, /selected && <small>\{selected\.summaryDiscriminator\}<\/small>/);
  assert.match(switcher, /\{loadError && <p className="client-switcher-error" role="alert">\{loadError\}<\/p>\}/);
  assert.match(switcher, /filtered\.length === 0 && !loadError/);
  assert.match(switcher, /filtered\.length === 0 && !loadError && !profilesLoading/);
  assert.match(switcher, /profilesTruncated[\s\S]*?The fetched saved-profile page contains only the newest \$\{loadedProfileCount\} records/);
  assert.match(switcher, /close\(\);\s*onManageTally\(\);/s);
  assert.match(switcher, /onKeyDown=[\s\S]*?event\.key === "Escape"/);
  assert.match(switcher, /Setup and verification are required before other clients can be read/);
  assert.match(app, /const identityDiscriminator = clientIdentityDiscriminator\(company\);/);
  assert.match(app, /const summaryDiscriminator = clientIdentitySummaryDiscriminator\(company\);/);
  assert.match(app, /summaryDiscriminator,/);
  assert.match(app, /searchText: `\$\{company\.name\} \$\{identityDiscriminator\}`/);
  assert.match(app, /Company no\. \$\{company\.company_number \?\? "not observed"\}/);
  assert.match(app, /Books from \$\{booksFrom\}/);
  assert.match(app, /GUID \$\{company\.guid \?\? "not observed"\}/);
  assert.match(app, /Endpoint \$\{company\.canonical_endpoint \?\? "not observed"\}/);
  assert.match(app, /disabled=\{childTallyReadCount > 0\}[\s\S]*?Outstandings/);
  assert.match(app, /aria-current=\{view === "clients" \? "page" : undefined\}[\s\S]*?disabled=\{childTallyReadCount > 0\}[\s\S]*?Compare clients/);
  assert.match(app, /onClick=\{\(\) => setView\("outstandings"\)\} disabled=\{childTallyReadCount > 0\} aria-describedby=\{childTallyReadCount > 0 \? "active-tally-read-note" : undefined\}>Open outstandings/);
  assert.match(app, /A Tally read is still in progress\. Wait before opening another live read\./);
  assert.match(app, /liveReadActionsLocked=\{childTallyReadCount > 0\}/);
  assert.match(outstandings, /liveReadNavigationLocked: boolean;/);
  assert.match(outstandings, /onClick=\{onViewAllClients\} disabled=\{liveReadNavigationLocked\}/);
  assert.match(allClients, /liveReadNavigationLocked: boolean;/);
  assert.match(allClients, /onClick=\{onBack\} disabled=\{liveReadNavigationLocked\}/);
  assert.match(allClients, /key=\{row\.companyGuid\}\s*disabled=\{liveReadNavigationLocked\}/);
  const mirrorProof = await readFile(new URL("../src/MirrorProofScreen.tsx", import.meta.url), "utf8");
  assert.match(mirrorProof, /liveReadActionsLocked: boolean;/);
  assert.match(mirrorProof, /disabled=\{!selectedCompanyRecord\?\.mirror_company_id \|\| !selectedCompanyLive \|\| snapshotActive \|\| snapshotStartOutcomeUnknown \|\| liveReadActionsLocked \|\| tallyAction !== null\}/);
  assert.match(mirrorProof, /disabled=\{liveReadActionsLocked \|\| tallyAction !== null\}/);
  assert.match(mirrorProof, /Wait before starting or resuming a Core Accounting read\./);
});

test("UX1 nav sends unreadable outstandings and client requests to Manage Tally", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const nav = app.slice(app.indexOf('<nav aria-label="Bridge operations">'), app.indexOf("</nav>"));

  assert.match(nav, /Outstandings/);
  assert.match(nav, /Compare clients/);
  assert.match(nav, /Manage Tally/);
  assert.match(nav, /Evidence dashboard/);
  assert.match(nav, /onClick=\{\(\) => setView\(selectedCompanyReadable \? "outstandings" : "companies"\)\}/);
  assert.match(nav, /onClick=\{\(\) => setView\(selectedCompanyReadable \? "clients" : "companies"\)\}/);
});

test("UX2 keeps report evidence distinct from Core Accounting history and hides operator tools behind Advanced", async () => {
  const [app, outstandings, mirrorProof, panel, focus] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/MirrorProofScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/evidence-drawer-focus.ts", import.meta.url), "utf8"),
  ]);
  const nav = app.slice(app.indexOf('<nav aria-label="Bridge operations">'), app.indexOf("</nav>"));
  const advanced = mirrorProof.slice(
    mirrorProof.indexOf('<details className="evidence-advanced">'),
    mirrorProof.lastIndexOf("</details>"),
  );

  assert.doesNotMatch(nav, /Mirror/);
  assert.match(outstandings, /className="outstandings-evidence-link" type="button" onClick=\{\(event\) => onOpenEvidence\(reportEvidence, event\.currentTarget\)\}/);
  assert.match(outstandings, /asOfYyyymmdd: inrCompleteResult\.report\.as_of_yyyymmdd/);
  assert.match(outstandings, /ageingAnchor: inrCompleteResult\.ageing_anchor/);
  assert.match(outstandings, /readProvenance: inrCompleteResult\.report/);
  assert.match(outstandings, /reasonCode: result\.reason_code/);
  assert.match(outstandings, /tallyReadAttempted: partialState\?\.tallyReadAttempted/);
  assert.match(app, /\{evidenceDrawerOpen && \(/);
  assert.match(app, /className="evidence-drawer"/);
  assert.match(app, /event\.key === "Escape"\) \{\s*closeEvidenceDrawer\(\);\s*return;/s);
  assert.match(app, /visibleDrawerTabStops\(event\.currentTarget\)/);
  assert.match(app, /<OutstandingsEvidencePanel evidence=\{outstandingsEvidence\} \/>/);
  assert.match(app, /Separate Core Accounting evidence/);
  assert.match(app, /\{selectedCompanyRecord\?\.mirror_company_id && \(/);
  assert.match(app, /Open local evidence/);
  assert.match(panel, /Report-bound Outstandings read/);
  assert.match(panel, /No Outstandings read attached/);
  assert.match(focus, /querySelectorAll<HTMLElement>\("\*"\)/);
  assert.match(focus, /element\.tabIndex >= 0/);
  assert.match(focus, /current\.tagName === "DETAILS"/);
  assert.match(focus, /window\.getComputedStyle\(current\)/);
  assert.match(focus, /getClientRects\(\)\.length > 0/);
  assert.doesNotMatch(focus, /DRAWER_FOCUSABLE/);
  assert.doesNotMatch(mirrorProof, /What “Verified” will require/);
  assert.match(advanced, /className="panel wide mirror-explorer"/);
  assert.match(advanced, /<h2>Pack readiness<\/h2>/);
  assert.match(advanced, /className="panel wide runtime-panel"/);
});

test("compatibility surface binds every UX1 Tally read entry control", async () => {
  const [surfaceBytes, clientSwitcher, mirrorProof] = await Promise.all([
    readFile(new URL("../docs/tally/compatibility/compatibility-surface.json", import.meta.url), "utf8"),
    readFile(new URL("../src/ClientSwitcher.tsx", import.meta.url)),
    readFile(new URL("../src/MirrorProofScreen.tsx", import.meta.url)),
  ]);
  const hashes = new Map(JSON.parse(surfaceBytes).files.map(({ path, sha256 }) => [path, sha256]));

  assert.equal(hashes.get("src/ClientSwitcher.tsx"), createHash("sha256").update(clientSwitcher).digest("hex"));
  assert.equal(hashes.get("src/MirrorProofScreen.tsx"), createHash("sha256").update(mirrorProof).digest("hex"));
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
  assert.match(styles, /\.clients-table\s*\{[\s\S]*?overflow-x:\s*auto;/);
  assert.match(styles, /\.clients-row\s*\{[\s\S]*?min-width:\s*680px;/);
  assert.match(styles, /@media \(max-width: 860px\)\s*\{[\s\S]*?\.client-switcher-current\s*\{\s*flex:\s*0 0 auto;/);

  const tableCount = [...mirrorProof.matchAll(/<table\b[^>]*>/g)].length;
  const scrollableTableCount = [...mirrorProof.matchAll(/<div className="table-wrap"[^>]*>\s*<table\b[^>]*>/g)].length;
  assert.equal(scrollableTableCount, tableCount, "every Mirror & Proof table must have a preceding table-wrap container");
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Recent durable Core Accounting runs" tabIndex=\{0\}>/);
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Paged local mirror records" tabIndex=\{0\}>/);
  assert.match(mirrorProof, /<div className="table-wrap" role="region" aria-label="Hash-linked local proof ledger" tabIndex=\{0\}>/);
});
