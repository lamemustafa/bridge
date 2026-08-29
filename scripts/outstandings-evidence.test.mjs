// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { companyIdentityKey, companyIdentityLabel } from "../src/company-identity.ts";
import { reportEvidenceDrawerEntry } from "../src/evidence-drawer-entry.ts";
import { readProvenance } from "../src/outstandings-provenance.ts";

test("evidence scope uses the Rust-owned read strategy, not a count inference", async () => {
  const [screen, panel, runtime] = await Promise.all([
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/tally/runtime.rs", import.meta.url), "utf8"),
  ]);

  assert.equal(readProvenance({ read_strategy: "native_bills", source_voucher_count: 0, open_receivable_bill_count: 0 }), "0 open bills read from Tally");
  assert.equal(readProvenance({ read_strategy: "voucher_scan", source_voucher_count: 0, open_receivable_bill_count: 0 }), "0 vouchers verified");
  assert.equal(readProvenance({ read_strategy: "voucher_scan", source_voucher_count: 1, open_receivable_bill_count: 2 }), "1 voucher verified");
  assert.match(screen, /readProvenance: \{\s*read_strategy: inrCompleteResult\.read_strategy,/s);
  assert.match(screen, /read_strategy: OutstandingsReadProvenance\["read_strategy"\]/);
  assert.match(screen, /readProvenance: \{\s*read_strategy: inrCompleteResult\.read_strategy,/s);
  assert.match(runtime, /read_strategy: OutstandingsReadStrategy::NativeBills,/);
  assert.match(runtime, /read_strategy: OutstandingsReadStrategy::VoucherScan,/);
  assert.match(panel, /readProvenance\(evidence\.readProvenance\)/);
  assert.doesNotMatch(panel, /sourceVoucherCount/);
});

test("selecting a saved company within local evidence issues no Tally invoke", async () => {
  const [app, screen] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(app, /liveReadSuppressed=\{evidenceDrawerOpen && evidenceDrawerEntry\.kind === "local-only"\}/);
  assert.match(screen, /liveReadSuppressed: boolean;/);
  assert.match(screen, /const readPermitted = !liveReadSuppressed && currencyReadPermitted && requestedAsOf !== null;/);
  assert.match(screen, /if \(liveReadSuppressed \|\| !company \|\| inrAssertedCompanyIdentity === companyIdentityFor\(company\)\) return;/);
  assert.match(screen, /\[config\.host, config\.port, company\?\.guid, company\?\.name, company\?\.company_number, company\?\.books_from_yyyymmdd, inrAssertedCompanyIdentity, liveReadSuppressed, onTallyReadActivityChange\]/);
});

test("unsupported currency evidence is withheld without amounts", async () => {
  const [screen, panel] = await Promise.all([
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8"),
  ]);
  const reportEvidence = screen.slice(
    screen.indexOf("const reportEvidence"),
    screen.indexOf("const load = React.useCallback"),
  );
  const withheldEvidence = reportEvidence.slice(
    reportEvidence.indexOf('state: "withheld"'),
    reportEvidence.indexOf("};", reportEvidence.indexOf('state: "withheld"')),
  );

  assert.match(screen, /const inrCompleteResult = isInrCompleteResult\(result\) \? result : null;/);
  assert.match(reportEvidence, /: inrCompleteResult\s*\? \{[\s\S]*?currencyAssertion: inrCompleteResult\.currency_assertion,[\s\S]*?receivableTotal: inrCompleteResult\.report\.receivable_total,/);
  assert.match(withheldEvidence, /state: "withheld"/);
  assert.doesNotMatch(withheldEvidence, /receivableTotal|payableTotal/);
  assert.match(panel, /evidence\.state === "complete" \? "Complete result" : evidence\.state === "partial" \? "Partial result" : evidence\.title/);
});

test("evidence identity distinguishes same-named books by the pinned composite key", async () => {
  const panel = await readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8");
  const shared = {
    canonical_origin: "http://127.0.0.1:9000",
    company_guid: "same-guid",
    company_number: "100001",
    company_name: "Same Name",
  };
  const fy26 = companyIdentityKey({ ...shared, books_from_yyyymmdd: "20250401" });
  const fy27 = companyIdentityKey({ ...shared, books_from_yyyymmdd: "20260401" });

  assert.notEqual(fy26, fy27);
  assert.notEqual(companyIdentityLabel(fy26), companyIdentityLabel(fy27));
  assert.match(companyIdentityLabel(fy26), /Same Name · Company no\. 100001 · Books from 20250401 · GUID same-guid · Endpoint http:\/\/127\.0\.0\.1:9000/);
  assert.match(panel, /companyIdentityLabel\(evidence\.companyIdentity\)/);
});

test("local-only and failed report reads have distinct no-evidence drawer entries", async () => {
  const [app, panel, screen] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);
  const localEvidenceEntry = app.slice(
    app.indexOf('{selectedCompanyRecord?.mirror_company_id && ('),
    app.indexOf('{setupConnectionComplete && ('),
  );
  const openDrawer = app.slice(
    app.indexOf('const openEvidenceDrawer'),
    app.indexOf('const closeEvidenceDrawer'),
  );

  assert.match(localEvidenceEntry, /Open local evidence/);
  assert.match(localEvidenceEntry, /openEvidenceDrawer\(\{ kind: "local-only" \}, event\.currentTarget\)/);
  assert.doesNotMatch(localEvidenceEntry, /selectedCompanyReadable/);
  assert.doesNotMatch(openDrawer, /invoke\(|setView\("outstandings"\)/);
  assert.deepEqual(reportEvidenceDrawerEntry(null, null), { kind: "report-not-read" });
  assert.deepEqual(reportEvidenceDrawerEntry(null, "Tally connection ended"), {
    kind: "report-read-failed",
    message: "Tally connection ended",
  });
  assert.match(screen, /onOpenEvidence\(reportEvidenceDrawerEntry\(reportEvidence, error\), event\.currentTarget\)/);
  assert.match(panel, /entry\.kind === "local-only"/);
  assert.match(panel, /This drawer was opened for local evidence review, not from an Outstandings report\./);
  assert.match(panel, /entry\.kind === "report-read-failed"/);
  assert.match(panel, /Outstandings read failed/);
  assert.match(panel, /Bridge could not complete the report-bound read: \{entry\.message\}/);
  assert.doesNotMatch(panel.slice(panel.indexOf('entry.kind === "report-read-failed"'), panel.indexOf("const { evidence } = entry")), /local evidence review/);
});
