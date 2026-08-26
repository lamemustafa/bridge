// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { canStartOutstandingsRead } from "../src/outstandings-currency.ts";

const company = "synthetic-company-identity";

test("an outstandings read cannot start before an explicit INR assertion", () => {
  assert.equal(canStartOutstandingsRead(company, null), false);
  assert.equal(canStartOutstandingsRead(null, company), false);
  assert.equal(canStartOutstandingsRead(company, company), true);
});

test("an INR assertion cannot authorize a different selected company", () => {
  assert.equal(
    canStartOutstandingsRead("different-company-identity", company),
    false,
  );
});
