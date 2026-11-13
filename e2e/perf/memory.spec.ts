// What the page keeps after scrolling a large repository top to bottom (plan 1, task 13), in
// Chromium against a release gg-devserver. Runs only with GG_PERF=1 (see playwright.config.ts):
//
//   GG_PERF=1 pnpm e2e e2e/perf/memory.spec.ts
//
// WebKit gives the page no way to measure its heap, so the JavaScript heap is measured here
// instead (Chrome DevTools Protocol), after a forced garbage collection: before the scroll,
// right after it, and after the 20 clicks of the click probe. A heap that keeps growing with
// the rows scrolled past would be a leak in the app; one that returns to its size before the
// scroll means the memory WebKit keeps afterwards is its allocator's, not the page's.
import { existsSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { startDevserver, stopDevserver } from "../fixtures";

const ROOT = path.resolve(import.meta.dirname, "../..");
const REPO = process.env.GG_PERF_R20K ?? path.join(homedir(), ".cache/gitgraph-perf/r20k");

test.skip(process.env.GG_PERF !== "1", "performance run: set GG_PERF=1");
test.use({ viewport: { width: 1280, height: 800 } });

test("the page's heap after scrolling r20k top to bottom", async ({ page }, info) => {
  test.setTimeout(180_000);
  if (!existsSync(REPO)) throw new Error(`${REPO} is missing: run scripts/gen-synthetic-repo.sh ${REPO} 20000 200`);
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-perf-")));
  const server = await startDevserver(
    path.join(root, "config"),
    root,
    path.join(ROOT, "target", "release", "gg-devserver"),
  );
  try {
    const cdp = await page.context().newCDPSession(page);
    const heapMb = async () => {
      await cdp.send("HeapProfiler.collectGarbage");
      const { usedSize } = await cdp.send("Runtime.getHeapUsage");
      return Math.round((usedSize / 1048576) * 10) / 10;
    };
    await page.addInitScript((url) => {
      const w = window as unknown as Record<string, unknown>;
      w.__GG_DEV_URL__ = url;
      w.__GG_PROBE__ = {};
    }, server.url);
    await page.goto("/");
    page.once("dialog", (dialog) => dialog.accept(REPO));
    await page.getByRole("button", { name: "Open repository" }).first().click();
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid).toHaveAttribute("aria-rowcount", "20001", { timeout: 60_000 });
    await page.waitForTimeout(1000);
    const before = await heapMb();

    await grid.evaluate(async (el) => {
      const max = el.scrollHeight - el.clientHeight;
      for (let k = 1; k <= 200; k++) {
        el.scrollTop = Math.round((max * k) / 200);
        await new Promise((r) => setTimeout(r, 30));
      }
      await new Promise((r) => setTimeout(r, 500));
    });
    const scrolled = await heapMb();
    for (let k = 0; k < 20; k++) {
      await grid.locator('[role="row"][aria-busy="false"]:not([hidden]) .subject').nth(3 + (k % 15)).click();
      await page.waitForTimeout(100);
    }
    await page.waitForTimeout(500);
    const clicked = await heapMb();

    const line = `JS heap after GC, MB: before ${before}, after the scroll ${scrolled}, after 20 clicks ${clicked}`;
    console.log(line);
    info.annotations.push({ type: "perf", description: line });
    expect.soft(scrolled - before, "heap kept by the scroll, MB").toBeLessThan(10);
  } finally {
    await stopDevserver(server);
    rmSync(root, { recursive: true, force: true });
  }
});
