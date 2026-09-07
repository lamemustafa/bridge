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

  mocks.invoke.mockResolvedValueOnce(null);
  await act(async () => {
    button(host, "Choose another file").click();
  });
  expect(host.textContent).toContain("Bridge confirmed the original Journal and its saved batch.");
  expect([...host.querySelectorAll("button")].some((item) => item.textContent?.includes("Post Journal"))).toBe(false);

  root.unmount();
});

test("reports a busy Journal action until the native post result is rendered", async () => {
  const busyStates: boolean[] = [];
  let resolvePost!: (value: unknown) => void;
  const pendingPost = new Promise((resolve) => {
    resolvePost = resolve;
  });
  mocks.invoke.mockResolvedValueOnce(review);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);

  await act(async () => {
    root.render(<JournalPostingScreen config={config} onBusyChange={(busy) => busyStates.push(busy)} />);
  });
  await act(async () => {
    button(host, "Choose Journal file").click();
  });

  mocks.invoke.mockReturnValueOnce(pendingPost);
  await act(async () => {
    button(host, "Post Journal").click();
    await Promise.resolve();
  });
  expect(busyStates.at(-1)).toBe(true);
  expect(button(host, "Review approval dialog").disabled).toBe(true);

  await act(async () => {
    resolvePost({
      batchId: review.batchId,
      result: { result: { dispatch: { state: "posted_verified", resent: false } } },
    });
    await pendingPost;
  });
  expect(busyStates.at(-1)).toBe(false);
  expect(host.textContent).toContain("Bridge confirmed the original Journal and its saved batch.");
  expect([...host.querySelectorAll("button")].some((item) => item.textContent?.includes("Post Journal"))).toBe(false);
  root.unmount();
});

test("shows retained response evidence only inside collapsed recovery details", async () => {
  mocks.invoke.mockResolvedValueOnce(review).mockResolvedValueOnce({
    batchId: review.batchId,
    result: {
      result: {
        dispatch: { state: "reconciliation_required", resent: false },
        attempt_recorded: true,
        dispatch_response: {
          request_sha256: "a".repeat(64),
          response_sha256: "b".repeat(64),
          bytes: 538,
          outcome: {
            application_status: "success",
            counters: {
              created: 1,
              altered: 0,
              deleted: 0,
              ignored: 0,
              errors: 0,
              cancelled: 0,
              exceptions: 0,
              line_error_count: 0,
            },
            exceptions_were_reported: true,
          },
        },
        error: { code: "import_ledger_append_failed", message: "Reconcile the original batch." },
      },
    },
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);

  await act(async () => {
    root.render(<JournalPostingScreen config={config} />);
  });
  await act(async () => {
    button(host, "Choose Journal file").click();
  });
  await act(async () => {
    button(host, "Post Journal").click();
  });

  const details = host.querySelector("details");
  expect(details?.textContent).toContain("Tally's response is retained for reconciliation; Bridge only confirms posting after a matching Journal readback.");
  expect(details?.textContent).toContain("success");
  expect(details?.textContent).toContain("Created 1");
  expect(details?.textContent).toContain("Line errors 0");
  expect(details?.textContent).toContain("a".repeat(64));
  expect(host.textContent).toContain("Reconcile original batch");
  root.unmount();
});

for (const [code, expected] of [
  ["import_approval_timed_out", "The approval dialog expired before Bridge could post this Journal."],
  ["import_approval_declined", "The Journal was not posted because approval was declined."],
] as const) {
  test(`explains ${code} while keeping Post Journal available`, async () => {
    mocks.invoke.mockResolvedValueOnce(review).mockResolvedValueOnce({
      batchId: review.batchId,
      result: {
        result: {
          dispatch: { state: "not_dispatched" },
          attempt_recorded: false,
          error: { code, message: "No posting attempt was recorded." },
        },
      },
    });
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);

    await act(async () => {
      root.render(<JournalPostingScreen config={config} />);
    });
    await act(async () => {
      button(host, "Choose Journal file").click();
    });
    await act(async () => {
      button(host, "Post Journal").click();
    });

    expect(host.textContent).toContain(expected);
    expect(host.textContent).toContain(code);
    expect(button(host, "Post Journal")).toBeTruthy();
    root.unmount();
  });
}
