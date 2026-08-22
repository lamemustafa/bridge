// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { asOfYyyymmdd, todayAsDateInput } from "../src/outstandings-as-of.ts";

test("the visible as-of control defaults to the operator's local calendar date", () => {
  assert.equal(todayAsDateInput(new Date(2026, 7, 22, 0, 1)), "2026-08-22");
  assert.equal(asOfYyyymmdd("2026-08-22"), "20260822");
  assert.equal(asOfYyyymmdd("2026-8-22"), null);
});

test("the selected as-of is sent to Tally and carried into every export", async () => {
  const screen = await readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8");

  assert.match(screen, /type="date"/);
  assert.match(screen, /as_of_yyyymmdd: requestedAsOf/);
  assert.match(screen, /row\(text\("As of"\), text\(formatDate\(report\.as_of_yyyymmdd\)\)\)/);
  assert.equal((screen.match(/as_of_yyyymmdd: result\.report\.as_of_yyyymmdd/g) ?? []).length, 2);
});
