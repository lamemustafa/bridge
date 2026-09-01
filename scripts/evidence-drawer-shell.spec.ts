import { expect, test } from "@playwright/test";

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
