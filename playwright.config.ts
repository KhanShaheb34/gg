import { defineConfig, devices } from "@playwright/test";

// GG_E2E_PORT: another port for the page's vite server, when 1420 is taken by another checkout's.
const port = Number(process.env.GG_E2E_PORT ?? 1420);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "e2e",
  // Performance runs (e2e/perf) only with GG_PERF=1: they need large generated repositories
  // and a release build, and they measure rather than check behaviour.
  testIgnore: process.env.GG_PERF === "1" ? [] : ["perf/**"],
  // Builds gg-devserver once for the whole run (e2e/global-setup.ts).
  globalSetup: "./e2e/global-setup.ts",
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  // Each test runs its own gg-devserver and git; keep the load on the machine modest.
  workers: 2,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: port === 1420 ? "pnpm dev" : `pnpm dev --port ${port}`,
    url: baseURL,
    reuseExistingServer: !process.env.CI,
  },
});
