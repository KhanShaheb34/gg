import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Set by `tauri dev` when targeting a remote device; unset on desktop.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  // Keep Rust compiler errors visible when run under `tauri dev`.
  clearScreen: false,
  server: {
    // Tauri's devUrl and Playwright's webServer both expect exactly this port.
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
    // The page's console stays in the browser: e2e tests stop their devserver while rows
    // may still be loading, and those expected warnings would otherwise fill the test output.
    forwardConsole: false,
  },
  test: {
    // Unit tests only; e2e/*.spec.ts belongs to Playwright.
    include: ["src/**/*.test.ts"],
    // Dates are formatted in local time; pin it so the expected strings hold everywhere.
    env: { TZ: "UTC" },
  },
});
