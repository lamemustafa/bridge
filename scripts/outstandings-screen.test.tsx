// @vitest-environment jsdom
// SPDX-License-Identifier: Apache-2.0

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { OutstandingsScreen } from "../src/OutstandingsScreen";

const config = { host: "127.0.0.1", port: 9000 };
const company = {
  name: "Synthetic Accounts",
  guid: "00000000-0000-0000-0000-000000000000",
  company_number: "100001",
  books_from_yyyymmdd: "20260401",
  canonical_origin: "http://127.0.0.1:9000",
};

function defaultProps(): React.ComponentProps<typeof OutstandingsScreen> {
  return {
    config,
    company,
    onChangeSetup: () => {},
    onOpenEvidence: () => {},
    liveReadNavigationLocked: false,
    liveReadSuppressed: false,
    asOf: "2026-09-08",
    onAsOfChange: () => {},
    onTallyReadActivityChange: () => {},
    onExportNoticeChange: () => {},
  };
}

/// The dashboard only issues `fetch_tally_outstandings` once Tally's own
/// base-currency read (`detect_tally_base_currency`) has settled to INR --
/// see the effect chain in OutstandingsScreen.tsx. Draining several
/// microtask turns inside one `act` lets that resolve, the resulting
/// re-render commit, and the dependent read effect fire, all before the
/// test makes an assertion.
async function flush(turns = 8) {
  await act(async () => {
    for (let i = 0; i < turns; i += 1) await Promise.resolve();
  });
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

// bridge#471: OutstandingsScreen's `operatorMessage` used to discard the
// backend's `remediation` field. This pins the shared formatter's behaviour
// on the screen the issue names as its second acceptance case.
test("a refused outstandings read shows the backend's remediation and code", async () => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "detect_tally_base_currency") {
      return Promise.resolve({ is_inr: true, mailing_name: "Indian Rupee", currency_count: 1 });
    }
    if (command === "fetch_tally_outstandings") {
      return Promise.reject({
        code: "endpoint_unreachable",
        category: "Endpoint configuration",
        message: "The local Tally endpoint could not complete the read-only request.",
        retry: "after_change",
        local_state_changed: false,
        tally_state_may_have_changed: true,
        remediation: "Confirm Tally is running with the XML server enabled, then probe the loopback endpoint again.",
      });
    }
    return Promise.resolve(null);
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<OutstandingsScreen {...defaultProps()} />));
  await flush();

  const alert = host.querySelector('[role="alert"]');
  expect(alert?.textContent).toContain("The local Tally endpoint could not complete the read-only request.");
  expect(alert?.textContent).toContain("[endpoint_unreachable]");
  expect(alert?.textContent).toContain("Confirm Tally is running with the XML server enabled, then probe the loopback endpoint again.");
  root.unmount();
});

// The issue records that some outstandings commands return a plain `String`
// rather than the structured envelope -- there is no `remediation` to show,
// and the formatter must not invent one or mangle the string.
test("a plain-string outstandings failure renders unchanged, with no invented remediation", async () => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "detect_tally_base_currency") {
      return Promise.resolve({ is_inr: true, mailing_name: "Indian Rupee", currency_count: 1 });
    }
    if (command === "fetch_tally_outstandings") {
      return Promise.reject("Bridge could not save the working-paper export destination.");
    }
    return Promise.resolve(null);
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<OutstandingsScreen {...defaultProps()} />));
  await flush();

  const alert = host.querySelector('[role="alert"]');
  expect(alert?.textContent).toContain("Bridge could not save the working-paper export destination.");
  expect(alert?.textContent).not.toContain("undefined");
  expect(alert?.textContent).not.toMatch(/\[.*\]/);
  root.unmount();
});

// bridge#604: only a book with one Currency master may be confirmed as INR by
// hand. With several, or when the currency read fails, the screen offers no
// confirmation and never asks for outstandings; with one master Tally does not
// name INR, the confirmation says what Tally reported.
async function renderWithCurrency(detect: () => Promise<unknown>) {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "detect_tally_base_currency") return detect();
    return Promise.resolve(null);
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<OutstandingsScreen {...defaultProps()} />));
  await flush();
  const invoked = mocks.invoke.mock.calls.map(([command]) => command);
  return { host, root, invoked };
}

test("a book with several currencies is not offered an INR confirmation and is not read", async () => {
  const { host, root, invoked } = await renderWithCurrency(() =>
    Promise.resolve({ is_inr: false, symbol: "", mailing_name: "", currency_count: 2 }),
  );
  expect(host.textContent).toContain("Multi-currency books are not supported yet");
  expect(host.querySelector("button")?.textContent ?? "").not.toContain("This company uses INR");
  expect(host.textContent).not.toContain("This company uses INR");
  expect(invoked).not.toContain("fetch_tally_outstandings");
  root.unmount();
});

test("a failed currency read is not offered an INR confirmation and is not read", async () => {
  const { host, root, invoked } = await renderWithCurrency(() => Promise.reject({ code: "endpoint_unreachable" }));
  expect(host.textContent).toContain("could not read this company");
  expect(host.textContent).not.toContain("This company uses INR");
  expect(invoked).not.toContain("fetch_tally_outstandings");
  root.unmount();
});

test("one currency Tally does not name INR is confirmed against what Tally reported", async () => {
  const { host, root, invoked } = await renderWithCurrency(() =>
    Promise.resolve({ is_inr: false, symbol: "$", mailing_name: "US Dollars", currency_count: 1 }),
  );
  expect(host.textContent).toContain("Tally reports this company’s currency as US Dollars ($).");
  expect(host.textContent).toContain("Confirm only if this company’s books are in Indian rupees.");
  const confirm = [...host.querySelectorAll("button")].find((button) => button.textContent === "This company uses INR");
  expect(confirm).toBeDefined();
  expect(invoked).not.toContain("fetch_tally_outstandings");
  root.unmount();
});

test("a currency read that names no master is not offered an INR confirmation", async () => {
  const { host, root, invoked } = await renderWithCurrency(() =>
    Promise.resolve({ is_inr: false, symbol: "", mailing_name: "", currency_count: 0 }),
  );
  expect(host.textContent).toContain("could not read this company");
  expect(host.textContent).not.toContain("This company uses INR");
  expect(invoked).not.toContain("fetch_tally_outstandings");
  root.unmount();
});

test("confirming one currency Tally does not name INR reads outstandings under the INR assertion", async () => {
  const { host, root } = await renderWithCurrency(() =>
    Promise.resolve({ is_inr: false, symbol: "Rs.", mailing_name: "", currency_count: 1 }),
  );
  // An empty mailing name shows the master's name alone.
  expect(host.textContent).toContain("Tally reports this company’s currency as Rs..");
  const confirm = [...host.querySelectorAll("button")].find((button) => button.textContent === "This company uses INR");
  await act(async () => confirm?.click());
  await flush();
  const fetches = mocks.invoke.mock.calls.filter(([command]) => command === "fetch_tally_outstandings");
  expect(fetches).toHaveLength(1);
  expect(fetches[0][1]).toMatchObject({ request: { currency_assertion: "INR" } });
  root.unmount();
});

// bridge#551: party statements come only from the source Bridge holds for the
// completed read. Without its handle the screen offers no statement control;
// with one, the batch controls appear.
function completeResult(partyStatementSourceId?: string) {
  return {
    state: "complete",
    report: {
      company_name: "Synthetic Accounts",
      as_of_yyyymmdd: "20260908",
      receivable_total: "100.00",
      payable_total: "0",
      has_unaged_receivable: false,
      ageing: { days_0_30: "100.00", days_31_60: "0", days_61_90: "0", days_90_plus: "0" },
      open_receivable_bill_count: 1,
      ageing_bill_counts: { days_0_30: 1, days_31_60: 0, days_61_90: 0, days_90_plus: 0 },
      top_parties: [],
      source_voucher_count: 0,
      source_bytes: 1,
    },
    read_strategy: "native_bills",
    currency_assertion: "INR",
    ageing_anchor: "due_date",
    synced_at_unix_ms: 1,
    unallocated_total: "0",
    statement_open_bills: [{
      party: "Synthetic Party",
      reference: "SYNTHETIC-1",
      bill_date: "20260901",
      due_date: "20260901",
      amount: "100.00",
      age_days: 7,
      kind: "receivable",
    }],
    ...(partyStatementSourceId ? { party_statement_source_id: partyStatementSourceId } : {}),
  };
}

test("statement controls appear only with the source Bridge holds", async () => {
  for (const [sourceId, offered] of [["synthetic-statements", true], [undefined, false]] as const) {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "detect_tally_base_currency") {
        return Promise.resolve({ is_inr: true, mailing_name: "Indian Rupee", currency_count: 1 });
      }
      if (command === "fetch_tally_outstandings") return Promise.resolve(completeResult(sourceId));
      return Promise.resolve(null);
    });
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(<OutstandingsScreen {...defaultProps()} />));
    await flush();
    const labels = [...host.querySelectorAll("button")].map((button) => button.textContent ?? "");
    expect(labels.some((label) => label.includes("All Excel statements")), String(sourceId)).toBe(offered);
    expect(labels.some((label) => label.includes("All PDF statements")), String(sourceId)).toBe(offered);
    root.unmount();
    host.remove();
    mocks.invoke.mockReset();
  }
});
