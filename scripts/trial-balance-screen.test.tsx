// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { TrialBalanceScreen } from "../src/TrialBalanceScreen";

const company = {
  name: "Synthetic Accounts",
  guid: "00000000-0000-4000-8000-000000000001",
  company_number: "100001",
  books_from_yyyymmdd: "20260401",
  canonical_origin: "http://127.0.0.1:9000",
};

const report = {
  read: {
    company_guid: company.guid,
    company_name: company.name,
    from: "20260401",
    to: "20260908",
    currency: { symbol: "₹", mailing_name: "Indian Rupee", currency_count: 1, decimal_places: 2, is_inr: true },
    report: {
      rows: [{ name: "Cash", guid: "cash", opening: { state: "present", value: "1234567.8" }, debit: { state: "present_empty" }, credit: { state: "present", value: "100.00" }, closing: { state: "present", value: "1234467.80" } }],
    },
    totals: { opening: { sum: "1234567.8", empty_count: 0 }, debit: { sum: "0", empty_count: 1 }, credit: { sum: "100.00", empty_count: 0 }, closing: { sum: "1234467.80", empty_count: 0 } },
    read_at: Date.parse("2026-09-08T10:00:00Z"),
    evidence: { request_sha256: "a", response_sha256: "b", bytes: 10 },
  },
  export_id: "export-1",
};

function button(host: HTMLElement, text: string) {
  const match = [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.includes(text));
  if (!match) throw new Error(`Missing button: ${text}`);
  return match;
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("renders exact amounts and exports the captured report without another Tally read", async () => {
  mocks.invoke.mockResolvedValueOnce(report).mockResolvedValueOnce("/tmp/trial-balance.xlsx");
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("₹1,234,567.80");
  expect(host.textContent).toContain("—");
  expect(host.textContent).toContain("Difference in opening balances");
  await act(async () => button(host, "Excel").click());
  expect(mocks.invoke).toHaveBeenLastCalledWith("export_tally_trial_balance", { exportId: "export-1" });
  expect(host.textContent).toContain("/tmp/trial-balance.xlsx");
  root.unmount();
});

test("drops a stale response after the report scope changes", async () => {
  let resolve!: (value: unknown) => void;
  const pending = new Promise((done) => { resolve = done; });
  mocks.invoke.mockReturnValueOnce(pending);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => button(host, "Refresh report").click());
  const changed = { ...company, guid: "00000000-0000-4000-8000-000000000002", name: "Other Accounts" };
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={changed} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => { resolve(report); await pending; });
  expect(host.textContent).not.toContain("₹1,234,567.80");
  root.unmount();
});
