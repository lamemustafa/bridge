// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { groupOpenBillsByParty } from "../src/outstandings-bills.ts";

function bill(party, reference, amount = "100") {
  return {
    party,
    reference,
    bill_date: "20260101",
    due_date: "20260101",
    amount,
    age_days: 10,
    kind: "receivable",
  };
}

test("statement rows absent entirely (voucher-scan path) grouped as null, not an empty map", () => {
  assert.equal(groupOpenBillsByParty(undefined, 2_000), null);
});

// This is the failing-before-fix case: a party genuinely owing money, whose
// bill rows are simply too numerous to render by default, must never render
// the same as a party with zero bills.
test("a party outside its own display cap is represented as not-loaded, distinct from zero bills", () => {
  const bills = Array.from({ length: 2_500 }, (_, index) => bill("Large Debtor Pvt Ltd", `INV-${index}`));
  const grouped = groupOpenBillsByParty(bills, 2_000);
  const state = grouped.get("Large Debtor Pvt Ltd");
  assert.ok(state, "a party with real bill rows must be present in the map");
  assert.equal(state.status, "not_loaded");
  assert.equal(state.shown.length, 2_000);
  assert.equal(state.bills.length, 2_500);
  // The complete rows are already in memory -- nothing here is actually
  // missing, only unrendered by default.
  assert.deepEqual(state.bills.slice(0, 2_000), state.shown);
});

test("a party genuinely with zero bills is absent from the map, not present-and-empty", () => {
  const bills = [bill("Other Party", "INV-1")];
  const grouped = groupOpenBillsByParty(bills, 2_000);
  assert.equal(grouped.get("Zero Bill Party"), undefined);
  // Distinguishing "absent" from "present but empty" is the point: an absent
  // entry is what the rendering path reads as "no bill references" -- a real
  // zero, not an unloaded one.
});

test("a party within its own cap is fully loaded, with no shown/bills split", () => {
  const bills = [bill("Small Debtor", "INV-1"), bill("Small Debtor", "INV-2")];
  const grouped = groupOpenBillsByParty(bills, 2_000);
  const state = grouped.get("Small Debtor");
  assert.equal(state.status, "loaded");
  assert.equal(state.bills.length, 2);
});

// The bug this module fixes: capping the flattened, cross-party list before
// grouping could push an entirely different, small party's bills past the
// cutoff -- collapsing its real exposure into the same "no bills" rendering
// as a true zero. Capping per party after grouping means a party's own bill
// count is the only thing that can ever cap it.
test("one party's large bill count never displaces another party's bills from the map", () => {
  const bills = [
    ...Array.from({ length: 2_400 }, (_, index) => bill("Alphabetically First Ltd", `INV-${index}`)),
    bill("Zzz Small Party", "ONLY-BILL"),
  ];
  const grouped = groupOpenBillsByParty(bills, 2_000);
  const small = grouped.get("Zzz Small Party");
  assert.ok(small, "a small party placed after a large one in source order must still be represented");
  assert.equal(small.status, "loaded");
  assert.equal(small.bills.length, 1);
});

test("grouping drops no bill: every row is reachable through some party's state", () => {
  const bills = [
    ...Array.from({ length: 2_200 }, (_, index) => bill("Big Party", `INV-${index}`)),
    bill("Small Party", "INV-A"),
    bill("Small Party", "INV-B"),
  ];
  const grouped = groupOpenBillsByParty(bills, 2_000);
  let total = 0;
  for (const state of grouped.values()) {
    total += state.bills.length;
  }
  assert.equal(total, bills.length);
});
