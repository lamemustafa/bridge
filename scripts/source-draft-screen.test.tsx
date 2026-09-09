// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

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

function enableNativeWindowRuntime() {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: { metadata: { currentWindow: { label: "main" } } },
  });
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

beforeEach(() => {
  mocks.listen.mockImplementation(() => new Promise<never>(() => {}));
});

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
  mocks.listen.mockReset();
  mocks.unlisten.mockReset();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
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

test("clears saved status when a proposal changes after saving", async () => {
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce({ ...draft, revision: 2 });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Save draft").click());
  expect(host.textContent).toContain("Draft saved locally as JSON.");

  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  expect(host.textContent).not.toContain("Draft saved locally as JSON.");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Unsaved ledger");
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
  expect(host.querySelector("#source-draft-editor-heading")).toBeNull();
  await act(async () => button(host, "#26").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Second page ledger");
  await act(async () => button(host, "Previous").click());
  expect(host.querySelector("#source-draft-editor-heading")).toBeNull();
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

test("keeps a dirty native lifecycle request local until the matching response", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  let pending: { request_id: string; kind: "close" | "exit" } | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      pending = null;
      return Promise.resolve();
    }
    return Promise.resolve();
  });
  const reveal = vi.fn();
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { onNativeLifecycleRequested: reveal });
  await act(async () => {});
  expect(mocks.listen).toHaveBeenCalledWith("source-draft-lifecycle-requested", expect.any(Function));
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  const close = { request_id: "close-1", kind: "close" as const };
  pending = close;
  await act(async () => lifecycleListener?.({ payload: close }));
  expect(reveal).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("Discard unsaved proposals and close this window?");
  expect(button(host, "Choose new source").disabled).toBe(true);
  expect(button(host, "Keep editing").disabled).toBe(false);
  await act(async () => button(host, "Keep editing").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_cancel_source_draft_lifecycle_request", { request: close });

  const exit = { request_id: "exit-1", kind: "exit" as const };
  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));
  await act(async () => button(host, "Discard and quit").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: exit });
  root.unmount();
});

test("ignores an event whose request is no longer pending", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  let pending: { request_id: string; kind: "close" | "exit" } | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  const current = { request_id: "close-b", kind: "close" as const };
  pending = current;
  await act(async () => lifecycleListener?.({ payload: { request_id: "close-a", kind: "close" } }));
  expect(host.textContent).not.toContain("Discard unsaved proposals and close this window?");
  await act(async () => lifecycleListener?.({ payload: current }));
  expect(host.textContent).toContain("Discard unsaved proposals and close this window?");
  root.unmount();
});

test("does not let a late query for a cancelled request replace a newer dialog", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const first = { request_id: "close-a", kind: "close" as const };
  const second = { request_id: "exit-b", kind: "exit" as const };
  let pending: typeof first | typeof second | null = null;
  let deferFirst = false;
  let resolveFirst!: (request: typeof first) => void;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") {
      if (deferFirst && pending === first) return new Promise(resolve => { resolveFirst = resolve; });
      return Promise.resolve(pending);
    }
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      pending = null;
      return Promise.resolve();
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  pending = first;
  await act(async () => lifecycleListener?.({ payload: first }));
  expect(host.textContent).toContain("Discard unsaved proposals and close this window?");

  deferFirst = true;
  lifecycleListener?.({ payload: first });
  await act(async () => {});
  await act(async () => button(host, "Keep editing").click());
  pending = second;
  deferFirst = false;
  await act(async () => lifecycleListener?.({ payload: second }));
  expect(host.textContent).toContain("Discard unsaved proposals and quit Bridge?");

  await act(async () => resolveFirst(first));
  expect(host.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  root.unmount();
});

test("reports native listener registration failure", async () => {
  enableNativeWindowRuntime();
  mocks.listen.mockRejectedValueOnce(new Error("event permission denied"));
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => {});
  expect(host.textContent).toContain("Bridge could not install native close protection: event permission denied");
  root.unmount();
});

test("completes a native lifecycle request that was pending before listener registration", async () => {
  enableNativeWindowRuntime();
  mocks.listen.mockResolvedValueOnce(mocks.unlisten);
  const pending = { request_id: "pending-1", kind: "exit" as const };
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => {});
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: pending });
  root.unmount();
});

test("unregisters a listener that resolves after the source draft unmounts", async () => {
  enableNativeWindowRuntime();
  let resolveListen!: (unlisten: () => void) => void;
  mocks.listen.mockImplementation(() => new Promise(resolve => { resolveListen = resolve; }));
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  root.unmount();
  await act(async () => resolveListen(mocks.unlisten));
  expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_pending_source_draft_lifecycle_request");
});
