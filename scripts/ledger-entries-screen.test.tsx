// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom

import React from "react";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { LedgerEntriesScreen } from "../src/LedgerEntriesScreen";

const company = {
  name: "Synthetic company",
  guid: "11111111-1111-1111-1111-111111111111",
  company_number: "100001",
  books_from_yyyymmdd: "20250401",
  canonical_origin: "http://127.0.0.1:9000",
};

function enter(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("ledger investigation has no automatic read and only invokes after Show entries", async () => {
  mocks.invoke.mockResolvedValue({ company: {}, read_at: "2026-09-08T00:00:00Z", evidence: { state: "complete", bytes: 1 }, truncated: false, result: { state: "complete", items: [], offset: 0, total: 0, profile: "agent_vouchers_v1_filters" } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<LedgerEntriesScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} locked={false} onReadActivity={() => {}} />);
  });
  expect(mocks.invoke).not.toHaveBeenCalled();

  const inputs = host.querySelectorAll<HTMLInputElement>("input");
  await act(async () => {
    enter(inputs[0], "Cash");
    enter(inputs[1], "2026-04-01");
    enter(inputs[2], "2026-04-30");
  });
  expect(mocks.invoke).not.toHaveBeenCalled();
  await act(async () => {
    host.querySelector<HTMLFormElement>("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(mocks.invoke).toHaveBeenCalledWith("fetch_selected_ledger_entries", expect.objectContaining({
    request: expect.objectContaining({ ledger: "Cash", from: "20260401", to: "20260430", offset: 0, limit: 100 }),
  }));
  expect(host.textContent).toContain("No vouchers in the complete source matched this ledger.");
  expect(host.textContent).toContain("Observed");
  expect(host.textContent).toContain("requested ledger Cash, 2026-04-01 to 2026-04-30.");
  root.unmount();
});

test("changing the selected period or company clears a stale result without another read", async () => {
  mocks.invoke.mockResolvedValue({ company: {}, read_at: "2026-09-08T00:00:00Z", evidence: { state: "complete", bytes: 1 }, truncated: false, result: { state: "complete", items: [], offset: 0, total: 0, profile: "agent_vouchers_v1_filters" } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<LedgerEntriesScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} locked={false} onReadActivity={() => {}} />);
  });
  const inputs = host.querySelectorAll<HTMLInputElement>("input");
  await act(async () => {
    enter(inputs[0], "Cash"); enter(inputs[1], "2026-04-01"); enter(inputs[2], "2026-04-30");
    host.querySelector<HTMLFormElement>("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(host.textContent).toContain("No vouchers in the complete source matched this ledger.");
  mocks.invoke.mockClear();
  await act(async () => { enter(inputs[2], "2026-05-01"); });
  expect(host.textContent).not.toContain("No vouchers in the complete source matched this ledger.");
  expect(mocks.invoke).not.toHaveBeenCalled();
  root.unmount();
});

test("a partial source never presents zero matches as a complete result", async () => {
  mocks.invoke.mockResolvedValue({ company: {}, read_at: "2026-09-08T00:00:00Z", evidence: { state: "partial", reason_code: "window_contradicted", bytes: 1 }, truncated: false, result: { state: "partial", items: [], offset: 0, total: 0, profile: "agent_vouchers_v1_filters" } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => { root.render(<LedgerEntriesScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} locked={false} onReadActivity={() => {}} />); });
  const inputs = host.querySelectorAll<HTMLInputElement>("input");
  await act(async () => {
    enter(inputs[0], "Cash"); enter(inputs[1], "2026-04-01"); enter(inputs[2], "2026-04-30");
    host.querySelector<HTMLFormElement>("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(host.textContent).toContain("could not establish a complete source");
  expect(host.textContent).not.toContain("No vouchers in the complete source matched this ledger.");
  root.unmount();
});


test("scope turnover clears the pending indicator before the obsolete read settles", async () => {
  let resolveRead: ((value: unknown) => void) | undefined;
  mocks.invoke.mockImplementation(() => new Promise((resolve) => { resolveRead = resolve; }));
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => { root.render(<LedgerEntriesScreen config={{ host: "127.0.0.1", port: 9000 }} company={company} locked={false} onReadActivity={() => {}} />); });
  const inputs = host.querySelectorAll<HTMLInputElement>("input");
  await act(async () => {
    enter(inputs[0], "Cash"); enter(inputs[1], "2026-04-01"); enter(inputs[2], "2026-04-30");
    host.querySelector<HTMLFormElement>("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(host.textContent).toContain("Reading entries…");
  await act(async () => { enter(inputs[2], "2026-05-01"); });
  expect(host.textContent).toContain("Show entries");
  expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled).toBe(false);
  await act(async () => { resolveRead?.({ company: {}, read_at: "2026-09-08T00:00:00Z", evidence: { state: "complete", bytes: 1 }, truncated: false, result: { state: "complete", items: [], offset: 0, total: 0, profile: "agent_vouchers_v1_filters" } }); });
  expect(host.textContent).not.toContain("No vouchers in the complete source matched this ledger.");
  root.unmount();
});
