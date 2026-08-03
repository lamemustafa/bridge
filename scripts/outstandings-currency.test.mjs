// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { canStartOutstandingsRead } from "../src/outstandings-currency.ts";

const company = { name: "Synthetic Company", guid: "synthetic-guid" };

test("an outstandings read cannot start before an explicit INR assertion", () => {
  assert.equal(canStartOutstandingsRead(company, null), false);
  assert.equal(canStartOutstandingsRead(undefined, "synthetic-guid"), false);
  assert.equal(canStartOutstandingsRead(company, "synthetic-guid"), true);
});

test("an INR assertion cannot authorize a different selected company", () => {
  assert.equal(
    canStartOutstandingsRead({ name: "Different Company", guid: "different-guid" }, "synthetic-guid"),
    false,
  );
});
