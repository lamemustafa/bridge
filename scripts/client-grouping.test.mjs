// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { groupClientRows } from "../src/client-grouping.ts";

test("applying a group label preserves every company figure byte-for-byte", () => {
  const row = {
    companyGuid: "synthetic-company-guid",
    company: "Synthetic Components Ltd",
    receivable: 482001.25,
    overdue: 120400.5,
    unallocated: 7580.75,
    oldest: 97,
    complete: true,
  };
  const before = Buffer.from(JSON.stringify(row));

  const grouped = groupClientRows([row], { "synthetic-company-guid": "North practice" });
  const groupedRow = grouped.groups[0].rows[0];

  assert.deepEqual(Buffer.from(JSON.stringify(groupedRow)), before);
  assert.deepEqual(grouped.groups[0].totals, {
    receivable: 482001.25,
    overdue: 120400.5,
    unallocated: 7580.75,
  });
  assert.equal(grouped.ungroupedRows.length, 0);
});

test("ungrouped companies remain separate and receive no synthetic total", () => {
  const rows = [
    { companyGuid: "synthetic-a", receivable: 10, overdue: 2, unallocated: 1 },
    { companyGuid: "synthetic-b", receivable: 20, overdue: 3, unallocated: 4 },
  ];

  const grouped = groupClientRows(rows, {});

  assert.deepEqual(grouped.groups, []);
  assert.equal(grouped.ungroupedRows.length, 2);
});
