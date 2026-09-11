import { expect, test } from "@playwright/test";

// UI control-state evidence only. These responses are not Tally/source parser fixtures.
test("local proposals survive shell navigation and save without source or Tally authority", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const draft = {
      draft_id: "11111111-1111-4111-8111-111111111111", revision: 1,
      source_filename: "review-source.xml", source_sha256: "a".repeat(64),
      source_notices: [{ kind: "Ignored TALLYMESSAGE/LEDGER", count: 2 }],
      rows: [{ position: 1, source_remote_id: "source-reference", source_date: "20260401", source_voucher_type: "Receipt", source_narration: null, source_omitted_fields: ["VOUCHER/VOUCHERNUMBER"],
        entries: [{ position: 1, source_ledger: "Source ledger", source_amount: "-25.00", source_polarity: null }],
        proposal: { date: null, voucher_type: null, narration: null, notes: "", entries: [{ ledger: null, side: null, amount: null }] },
      }],
      // The backend always emits this list; an empty one means no target in this
      // draft is currently bound to a live catalog read.
      current_catalog_bindings: [],
    };
    const calls: { command: string; args: unknown }[] = [];
    Object.assign(window, { sourceDraftCalls: calls });
    window.__TAURI_INTERNALS__ = {
      invoke: async (command: string, args: unknown) => {
        calls.push({ command, args });
        if (command === "desktop_pick_source_draft") return structuredClone(draft);
        if (command === "desktop_save_source_draft") {
          const request = (args as { request: { proposals: typeof draft.rows[number]["proposal"][] } }).request;
          return { ...structuredClone(draft), revision: 2, rows: draft.rows.map((row, index) => ({ ...row, proposal: request.proposals[index] })) };
        }
        if (command === "tally_persisted_company_profiles") return { profiles: [], total_profiles: 0, limit: 100, truncated: false };
        if (command === "tally_write_fixture_enrollment_status") return { fixture_state: "not_enrolled", candidate_gate: "not_enrolled", write_capability: "unknown" };
        return [];
      },
    } as typeof window.__TAURI_INTERNALS__;
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Prepare file", exact: true }).click();
  await page.getByRole("button", { name: "Choose source XML", exact: true }).click();
  await page.getByLabel("Proposed date", { exact: true }).fill("2026-04-02");
  await page.getByLabel("Preparation notes", { exact: true }).fill("Keep this question for review");
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await page.getByRole("button", { name: "Investigate ledger", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Ledger entries", exact: true, level: 1 })).toBeVisible();
  await page.getByRole("button", { name: "Back to Overview", exact: true }).click();
  await page.getByRole("button", { name: "Trial Balance", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Trial Balance", exact: true, level: 1 })).toBeVisible();
  await page.getByRole("button", { name: "Prepare file", exact: true }).click();
  await expect(page.getByLabel("Proposed date", { exact: true })).toHaveValue("2026-04-02");
  await expect(page.getByLabel("Preparation notes", { exact: true })).toHaveValue("Keep this question for review");
  await page.getByRole("button", { name: "Save draft", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "Draft saved" })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { sourceDraftCalls: { command: string; args: unknown }[] }).sourceDraftCalls);
  const saved = calls.find((call) => call.command === "desktop_save_source_draft");
  expect(saved?.args).toEqual({ request: { draft_id: "11111111-1111-4111-8111-111111111111", revision: 1, proposals: [{ date: "20260402", voucher_type: null, narration: null, notes: "Keep this question for review", entries: [{ ledger: null, side: null, amount: null }] }] } });
  expect(calls.filter((call) => /^(fetch_tally_|fetch_selected_ledger_entries|fetch_standard_tally_ledger_catalog|desktop_post_|build_import|post_import)/.test(call.command))).toEqual([]);
  await page.getByLabel("Preparation notes", { exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("desktop.png"), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByLabel("Preparation notes", { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("narrow.png"), fullPage: true });
});
