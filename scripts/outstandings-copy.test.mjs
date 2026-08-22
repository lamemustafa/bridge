// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { isNonRetryableOutstandingsBoundary, outstandingsAgeingAnchorLabel, outstandingsAgeingDisclosure, outstandingsPartialReason, outstandingsPartialState } from "../src/outstandings-copy.ts";

test("the backend ageing anchor is disclosed in the bucket label", () => {
  assert.equal(outstandingsAgeingAnchorLabel("due_date"), "aged from due date");
  assert.equal(outstandingsAgeingAnchorLabel("bill_date"), "aged from bill date");
});

test("new native and sweep boundaries have operator-readable reasons", () => {
  assert.equal(
    outstandingsPartialReason(
      "native_outstandings_as_of_refused",
      "20260822",
      "20260731",
    ),
    "Tally refused the requested as-of date (20260822) and returned overdue days as of 20260731, so Bridge withheld the totals",
  );
  assert.match(outstandingsPartialReason("native_overdue_crosscheck_mismatch"), /overdue-day cross-check/i);
  assert.match(
    outstandingsPartialReason("native_outstandings_as_of_unconfirmed_without_bill_references"),
    /no bill references/i,
  );
  assert.match(outstandingsPartialReason("company_currency_probe_failed"), /base currency/i);
  assert.match(outstandingsPartialReason("company_base_currency_not_inr"), /not INR/i);
  assert.match(outstandingsPartialReason("company_outstandings_read_failed"), /company read failed/i);
});

test("an empty bill report with ledger money names the unconfirmed as-of boundary", () => {
  const state = outstandingsPartialState("native_outstandings_as_of_unconfirmed_without_bill_references");

  assert.equal(state.title, "Tally did not confirm this as-of date");
  assert.match(state.message, /ledger still carried a balance/i);
  assert.match(state.message, /could not confirm the requested as-of date/i);
  assert.equal(state.tallyReadAttempted, true);
});

test("a refused period is distinct from a row-level disagreement", () => {
  const refused = outstandingsPartialState(
    "native_outstandings_as_of_refused",
    "20260822",
    "20260731",
  );
  const inconsistent = outstandingsPartialState("native_overdue_crosscheck_mismatch");

  assert.equal(refused.title, "Tally did not accept this as-of date");
  assert.match(refused.message, /requested as-of date \(20260822\)/i);
  assert.match(refused.message, /as of 20260731/i);
  assert.notEqual(refused.message, inconsistent.message);
});

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
  assert.equal(state.tallyReadAttempted, true);
  assert.match(state.message, /completed a coverage check/i);
  assert.match(state.message, /bill-wise opening balances/i);
  assert.match(state.message, /repeating the same scan won't resolve this/i);
  assert.equal(isNonRetryableOutstandingsBoundary("ledger_opening_bills_not_covered"), true);
});

test("pre-admission boundaries do not claim a Tally read", () => {
  assert.equal(outstandingsPartialState("outstandings_segment_sizing_uncalibrated").tallyReadAttempted, false);
  assert.equal(outstandingsPartialState("unallocated_direct_postings_not_covered").tallyReadAttempted, false);
});

test("opening-bill coverage reports a completed check instead of an unread request", async () => {
  const screen = await readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8");

  assert.match(screen, /const tallyReadAttempted = result\?\.state === "partial" && partialState\?\.tallyReadAttempted;/);
  assert.match(screen, /: tallyReadAttempted\s*\? `Checked \$\{relativeTime\(result\.synced_at_unix_ms\)\}`\s*: "No Tally data was read"/s);
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

test("a path that can prove the unallocated balance says so instead of disclaiming it", () => {
  // The voucher scan derives bills from vouchers and genuinely cannot
  // establish the unallocated remainder, so its disclaimer is honest. The
  // native bills path recovers that figure exactly from the party ledgers, so
  // repeating "Bridge does not show an On Account amount" there would be false
  // while a screen right above it displays exactly that amount.
  const known = outstandingsAgeingDisclosure(true, true);
  assert.match(known, /shown as Unallocated above/i);
  assert.doesNotMatch(known, /cannot prove/i);
  assert.doesNotMatch(known, /does not show/i);

  // Absent knowledge must keep the original disclaimer, never silently claim a
  // figure it does not have.
  assert.match(outstandingsAgeingDisclosure(true, false), /cannot prove the full unallocated balance/i);
  assert.equal(outstandingsAgeingDisclosure(false, true), null);
});
