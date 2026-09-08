// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { formatAmount, TrialBalanceScreen } from "../src/TrialBalanceScreen";

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
      rows: [{ name: "Cash", guid: "cash", opening: { state: "present", value: "1234567.89" }, debit: { state: "present", value: "-1.25" }, credit: { state: "present", value: "100.00" }, closing: { state: "present", value: "-7277.00" } }],
    },
    totals: { opening: { sum: "1234567.89", empty_count: 0 }, debit: { sum: "-1.25", empty_count: 0 }, credit: { sum: "100.00", empty_count: 0 }, closing: { sum: "1234467.80", empty_count: 0 } },
    read_at: "2026-09-08T10:00:00Z",
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

test("preserves extra fractional precision when currency decimals are zero", () => {
  expect(formatAmount({ state: "present", value: "1.25" }, "¤", 0)).toBe("¤1.25");
});

test("renders exact amounts and exports the captured report without another Tally read", async () => {
  mocks.invoke.mockResolvedValueOnce(report).mockResolvedValueOnce("/tmp/trial-balance.xlsx");
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("₹12,34,567.89 Cr");
  expect(host.textContent).toContain("₹1.25");
  expect(host.textContent).toContain("₹7,277.00 Dr");
  expect(host.textContent).not.toContain("−₹1.25");
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
  expect(host.querySelector(".trial-balance-report")).toBeNull();
  expect(button(host, "Excel").disabled).toBe(true);
  root.unmount();
});

test("paginates captured rows locally without rereading or changing totals", async () => {
  const manyRows = Array.from({ length: 101 }, (_, index) => ({
    name: `Ledger ${index + 1}`,
    guid: `ledger-${index + 1}`,
    opening: { state: "present_empty" as const },
    debit: { state: "present_empty" as const },
    credit: { state: "present_empty" as const },
    closing: { state: "present_empty" as const },
  }));
  mocks.invoke.mockResolvedValueOnce({ ...report, read: { ...report.read, report: { rows: manyRows } } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("Rows 1–100 of 101");
  expect(host.textContent).toContain("Difference in opening balances₹12,34,567.89");
  await act(async () => button(host, "Next").click());
  expect(host.textContent).toContain("Rows 101–101 of 101");
  expect(host.textContent).toContain("Difference in opening balances₹12,34,567.89");
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  root.unmount();
});
