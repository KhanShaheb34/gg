// Runs once before any test worker starts (playwright.config.ts `globalSetup`).
import { buildDevserver } from "./fixtures";

export default async function globalSetup(): Promise<void> {
  // Builds and warms gg-devserver here rather than in each worker, so no cargo build (or
  // first start of a freshly linked binary) overlaps a test's start-up time limit.
  await buildDevserver();
}
