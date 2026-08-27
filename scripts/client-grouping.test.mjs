// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { applyClientGroupLabel, groupClientRows, isLatestClientGroupLabelSave, issueClientGroupLabelSave, reconcileLoadedSortPreference, rollbackFailedClientGroupLabel, sumExactDecimals } from "../src/client-grouping.ts";
import { companyIdentityKey } from "../src/company-identity.ts";

// Mirrors AllClientsScreen's saveGroupLabel wiring exactly (issue a stamp,
// apply the optimistic edit, then on settle check the stamp against the
// per-company sequence before touching `persisted`, `labels`, or `error`)
// so these tests exercise the same ordering guard the component runs,
// without needing to render React.
function createGroupLabelSaveHarness(initialPersisted) {
  const state = {
    persisted: { ...initialPersisted },
    labels: { ...initialPersisted },
    error: null,
    sequence: {},
  };

  function issue(companyGuid, attemptedLabel) {
    state.error = null;
    const issued = issueClientGroupLabelSave(state.sequence, companyGuid);
    state.sequence = issued.sequence;
    const stamp = issued.stamp;
    state.labels = applyClientGroupLabel(state.labels, companyGuid, attemptedLabel);
    return {
      succeed() {
        if (!isLatestClientGroupLabelSave(state.sequence, companyGuid, stamp)) return;
        state.persisted = applyClientGroupLabel(state.persisted, companyGuid, attemptedLabel);
      },
      fail() {
        if (!isLatestClientGroupLabelSave(state.sequence, companyGuid, stamp)) return;
        state.labels = rollbackFailedClientGroupLabel(state.labels, companyGuid, attemptedLabel, state.persisted);
        state.error = "Bridge could not save this group label. The previous label was restored; your figures are unchanged.";
      },
    };
  }

  function type(companyGuid, label) {
    state.labels = applyClientGroupLabel(state.labels, companyGuid, label);
  }

  return { issue, type, state };
}

test("a failed optimistic label save restores only the value that actually failed", () => {
  const persisted = { "synthetic-company-guid": "Original" };
  const optimistic = applyClientGroupLabel(persisted, "synthetic-company-guid", "North");
  assert.deepEqual(
    rollbackFailedClientGroupLabel(optimistic, "synthetic-company-guid", "North", persisted),
    persisted,
  );

  const newerEdit = applyClientGroupLabel(optimistic, "synthetic-company-guid", "Newer edit");
  assert.deepEqual(
    rollbackFailedClientGroupLabel(newerEdit, "synthetic-company-guid", "North", persisted),
    newerEdit,
    "a late failure must not erase typing that happened after the failed attempt",
  );
  assert.deepEqual(
    rollbackFailedClientGroupLabel({ "synthetic-company-guid": "North" }, "synthetic-company-guid", "North", {}),
    {},
    "a failed first save returns the company to ungrouped",
  );
});

test("an older save settling after a newer save has already failed does not disturb the rolled-back UI", () => {
  const harness = createGroupLabelSaveHarness({ "synthetic-company-guid": "Original" });
  const older = harness.issue("synthetic-company-guid", "Older label");
  const newer = harness.issue("synthetic-company-guid", "Newer label");

  // The newer save is still the latest issued when it settles, so its
  // failure is genuine: it rolls back and surfaces the error.
  newer.fail();
  assert.equal(harness.state.labels["synthetic-company-guid"], "Original");
  assert.ok(harness.state.error, "the newest failing save must surface the error");

  // The older save now settles too, after the newer one already failed.
  // It has been superseded, so its success must be inert: no second,
  // stale rollback/update, and `persisted` must stay in lockstep with
  // what the UI already shows.
  older.succeed();
  assert.equal(
    harness.state.labels["synthetic-company-guid"],
    "Original",
    "a stale success settling after a legitimate rollback must not disturb it",
  );
  assert.equal(
    harness.state.persisted["synthetic-company-guid"],
    "Original",
    "persisted must not silently diverge from what the UI displays",
  );
});

test("a stale success does not overwrite the record of a newer success", () => {
  const harness = createGroupLabelSaveHarness({});
  const older = harness.issue("synthetic-company-guid", "First");
  const newer = harness.issue("synthetic-company-guid", "Second");

  newer.succeed();
  assert.equal(harness.state.persisted["synthetic-company-guid"], "Second");

  older.succeed();
  assert.equal(
    harness.state.persisted["synthetic-company-guid"],
    "Second",
    "a slower, superseded success must not overwrite a newer save's outcome",
  );
});

test("the latest issued save failing still rolls back the UI and surfaces the error", () => {
  const harness = createGroupLabelSaveHarness({ "synthetic-company-guid": "Original" });
  const save = harness.issue("synthetic-company-guid", "Attempted");

  save.fail();

  assert.equal(harness.state.labels["synthetic-company-guid"], "Original");
  assert.match(harness.state.error ?? "", /could not save/);
});

test("the existing 'user typed something else' guard still blocks rollback for the latest issued save", () => {
  const harness = createGroupLabelSaveHarness({ "synthetic-company-guid": "Original" });
  const save = harness.issue("synthetic-company-guid", "Attempted");

  // The user keeps typing after this save was fired, before it settles.
  harness.type("synthetic-company-guid", "Freshly typed");

  save.fail();

  assert.equal(
    harness.state.labels["synthetic-company-guid"],
    "Freshly typed",
    "typing after the failed attempt must not be clobbered by its rollback, even though it was the latest issued save",
  );
});

test("a late preference load cannot overwrite a sort chosen during startup", () => {
  const current = { key: "client", desc: false };
  const persisted = { key: "overdue", desc: true };
  assert.deepEqual(reconcileLoadedSortPreference(current, persisted, true), current);
  assert.deepEqual(reconcileLoadedSortPreference(current, persisted, false), persisted);
});

test("applying a group label preserves every company figure byte-for-byte", () => {
  const row = {
    companyGuid: "synthetic-company-guid",
    company: "Synthetic Components Ltd",
    exactAmounts: { receivable: "482001.25", overdue: "120400.5", unallocated: "7580.75" },
    oldest: 97,
    complete: true,
  };
  const before = Buffer.from(JSON.stringify(row));

  const grouped = groupClientRows([row], { "synthetic-company-guid": "North practice" });
  const groupedRow = grouped.groups[0].rows[0];

  assert.deepEqual(Buffer.from(JSON.stringify(groupedRow)), before);
  assert.deepEqual(grouped.groups[0].totals, {
    receivable: "482001.25",
    overdue: "120400.5",
    unallocated: "7580.75",
  });
  assert.equal(grouped.ungroupedRows.length, 0);
});

test("ungrouped companies remain separate and receive no synthetic total", () => {
  const rows = [
    { companyGuid: "synthetic-a", exactAmounts: { receivable: "10", overdue: "2", unallocated: "1" } },
    { companyGuid: "synthetic-b", exactAmounts: { receivable: "20", overdue: "3", unallocated: "4" } },
  ];

  const grouped = groupClientRows(rows, {});

  assert.deepEqual(grouped.groups, []);
  assert.equal(grouped.ungroupedRows.length, 2);
});

test("raw GUID labels intentionally group split books", () => {
  const rows = [
    { companyGuid: "[composite-parent]", sourceGuid: "legacy-split", exactAmounts: { receivable: "10", overdue: "2", unallocated: "1" } },
    { companyGuid: "[composite-child]", sourceGuid: "legacy-split", exactAmounts: { receivable: "20", overdue: "3", unallocated: "4" } },
  ];
  const grouped = groupClientRows(rows, { "legacy-split": "Split practice" });
  assert.equal(grouped.groups.length, 1);
  assert.deepEqual(grouped.groups[0].totals, { receivable: "30", overdue: "5", unallocated: "5" });
});

test("the shared composite key keeps endpoint, GUID, number, name, and books-from distinct", () => {
  const base = {
    canonical_origin: "http://127.0.0.1:9000",
    company_guid: "SAME-GUID",
    company_number: "100001",
    company_name: "Client Book",
    books_from_yyyymmdd: "20260401",
  };
  const key = companyIdentityKey(base);
  assert.equal(key, companyIdentityKey({ ...base, company_guid: "same-guid" }));
  for (const changed of [
    { canonical_origin: "http://127.0.0.1:9001" },
    { company_number: "100002" },
    { company_name: "Client Book FY27" },
    { books_from_yyyymmdd: "20270401" },
  ]) {
    assert.notEqual(key, companyIdentityKey({ ...base, ...changed }));
  }
});

test("group totals preserve decimal precision without IEEE-754 rounding", () => {
  assert.equal(sumExactDecimals(["9007199254740993", "0.01", "-1"]), "9007199254740992.01");
  assert.equal(sumExactDecimals(["42.00", "0.10"]), "42.1");
  assert.equal(sumExactDecimals(["10", "not-an-amount"]), undefined);
});

test("client amount views fail visibly instead of coercing or flipping amounts", async () => {
  const [allClients, outstandings] = await Promise.all([
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(allClients, /Amount unavailable/);
  assert.doesNotMatch(allClients, /Math\.abs/);
  assert.match(outstandings, /Bridge could not read an outstandings amount/);
  assert.doesNotMatch(outstandings, /Math\.abs/);
});

test("all-client responses carry the pinned composite tuple back to the open action", async () => {
  const [allClients, commands, main, companyIdentity] = await Promise.all([
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8"),
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/company-identity.ts", import.meta.url), "utf8"),
  ]);

  assert.match(commands, /pub company_guid: String/);
  assert.match(commands, /company_guid: selected\.company_guid/);
  assert.match(commands, /company_number: selected\.company_number/);
  assert.match(commands, /books_from_yyyymmdd: selected\.books_from_yyyymmdd/);
  assert.match(allClients, /companyGuid: companyIdentityKey\(\{/);
  assert.match(allClients, /company\.company_number === row\.companyNumber/);
  assert.match(allClients, /disabled=\{!groupLabelsReady\}/);
  assert.match(allClients, /canonical_origin: entry\.canonical_origin/);
  assert.doesNotMatch(companyIdentity, /canonicalOriginForConfig/);
  assert.match(allClients, /key=\{row\.companyGuid\}/);
  assert.match(allClients, /groupLabels\[row\.sourceGuid\]/);
  assert.doesNotMatch(allClients, /companies\.find\(\(company\) => company\.name === entry\.company\)/);
  assert.match(main, /const completeCurrentProbeCompanies = currentProbeCompanyList\.filter\(/);
  assert.match(main, /companies=\{completeCurrentProbeCompanies/);
  assert.match(main, /openBookCount=\{completeCurrentProbeCompanies\.length\}/);
});
