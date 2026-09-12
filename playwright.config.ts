import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./scripts",
  testMatch: ["evidence-drawer-focus.spec.ts", "evidence-drawer-shell.spec.ts", "source-draft-shell.spec.ts"],
  webServer: {
    command: "corepack pnpm exec vite --host 127.0.0.1 --port 4173",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: !process.env.CI,
  },
  use: {
    baseURL: "http://127.0.0.1:4173",
    headless: true,
  },
  // Browser-engine coverage only; this does not exercise a packaged Tauri shell.
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
});
