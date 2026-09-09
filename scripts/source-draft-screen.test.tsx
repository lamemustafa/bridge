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

const catalogScope = {
  config: { host: "127.0.0.1", port: 9000 },
  selected_company: {
    display_name: "Synthetic review company",
    company_guid: "00000000-0000-4000-8000-000000000001",
    company_number: "1",
    books_from_yyyymmdd: "20260401",
  },
};

const catalog = {
  capture_id: "00000000-0000-4000-8000-000000000099",
  source_sha256: draft.source_sha256,
  targets: ["Existing target"],
  evidence: { request_sha256: "b".repeat(64), response_sha256: "c".repeat(64), bytes: 100, state: "complete" as const },
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

test("requires an explicit current-session re-read before treating a saved matching target as selected, then clears and reloads it", async () => {
  const savedTarget = {
    ...draft,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Existing target" }] },
    } : item),
  };
  const applied = { ...savedTarget, revision: 2 };
  mocks.invoke
    .mockResolvedValueOnce(savedTarget)
    .mockResolvedValueOnce(catalog)
    .mockResolvedValueOnce(applied)
    .mockResolvedValueOnce(undefined)
    .mockResolvedValueOnce(catalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  expect(target.value).toBe("");
  expect(host.textContent).toContain("Saved unverified target: Existing target. Select it to check it against this current capture.");

  await act(async () => setValue(target, "Existing target"));
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_apply_source_draft_existing_ledger_target", {
    request: expect.objectContaining({
      draft_id: draft.draft_id,
      revision: draft.revision,
      capture_id: catalog.capture_id,
      row_position: 1,
      entry_position: 1,
      target_name: "Existing target",
      proposals: savedTarget.rows.map((item) => item.proposal),
    }),
  });
  expect(host.textContent).toContain("This current-session target was re-read and bound. It remains an unapproved proposal.");

  await act(async () => button(host, "Clear target").click());
  expect(mocks.invoke).toHaveBeenNthCalledWith(4, "desktop_invalidate_source_draft_existing_ledger_targets");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("");
  expect(host.textContent).toContain("Load existing ledgers to choose a target.");

  await act(async () => button(host, "Load existing ledgers").click());
  expect(mocks.invoke).toHaveBeenNthCalledWith(5, "desktop_load_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, ...catalogScope },
  });
  expect(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")?.value).toBe("");
  root.unmount();
});

test("clears the visible catalogue and blocks a new read until the native company-scope invalidation completes", async () => {
  let resolveFirstInvalidation!: () => void;
  let resolveSecondInvalidation!: () => void;
  const firstInvalidation = new Promise<void>((resolve) => { resolveFirstInvalidation = resolve; });
  const secondInvalidation = new Promise<void>((resolve) => { resolveSecondInvalidation = resolve; });
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(catalog)
    .mockReturnValueOnce(firstInvalidation)
    .mockReturnValueOnce(secondInvalidation);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")).toBeTruthy();

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-two" />));
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_invalidate_source_draft_existing_ledger_targets");
  expect(button(host, "Load existing ledgers").disabled).toBe(true);
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')).toBeTruthy();

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-three" />));
  expect(mocks.invoke).toHaveBeenCalledTimes(3);
  resolveFirstInvalidation();
  await act(async () => { await firstInvalidation; });
  expect(mocks.invoke).toHaveBeenNthCalledWith(4, "desktop_invalidate_source_draft_existing_ledger_targets");
  expect(button(host, "Load existing ledgers").disabled).toBe(true);

  resolveSecondInvalidation();
  await act(async () => { await secondInvalidation; });
  expect(button(host, "Load existing ledgers").disabled).toBe(false);
  root.unmount();
});

test("keeps the Tally read lock through parent rerenders until catalog reads settle", async () => {
  let resolveLoad!: (value: unknown) => void;
  let resolveApply!: (value: unknown) => void;
  const pendingLoad = new Promise((resolve) => { resolveLoad = resolve; });
  const pendingApply = new Promise((resolve) => { resolveApply = resolve; });
  const applied = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0
      ? { ...item, proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Existing target" }] } }
      : item),
  };
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_load_source_draft_existing_ledger_targets") return pendingLoad;
    if (command === "desktop_apply_source_draft_existing_ledger_target") return pendingApply;
    return Promise.resolve();
  });
  const tallyReadStates: boolean[] = [];
  function RerenderingParent() {
    const [, setBusy] = React.useState(false);
    return <SourceDraftScreen
      onBusyChange={setBusy}
      onTallyReadActivityChange={(active) => tallyReadStates.push(active)}
      catalogScope={catalogScope}
      catalogScopeKey="company-one"
    />;
  }
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<RerenderingParent />));
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(tallyReadStates).toEqual([true]);
  resolveLoad(catalog);
  await act(async () => { await pendingLoad; });
  expect(tallyReadStates).toEqual([true, false]);

  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Existing target"));
  expect(tallyReadStates).toEqual([true, false, true]);
  resolveApply(applied);
  await act(async () => { await pendingApply; });
  expect(tallyReadStates).toEqual([true, false, true, false]);
  root.unmount();
});

test("reconciles a catalog apply committed before a concurrent scope invalidation", async () => {
  let resolveApply!: (value: unknown) => void;
  let resolveInvalidation!: () => void;
  const pendingApply = new Promise((resolve) => { resolveApply = resolve; });
  const pendingInvalidation = new Promise<void>((resolve) => { resolveInvalidation = resolve; });
  const applied = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0
      ? { ...item, proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Existing target" }] } }
      : item),
  };
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_load_source_draft_existing_ledger_targets") return Promise.resolve(catalog);
    if (command === "desktop_apply_source_draft_existing_ledger_target") return pendingApply;
    if (command === "desktop_invalidate_source_draft_existing_ledger_targets") return pendingInvalidation;
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Existing target"));
  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-two" />));
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_invalidate_source_draft_existing_ledger_targets");

  resolveApply(applied);
  await act(async () => { await pendingApply; });
  expect(host.textContent).toContain("revision 2");
  expect(host.querySelector("#source-draft-1-entry-0-ledger")?.tagName).toBe("INPUT");
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("Existing target");

  resolveInvalidation();
  await act(async () => { await pendingInvalidation; });
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

test("holds a native close until a pending catalog apply settles and an operator explicitly confirms", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  let pending: { request_id: string; kind: "close" | "exit" } | null = null;
  let resolveApply!: (value: unknown) => void;
  const pendingApply = new Promise((resolve) => { resolveApply = resolve; });
  const applied = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0
      ? { ...item, proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Existing target" }] } }
      : item),
  };
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_load_source_draft_existing_ledger_targets") return Promise.resolve(catalog);
    if (command === "desktop_apply_source_draft_existing_ledger_target") return pendingApply;
    if (command === "desktop_complete_source_draft_lifecycle_request") return Promise.resolve();
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => {});
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  await act(async () => setValue(target, "Existing target"));
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_apply_source_draft_existing_ledger_target", expect.anything());

  const close = { request_id: "close-during-apply", kind: "close" as const };
  pending = close;
  await act(async () => lifecycleListener?.({ payload: close }));
  expect(host.textContent).toContain("Close this window?");
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });
  expect(button(host, "Discard and close").disabled).toBe(true);
  expect(button(host, "Keep editing").disabled).toBe(true);

  resolveApply(applied);
  await act(async () => { await pendingApply; });
  expect(button(host, "Discard and close").disabled).toBe(false);
  await act(async () => button(host, "Discard and close").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });
  root.unmount();
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

test("reveals a hidden draft for a guarded native quit request", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "exit-hidden", kind: "exit" as const };
  let pending: typeof exit | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });

  function HiddenDraftShell() {
    const [visible, setVisible] = React.useState(false);
    return (
      <div data-testid="draft-shell" hidden={!visible}>
        <SourceDraftScreen onNativeLifecycleRequested={() => setVisible(true)} />
      </div>
    );
  }

  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<HiddenDraftShell />));
  const shell = host.querySelector<HTMLElement>("[data-testid=draft-shell]")!;
  expect(shell.hidden).toBe(true);

  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));

  expect(shell.hidden).toBe(false);
  expect(host.textContent).toContain("Discard unsaved proposals and quit Bridge?");
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
