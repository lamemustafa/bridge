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
