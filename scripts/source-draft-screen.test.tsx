// @vitest-environment jsdom

import React, { act } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

import { SourceDraftScreen } from "../src/SourceDraftScreen";
import { JournalPostingScreen } from "../src/JournalPostingScreen";

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

const journalReview = {
  batchId: "bridge-00000000-0000-4000-8000-000000000010",
  sha256: "b".repeat(64),
  company: {
    name: "Synthetic Accounts",
    guid: "00000000-0000-4000-8000-000000000011",
    companyNumber: "100001",
    booksFrom: "20260401",
  },
  builtAt: "2026-09-10T00:00:00Z",
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

const journalConfig = { host: "127.0.0.1", port: 9001 };

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
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => {});
  expect(mocks.listen).toHaveBeenCalledWith("source-draft-lifecycle-requested", expect.any(Function));
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  const close = { request_id: "close-1", kind: "close" as const };
  pending = close;
  await act(async () => lifecycleListener?.({ payload: close }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and close this window?");
  expect(button(host, "Choose new source").disabled).toBe(true);
  expect(button(document.body, "Keep editing").disabled).toBe(false);
  await act(async () => button(document.body, "Keep editing").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_cancel_source_draft_lifecycle_request", { request: close });

  const exit = { request_id: "exit-1", kind: "exit" as const };
  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));
  await act(async () => button(document.body, "Discard and quit").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: exit });
  root.unmount();
});

test("keeps an active Journal mounted while a hidden source draft shows its native confirmation", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "exit-hidden", kind: "exit" as const };
  let pending: typeof exit | null = null;
  let resolvePost!: (value: unknown) => void;
  const pendingPost = new Promise((resolve) => { resolvePost = resolve; });
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_pick_journal_for_review") return Promise.resolve(journalReview);
    if (command === "desktop_post_reviewed_journal") return pendingPost;
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      pending = null;
      return Promise.resolve();
    }
    return Promise.resolve();
  });

  function ActiveJournalShell() {
    const journalBusy = React.useRef(false);
    const [, rerender] = React.useState(0);
    const onJournalBusyChange = (next: boolean) => {
      journalBusy.current = next;
      rerender((value) => value + 1);
    };
    const isNativeLifecycleCompletionBlocked = React.useCallback(() => journalBusy.current, []);
    return (
      <>
        <JournalPostingScreen config={journalConfig} onBusyChange={onJournalBusyChange} />
        <div data-testid="draft-shell" hidden>
          <SourceDraftScreen isNativeLifecycleCompletionBlocked={isNativeLifecycleCompletionBlocked} />
        </div>
      </>
    );
  }

  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<ActiveJournalShell />));
  const shell = host.querySelector<HTMLElement>("[data-testid=draft-shell]")!;
  expect(shell.hidden).toBe(true);

  await act(async () => button(host, "Choose Journal file").click());
  await act(async () => {
    button(host, "Post Journal").click();
    await Promise.resolve();
  });
  await act(async () => button(host, "Choose source XML").click());
  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));

  expect(shell.hidden).toBe(true);
  expect(host.textContent).toContain("Review a Bridge Journal");
  expect(button(host, "Review approval dialog").disabled).toBe(true);
  expect(document.body.textContent).toContain("A Journal action is in progress.");
  expect(button(document.body, "Discard and quit").disabled).toBe(true);
  await act(async () => button(document.body, "Keep editing").click());
  await act(async () => resolvePost({
    batchId: journalReview.batchId,
    result: { result: { dispatch: { state: "posted_verified", resent: false } } },
  }));
  expect(host.textContent).toContain("Bridge confirmed the original Journal and its saved batch.");
  root.unmount();
});

test("restores the opener after the parent removes lifecycle inertness", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "focus-exit", kind: "exit" as const };
  let pending: typeof exit | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      pending = null;
      return Promise.resolve();
    }
    return Promise.resolve();
  });

  function LifecycleFocusShell() {
    const [lifecycleOpen, setLifecycleOpen] = React.useState(false);
    const [restoreFocus, setRestoreFocus] = React.useState<(() => void) | null>(null);
    const blockCompletion = React.useCallback(() => true, []);
    const onClosed = React.useCallback((restore: () => void) => {
      setRestoreFocus(() => restore);
    }, []);
    React.useEffect(() => {
      if (lifecycleOpen || !restoreFocus) return;
      restoreFocus();
      setRestoreFocus(null);
    }, [lifecycleOpen, restoreFocus]);
    return (
      <>
        <div data-testid="lifecycle-shell" inert={lifecycleOpen || undefined}>
          <button type="button">Journal opener</button>
          <div hidden>
            <SourceDraftScreen
              isNativeLifecycleCompletionBlocked={blockCompletion}
              onNativeLifecycleModalChange={setLifecycleOpen}
              onNativeLifecycleModalClosed={onClosed}
            />
          </div>
        </div>
        {createPortal(
          <aside
            data-testid="evidence-drawer"
            inert={lifecycleOpen || undefined}
            aria-hidden={lifecycleOpen || undefined}
          >
            Evidence drawer
          </aside>,
          document.body,
        )}
      </>
    );
  }

  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<LifecycleFocusShell />));
  const opener = button(host, "Journal opener");
  opener.focus();
  expect(document.activeElement).toBe(opener);

  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));
  const shell = host.querySelector<HTMLElement>("[data-testid=lifecycle-shell]")!;
  const drawer = document.body.querySelector<HTMLElement>("[data-testid=evidence-drawer]")!;
  expect(shell.hasAttribute("inert")).toBe(true);
  expect(drawer.hasAttribute("inert")).toBe(true);
  expect(drawer.getAttribute("aria-hidden")).toBe("true");
  expect(document.activeElement).toBe(document.body.querySelector("[role=alertdialog]"));

  await act(async () => button(document.body, "Keep editing").click());
  expect(shell.hasAttribute("inert")).toBe(false);
  expect(drawer.hasAttribute("inert")).toBe(false);
  expect(drawer.hasAttribute("aria-hidden")).toBe(false);
  expect(document.activeElement).toBe(opener);
  root.unmount();
});

test("keeps a newer native request while an earlier cancellation reply resolves", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const first = { request_id: "close-a", kind: "close" as const };
  const second = { request_id: "exit-b", kind: "exit" as const };
  let pending: typeof first | typeof second | null = null;
  let resolveCancel!: () => void;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") return new Promise<void>((resolve) => { resolveCancel = resolve; });
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  pending = first;
  await act(async () => lifecycleListener?.({ payload: first }));
  await act(async () => button(document.body, "Keep editing").click());

  pending = second;
  await act(async () => lifecycleListener?.({ payload: second }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");

  await act(async () => resolveCancel());
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  root.unmount();
});

test("blocks a clean native close synchronously while its completion is pending", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "clean-close", kind: "close" as const };
  let pending: typeof close | null = null;
  let resolveComplete!: () => void;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_complete_source_draft_lifecycle_request") return new Promise<void>((resolve) => { resolveComplete = resolve; });
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host);
  await act(async () => button(host, "Choose source XML").click());
  const ledger = host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!;

  pending = close;
  await act(async () => {
    lifecycleListener?.({ payload: close });
    await Promise.resolve();
  });
  expect(ledger.disabled).toBe(true);
  setValue(ledger, "Late proposal");
  expect(ledger.value).toBe("");
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });

  await act(async () => resolveComplete());
  root.unmount();
});

test("waits for a Journal action before allowing a native lifecycle completion", async () => {
  enableNativeWindowRuntime();
  let lifecycleListener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    lifecycleListener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "exit-during-journal", kind: "exit" as const };
  let pending: typeof exit | null = null;
  let blocked = true;
  let rerender!: () => void;
  const isBlocked = () => blocked;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  function JournalBusyShell() {
    const [, setRevision] = React.useState(0);
    rerender = () => setRevision((value) => value + 1);
    return <SourceDraftScreen isNativeLifecycleCompletionBlocked={isBlocked} />;
  }
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<JournalBusyShell />));
  await act(async () => button(host, "Choose source XML").click());

  pending = exit;
  await act(async () => lifecycleListener?.({ payload: exit }));
  expect(document.body.textContent).toContain("A Journal action is in progress.");
  expect(button(document.body, "Discard and quit").disabled).toBe(true);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: exit });

  blocked = false;
  await act(async () => rerender());
  expect(button(document.body, "Discard and quit").disabled).toBe(false);
  await act(async () => button(document.body, "Discard and quit").click());
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
  expect(document.body.textContent).not.toContain("Discard unsaved proposals and close this window?");
  await act(async () => lifecycleListener?.({ payload: current }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and close this window?");
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
  expect(document.body.textContent).toContain("Discard unsaved proposals and close this window?");

  deferFirst = true;
  lifecycleListener?.({ payload: first });
  await act(async () => {});
  await act(async () => button(document.body, "Keep editing").click());
  pending = second;
  deferFirst = false;
  await act(async () => lifecycleListener?.({ payload: second }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");

  await act(async () => resolveFirst(first));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
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
