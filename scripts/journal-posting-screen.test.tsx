// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { JournalPostingScreen } from "../src/JournalPostingScreen";

const review = {
  batchId: "bridge-00000000-0000-4000-8000-000000000001",
  sha256: "a".repeat(64),
  company: {
    name: "Synthetic Accounts",
    guid: "00000000-0000-4000-8000-000000000002",
    companyNumber: "100001",
    booksFrom: "20260401",
  },
  builtAt: "2026-09-07T00:00:00Z",
  dispatched: false,
  responseRecorded: false,
  details: {
    date: "20260901",
    reference: "REF-1",
    narration: "Synthetic test only",
    entries: [
      { ledger: "Expense", side: "Dr" as const, amount: "12.50" },
      { ledger: "Cash", side: "Cr" as const, amount: "12.50" },
    ],
    totalDebit: "12.50",
    totalCredit: "12.50",
  },
};

const config = { host: "127.0.0.1", port: 9001 };

function button(host: HTMLElement, label: string) {
  const match = [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.includes(label));
  if (!match) throw new Error(`Missing button: ${label}`);
  return match;
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("renders the review flow and keeps safe recovery actions after each backend result", async () => {
  mocks.invoke.mockResolvedValueOnce(review);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);

  await act(async () => {
    root.render(<JournalPostingScreen config={config} />);
  });
  await act(async () => {
    button(host, "Choose Journal file").click();
  });
  expect(host.textContent).toContain("Synthetic Accounts");
  expect(host.textContent).toContain("2026-09-01");
  expect(host.textContent).toContain("Synthetic test only");
  expect(host.textContent).toContain("Expense");
  expect(host.textContent).toContain("Total debit");
  expect(host.textContent).toContain("Total credit");

  mocks.invoke.mockResolvedValueOnce({
    batchId: review.batchId,
    result: {
      result: {
        dispatch: { state: "not_dispatched" },
        attempt_recorded: false,
        error: { message: "No posting attempt was recorded." },
      },
    },
  });
  await act(async () => {
    button(host, "Post Journal").click();
  });
  expect(button(host, "Post Journal")).toBeTruthy();
  expect(host.textContent).toContain("No posting attempt was recorded.");

  mocks.invoke.mockRejectedValueOnce({ message: "The post result is uncertain." });
  await act(async () => {
    button(host, "Post Journal").click();
  });
  expect(button(host, "Reconcile original batch")).toBeTruthy();

  mocks.invoke.mockResolvedValueOnce({
    batchId: review.batchId,
    result: {
      result: {
        dispatch: { state: "previous_attempt_reconciled", resent: false },
      },
    },
  });
  await act(async () => {
    button(host, "Reconcile original batch").click();
  });
  expect(host.textContent).toContain("Bridge confirmed the original Journal and its saved batch.");
  expect(button(host, "Choose another file")).toBeTruthy();
  expect([...host.querySelectorAll("button")].some((item) => item.textContent?.includes("Post Journal"))).toBe(false);

  root.unmount();
});
