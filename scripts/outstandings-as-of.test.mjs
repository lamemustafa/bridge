// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import {
  allCompaniesOutstandingsInvokeArgument,
  asOfYyyymmdd,
  bulkPartyStatementsInvokeArgument,
  partyStatementInvokeArgument,
  singleCompanyOutstandingsInvokeArgument,
  todayAsDateInput,
} from "../src/outstandings-as-of.ts";

test("the visible as-of control defaults to the operator's local calendar date", () => {
  assert.equal(todayAsDateInput(new Date(2026, 7, 22, 0, 1)), "2026-08-22");
  assert.equal(asOfYyyymmdd("2026-08-22"), "20260822");
  assert.equal(asOfYyyymmdd("2026-8-22"), null);
});

test("the single-company request emits the selected canonical as-of date", () => {
  assert.deepEqual(
    singleCompanyOutstandingsInvokeArgument(
      { host: "127.0.0.1", port: 9000 },
      { name: "Bridge Validation Lab", guid: "guid-1" },
      "2026-08-17",
    ),
    {
      request: {
        config: { host: "127.0.0.1", port: 9000 },
        company: "Bridge Validation Lab",
        expected_company_guid: "guid-1",
        currency_assertion: "INR",
        as_of_yyyymmdd: "20260817",
      },
    },
  );
  assert.equal(singleCompanyOutstandingsInvokeArgument({ host: "127.0.0.1", port: 9000 }, { name: "Lab", guid: "guid-1" }, "2026-8-17"), null);
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
      },
    },
  );
});

test("Excel and PDF statement builders emit the report's actual as-of date", () => {
  const result = {
    report: { company_name: "Bridge Validation Lab", as_of_yyyymmdd: "20260801" },
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
        open_bills: [{ party: "Alpha", amount: "1" }],
        unallocated_by_party: [{ party: "Alpha", amount: "2" }],
      },
    },
  );
  assert.deepEqual(
    bulkPartyStatementsInvokeArgument(result, "/tmp/statements", "pdf").request,
    {
      company: "Bridge Validation Lab",
      as_of_yyyymmdd: "20260801",
      destination: "/tmp/statements",
      format: "pdf",
      open_bills: [{ party: "Alpha", amount: "1" }],
      unallocated_by_party: [{ party: "Alpha", amount: "2" }],
    },
  );
});
