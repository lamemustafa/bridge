// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { companyIdentityKey, companyIdentityLabel } from "../src/company-identity.ts";
import { readProvenance } from "../src/outstandings-provenance.ts";

test("evidence scope uses the report's shared bill-or-voucher formatter", async () => {
  const [screen, panel] = await Promise.all([
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsEvidencePanel.tsx", import.meta.url), "utf8"),
  ]);

  assert.equal(readProvenance({ source_voucher_count: 0, open_receivable_bill_count: 2 }), "2 open bills read from Tally");
  assert.equal(readProvenance({ source_voucher_count: 1, open_receivable_bill_count: 2 }), "1 voucher verified");
  assert.match(screen, /readProvenance: inrCompleteResult\.report/);
  assert.match(panel, /readProvenance\(evidence\.readProvenance\)/);
  assert.doesNotMatch(panel, /sourceVoucherCount/);
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

test("saved local evidence opens the drawer without entering a Tally read view", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const localEvidenceEntry = app.slice(
    app.indexOf('{selectedCompanyRecord?.mirror_company_id && ('),
    app.indexOf('{setupConnectionComplete && ('),
  );
  const openDrawer = app.slice(
    app.indexOf('const openEvidenceDrawer'),
    app.indexOf('const closeEvidenceDrawer'),
  );

  assert.match(localEvidenceEntry, /Open local evidence/);
  assert.match(localEvidenceEntry, /openEvidenceDrawer\(null, event\.currentTarget\)/);
  assert.doesNotMatch(localEvidenceEntry, /selectedCompanyReadable/);
  assert.doesNotMatch(openDrawer, /invoke\(|setView\("outstandings"\)/);
});
