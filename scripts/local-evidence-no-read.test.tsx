// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom

import React from "react";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { OutstandingsScreen } from "../src/OutstandingsScreen";
import { isLocalEvidenceReadSuppressed } from "../src/evidence-read-boundary";

const firstSavedCompany = {
  name: "Synthetic saved company one",
  guid: "11111111-1111-1111-1111-111111111111",
  company_number: "100001",
  books_from_yyyymmdd: "20250401",
  canonical_origin: "http://127.0.0.1:9000",
} satisfies NonNullable<React.ComponentProps<typeof OutstandingsScreen>["company"]>;

const secondSavedCompany = {
  ...firstSavedCompany,
  name: "Synthetic saved company two",
  guid: "22222222-2222-2222-2222-222222222222",
  company_number: "100002",
};

function LocalEvidenceHarness() {
  const [company, setCompany] = React.useState(firstSavedCompany);
  const [evidenceDrawerOpen, setEvidenceDrawerOpen] = React.useState(false);
  const [evidenceDrawerEntry] = React.useState({ kind: "local-only" } as const);
  return (
    <>
      <button type="button" onClick={() => setEvidenceDrawerOpen(true)}>Open local evidence</button>
      <button type="button" onClick={() => setCompany(secondSavedCompany)}>Change saved company</button>
      <OutstandingsScreen
        config={{ host: "127.0.0.1", port: 9000 }}
        company={company}
        onChangeSetup={() => {}}
        onOpenEvidence={() => {}}
        liveReadNavigationLocked={false}
        liveReadSuppressed={isLocalEvidenceReadSuppressed(evidenceDrawerOpen, evidenceDrawerEntry)}
        asOf="20260731"
        onAsOfChange={() => {}}
        onTallyReadActivityChange={() => {}}
        onExportNoticeChange={() => {}}
      />
    </>
  );
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("opening local evidence then selecting a saved company issues no live Tally invoke", async () => {
  mocks.invoke.mockResolvedValue({
    is_inr: false,
    mailing_name: "Synthetic saved company one",
    currency_count: 1,
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);

  await act(async () => {
    root.render(<LocalEvidenceHarness />);
  });
  mocks.invoke.mockClear();
  await act(async () => {
    host.querySelector<HTMLButtonElement>("button")?.click();
  });
  expect(mocks.invoke).not.toHaveBeenCalled();
  await act(async () => {
    host.querySelectorAll<HTMLButtonElement>("button")[1]?.click();
  });

  expect(mocks.invoke).not.toHaveBeenCalled();
  root.unmount();
});
