// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { outstandingsAgeingDisclosure, outstandingsPartialReason } from "../src/outstandings-copy.ts";

test("uncalibrated sizing says the voucher read was not sent", () => {
  const message = outstandingsPartialReason("outstandings_segment_sizing_uncalibrated");
  assert.match(message, /no voucher read was sent/i);
  assert.doesNotMatch(message, /empty|adjacent/i);
});

test("an over-budget plan says the voucher scan did not start", () => {
  assert.match(
    outstandingsPartialReason("outstandings_segment_plan_exceeds_budget"),
    /no voucher scan started/i,
  );
});

test("bill-wise opening balances explain why totals stay withheld", () => {
  const message = outstandingsPartialReason("ledger_opening_bills_not_covered");
  assert.match(message, /bill-wise opening balances/i);
  assert.match(message, /totals stay withheld/i);
});

test("deadline states keep the restart-before-next-sync instruction", () => {
  assert.match(
    outstandingsPartialReason("tally_segment_deadline_restart_recommended"),
    /restart before another sync/i,
  );
  assert.match(
    outstandingsPartialReason("tally_segment_latency_trending_restart_recommended"),
    /restart before another sync/i,
  );
});

test("unknown reason codes remain readable", () => {
  assert.equal(outstandingsPartialReason("date_partition_scope_mismatch"), "date partition scope mismatch");
});

test("unaged receivables disclose the ageing scope without inventing an On Account total", () => {
  const disclosure = outstandingsAgeingDisclosure(true);
  assert.match(disclosure, /excluded from these buckets/i);
  assert.match(disclosure, /no bill reference or age/i);
  assert.match(disclosure, /does not show an On Account amount/i);
  assert.match(disclosure, /cannot prove the full unallocated balance/i);
  assert.equal(outstandingsAgeingDisclosure(false), null);
});
