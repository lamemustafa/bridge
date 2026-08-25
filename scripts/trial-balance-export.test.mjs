import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { trialBalanceExportMessage, trialBalanceInvokeArgument } from "../src/trial-balance-export.ts";

test("trial balance export stays GUID and selected-date bound without inventing a currency", () => {
  assert.deepEqual(
    trialBalanceInvokeArgument(
      { host: "127.0.0.1", port: 9001 },
      { name: "Synthetic Books", guid: "synthetic-guid" },
      "20260814",
    ),
    {
      request: {
        config: { host: "127.0.0.1", port: 9001 },
        company: "Synthetic Books",
        expected_company_guid: "synthetic-guid",
        as_of_yyyymmdd: "20260814",
      },
    },
  );
  assert.equal(
    trialBalanceInvokeArgument(
      { host: "127.0.0.1", port: 9001 },
      { name: "Synthetic Books", guid: "synthetic-guid" },
      null,
    ),
    null,
  );
});

test("completed export copy names its book-to-date scope", () => {
  assert.equal(
    trialBalanceExportMessage({
      path: "/synthetic/trial-balance.xlsx",
      company: "Synthetic Books",
      from_yyyymmdd: "20250401",
      to_yyyymmdd: "20260814",
      ledger_count: 24,
      opening_difference: "0",
    }),
    "Book-to-date Trial Balance · 24 ledgers · 01-04-2025 to 14-08-2026",
  );
  assert.match(
    trialBalanceExportMessage({
      path: "/synthetic/trial-balance.xlsx",
      company: "Synthetic Books",
      from_yyyymmdd: "20250401",
      to_yyyymmdd: "20260814",
      ledger_count: 24,
      opening_difference: "425000.00",
    }),
    /includes opening difference 425000\.00 Cr/,
  );
});

test("screen and Tauri registration expose the Rust-owned export", async () => {
  const [screen, commands] = await Promise.all([
    readFile(new URL("../src/OutstandingsScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8"),
  ]);
  assert.match(screen, /invoke<TrialBalanceExportSummary>\("export_tally_trial_balance", argument\)/);
  assert.match(screen, /Building Trial Balance…/);
  assert.match(commands, /commands::export_tally_trial_balance/);
});
