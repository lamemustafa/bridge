// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { isNonRetryableOutstandingsBoundary, outstandingsAgeingDisclosure, outstandingsPartialReason, outstandingsPartialState } from "../src/outstandings-copy.ts";

test("uncalibrated sizing says the voucher read was not sent", () => {
  const message = outstandingsPartialReason("outstandings_segment_sizing_uncalibrated");
  assert.match(message, /no voucher read was sent/i);
  assert.doesNotMatch(message, /empty|adjacent/i);
});

test("an unavailable production reader does not invite a pointless refresh", () => {
  const state = outstandingsPartialState("outstandings_segment_sizing_uncalibrated");
  assert.equal(state.retryable, false);
  assert.match(state.title, /aren’t available yet/i);
  assert.match(state.message, /didn’t read anything from tally/i);
  assert.match(state.message, /changing tally settings won’t resolve this/i);
  assert.equal(isNonRetryableOutstandingsBoundary("outstandings_segment_sizing_uncalibrated"), true);
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

test("opening-bill coverage is a non-retryable Unit A boundary", () => {
  const state = outstandingsPartialState("ledger_opening_bills_not_covered");

  assert.equal(state.retryable, false);
  assert.match(state.message, /current read scope does not cover bill-wise opening balances/i);
  assert.match(state.message, /repeating the same scan won't resolve this/i);
  assert.equal(isNonRetryableOutstandingsBoundary("ledger_opening_bills_not_covered"), true);
});

test("unqualified direct postings withhold totals before a voucher read", () => {
  const message = outstandingsPartialReason("unallocated_direct_postings_not_covered");
  assert.match(message, /posted without a bill reference/i);
  assert.match(message, /totals stay withheld/i);
  assert.match(message, /before any voucher read/i);
});

test("unqualified direct postings do not invite a pointless retry", () => {
  assert.equal(isNonRetryableOutstandingsBoundary("unallocated_direct_postings_not_covered"), true);
  assert.equal(isNonRetryableOutstandingsBoundary("outstandings_segment_plan_exceeds_budget"), false);
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
