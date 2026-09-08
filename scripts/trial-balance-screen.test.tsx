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
    totals: { opening: { sum: "-1234567.89", empty_count: 0 }, debit: { sum: "-1.25", empty_count: 0 }, credit: { sum: "100.00", empty_count: 0 }, closing: { sum: "1234467.80", empty_count: 0 } },
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

async function chooseEndDate(host: HTMLElement) {
  const input = host.querySelectorAll<HTMLInputElement>('input[type="date"]')[1];
  await act(async () => {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    setValue?.call(input, "2026-04-30");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

function setDate(input: HTMLInputElement, value: string) {
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setValue?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
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
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("₹12,34,567.89 Cr");
  expect(host.textContent).toContain("₹1.25");
  expect(host.textContent).toContain("₹7,277.00 Dr");
  expect(host.textContent).not.toContain("−₹1.25");
  expect(host.textContent).toContain("Difference in opening balances");
  expect(host.textContent).toContain("Closing total");
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
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  const changed = { ...company, guid: "00000000-0000-4000-8000-000000000002", name: "Other Accounts" };
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={changed} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await act(async () => { resolve(report); await pending; });
  expect(host.querySelector(".trial-balance-report")).toBeNull();
  expect(button(host, "Excel").disabled).toBe(true);
  root.unmount();
});

test("clears a stale date refusal when either date changes", async () => {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  const dates = [...host.querySelectorAll<HTMLInputElement>('input[type="date"]')];
  await act(async () => { setDate(dates[1]!, "2026-03-31"); });
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("Choose a valid date range");
  await act(async () => { setDate(dates[0]!, "2026-03-01"); });
  expect(host.textContent).not.toContain("Choose a valid date range");
  await act(async () => { setDate(dates[0]!, "2026-04-01"); button(host, "Refresh report").click(); });
  expect(host.textContent).toContain("Choose a valid date range");
  await act(async () => { setDate(dates[1]!, "2026-04-01"); });
  expect(host.textContent).not.toContain("Choose a valid date range");
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
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("Rows 1–100 of 101");
  expect(host.textContent).toContain("Difference in opening balances₹12,34,567.89 Dr");
  expect(host.textContent).toContain("Closing total₹12,34,467.80 Cr");
  await act(async () => button(host, "Next").click());
  expect(host.textContent).toContain("Rows 101–101 of 101");
  expect(host.textContent).toContain("Difference in opening balances₹12,34,567.89");
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  root.unmount();
});

test("preserves book start and leaves end date for the operator without mode evidence", async () => {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  const dates = [...host.querySelectorAll<HTMLInputElement>('input[type="date"]')];
  expect(dates[0]?.value).toBe("2026-04-01");
  expect(dates[1]?.value).toBe("");
  expect(host.textContent).toContain("Choose the end date before reading");
  expect(host.textContent).toContain("Preview: validated with small synthetic companies");
  root.unmount();
});

test("locks date controls and activity navigation while Excel export saves", async () => {
  let resolveExport!: (value: string) => void;
  const exportPending = new Promise<string>((resolve) => { resolveExport = resolve; });
  const activity: number[] = [];
  mocks.invoke.mockResolvedValueOnce(report).mockReturnValueOnce(exportPending);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={(delta) => activity.push(delta)} />));
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  await act(async () => button(host, "Excel").click());
  expect(host.querySelector<HTMLInputElement>('input[type="date"]')?.disabled).toBe(true);
  expect(activity).toEqual([1, -1, 1]);
  await act(async () => button(host, "Refresh report").click());
  expect(mocks.invoke).toHaveBeenCalledTimes(2);
  resolveExport("/tmp/trial-balance.xlsx");
  await act(async () => { await exportPending; });
  expect(activity).toEqual([1, -1, 1, -1]);
  expect(host.querySelector<HTMLInputElement>('input[type="date"]')?.disabled).toBe(false);
  root.unmount();
});

test("unlocks the screen when Excel export fails and reports the error", async () => {
  mocks.invoke.mockResolvedValueOnce(report).mockRejectedValueOnce(new Error("save failed"));
  const activity: number[] = [];
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={(delta) => activity.push(delta)} />));
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  await act(async () => button(host, "Excel").click());
  expect(host.querySelector<HTMLInputElement>('input[type="date"]')?.disabled).toBe(false);
  expect(host.textContent).toContain("save failed");
  expect(activity).toEqual([1, -1, 1, -1]);
  root.unmount();
});

test("shows the observed closing empty count", async () => {
  const incomplete = { ...report, read: { ...report.read, totals: { ...report.read.totals, closing: { sum: "0", empty_count: 2 } } } };
  mocks.invoke.mockResolvedValueOnce(incomplete);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<TrialBalanceScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} liveReadNavigationLocked={false} liveReadSuppressed={false} onChangeSetup={() => {}} onTallyReadActivityChange={() => {}} />));
  await chooseEndDate(host);
  await act(async () => button(host, "Refresh report").click());
  expect(host.textContent).toContain("Closing total");
  expect(host.textContent).toContain("2 empty source values");
  root.unmount();
});
