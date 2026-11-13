// Scrolling speed on a large repository (plan 1, task 10), in Chromium against a release
// gg-devserver. Runs only with GG_PERF=1 (see playwright.config.ts):
//
//   GG_PERF=1 GG_PERF_R100K=~/.cache/gitgraph-perf/r100k pnpm e2e e2e/perf --headed --repeat-each=5
//
// The page runs the same measurement as the Tauri app does with GG_PERF_SCROLL=1
// (src/lib/perf/scrollProbe.ts): once the repository has fully loaded, the graph scrolls
// from top to bottom in 200 steps over 6 s while every requestAnimationFrame delta is
// recorded, then waits 300 ms and counts visible rows that are still skeletons.
// This measures Chromium, not the WKWebView the macOS app ships with.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { ScrollSummary } from "../../src/lib/perf/scrollProbe";
import { startDevserver, stopDevserver } from "../fixtures";

const ROOT = path.resolve(import.meta.dirname, "../..");
const REPO = process.env.GG_PERF_R100K ?? path.join(homedir(), ".cache/gitgraph-perf/r100k");

test.skip(process.env.GG_PERF !== "1", "performance run: set GG_PERF=1");
test.use({ viewport: { width: 1280, height: 800 } });

test.beforeAll(() => {
  if (!existsSync(REPO)) throw new Error(`${REPO} is missing: run scripts/gen-synthetic-repo.sh ${REPO} 100000 400`);
  execFileSync("cargo", ["build", "-q", "--release", "-p", "gg-devserver"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, CARGO_INCREMENTAL: "0" },
  });
});

test("scrolling r100k top to bottom", async ({ page }, info) => {
  test.setTimeout(120_000);
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-perf-")));
  const server = await startDevserver(
    path.join(root, "config"),
    root,
    path.join(ROOT, "target", "release", "gg-devserver"),
  );
  try {
    await page.addInitScript((url) => {
      const w = window as unknown as Record<string, unknown>;
      w.__GG_DEV_URL__ = url;
      w.__GG_PROBE__ = {};
      w.__GG_PERF_SCROLL__ = true;
    }, server.url);
    await page.goto("/");
    page.once("dialog", (dialog) => dialog.accept(REPO));
    await page.getByRole("button", { name: "Open repository" }).first().click();

    const handle = await page.waitForFunction(
      () => (window as unknown as { __GG_PROBE__: { summary?: unknown } }).__GG_PROBE__.summary,
      undefined,
      { timeout: 90_000, polling: 250 },
    );
    const s = (await handle.jsonValue()) as ScrollSummary;
    const line =
      `scroll r100k (${info.project.name}, ${process.env.HEADED ?? "see --headed"}): ` +
      `frames=${s.frames} p50=${s.p50.toFixed(1)} p95=${s.p95.toFixed(1)} p99=${s.p99.toFixed(1)} ` +
      `max=${s.max.toFixed(1)} ms, over 17 ms: ${s.over17}, skeletonMaxMs=${s.skeletonMaxMs.toFixed(0)}, ` +
      `frames with a skeleton > 150 ms: ${s.slowSkeletonFrames}, skeletons 300 ms after stop: ${s.skeletonsAfterStop}`;
    console.log(line);
    info.annotations.push({ type: "perf", description: line });

    expect(s.frames).toBeGreaterThan(100);
    expect.soft(s.p95, "p95 frame time, ms").toBeLessThanOrEqual(17);
    expect.soft(s.skeletonsAfterStop, "rows still skeletons 300 ms after scrolling stops").toBe(0);
  } finally {
    await stopDevserver(server);
    rmSync(root, { recursive: true, force: true });
  }
});
