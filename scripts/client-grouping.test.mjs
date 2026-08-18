// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { applyClientGroupLabel, groupClientRows, reconcileLoadedSortPreference, rollbackFailedClientGroupLabel, sumExactDecimals } from "../src/client-grouping.ts";

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

test("all-client responses carry the pinned GUID back to the open action", async () => {
  const [allClients, commands] = await Promise.all([
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8"),
  ]);

  assert.match(commands, /pub company_guid: String/);
  assert.match(commands, /company_guid: entry\.expected_company_guid/);
  assert.match(allClients, /companyGuid: entry\.company_guid/);
  assert.doesNotMatch(allClients, /companies\.find\(\(company\) => company\.name === entry\.company\)/);
});
