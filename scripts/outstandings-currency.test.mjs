// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { canStartOutstandingsRead } from "../src/outstandings-currency.ts";

const company = { name: "Synthetic Company", guid: "synthetic-guid" };

test("an outstandings read cannot start before an explicit INR assertion", () => {
  assert.equal(canStartOutstandingsRead(company, false), false);
  assert.equal(canStartOutstandingsRead(undefined, true), false);
  assert.equal(canStartOutstandingsRead(company, true), true);
});
