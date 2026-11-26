// Playwright config for the site's screenshots (scripts/shots.mjs). It runs a copy of the
// app's own e2e/screens.spec.ts, made by shots.mjs, against the same dev servers the e2e
// tests use: gg-devserver (built by e2e/global-setup.ts) and the Vite dev server.
import { defineConfig, devices } from "@playwright/test";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..", "..");
// GG_E2E_PORT: another port for the Vite dev server when 1420 is taken, as for the e2e tests.
const port = Number(process.env.GG_E2E_PORT ?? 1420);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: path.join(root, "site", ".shots", "spec"),
  outputDir: path.join(root, "site", ".shots", "test-results"),
  globalSetup: path.join(root, "e2e", "global-setup.ts"),
  workers: 2,
  timeout: 120_000,
  reporter: "list",
  use: { baseURL },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: port === 1420 ? "pnpm dev" : `pnpm dev --port ${port}`,
    cwd: root,
    url: baseURL,
    // Never another checkout's server: it could be serving other code.
    reuseExistingServer: false,
  },
});
