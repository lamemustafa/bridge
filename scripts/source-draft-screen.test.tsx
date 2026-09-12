// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import React, { act } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

import { SourceDraftScreen } from "../src/SourceDraftScreen";
import { NativeLifecycleController } from "../src/NativeLifecycleController";
import { ErrorBoundary, ReloadGuardContext, type ReloadGuard } from "../src/ErrorBoundary";
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
  current_catalog_bindings: [],
  catalog_generation: 0,
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
  bindings_state: "complete" as const,
  evidence: { request_sha256: "b".repeat(64), response_sha256: "c".repeat(64), bytes: 100, state: "complete" as const },
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

test("lists only unentered required operator choices without treating a filled proposal as approved", async () => {
  const partiallyEntered = {
    ...draft,
    rows: [{
      ...draft.rows[0],
      entries: [
        ...draft.rows[0].entries,
        { position: 2, source_ledger: "Second source ledger", source_amount: "12.50", source_polarity: "Cr" },
      ],
      proposal: {
        date: "",
        voucher_type: null,
        narration: "",
        notes: "",
        entries: [
          { ledger: "Unverified first target", side: "Dr" as const, amount: "12.50" },
          { ledger: "", side: null, amount: "" },
        ],
      },
    }],
  };
  const fullyEntered = {
    ...partiallyEntered,
    rows: [{
      ...partiallyEntered.rows[0],
      proposal: {
        date: "20260902",
        voucher_type: "Payment" as const,
        narration: null,
        notes: "",
        entries: [
          { ledger: "Unverified first target", side: "Dr" as const, amount: "12.50" },
          { ledger: "Unverified second target", side: "Cr" as const, amount: "12.50" },
        ],
      },
    }],
  };
  mocks.invoke.mockResolvedValueOnce(partiallyEntered).mockResolvedValueOnce(fullyEntered);
  const partialHost = document.createElement("div");
  document.body.append(partialHost);
  const partialRoot = await mount(partialHost);
  await act(async () => button(partialHost, "Choose source XML").click());
  expect(partialHost.textContent).toContain("2026-09-01");
  const pendingChoices = [...partialHost.querySelectorAll(".source-draft-catalogue-state")]
    .map((element) => element.textContent)
    .find((value) => value?.startsWith("Still to choose:"));
  expect(pendingChoices).toBe(
    "Still to choose: date, voucher type, entry 2 target ledger, entry 2 side, entry 2 amount.",
  );
  expect(pendingChoices).not.toContain("narration");
  expect(pendingChoices).not.toContain("notes");
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  partialRoot.unmount();

  const filledHost = document.createElement("div");
  document.body.append(filledHost);
  const filledRoot = await mount(filledHost);
  await act(async () => button(filledHost, "Choose source XML").click());
  expect(filledHost.textContent).not.toContain("Still to choose:");
  expect(filledHost.textContent).toContain("Unverified proposal");
  expect(filledHost.textContent).toContain("Saved unverified target: Unverified first target");
  expect(filledHost.textContent).not.toContain("Ready");
  expect(mocks.invoke).toHaveBeenCalledTimes(2);
  filledRoot.unmount();
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

test("groups and lists a synthetic stress catalogue without losing the narrowing", async () => {
  // Synthetic stress bound only: these generated targets exercise narrowing
  // and rendering at 2,000 entries. They are not a captured catalogue and do
  // not establish a live company's ledger count or production performance.
  const bulk = Array.from({ length: 2_000 }, (_, index) => `Bulk placeholder ledger ${String(index).padStart(4, "0")}`);
  const large = {
    ...catalog,
    targets: [...bulk, "Existing target"].sort(),
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: "Existing target",
      bound_basis: "exact_name",
      unbound_reason: null,
      candidates: [],
      candidate_count: 0,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(large);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  const groups = Array.from(target.querySelectorAll("optgroup"));
  expect(groups.map((group) => group.label)).toEqual(["Matched to this source line", "All 2001 existing ledgers"]);
  // The match leads, alone, out of two thousand and one.
  expect(Array.from(groups[0].querySelectorAll("option")).map((option) => option.value)).toEqual(["Existing target"]);
  // And the whole catalogue is still there: narrowing is a shortcut through
  // the list, never a restriction on it, at any size.
  expect(groups[1].querySelectorAll("option")).toHaveLength(2_001);
  expect(target.value).toBe("");
  root.unmount();
});

test("says so when the operator chooses a different ledger from the one binding matched", async () => {
  // A choice settles what the operator wants; it does not settle a
  // disagreement. Suppressing the summary whenever a `bound_target` merely
  // existed meant choosing B where the capture defended A left nothing on
  // screen saying the two differed — while the adjacent line said the target
  // had been re-read and bound, which reads as agreement.
  const twoTargets = {
    ...catalog,
    targets: ["Bound target", "Other target"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: "Bound target",
      bound_basis: "exact_name",
      unbound_reason: null,
      candidates: [],
      candidate_count: 0,
      candidate_count_is_lower_bound: false,
      candidate_listing: "none",
    }],
  };
  const applied = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Other target" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(twoTargets).mockResolvedValueOnce(applied);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  await act(async () => setValue(target, "Other target"));
  expect(target.value).toBe("Other target");
  expect(host.textContent).toContain("This current-session target was re-read and bound.");
  // The disagreement survives the choice, and says which ledger it was about.
  expect(host.textContent).toContain("Automatic binding matched Bound target for this source line, not the ledger chosen here.");
  root.unmount();
});

test("says nothing extra when the operator chooses the ledger binding matched", async () => {
  // The other half: agreement is silence. A summary repeating the binding
  // beside an identical choice is noise, and it is why the suppression exists.
  const agreeing = {
    ...catalog,
    targets: ["Bound target", "Other target"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: "Bound target",
      bound_basis: "exact_name",
      unbound_reason: null,
      candidates: [],
      candidate_count: 0,
      candidate_count_is_lower_bound: false,
      candidate_listing: "none",
    }],
  };
  const applied = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Bound target" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(agreeing).mockResolvedValueOnce(applied);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  await act(async () => setValue(target, "Bound target"));
  expect(target.value).toBe("Bound target");
  expect(host.textContent).not.toContain("Automatic binding matched");
  root.unmount();
});

test("keeps the binding result visible beside a saved target nobody has re-read", async () => {
  // `entry.ledger` alone is not a choice. A saved target from an earlier
  // session leaves `catalogSelections` empty, the control shows nothing
  // selected, and the status line calls the value unverified — so a summary
  // that took any persisted string for a current-session selection contradicted
  // both of its own neighbours, and withheld the result the operator needed to
  // judge the saved value against this capture.
  const savedTarget = {
    ...draft,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Existing target" }] },
    } : item),
  };
  const refused = {
    ...catalog,
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_near_miss",
      candidates: ["Existing target"],
      candidate_count: 1,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(savedTarget).mockResolvedValueOnce(refused);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  // Nothing is selected, and all three statements agree about that.
  expect(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")?.value).toBe("");
  expect(host.textContent).toContain("Saved unverified target: Existing target.");
  // The summary's unselected branch, in full: the refusal, the candidate count
  // and the guidance. Keyed on `entry.ledger` the "chosen" branch fired instead
  // and dropped the last two, so asserting only the refusal lead would pass
  // either way — both branches open with it.
  expect(host.textContent).toContain("No single ledger matched this source line. Nothing is chosen; 1 possible ledger is listed first, and the full list of 1 follows.");
  expect(host.textContent).not.toContain("Automatic binding did not resolve this line.");
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
  const applied = {
    ...savedTarget,
    revision: 2,
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
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
  expect(mocks.invoke).toHaveBeenNthCalledWith(4, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.value).toBe("");
  expect(host.textContent).toContain("Load existing ledgers to choose a target.");

  await act(async () => button(host, "Load existing ledgers").click());
  expect(mocks.invoke).toHaveBeenNthCalledWith(5, "desktop_load_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, ...catalogScope },
  });
  expect(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")?.value).toBe("");
  root.unmount();
});

test("lists the bound ledger first without selecting it, and keeps the whole catalogue reachable", async () => {
  const boundCatalog = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder", "Gamma placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: "Beta placeholder",
      bound_basis: "identifier" as const,
      unbound_reason: null,
      candidates: [],
      candidate_count: 0,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(boundCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  // Narrowing must never choose. A pre-selected value is an auto-resolution.
  expect(target.value).toBe("");
  const groups = Array.from(target.querySelectorAll("optgroup")).map((group) => group.label);
  expect(groups).toEqual(["Matched to this source line", "All 3 existing ledgers"]);
  const matched = Array.from(target.querySelectorAll("optgroup")[0].querySelectorAll("option")).map((option) => option.value);
  expect(matched).toEqual(["Beta placeholder"]);
  // The full catalogue stays reachable; narrowing is a shortcut, not a filter.
  const all = Array.from(target.querySelectorAll("optgroup")[1].querySelectorAll("option")).map((option) => option.value);
  expect(all).toEqual(["Alpha placeholder", "Beta placeholder", "Gamma placeholder"]);
  // `identifier` covers a numeric run and an alphanumeric code alike, and the
  // DTO does not say which. Claiming "a number" was wrong for every ledger that
  // carries a registration or part code instead.
  expect(host.textContent).toContain("Listed first because an identifier inside the ledger name matches this source line.");
  expect(host.textContent).not.toContain("a number inside");
  expect(host.textContent).not.toContain("recommended");
  root.unmount();
});

test("names the refusal when the name and the identifier point at different ledgers", async () => {
  // The one refusal where the operator has something to act on: both sides are
  // strong and they disagree. Flattened into the generic near-miss sentence, it
  // read as an ordinary weak match and the disagreement never reached anyone.
  const conflictCatalog = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder", "Gamma placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_identifier_name_conflict",
      candidates: ["Alpha placeholder", "Gamma placeholder"],
      candidate_count: 2,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(conflictCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("matches one existing ledger exactly, while an identifier inside it matches a different one");
  expect(host.textContent).not.toContain("No single ledger matched this source line");
  // Still a refusal: nothing is chosen, and the whole catalogue stays reachable.
  expect(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")?.value).toBe("");
  root.unmount();
});

test("distinguishes the two other refusals that are not weak matches", async () => {
  for (const [reason, phrase] of [
    ["master_binding_identifier_conflict", "do not agree on one existing ledger"],
    ["master_binding_name_ambiguous", "spaces against hyphens or slashes are set aside"],
  ] as const) {
    mocks.invoke.mockReset();
    mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce({
      ...catalog,
      targets: ["Alpha placeholder", "Beta placeholder", "Gamma placeholder"],
      bindings: [{
        row_position: 1,
        entry_position: 1,
        bound_target: null,
        bound_basis: null,
        unbound_reason: reason,
        candidates: ["Alpha placeholder", "Gamma placeholder"],
        candidate_count: 2,
        candidate_count_is_lower_bound: false,
        candidate_listing: "listed",
      }],
    });
    const host = document.createElement("div");
    document.body.append(host);
    const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
    await act(async () => button(host, "Choose source XML").click());
    await act(async () => button(host, "Load existing ledgers").click());
    expect(host.textContent).toContain(phrase);
    root.unmount();
    host.remove();
  }
});

test("the picker groups a real captured catalogue, not a shape the test invented", async () => {
  // Every other test here writes both the catalogue and its bindings, so they
  // show the component agrees with an assumed response. This one reads
  // `scripts/fixtures/source-draft-capture-bindings.json`, which is the DTO the
  // **producer** emits from a `StandardLedgerCatalogV1` response captured on
  // licensed TallyPrime 7.1 — nine real ledger names, including Devanagari, an
  // `&` name, and an NFD ledger beside NFC ones.
  //
  // The Rust test `the_binder_meets_a_real_catalogue_through_the_production_parse`
  // asserts that same file against what the binder actually produces, so the
  // two halves cannot drift: if the producer changes, that test fails rather
  // than leaving this one asserting a shape nothing emits.
  const captured = JSON.parse(
    readFileSync("scripts/fixtures/source-draft-capture-bindings.json", "utf8"),
  ) as SourceDraftCatalogTargets;

  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(captured);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  // Narrowing never chooses, on a real catalogue as on a fabricated one.
  expect(target.value).toBe("");
  const groups = Array.from(target.querySelectorAll("optgroup")).map((group) => group.label);
  expect(groups).toEqual(["Matched to this source line", `All ${captured.targets.length} existing ledgers`]);
  expect(
    Array.from(target.querySelectorAll("optgroup")[0].querySelectorAll("option")).map((option) => option.value),
  ).toEqual(["Cash"]);

  // The whole captured catalogue stays reachable, Devanagari and all.
  const all = Array.from(target.querySelectorAll("optgroup")[1].querySelectorAll("option")).map((option) => option.value);
  expect(all).toEqual(captured.targets);
  expect(all.some((name) => /^[\u0900-\u097F]/.test(name))).toBe(true);

  expect(host.textContent).toContain("Listed first because the exact ledger name matches this source line.");
  root.unmount();
});

test("a refusal reason survives the candidate listing being dropped", async () => {
  // The budget-exhaustion branch returned only the budget sentence, which
  // re-hid the strong disagreement the neighbouring branch had just been fixed
  // to show. Same defect, one branch over: a conflict arriving with no room to
  // list its candidates is still a conflict.
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce({
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_identifier_name_conflict",
      candidates: [],
      candidate_count: 6,
      candidate_count_is_lower_bound: true,
      candidate_listing: "truncated",
    }],
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("matches one existing ledger exactly, while an identifier inside it matches a different one");
  expect(host.textContent).toContain("ran out of room to list them");
  root.unmount();
});

test("choosing a target stops the screen saying nothing was chosen, without hiding why", async () => {
  // The refusal summary kept rendering beneath a selected target, saying
  // "nothing is chosen" directly beside the line telling the operator their
  // target was re-read and bound. The reason still matters after the choice —
  // an identifier and a name pointing at different ledgers is grounds to check
  // it — so it survives in the past tense rather than being hidden.
  const conflicted = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder", "Gamma placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_identifier_name_conflict",
      candidates: ["Alpha placeholder", "Gamma placeholder"],
      candidate_count: 2,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  // Selecting a target goes through the apply path, which re-reads: the draft
  // it returns is what puts the ledger on the entry.
  const chosen = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Alpha placeholder" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(conflicted)
    .mockResolvedValueOnce(chosen);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("Nothing is chosen;");

  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Alpha placeholder"));
  expect(host.textContent).not.toContain("Nothing is chosen;");
  expect(host.textContent).toContain("Automatic binding did not resolve this line.");
  expect(host.textContent).toContain("they disagree");
  root.unmount();
});

test("an empty list because the report ran out of room is not a family the name cannot separate", async () => {
  // Both arrive with an empty list and a nonzero count, and they call for
  // opposite actions. A withheld family is fixed by a fuller source name;
  // budget exhaustion on earlier rows is not fixed by anything written here,
  // and telling the operator to rewrite the name would be a wild goose chase.
  const exhaustedCatalog = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_near_miss",
      candidates: [],
      candidate_count: 7,
      candidate_count_is_lower_bound: true,
      candidate_listing: "truncated",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(exhaustedCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("ran out of room to list them");
  expect(host.textContent).not.toContain("tells them apart from none of them");
  expect(host.textContent).not.toContain("Use a fuller source name");
  root.unmount();
});

test("lists candidates first for a near miss and states that nothing was chosen", async () => {
  const nearMissCatalog = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder", "Gamma placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_near_miss",
      candidates: ["Alpha placeholder", "Gamma placeholder"],
      candidate_count: 2,
      candidate_count_is_lower_bound: false,
      candidate_listing: "listed",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(nearMissCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  expect(target.value).toBe("");
  const groups = Array.from(target.querySelectorAll("optgroup")).map((group) => group.label);
  expect(groups).toEqual(["Possible for this source line", "All 3 existing ledgers"]);
  const possible = Array.from(target.querySelectorAll("optgroup")[0].querySelectorAll("option")).map((option) => option.value);
  expect(possible).toEqual(["Alpha placeholder", "Gamma placeholder"]);
  expect(host.textContent).toContain("No single ledger matched this source line. Nothing is chosen; 2 possible ledgers are listed first, and the full list of 3 follows.");
  root.unmount();
});

test("reports a truncated candidate list truthfully and falls back to the flat catalogue without bindings", async () => {
  const truncatedCatalog = {
    ...catalog,
    targets: ["Alpha placeholder", "Beta placeholder"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_near_miss",
      candidates: ["Alpha placeholder"],
      candidate_count: 40,
      candidate_count_is_lower_bound: true,
      candidate_listing: "truncated",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(truncatedCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("1 of at least 40 possible ledger is listed first");
  root.unmount();
});

test("a family withheld under a different reason is not reported as a full report", async () => {
  // The second withheld shape. An identifier held by more masters than a
  // candidate list may show is withheld under `identifier_conflict`, not under
  // `no_discriminating_candidate` — and inferring the state from the reason
  // knew only the latter, so this one fell to the budget sentence and told the
  // operator the report had run out of room when it had declined to slice.
  // The two sentences give opposite advice, so this is not a wording defect.
  const withheldFamily = {
    ...catalog,
    targets: ["DN Party 001", "DN Party 002", "DN Party 003"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_identifier_conflict",
      candidates: [],
      candidate_count: 30,
      candidate_count_is_lower_bound: true,
      candidate_listing: "withheld",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(withheldFamily);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  expect(host.textContent).toContain("matches at least 30 existing ledgers and tells them apart from none of them, so none is listed");
  expect(host.textContent).toContain("The identifiers in this source line do not agree on one existing ledger");
  expect(host.textContent).toContain("either one of them appears in several, or they point at different ones");
  expect(host.textContent).not.toContain("ran out of room");
  root.unmount();
});

test("a materialized withheld family keeps its exact count", async () => {
  // Listing state and count precision are independent: a full prefix-family
  // union may deliberately withhold names without making its count an estimate.
  const withheldFamily = {
    ...catalog,
    targets: ["DN Party 001", "DN Party 002", "DN Party 003"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_no_discriminating_candidate",
      candidates: [],
      candidate_count: 30,
      candidate_count_is_lower_bound: false,
      candidate_listing: "withheld",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(withheldFamily);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  expect(host.textContent).toContain("matches 30 existing ledgers and tells them apart from none of them, so none is listed");
  expect(host.textContent).not.toContain("matches at least 30 existing ledgers");
  root.unmount();
});

test("a source line that separates no ledger says so instead of counting nothing", async () => {
  // The state the live measurement made necessary: the name reaches a whole
  // family and tells none of them apart, so listing an arbitrary slice would
  // put the right one out of view. The old copy printed "0 possible ledgers
  // are listed first", which is a count of nothing.
  const familyCatalog = {
    ...catalog,
    targets: ["DN Party 001", "DN Party 002", "DN Party 003"],
    bindings: [{
      row_position: 1,
      entry_position: 1,
      bound_target: null,
      bound_basis: null,
      unbound_reason: "master_binding_no_discriminating_candidate",
      candidates: [],
      candidate_count: 120,
      candidate_count_is_lower_bound: true,
      candidate_listing: "withheld",
    }],
  };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(familyCatalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  expect(host.textContent).toContain("matches at least 120 existing ledgers and tells them apart from none of them, so none is listed");
  expect(host.textContent).not.toContain("0 possible");
  expect(host.textContent).not.toContain("listed first;");
  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  // No misleading "Possible" heading over an empty group, and the full list stays.
  const groups = Array.from(target.querySelectorAll("optgroup")).map((group) => group.label);
  expect(groups).toEqual(["Existing ledgers"]);
  expect(Array.from(target.querySelectorAll("option")).map((option) => option.value))
    .toEqual(["", "DN Party 001", "DN Party 002", "DN Party 003"]);
  root.unmount();
});

test("a capture whose narrowing could not run says so rather than looking unnarrowed", async () => {
  const unavailable = { ...catalog, bindings: [], bindings_state: "unavailable" as const };
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(unavailable);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  expect(host.textContent).toContain("could not narrow this source's lines, so every row lists the full catalogue");
  root.unmount();
});

test("a capture without bindings still renders the whole catalogue and claims nothing", async () => {
  // An older capture, or one the backend could not narrow, must not lose the
  // list the operator came for.
  mocks.invoke.mockResolvedValueOnce(draft).mockResolvedValueOnce(catalog);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  const groups = Array.from(target.querySelectorAll("optgroup")).map((group) => group.label);
  expect(groups).toEqual(["Existing ledgers"]);
  expect(Array.from(target.querySelectorAll("option")).map((option) => option.value)).toEqual(["", "Existing target"]);
  expect(host.textContent).not.toContain("listed first");
  root.unmount();
});

test("shows a Tally-changed warning instead of a current-session label when retained target revalidation fails", async () => {
  const twoTargetCatalog = { ...catalog, targets: ["Target A", "Target B"] };
  const boundA = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Target A" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  const boundBOnly = {
    ...boundA,
    revision: 3,
    rows: boundA.rows.map((item, index) => index === 1 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Target B" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 2, entry_position: 1 }],
  };
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(twoTargetCatalog)
    .mockResolvedValueOnce(boundA)
    .mockResolvedValueOnce(boundBOnly);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Target A"));
  expect(host.textContent).toContain("This current-session target was re-read and bound.");

  await act(async () => button(host, "#2").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-2-entry-0-ledger")!, "Target B"));
  expect(host.textContent).toContain("This current-session target was re-read and bound.");

  await act(async () => button(host, "#1").click());
  expect(host.textContent).toContain("Tally changed after this target was bound. Saved unverified target: Target A.");
  expect(host.textContent).not.toContain("This current-session target was re-read and bound.");
  root.unmount();
});

test("a refused apply clears a current-session label the same read disproved", async () => {
  const twoTargetCatalog = { ...catalog, targets: ["Target A", "Target B"] };
  const boundA = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Target A" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  // Tally renamed Target A, so selecting it again is refused. That refusal was
  // decided by a fresh read, which also disproves row 1 -- and says so.
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(twoTargetCatalog)
    .mockResolvedValueOnce(boundA)
    .mockRejectedValueOnce({
      code: "source_draft_catalogue_target_changed",
      message: "The selected existing ledger changed before Bridge could apply it.",
      remediation: "Load existing ledgers again and make a fresh selection.",
      current_catalog_bindings: [],
    });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Target A"));
  expect(host.textContent).toContain("This current-session target was re-read and bound.");

  await act(async () => button(host, "#2").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-2-entry-0-ledger")!, "Target A"));
  expect(host.textContent).toContain("The selected existing ledger changed before Bridge could apply it.");

  await act(async () => button(host, "#1").click());
  expect(host.textContent).not.toContain("This current-session target was re-read and bound.");
  expect(host.textContent).toContain("Tally changed after this target was bound. Saved unverified target: Target A.");
  root.unmount();
});

test("an unrelated apply failure carries no binding evidence and leaves the label alone", async () => {
  const twoTargetCatalog = { ...catalog, targets: ["Target A", "Target B"] };
  const boundA = {
    ...draft,
    revision: 2,
    rows: draft.rows.map((item, index) => index === 0 ? {
      ...item,
      proposal: { ...item.proposal, entries: [{ ...item.proposal.entries[0], ledger: "Target A" }] },
    } : item),
    current_catalog_bindings: [{ row_position: 1, entry_position: 1 }],
  };
  // A transport failure proves nothing about any binding, so it must not be
  // read as "the read disproved them all".
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(twoTargetCatalog)
    .mockResolvedValueOnce(boundA)
    .mockRejectedValueOnce({
      code: "source_draft_catalogue_unreachable",
      message: "Bridge could not reach Tally.",
      remediation: "Check Tally and retry.",
    });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Target A"));
  await act(async () => button(host, "#2").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-2-entry-0-ledger")!, "Target B"));
  expect(host.textContent).toContain("Bridge could not reach Tally.");

  await act(async () => button(host, "#1").click());
  expect(host.textContent).toContain("This current-session target was re-read and bound.");
  root.unmount();
});

test("renders unusual ledger spaces visibly while binding the exact selected catalog target", async () => {
  const whitespaceCatalog = { ...catalog, targets: ["Cash", " Cash "] };
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockResolvedValueOnce(whitespaceCatalog)
    .mockResolvedValueOnce({ ...draft, revision: 2 });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());

  const target = host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!;
  const options = [...target.options];
  expect(options.find((option) => option.value === "Cash")?.text).toBe("Cash");
  expect(options.find((option) => option.value === " Cash ")?.text).toBe("␠Cash␠");
  expect(options.find((option) => option.value === "Cash")?.text).not.toBe(options.find((option) => option.value === " Cash ")?.text);

  await act(async () => setValue(target, " Cash "));
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_apply_source_draft_existing_ledger_target", {
    request: expect.objectContaining({ target_name: " Cash " }),
  });
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
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });
  expect(button(host, "Load existing ledgers").disabled).toBe(true);
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')).toBeTruthy();

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-three" />));
  expect(mocks.invoke).toHaveBeenCalledTimes(3);
  resolveFirstInvalidation();
  await act(async () => { await firstInvalidation; });
  expect(mocks.invoke).toHaveBeenNthCalledWith(4, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });
  expect(button(host, "Load existing ledgers").disabled).toBe(true);

  resolveSecondInvalidation();
  await act(async () => { await secondInvalidation; });
  expect(button(host, "Load existing ledgers").disabled).toBe(false);
  root.unmount();
});

test("names a later invalidation with the generation the previous one returned, not the stale value it was queued with", async () => {
  let resolveFirstInvalidation!: (generation: number) => void;
  const firstInvalidation = new Promise<number>((resolve) => { resolveFirstInvalidation = resolve; });
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockReturnValueOnce(firstInvalidation)
    .mockResolvedValueOnce(8);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-two" />));
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });

  // Resolved and settled here, before a further scope change queues the next
  // invalidation, so the returned generation has already been folded back
  // into draft state by the time it is captured below.
  resolveFirstInvalidation(7);
  await act(async () => { await firstInvalidation; });

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-three" />));
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: 7 },
  });
  root.unmount();
});

test("does not let a save's stale generation regress one an invalidation already advanced, so the next invalidation still names the advanced value", async () => {
  let resolveSave!: (value: unknown) => void;
  let resolveInvalidation!: (generation: number) => void;
  const pendingSave = new Promise((resolve) => { resolveSave = resolve; });
  const pendingInvalidation = new Promise<number>((resolve) => { resolveInvalidation = resolve; });
  mocks.invoke
    .mockResolvedValueOnce(draft)
    .mockReturnValueOnce(pendingSave)
    .mockReturnValueOnce(pendingInvalidation)
    .mockResolvedValueOnce(99);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());

  // Issued first, but its DTO is left pending -- it will not resolve until
  // after the invalidation below has already advanced the generation.
  await act(async () => button(host, "Save draft").click());

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-two" />));
  expect(mocks.invoke).toHaveBeenNthCalledWith(3, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });
  resolveInvalidation(5);
  await act(async () => { await pendingInvalidation; });

  // The save command started before the invalidation and names the
  // generation the draft had when it was issued -- older than the 5 the
  // invalidation just folded in. Resolving it now must not drag the token
  // back down to that stale value.
  resolveSave({ ...draft, revision: 2, catalog_generation: draft.catalog_generation });
  await act(async () => { await pendingSave; });

  await act(async () => root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-three" />));
  expect(mocks.invoke).toHaveBeenNthCalledWith(4, "desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: 5 },
  });
  root.unmount();
});

test("returns the editor and ledger read control to the current scope after native invalidation rejects", async () => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_invalidate_source_draft_existing_ledger_targets") {
      return Promise.reject(new Error("native invalidation unavailable"));
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mount(host, { catalogScope, catalogScopeKey: "company-one" });
  await act(async () => button(host, "Choose source XML").click());

  await act(async () => {
    root.render(<SourceDraftScreen catalogScope={catalogScope} catalogScopeKey="company-two" />);
    await Promise.resolve();
  });

  expect(host.textContent).toContain("native invalidation unavailable");
  expect(button(host, "Load existing ledgers").disabled).toBe(false);
  expect(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')?.disabled).toBe(false);
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
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_invalidate_source_draft_existing_ledger_targets", {
    request: { draft_id: draft.draft_id, generation: draft.catalog_generation },
  });

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
  await act(async () => button(document.body, "Keep editing").click());
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

function ProtectedSourceShell({
  crashAfterDirty = false,
  crashAfterBusy = false,
  showJournal = false,
  initialSourceBusy = false,
  catalogScope: protectedCatalogScope,
  catalogScopeKey,
  authorizedReload = false,
}: {
  crashAfterDirty?: boolean;
  crashAfterBusy?: boolean;
  showJournal?: boolean;
  initialSourceBusy?: boolean;
  catalogScope?: React.ComponentProps<typeof SourceDraftScreen>["catalogScope"];
  catalogScopeKey?: string;
  authorizedReload?: boolean;
}) {
  const sourceDirtyRef = React.useRef(false);
  const sourceActionBusyRef = React.useRef(initialSourceBusy);
  const journalActionBusyRef = React.useRef(false);
  const lifecyclePendingRef = React.useRef(false);
  const authorizedReloadRef = React.useRef(authorizedReload);
  authorizedReloadRef.current = authorizedReload;
  const [ready, setReady] = React.useState(false);
  const [protectionError, setProtectionError] = React.useState<string | null>(null);
  const [lifecycleOpen, setLifecycleOpen] = React.useState(false);
  const [crash, setCrash] = React.useState(false);
  const [restoreFocus, setRestoreFocus] = React.useState<(() => void) | null>(null);
  const [, setBusyRevision] = React.useState(0);
  const onProtectionChange = React.useCallback((next: boolean, error: string | null) => {
    setReady(next);
    setProtectionError(error);
  }, []);
  const onDirtyChange = React.useCallback((next: boolean) => {
    sourceDirtyRef.current = next;
    if (next && crashAfterDirty) setCrash(true);
  }, [crashAfterDirty]);
  const onBusyChange = React.useCallback((next: boolean) => {
    sourceActionBusyRef.current = next;
    setBusyRevision((value) => value + 1);
    if (next && crashAfterBusy) setCrash(true);
  }, [crashAfterBusy]);
  const restore = React.useCallback((next: () => void) => setRestoreFocus(() => next), []);
  React.useEffect(() => {
    if (lifecycleOpen || !restoreFocus) return;
    restoreFocus();
    setRestoreFocus(null);
  }, [lifecycleOpen, restoreFocus]);
  return (
    <>
      <NativeLifecycleController
        sourceDraftDirtyRef={sourceDirtyRef}
        sourceDraftActionBusyRef={sourceActionBusyRef}
        journalActionBusyRef={journalActionBusyRef}
        lifecyclePendingRef={lifecyclePendingRef}
        authorizedReloadRef={authorizedReloadRef}
        onProtectionChange={onProtectionChange}
        onModalChange={setLifecycleOpen}
        onModalClosed={restore}
      />
      <div data-testid="lifecycle-shell" inert={lifecycleOpen || undefined}>
        {showJournal && (
          <JournalPostingScreen
            config={journalConfig}
            lifecycleAdmissionReady={ready}
            lifecycleInteractionBlocked={lifecycleOpen}
            isLifecycleInteractionBlocked={() => lifecyclePendingRef.current}
            lifecycleAdmissionError={protectionError}
            onBusyChange={(next) => { journalActionBusyRef.current = next; }}
          />
        )}
        <div hidden={showJournal}>
          <ErrorBoundary label="Prepare file">
            {crash ? <ThrowingSourceDraft /> : (
              <SourceDraftScreen
                onBusyChange={onBusyChange}
                onDirtyChange={onDirtyChange}
                editingEnabled={ready}
                lifecycleInteractionBlocked={lifecycleOpen}
                isLifecycleInteractionBlocked={() => lifecyclePendingRef.current}
                protectionError={protectionError}
                catalogScope={protectedCatalogScope}
                catalogScopeKey={catalogScopeKey}
              />
            )}
          </ErrorBoundary>
        </div>
      </div>
      {createPortal(
        <aside data-testid="evidence-drawer" inert={lifecycleOpen || undefined} aria-hidden={lifecycleOpen || undefined}>
          Evidence drawer
        </aside>,
        document.body,
      )}
    </>
  );
}

function ThrowingSourceDraft(): never {
  throw new Error("synthetic source screen failure");
}

async function mountProtected(host: HTMLElement, options: { crashAfterDirty?: boolean; crashAfterBusy?: boolean; showJournal?: boolean; initialSourceBusy?: boolean; catalogScope?: React.ComponentProps<typeof SourceDraftScreen>["catalogScope"]; catalogScopeKey?: string; authorizedReload?: boolean } = {}) {
  const root = createRoot(host);
  await act(async () => root.render(<ProtectedSourceShell {...options} />));
  return root;
}

test("holds a native close through a pending catalog apply, then requires an explicit discard", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "close-during-catalog-apply", kind: "close" as const };
  let pending: typeof close | null = null;
  let resolveApply!: (value: typeof draft) => void;
  const pendingApply = new Promise<typeof draft>((resolve) => { resolveApply = resolve; });
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
  const root = await mountProtected(host, {
    crashAfterDirty: true,
    catalogScope,
    catalogScopeKey: "company-one",
  });
  await act(async () => button(host, "Choose source XML").click());
  await act(async () => button(host, "Load existing ledgers").click());
  await act(async () => setValue(host.querySelector<HTMLSelectElement>("#source-draft-1-entry-0-ledger")!, "Existing target"));

  pending = close;
  await act(async () => listener?.({ payload: close }));
  expect(document.body.textContent).toContain("A local source-draft action is in progress.");
  expect(button(document.body, "Discard and close").disabled).toBe(true);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });

  resolveApply({ ...draft, revision: 2 });
  await act(async () => { await pendingApply; });
  expect(host.textContent).toContain("Prepare file hit a problem");
  expect(button(document.body, "Discard and close").disabled).toBe(false);
  await act(async () => button(document.body, "Discard and close").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });
  root.unmount();
});

test("keeps a dirty lifecycle request local through the stable controller", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "close-1", kind: "close" as const };
  let pending: typeof close | null = null;
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
  const root = await mountProtected(host);
  await act(async () => {});
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_register_source_draft_lifecycle_renderer", expect.objectContaining({ token: expect.any(String) }));
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  pending = close;
  await act(async () => listener?.({ payload: close }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and close this window?");
  expect(button(document.body, "Keep editing").disabled).toBe(false);
  await act(async () => button(document.body, "Keep editing").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_cancel_source_draft_lifecycle_request", { request: close });
  root.unmount();
});

test("disables source and Journal admission when listener registration fails before readiness", async () => {
  enableNativeWindowRuntime();
  mocks.listen.mockRejectedValueOnce(new Error("event permission denied"));
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => {});
  expect(host.textContent).toContain("Bridge could not install native close protection: event permission denied");
  expect(button(host, "Choose source XML").disabled).toBe(true);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_register_source_draft_lifecycle_renderer", expect.anything());
  root.unmount();
});

test("uses the controller after the source screen ErrorBoundary unmounts", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "exit-after-source-error", kind: "exit" as const };
  let pending: typeof exit | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") return Promise.resolve();
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host, { crashAfterDirty: true });
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  expect(host.textContent).toContain("Prepare file hit a problem");

  pending = exit;
  await act(async () => listener?.({ payload: exit }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: exit });
  await act(async () => button(document.body, "Keep editing").click());
  root.unmount();
});

test("keeps the reload guard after a dirty source screen render failure and leaves clean reloads alone", async () => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const dirtyHost = document.createElement("div");
  document.body.append(dirtyHost);
  const dirtyRoot = await mountProtected(dirtyHost, { crashAfterDirty: true });
  await act(async () => button(dirtyHost, "Choose source XML").click());
  setValue(dirtyHost.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  expect(dirtyHost.textContent).toContain("Prepare file hit a problem");
  const dirtyReload = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(dirtyReload);
  expect(dirtyReload.defaultPrevented).toBe(true);
  dirtyRoot.unmount();

  const cleanHost = document.createElement("div");
  document.body.append(cleanHost);
  const cleanRoot = await mountProtected(cleanHost);
  const cleanReload = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(cleanReload);
  expect(cleanReload.defaultPrevented).toBe(false);
  cleanRoot.unmount();
});

test("consumes an authorized reload without clearing the dirty source-draft signal", async () => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host, { authorizedReload: true });
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");
  const reload = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(reload);
  expect(reload.defaultPrevented).toBe(false);
  const laterReload = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(laterReload);
  expect(laterReload.defaultPrevented).toBe(true);
  root.unmount();
});

test("uses the fallback Reload button to confirm dirty proposals and block active lifecycle or Journal work", async () => {
  const renderFallback = async (guard: ReloadGuard, onReload: () => void) => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(
      <ReloadGuardContext.Provider value={guard}>
        <ErrorBoundary label="Prepare file" onReload={onReload}>
          <ThrowingSourceDraft />
        </ErrorBoundary>
      </ReloadGuardContext.Provider>,
    ));
    return { host, root };
  };
  const ref = <T,>(value: T) => ({ current: value }) as React.MutableRefObject<T>;
  const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
  try {
    let dirtyChecks = 0;
    let resolveDirtyConfirmation!: (value: boolean) => void;
    const dirtyGuard: ReloadGuard = {
      sourceDraftDirtyRef: ref(true),
      sourceDraftActionBusyRef: ref(false),
      journalActionBusyRef: ref(false),
      lifecyclePendingRef: ref(false),
      reloadAdmissionRef: ref<string | null>(null),
      authorizedReloadRef: ref(false),
      inspectNativeLifecyclePending: () => {
        dirtyChecks += 1;
        return dirtyChecks <= 2 ? Promise.resolve(false) : new Promise((resolve) => { resolveDirtyConfirmation = resolve; });
      },
    };
    let dirtyReloads = 0;
    let dirtyReloadAuthorized = false;
    const dirty = await renderFallback(dirtyGuard, () => {
      dirtyReloadAuthorized = dirtyGuard.authorizedReloadRef.current;
      dirtyReloads += 1;
    });
    await act(async () => button(dirty.host, "Reload").click());
    expect(dirty.host.textContent).toContain("Discard unsaved proposals and reload?");
    await act(async () => button(dirty.host, "Keep current window").click());
    expect(dirtyReloads).toBe(0);
    expect(dirtyGuard.sourceDraftDirtyRef.current).toBe(true);
    await act(async () => button(dirty.host, "Reload").click());
    expect(dirty.host.textContent).toContain("Discard unsaved proposals and reload?");
    await act(async () => {
      button(dirty.host, "Reload and discard").click();
      await Promise.resolve();
    });
    expect(button(dirty.host, "Keep current window").disabled).toBe(true);
    await act(async () => resolveDirtyConfirmation(false));
    expect(dirtyReloads).toBe(1);
    expect(dirtyReloadAuthorized).toBe(true);
    expect(dirtyGuard.authorizedReloadRef.current).toBe(false);
    dirty.root.unmount();

    const blockedGuard: ReloadGuard = {
      sourceDraftDirtyRef: ref(false),
      sourceDraftActionBusyRef: ref(false),
      journalActionBusyRef: ref(false),
      lifecyclePendingRef: ref(true),
      reloadAdmissionRef: ref<string | null>(null),
      authorizedReloadRef: ref(false),
      inspectNativeLifecyclePending: async () => false,
    };
    let blockedReloads = 0;
    const blocked = await renderFallback(blockedGuard, () => { blockedReloads += 1; });
    await act(async () => button(blocked.host, "Reload").click());
    expect(blocked.host.textContent).toContain("native close request is still pending");
    blockedGuard.lifecyclePendingRef.current = false;
    blockedGuard.journalActionBusyRef.current = true;
    await act(async () => button(blocked.host, "Reload").click());
    expect(blocked.host.textContent).toContain("Journal action is still in progress");
    blockedGuard.journalActionBusyRef.current = false;
    blockedGuard.sourceDraftActionBusyRef.current = true;
    await act(async () => button(blocked.host, "Reload").click());
    expect(blocked.host.textContent).toContain("source-draft action is still in progress");
    expect(blockedReloads).toBe(0);
    blocked.root.unmount();

    const cleanGuard: ReloadGuard = {
      sourceDraftDirtyRef: ref(false),
      sourceDraftActionBusyRef: ref(false),
      journalActionBusyRef: ref(false),
      lifecyclePendingRef: ref(false),
      reloadAdmissionRef: ref<string | null>(null),
      authorizedReloadRef: ref(false),
      inspectNativeLifecyclePending: async () => false,
    };
    let cleanReloads = 0;
    const clean = await renderFallback(cleanGuard, () => { cleanReloads += 1; });
    await act(async () => button(clean.host, "Reload").click());
    expect(cleanReloads).toBe(1);
    clean.root.unmount();

    let resolveNativeCheck!: (value: boolean) => void;
    const raceGuard: ReloadGuard = {
      sourceDraftDirtyRef: ref(false),
      sourceDraftActionBusyRef: ref(false),
      journalActionBusyRef: ref(false),
      lifecyclePendingRef: ref(false),
      reloadAdmissionRef: ref<string | null>(null),
      authorizedReloadRef: ref(false),
      inspectNativeLifecyclePending: () => new Promise((resolve) => { resolveNativeCheck = resolve; }),
    };
    let raceReloads = 0;
    const race = await renderFallback(raceGuard, () => { raceReloads += 1; });
    await act(async () => button(race.host, "Reload").click());
    expect(raceGuard.reloadAdmissionRef.current).not.toBeNull();
    raceGuard.lifecyclePendingRef.current = true;
    await act(async () => resolveNativeCheck(false));
    expect(race.host.textContent).toContain("native close request is still pending");
    expect(raceGuard.reloadAdmissionRef.current).toBeNull();
    expect(raceReloads).toBe(0);
    race.root.unmount();

    const deferredResolvers: Array<(value: boolean) => void> = [];
    const ownerGuard: ReloadGuard = {
      sourceDraftDirtyRef: ref(false),
      sourceDraftActionBusyRef: ref(false),
      journalActionBusyRef: ref(false),
      lifecyclePendingRef: ref(false),
      reloadAdmissionRef: ref<string | null>(null),
      authorizedReloadRef: ref(false),
      inspectNativeLifecyclePending: () => new Promise((resolve) => { deferredResolvers.push(resolve); }),
    };
    const firstBoundary = await renderFallback(ownerGuard, () => undefined);
    await act(async () => button(firstBoundary.host, "Reload").click());
    const firstToken = ownerGuard.reloadAdmissionRef.current;
    expect(firstToken).not.toBeNull();
    firstBoundary.root.unmount();
    expect(ownerGuard.reloadAdmissionRef.current).toBeNull();

    const secondBoundary = await renderFallback(ownerGuard, () => undefined);
    await act(async () => button(secondBoundary.host, "Reload").click());
    const secondToken = ownerGuard.reloadAdmissionRef.current;
    expect(secondToken).not.toBeNull();
    expect(secondToken).not.toBe(firstToken);
    await act(async () => deferredResolvers[0](false));
    expect(ownerGuard.reloadAdmissionRef.current).toBe(secondToken);
    secondBoundary.root.unmount();
  } finally {
    consoleError.mockRestore();
  }
});

test("does not let a late failed request lookup replace a newer dialog", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const first = { request_id: "close-a", kind: "close" as const };
  const second = { request_id: "exit-b", kind: "exit" as const };
  let pending: typeof first | typeof second | null = null;
  let rejectFirst!: (error: Error) => void;
  let deferFirst = false;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") {
      if (deferFirst) return new Promise((_resolve, reject) => { rejectFirst = reject; });
      return Promise.resolve(pending);
    }
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  deferFirst = true;
  listener?.({ payload: first });
  await act(async () => {});
  pending = second;
  deferFirst = false;
  await act(async () => listener?.({ payload: second }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  await act(async () => rejectFirst(new Error("late lookup failure")));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  root.unmount();
});

test("unregisters a resolved native registration with its exact token", async () => {
  enableNativeWindowRuntime();
  mocks.listen.mockResolvedValueOnce(mocks.unlisten);
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => {});
  const register = mocks.invoke.mock.calls.find(([command]) => command === "desktop_register_source_draft_lifecycle_renderer");
  expect(register).toBeTruthy();
  root.unmount();
  expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_unregister_source_draft_lifecycle_renderer", register?.[1]);
});



test("retains a source action busy admission after its ErrorBoundary unmounts", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "close-during-source-action", kind: "close" as const };
  let pending: typeof close | null = null;
  let resolvePick!: (value: typeof draft) => void;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return new Promise(resolve => { resolvePick = resolve; });
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host, { crashAfterBusy: true });
  await act(async () => button(host, "Choose source XML").click());
  expect(host.textContent).toContain("Prepare file hit a problem");

  pending = close;
  await act(async () => listener?.({ payload: close }));
  expect(document.body.textContent).toContain("A local source-draft action is in progress.");
  expect(button(document.body, "Discard and close").disabled).toBe(true);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });

  await act(async () => resolvePick(draft));
  root.unmount();
});


test("does not reopen a cancelled request from a duplicate event", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "close-cancel-duplicate", kind: "close" as const };
  let pending: typeof close | null = null;
  let resolveCancel!: () => void;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      return new Promise<void>((resolve) => { resolveCancel = resolve; });
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  pending = close;
  await act(async () => listener?.({ payload: close }));
  await act(async () => button(document.body, "Keep editing").click());
  listener?.({ payload: close });
  await act(async () => resolveCancel());
  expect(document.body.textContent).not.toContain("Discard unsaved proposals and close this window?");
  const pendingLookups = mocks.invoke.mock.calls.filter(([command]) => command === "desktop_pending_source_draft_lifecycle_request");
  expect(pendingLookups).toHaveLength(2);
  root.unmount();
});

test("blocks source admission until a deferred native close lookup proves no request is pending", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  let deferLookup = false;
  let resolveLookup!: (value: null) => void;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") {
      if (deferLookup) return new Promise<null>((resolve) => { resolveLookup = resolve; });
      return Promise.resolve(null);
    }
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  deferLookup = true;
  listener?.({ payload: { request_id: "close-before-lookup", kind: "close" } });
  await act(async () => {});
  await act(async () => button(host, "Choose source XML").click());
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_pick_source_draft");

  await act(async () => resolveLookup(null));
  await act(async () => button(host, "Choose source XML").click());
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_pick_source_draft");
  root.unmount();
});

test("promotes a newer native request returned by a deferred lookup instead of unlocking local admission", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  let deferLookup = false;
  let resolveLookup!: (value: { request_id: string; kind: "close" | "exit" }) => void;
  let pending: { request_id: string; kind: "close" | "exit" } | null = null;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") {
      if (deferLookup) {
        deferLookup = false;
        return new Promise((resolve) => { resolveLookup = resolve; });
      }
      return Promise.resolve(pending);
    }
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  deferLookup = true;
  listener?.({ payload: { request_id: "delayed-a", kind: "close" } });
  const newer = { request_id: "authoritative-b", kind: "exit" as const };
  pending = newer;
  await act(async () => resolveLookup(newer));
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: newer });
  root.unmount();
});

test("re-queries a newer native request received while cancelling the current one", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  let pending: { request_id: string; kind: "close" | "exit" } | null = null;
  let resolveCancel!: () => void;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      return new Promise<void>((resolve) => { resolveCancel = resolve; });
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  const first = { request_id: "cancel-a", kind: "close" as const };
  pending = first;
  await act(async () => listener?.({ payload: first }));
  await act(async () => button(document.body, "Keep editing").click());

  const second = { request_id: "cancel-b", kind: "exit" as const };
  pending = second;
  listener?.({ payload: second });
  await act(async () => resolveCancel());
  expect(document.body.textContent).toContain("Discard unsaved proposals and quit Bridge?");
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: second });
  root.unmount();
});

test("keeps the mounted Journal outcome and blocks lifecycle completion while posting", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const exit = { request_id: "exit-during-journal", kind: "exit" as const };
  let pending: typeof exit | null = null;
  let resolvePost!: (value: unknown) => void;
  const pendingPost = new Promise((resolve) => { resolvePost = resolve; });
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_journal_for_review") return Promise.resolve(journalReview);
    if (command === "desktop_post_reviewed_journal") return pendingPost;
    if (command === "desktop_cancel_source_draft_lifecycle_request") {
      pending = null;
      return Promise.resolve();
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host, { showJournal: true });
  await act(async () => button(host, "Choose Journal file").click());
  await act(async () => {
    button(host, "Post Journal").click();
    await Promise.resolve();
  });

  pending = exit;
  await act(async () => listener?.({ payload: exit }));
  expect(host.textContent).toContain("Review a Bridge Journal");
  expect(document.body.textContent).toContain("A Journal action is in progress.");
  expect(button(document.body, "Discard and quit").disabled).toBe(true);
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: exit });

  await act(async () => button(document.body, "Keep editing").click());
  await act(async () => resolvePost({
    batchId: journalReview.batchId,
    result: { result: { dispatch: { state: "posted_verified", resent: false } } },
  }));
  expect(host.textContent).toContain("Bridge confirmed the original Journal and its saved batch.");
  root.unmount();
});

test("blocks a clean close synchronously before its native completion resolves", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const close = { request_id: "clean-close", kind: "close" as const };
  let pending: typeof close | null = null;
  let resolveComplete!: () => void;
  let ledger: HTMLInputElement | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    if (command === "desktop_complete_source_draft_lifecycle_request") {
      setValue(ledger!, "Late proposal");
      return new Promise<void>((resolve) => { resolveComplete = resolve; });
    }
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  ledger = host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!;

  pending = close;
  await act(async () => {
    listener?.({ payload: close });
    await Promise.resolve();
  });
  expect(ledger.disabled).toBe(true);
  expect(ledger.value).toBe("");
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: close });

  await act(async () => resolveComplete());
  root.unmount();
});

test("uses the authoritative native pending request when an event payload is stale", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
    return mocks.unlisten;
  });
  const current = { request_id: "close-current", kind: "close" as const };
  let pending: typeof current | null = null;
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    if (command === "desktop_pick_source_draft") return Promise.resolve(draft);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => button(host, "Choose source XML").click());
  setValue(host.querySelector<HTMLInputElement>('input[placeholder="Unverified ledger name"]')!, "Unsaved ledger");

  pending = current;
  await act(async () => listener?.({ payload: { request_id: "close-stale", kind: "close" } }));
  expect(document.body.textContent).toContain("Discard unsaved proposals and close this window?");
  expect(mocks.invoke).not.toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: current });
  root.unmount();
});

test("recovers a request pending before listener registration", async () => {
  enableNativeWindowRuntime();
  mocks.listen.mockResolvedValueOnce(mocks.unlisten);
  const pending = { request_id: "pending-at-registration", kind: "exit" as const };
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "desktop_pending_source_draft_lifecycle_request") return Promise.resolve(pending);
    return Promise.resolve();
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host);
  await act(async () => {});
  expect(mocks.invoke).toHaveBeenCalledWith("desktop_complete_source_draft_lifecycle_request", { request: pending });
  root.unmount();
});

test("restores the opener after lifecycle inertness clears", async () => {
  enableNativeWindowRuntime();
  let listener: ((event: { payload: { request_id: string; kind: "close" | "exit" } }) => void) | undefined;
  mocks.listen.mockImplementation(async (_event, handler) => {
    listener = handler;
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
  const host = document.createElement("div");
  document.body.append(host);
  const root = await mountProtected(host, { initialSourceBusy: true });
  const opener = button(host, "Choose source XML");
  opener.focus();
  pending = exit;
  await act(async () => listener?.({ payload: exit }));
  const shell = host.querySelector<HTMLElement>("[data-testid=lifecycle-shell]")!;
  const drawer = document.body.querySelector<HTMLElement>("[data-testid=evidence-drawer]")!;
  expect(shell.hasAttribute("inert")).toBe(true);
  expect(drawer.hasAttribute("inert")).toBe(true);
  expect(drawer.getAttribute("aria-hidden")).toBe("true");
  expect(document.activeElement).toBe(document.body.querySelector("[role=alertdialog]"));

  await act(async () => button(document.body, "Keep editing").click());
  expect(shell.hasAttribute("inert")).toBe(false);
  expect(drawer.hasAttribute("inert")).toBe(false);
  expect(drawer.getAttribute("aria-hidden")).not.toBe("true");
  expect(document.activeElement).toBe(opener);
  root.unmount();
});
