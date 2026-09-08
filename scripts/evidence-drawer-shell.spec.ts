import { expect, test, type Page } from "@playwright/test";

const savedCompany = {
  name: "Focus Test Company",
  guid: "01234567-89ab-cdef-0123-456789abcdef",
  company_number: "1",
  books_from_yyyymmdd: "20240401",
  guid_observed: true,
  mirror_company_id: "focus-test-company",
  canonical_endpoint: "http://127.0.0.1:9000",
};

test("the mounted shell makes its background inert while the evidence drawer stays interactive", async ({ page }) => {
  await page.addInitScript((company) => {
    window.__TAURI_INTERNALS__ = {
      invoke: async (command: string) => {
        if (command === "tally_persisted_company_profiles") {
          return { profiles: [company], total_profiles: 1, limit: 100, truncated: false };
        }
        if (command === "tally_runtime_snapshots" || command === "tally_recent_snapshot_runs") return [];
        if (command === "tally_write_fixture_enrollment_status") {
          return { fixture_state: "not_enrolled", candidate_gate: "not_enrolled", write_capability: "unknown" };
        }
        return [];
      },
    } as typeof window.__TAURI_INTERNALS__;
  }, savedCompany);

  await page.goto("/");
  await page.getByRole("button", { name: "Switch client" }).click();
  await page.getByRole("button", { name: /Focus Test Company/ }).click();

  const opener = page.getByRole("button", { name: "Open local evidence" });
  await expect(opener).toBeVisible();
  await opener.click();

  const shell = page.locator(".shell");
  const dialog = page.getByRole("dialog", { name: "Local evidence and limits" });
  const close = dialog.getByRole("button", { name: "Close" });
  await expect(shell).toHaveAttribute("inert", "");
  await expect(shell).toHaveAttribute("aria-hidden", "true");
  await expect(dialog).toBeVisible();
  await expect(dialog).not.toHaveAttribute("inert");
  await expect(close).toBeEnabled();
  await expect(close).toBeFocused();

  await close.click();
  await expect(dialog).toHaveCount(0);
  await expect(shell).not.toHaveAttribute("inert");
  await expect(shell).not.toHaveAttribute("aria-hidden");
  await expect(opener).toBeFocused();
});

// These are UI control-state responses; no Tally transport or protocol fixture is involved.
async function openLostResume(page: Page, discoverWorker: boolean) {
  await page.addInitScript(({ company, discoverWorker }) => {
    const run = {
      run_id: "interrupted-run", phase: "extract", requires_resume: true,
      resume_available: true, mirror_company_id: company.mirror_company_id,
      pack_id: "core_accounting", requested_from_yyyymmdd: "20260401",
      requested_to_yyyymmdd: "20260401", active_window_id: null,
      completed_windows: 0, total_windows: 1, verification: null,
      proof_id: null, proof_sha256: null, gap_codes: [], warning_codes: [], failure_code: null,
    };
    let resumed = false;
    const calls: { command: string; args: unknown }[] = [];
    Object.assign(window, { snapshotTestCalls: calls });
    window.__TAURI_INTERNALS__ = {
      invoke: async (command: string, args: unknown) => {
        calls.push({ command, args });
        if (command === "tally_persisted_company_profiles") {
          return { profiles: [company], total_profiles: 1, limit: 100, truncated: false };
        }
        if (command === "tally_recent_snapshot_runs") return resumed && !discoverWorker ? [] : [{ ...run }];
        if (command === "resume_tally_core_snapshot") {
          resumed = true;
          run.requires_resume = false;
          run.resume_available = false;
          throw new Error("Synthetic lost IPC reply");
        }
        if (command === "tally_snapshot_status") return { ...run };
        if (command === "cancel_tally_snapshot") {
          run.phase = "cancelled";
          return true;
        }
        if (command === "tally_write_fixture_enrollment_status") {
          return { fixture_state: "not_enrolled", candidate_gate: "not_enrolled", write_capability: "unknown" };
        }
        return [];
      },
    } as typeof window.__TAURI_INTERNALS__;
  }, { company: savedCompany, discoverWorker });
  await page.goto("/");
  await page.getByRole("button", { name: "Switch client" }).click();
  await page.getByRole("button", { name: /Focus Test Company/ }).click();
  await page.getByRole("button", { name: "Open local evidence" }).click();
  const dialog = page.getByRole("dialog", { name: "Local evidence and limits" });
  await dialog.getByRole("button", { name: "Inspect", exact: true }).click();
  await dialog.getByRole("button", { name: "Resume interrupted run" }).click();
  await expect(dialog.getByRole("button", { name: "Refresh and confirm no active run" })).toBeVisible();
  return dialog;
}

test("a lost resume reply reattaches polling and cancellation to its discovered worker", async ({ page }) => {
  const dialog = await openLostResume(page, true);
  await expect(dialog.getByRole("button", { name: "Cancel active run" })).toBeVisible();
  const commands = () => page.evaluate(() => (window as unknown as {
    snapshotTestCalls: { command: string; args: unknown }[];
  }).snapshotTestCalls);
  await expect.poll(async () => (await commands()).filter((call) => call.command === "tally_snapshot_status").length).toBeGreaterThan(0);
  await dialog.getByRole("button", { name: "Cancel active run" }).click();
  await expect.poll(async () => (await commands()).find((call) => call.command === "cancel_tally_snapshot")?.args).toEqual({ runId: "interrupted-run" });
  await expect(dialog.getByRole("button", { name: "Cancel active run" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("button", { name: "Review Journal file" })).toBeDisabled();
  await page.getByRole("button", { name: "Companies", exact: true }).click();
  await page.getByRole("button", { name: "Open local evidence" }).click();
  await dialog.getByRole("button", { name: "Refresh and confirm no active run" }).click();
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("button", { name: "Review Journal file" })).toBeEnabled();
});

test("an unresolved lost reply cannot unlock Settings or Journal posting by navigation", async ({ page }) => {
  const dialog = await openLostResume(page, false);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("button", { name: "Settings", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Review Journal file" })).toBeDisabled();
  await page.getByRole("button", { name: "Companies", exact: true }).click();
  await expect(page.getByRole("button", { name: "Open Settings", exact: true })).toBeDisabled();
  await page.getByRole("button", { name: "Open local evidence" }).click();
  await dialog.getByRole("button", { name: "Refresh and confirm no active run" }).click();
  await expect(dialog.getByRole("button", { name: "Refresh and confirm no active run" })).toBeVisible();
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("button", { name: "Settings", exact: true })).toBeDisabled();
});
