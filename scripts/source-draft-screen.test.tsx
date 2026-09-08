// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { SourceDraftScreen } from "../src/SourceDraftScreen";

function row(position: number) {
  return {
    position,
    source_remote_id: `source-${position}`,
    source_date: "20260901",
    source_voucher_type: "Bank statement row",
    source_narration: position === 2 ? null : `Source narration ${position}`,
    source_omitted_fields: position === 1 ? ["BANKALLOCATION"] : [],
    entries: [{ position: 1, source_ledger: "Source ledger", source_amount: "12.50", source_polarity: position === 2 ? null : "Dr" }],
    proposal: { date: null, voucher_type: null, narration: null, notes: "", entries: [{ ledger: null, side: null, amount: null }] },
  };
}

const draft = {
  draft_id: "00000000-0000-4000-8000-000000000001",
  revision: 1,
  source_filename: "source.xml",
  source_sha256: "a".repeat(64),
  source_notices: [{ kind: "Non-voucher records retained", count: 3 }],
  rows: [row(1), row(2)],
};

function button(host: HTMLElement, label: string) {
  const match = [...host.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.includes(label));
  if (!match) throw new Error(`Missing button: ${label}`);
  return match;
}

function setValue(element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(element.constructor.prototype, "value")?.set
    ?? Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(element, value);
  element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  if (element instanceof HTMLInputElement && element.type === "date") element.dispatchEvent(new Event("change", { bubbles: true }));
}

async function mount(host: HTMLElement, props: React.ComponentProps<typeof SourceDraftScreen> = {}) {
  const root = createRoot(host);
  await act(async () => root.render(<SourceDraftScreen {...props} />));
  return root;
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("keeps source observations read-only and saves only row-ordered proposals", async () => {
  mocks.invoke.mockResolvedValueOnce(draft);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());

  expect(host.textContent).toContain("source.xml");
  expect(host.textContent).toContain("Source ledger");
  expect(host.textContent).toContain("Other source fields retained in original XML (1)");
  expect(host.textContent).toContain("Source-level notices (1)");
  expect(host.textContent).toContain("Non-voucher records retained");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')).toBeTruthy();
  expect(host.querySelector<HTMLLabelElement>('label[for="source-draft-1-notes"]')?.querySelector("textarea")).toBeNull();

  setValue(host.querySelector<HTMLInputElement>('input[type="date"]')!, "2026-09-02");
  setValue(host.querySelector<HTMLSelectElement>(".source-draft-proposal select")!, "Payment");
  setValue(host.querySelector<HTMLTextAreaElement>(".source-draft-proposal textarea")!, "Prepared narration");
  setValue(host.querySelectorAll<HTMLTextAreaElement>(".source-draft-proposal textarea")[1]!, "Needs ledger mapping");
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unverified Cash");
  setValue(host.querySelectorAll<HTMLSelectElement>(".source-draft-entry select")[0]!, "Dr");
  setValue(host.querySelector<HTMLInputElement>('input[inputmode="decimal"]')!, "12.50");

  const saved = { ...draft, revision: 2 };
  mocks.invoke.mockResolvedValueOnce(saved);
  await act(async () => button(host, "Save draft").click());
  expect(mocks.invoke).toHaveBeenNthCalledWith(1, "desktop_pick_source_draft");
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, "desktop_save_source_draft", {
    request: {
      draft_id: draft.draft_id,
      revision: draft.revision,
      proposals: [expect.objectContaining({ date: "20260902", voucher_type: "Payment", narration: "Prepared narration", notes: "Needs ledger mapping" }), draft.rows[1].proposal],
    },
  });
  expect(host.textContent).toContain("Draft saved locally as JSON.");
  expect(host.textContent).toContain("Source ledger");
  root.unmount();
});

test("allows an untouched new source draft to be saved locally", async () => {
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce({ ...draft, revision: 2 });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  expect(host.textContent).toContain("2 rows without a proposal");
  expect(host.textContent).toContain("No proposal");
  expect(button(host, "Save draft").disabled).toBe(false);
  await act(async () => button(host, "Save draft").click());
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, "desktop_save_source_draft", {
    request: { draft_id: draft.draft_id, revision: draft.revision, proposals: [draft.rows[0].proposal, draft.rows[1].proposal] },
  });
  expect(host.textContent).toContain("Draft saved locally as JSON.");
  root.unmount();
});

test("preserves dirty edits through native cancel, failed save, and explicit discard confirmation", async () => {
  mocks.invoke.mockResolvedValueOnce(draft);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Local proposal");
  expect(host.textContent).toContain("Proposal started");

  mocks.invoke.mockResolvedValueOnce(null);
  await act(async () => button(host, "Choose new source").click());
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Local proposal");

  mocks.invoke.mockReset();
  mocks.invoke.mockRejectedValueOnce(new Error("save unavailable"));
  await act(async () => button(host, "Save draft").click());
  expect(host.textContent).toContain("save unavailable");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Local proposal");

  await act(async () => button(host, "Choose new source").click());
  expect(host.textContent).toContain("Discard unsaved proposals?");
  await act(async () => button(host, "Keep editing").click());
  expect(host.textContent).not.toContain("Discard unsaved proposals?");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Local proposal");

  const replacement = { ...draft, source_filename: "replacement.xml" };
  mocks.invoke.mockResolvedValueOnce(replacement);
  await act(async () => button(host, "Choose new source").click());
  await act(async () => button(host, "Discard and open").click());
  expect(host.textContent).toContain("replacement.xml");
  expect(host.textContent).not.toContain("Local proposal");
  root.unmount();
});

test("keeps edits scoped to a row across 25-row pagination", async () => {
  const manyRows = Array.from({ length: 26 }, (_, index) => row(index + 1));
  const manyDraft = { ...draft, rows: manyRows };
  mocks.invoke.mockResolvedValueOnce(manyDraft);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Next").click());
  await act(async () => button(host, "#26").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Second page ledger");
  await act(async () => button(host, "Previous").click());
  await act(async () => button(host, "#1").click());
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("");
  await act(async () => button(host, "Next").click());
  await act(async () => button(host, "#26").click());
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Second page ledger");
  root.unmount();
});

test("does not update state after a late save result on unmount", async () => {
  mocks.invoke.mockResolvedValueOnce(draft);
  const busyStates: boolean[] = [];
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { onBusyChange: (busy) => busyStates.push(busy) });
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Late edit");
  let resolveSave!: (value: unknown) => void;
  const pendingSave = new Promise((resolve) => { resolveSave = resolve; });
  mocks.invoke.mockReturnValueOnce(pendingSave);
  await act(async () => button(host, "Save draft").click());
  root.unmount();
  resolveSave(draft);
  await act(async () => { await pendingSave; });
  expect(busyStates).toEqual([true, false, true, false]);
});
