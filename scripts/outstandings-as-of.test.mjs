// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  allCompaniesOutstandingsInvokeArgument,
  automaticOutstandingsAsOf,
  asOfBoundValueForAsOf,
  asOfYyyymmdd,
  bulkPartyStatementsInvokeArgument,
  operatorSelectedOutstandingsAsOf,
  partyStatementInvokeArgument,
  refreshAutomaticOutstandingsAsOf,
  settleAsOfBoundValue,
  singleCompanyOutstandingsInvokeArgument,
  todayAsDateInput,
  workingPaperInvokeArgument,
} from "../src/outstandings-as-of.ts";

test("the visible as-of control defaults to the operator's local calendar date", () => {
  assert.equal(todayAsDateInput(new Date(2026, 7, 22, 0, 1)), "2026-08-22");
  assert.equal(asOfYyyymmdd("2026-08-22"), "20260822");
  assert.equal(asOfYyyymmdd("2026-8-22"), null);
});

test("an automatic as-of date rolls over locally without replacing an operator selection", () => {
  const beforeMidnight = new Date(2026, 7, 22, 23, 59, 59);
  const afterMidnight = new Date(2026, 7, 23, 0, 0, 1);

  assert.deepEqual(
    refreshAutomaticOutstandingsAsOf(automaticOutstandingsAsOf(beforeMidnight), afterMidnight),
    { value: "2026-08-23", operatorSelected: false },
  );
  assert.deepEqual(
    refreshAutomaticOutstandingsAsOf(operatorSelectedOutstandingsAsOf("2026-08-01"), afterMidnight),
    { value: "2026-08-01", operatorSelected: true },
  );
});

test("all-client rows are invalidated at rollover and a late old-date sweep is discarded", () => {
  const beforeMidnight = automaticOutstandingsAsOf(new Date(2026, 7, 22, 23, 59, 59));
  const afterMidnight = refreshAutomaticOutstandingsAsOf(
    beforeMidnight,
    new Date(2026, 7, 23, 0, 0, 1),
  );
  const entries = [{ company: "Synthetic Company", company_guid: "synthetic-guid" }];
  const completedBeforeRollover = settleAsOfBoundValue(
    asOfYyyymmdd(beforeMidnight.value),
    asOfYyyymmdd(beforeMidnight.value),
    entries,
  );

  assert.deepEqual(
    asOfBoundValueForAsOf(completedBeforeRollover, asOfYyyymmdd(beforeMidnight.value)),
    entries,
  );
  assert.equal(
    asOfBoundValueForAsOf(completedBeforeRollover, asOfYyyymmdd(afterMidnight.value)),
    null,
  );
  assert.equal(
    settleAsOfBoundValue(
      asOfYyyymmdd(afterMidnight.value),
      asOfYyyymmdd(beforeMidnight.value),
      entries,
    ),
    null,
  );
});

test("single-client totals are invalidated at rollover and a late old-date read is discarded", () => {
  const beforeMidnight = automaticOutstandingsAsOf(new Date(2026, 7, 22, 23, 59, 59));
  const afterMidnight = refreshAutomaticOutstandingsAsOf(
    beforeMidnight,
    new Date(2026, 7, 23, 0, 0, 1),
  );
  const completeResult = { state: "complete", report: { as_of_yyyymmdd: "20260822" } };
  const completedBeforeRollover = settleAsOfBoundValue(
    asOfYyyymmdd(beforeMidnight.value),
    asOfYyyymmdd(beforeMidnight.value),
    completeResult,
  );

  assert.deepEqual(
    asOfBoundValueForAsOf(completedBeforeRollover, asOfYyyymmdd(beforeMidnight.value)),
    completeResult,
  );
  assert.equal(
    asOfBoundValueForAsOf(completedBeforeRollover, asOfYyyymmdd(afterMidnight.value)),
    null,
  );
  assert.equal(
    settleAsOfBoundValue(
      asOfYyyymmdd(afterMidnight.value),
      asOfYyyymmdd(beforeMidnight.value),
      completeResult,
    ),
    null,
  );
});

test("both outstandings screens wire date-bound rendering and stale-settlement rejection", async () => {
  const [singleClient, allClients] = await Promise.all([
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/AllClientsScreen.tsx", import.meta.url), "utf8"),
  ]);

  for (const source of [singleClient, allClients]) {
    assert.match(source, /asOfBoundValueForAsOf/);
    assert.match(source, /settleAsOfBoundValue/);
    assert.match(source, /requestVersion\.current \+= 1/);
  }
});

test("the single-company request emits the selected canonical as-of date", () => {
  assert.deepEqual(
    singleCompanyOutstandingsInvokeArgument(
      { host: "127.0.0.1", port: 9000 },
      { name: "Bridge Validation Lab", guid: "guid-1" },
      "2026-08-17",
      "bill_date",
    ),
    {
      request: {
        config: { host: "127.0.0.1", port: 9000 },
        company: "Bridge Validation Lab",
        expected_company_guid: "guid-1",
        currency_assertion: "INR",
        as_of_yyyymmdd: "20260817",
        ageing_anchor: "bill_date",
      },
    },
  );
  assert.equal(singleCompanyOutstandingsInvokeArgument({ host: "127.0.0.1", port: 9000 }, { name: "Lab", guid: "guid-1" }, "2026-8-17", "due_date"), null);
});

test("compare clients emits the same selected canonical as-of date", () => {
  assert.deepEqual(
    allCompaniesOutstandingsInvokeArgument(
      { host: "127.0.0.1", port: 9000 },
      [
        { name: "Bridge Validation Lab", guid: "guid-1" },
        { name: "Bridge Ageing Lab", guid: "guid-2" },
      ],
      "2026-08-17",
      "bill_date",
    ),
    {
      request: {
        config: { host: "127.0.0.1", port: 9000 },
        companies: [
          { company: "Bridge Validation Lab", expected_company_guid: "guid-1" },
          { company: "Bridge Ageing Lab", expected_company_guid: "guid-2" },
        ],
        currency_assertion: "INR",
        as_of_yyyymmdd: "20260817",
        ageing_anchor: "bill_date",
      },
    },
  );
});

test("Excel and PDF statement builders emit the report's actual as-of date", () => {
  const result = {
    report: { company_name: "Bridge Validation Lab", as_of_yyyymmdd: "20260801" },
    ageing_anchor: "bill_date",
    statement_open_bills: [{ party: "Alpha", amount: "1" }],
    statement_unallocated_by_party: [{ party: "Alpha", amount: "2" }],
  };

  assert.deepEqual(
    partyStatementInvokeArgument(result, "Alpha", "xlsx"),
    {
      request: {
        company: "Bridge Validation Lab",
        as_of_yyyymmdd: "20260801",
        party: "Alpha",
        format: "xlsx",
        ageing_anchor: "bill_date",
        open_bills: [{ party: "Alpha", amount: "1" }],
        unallocated_by_party: [{ party: "Alpha", amount: "2" }],
      },
    },
  );
  assert.deepEqual(
    bulkPartyStatementsInvokeArgument(result, "/tmp/statements", "synthetic-approval", "pdf").request,
    {
      company: "Bridge Validation Lab",
      as_of_yyyymmdd: "20260801",
      destination: "/tmp/statements",
      approval_id: "synthetic-approval",
      format: "pdf",
      ageing_anchor: "bill_date",
      open_bills: [{ party: "Alpha", amount: "1" }],
      unallocated_by_party: [{ party: "Alpha", amount: "2" }],
    },
  );
});

test("working-paper export sends only the opaque Rust-owned read binding", () => {
  assert.deepEqual(
    workingPaperInvokeArgument({ working_paper_export_id: "synthetic-export-id" }),
    { request: { export_id: "synthetic-export-id" } },
  );
  assert.equal(workingPaperInvokeArgument({}), null);
  assert.equal(workingPaperInvokeArgument({ working_paper_export_id: "" }), null);
});
