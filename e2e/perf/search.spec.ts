// Scrolling with a search active (task 11 review): the same top-to-bottom scroll as
// scroll.spec.ts, on r20k, once without a search and once with a broad one ("e", which hits
// about 97% of the rows, each with its match marked), in Chromium against a release
// gg-devserver. Runs only with GG_PERF=1:
//
//   GG_PERF=1 pnpm e2e e2e/perf/search.spec.ts --repeat-each=3
//
// Prints one line per run; the two should match. Headless Chromium paces frames at 60 Hz,
// so on a fast machine both read 16.7 ms whatever the work; GG_PERF_THROTTLE=6 slows the
// page's CPU six times (Chrome DevTools throttling) so that extra work per frame shows.
// This measures Chromium, not WKWebView.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type { ScrollSummary } from "../../src/lib/perf/scrollProbe";
import { startDevserver, stopDevserver } from "../fixtures";

const ROOT = path.resolve(import.meta.dirname, "../..");
const REPO = process.env.GG_PERF_R20K ?? path.join(homedir(), ".cache/gitgraph-perf/r20k");
const THROTTLE = Number(process.env.GG_PERF_THROTTLE ?? "1");

test.skip(process.env.GG_PERF !== "1", "performance run: set GG_PERF=1");
test.use({ viewport: { width: 1280, height: 800 } });

test.beforeAll(() => {
  if (!existsSync(REPO)) throw new Error(`${REPO} is missing: run scripts/gen-synthetic-repo.sh ${REPO} 20000 200`);
  execFileSync("cargo", ["build", "-q", "--release", "-p", "gg-devserver"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, CARGO_INCREMENTAL: "0" },
  });
});

for (const query of ["", "e"]) {
  test(`scrolling r20k ${query ? `with search "${query}" active` : "without a search"}`, async ({ page }, info) => {
    test.setTimeout(120_000);
    const root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-perf-")));
    const server = await startDevserver(path.join(root, "config"), root, path.join(ROOT, "target", "release", "gg-devserver"));
    try {
      await page.addInitScript(
        ({ url, query }) => {
          const w = window as unknown as Record<string, unknown>;
          w.__GG_DEV_URL__ = url;
          w.__GG_PROBE__ = {};
          w.__GG_PERF_SCROLL__ = true;
          if (query) w.__GG_PERF_SEARCH__ = query;
        },
        { url: server.url, query },
      );
      if (THROTTLE > 1) {
        const cdp = await page.context().newCDPSession(page);
        await cdp.send("Emulation.setCPUThrottlingRate", { rate: THROTTLE });
      }
      await page.goto("/");
      page.once("dialog", (dialog) => dialog.accept(REPO));
      await page.getByRole("button", { name: "Open repository" }).first().click();
      const handle = await page.waitForFunction(
        () => (window as unknown as { __GG_PROBE__: { summary?: unknown } }).__GG_PROBE__.summary,
        undefined,
        { timeout: 90_000, polling: 250 },
      );
      const s = (await handle.jsonValue()) as ScrollSummary;
      const marked = await page.locator("mark:not(:empty)").count();
      const line =
        `scroll r20k cpu/${THROTTLE} search=${JSON.stringify(query)}: frames=${s.frames} p50=${s.p50.toFixed(1)} ` +
        `p95=${s.p95.toFixed(1)} p99=${s.p99.toFixed(1)} max=${s.max.toFixed(1)} ms, over 17 ms: ${s.over17}, ` +
        `over 25 ms: ${s.over25}, skeletonMaxMs=${s.skeletonMaxMs.toFixed(0)}, ` +
        `skeletons 300 ms after stop: ${s.skeletonsAfterStop}, marks on screen at the end: ${marked}`;
      console.log(line);
      info.annotations.push({ type: "perf", description: line });
      expect(s.frames).toBeGreaterThan(100);
      if (query) expect(marked).toBeGreaterThan(10);
      expect.soft(s.skeletonsAfterStop, "rows still skeletons 300 ms after scrolling stops").toBe(0);
    } finally {
      await stopDevserver(server);
      rmSync(root, { recursive: true, force: true });
    }
  });
}
