// Click feedback and refresh latency on a 20,000-commit repository (plan 1, task 10, spec
// §7), in Chromium against a release gg-devserver. Runs only with GG_PERF=1:
//
//   GG_PERF=1 pnpm e2e e2e/perf/interaction.spec.ts
//
// - Click feedback: a real mouse click on 20 visible rows; from the pointerdown event's
//   timestamp to the first animation frame in which the row is selected (the frame that
//   paints the selection). Target < 100 ms.
// - Refresh: with the graph scrolled to the middle, a commit (5 times) and a branch moved to
//   a visible commit (5 times) are made with git on disk; from git's exit to the first
//   animation frame showing the new row (or the moved label). Target < 1 s, and the row at
//   the top of the viewport stays at the same offset (no scroll jump).
// The repository is a fresh r20k from scripts/gen-synthetic-repo.sh in a temp folder, since
// the test changes it. This measures Chromium, not WKWebView.
import { execFileSync } from "node:child_process";
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { percentile } from "../../src/lib/perf/scrollProbe";
import { startDevserver, stopDevserver } from "../fixtures";

const ROOT = path.resolve(import.meta.dirname, "../..");

test.skip(process.env.GG_PERF !== "1", "performance run: set GG_PERF=1");
test.use({ viewport: { width: 1280, height: 800 } });

let root = "";
let repo = "";

test.beforeAll(() => {
  execFileSync("cargo", ["build", "-q", "--release", "-p", "gg-devserver"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, CARGO_INCREMENTAL: "0" },
  });
  root = realpathSync(mkdtempSync(path.join(tmpdir(), "gg-perf-")));
  repo = path.join(root, "r20k");
  execFileSync(path.join(ROOT, "scripts", "gen-synthetic-repo.sh"), [repo, "20000", "200"], { stdio: "ignore" });
});

test.afterAll(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});

const git = (...args: string[]) =>
  execFileSync("git", args, {
    cwd: repo,
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_AUTHOR_NAME: "Probe",
      GIT_AUTHOR_EMAIL: "probe@example.com",
      GIT_COMMITTER_NAME: "Probe",
      GIT_COMMITTER_EMAIL: "probe@example.com",
    },
  }).trim();

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });

async function openRepo(page: Page) {
  const server = await startDevserver(path.join(root, `config-${Date.now()}`), root, path.join(ROOT, "target", "release", "gg-devserver"));
  await page.addInitScript((url) => {
    (window as unknown as Record<string, unknown>).__GG_DEV_URL__ = url;
  }, server.url);
  await page.goto("/");
  page.once("dialog", (dialog) => dialog.accept(repo));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", /^2\d{4}$/);
  await expect(grid(page).locator('[role="row"][data-index="0"] .subject')).not.toBeEmpty();
  return server;
}

/** The first row fully below the header: its index, subject and offset from the header. */
function anchor(page: Page) {
  return grid(page).evaluate((el) => {
    const top = el.querySelector("[data-header]")!.getBoundingClientRect().bottom;
    const rows = [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]')]
      .filter((r) => !r.hidden && r.getBoundingClientRect().top >= top - 0.5)
      .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    const r = rows[0];
    return {
      subject: r.querySelector(".subject")!.textContent,
      hash: r.querySelector(".hash")!.textContent,
      offset: r.getBoundingClientRect().top - top,
    };
  });
}

test("click feedback on r20k", async ({ page }, info) => {
  test.setTimeout(120_000);
  const server = await openRepo(page);
  try {
    await page.evaluate(() => {
      const w = window as unknown as { __down: number };
      document.addEventListener("pointerdown", (e) => (w.__down = e.timeStamp), true);
    });
    const latencies: number[] = [];
    for (let k = 0; k < 20; k++) {
      const target = grid(page).locator(`[role="row"][data-index="${2 + (k % 20)}"]`);
      const index = 2 + (k % 20);
      // Resolves with ms from pointerdown to the first frame in which the row is selected.
      const measured = page.evaluate(
        (i) =>
          new Promise<number>((resolve) => {
            const w = window as unknown as { __down: number };
            const check = () => {
              const row = document.querySelector(`[role="row"][data-index="${i}"]`);
              if (row?.getAttribute("aria-selected") === "true") resolve(performance.now() - w.__down);
              else requestAnimationFrame(check);
            };
            requestAnimationFrame(check);
          }),
        index,
      );
      await target.locator(".subject").click();
      latencies.push(await measured);
      await page.waitForTimeout(100);
    }
    const line =
      `click feedback r20k (chromium): ${latencies.length} clicks, p50=${percentile(latencies, 50).toFixed(1)} ` +
      `p95=${percentile(latencies, 95).toFixed(1)} max=${Math.max(...latencies).toFixed(1)} ms`;
    console.log(line);
    info.annotations.push({ type: "perf", description: line });
    expect.soft(percentile(latencies, 95), "p95 click feedback, ms").toBeLessThan(100);
  } finally {
    await stopDevserver(server);
  }
});

test("refresh after an on-disk change on r20k, scrolled to the middle", async ({ page }, info) => {
  test.setTimeout(180_000);
  const server = await openRepo(page);
  try {
    await grid(page).evaluate((el) => (el.scrollTop = 10_000 * 26 + 7));
    await expect(grid(page).locator('[role="row"][data-index="10000"] .subject')).not.toBeEmpty();
    await page.waitForTimeout(500);

    const results: { kind: string; ms: number; jump: number }[] = [];
    for (let k = 0; k < 10; k++) {
      const commit = k % 2 === 0;
      const before = await anchor(page);
      const label = `probe-${k}`;
      // In the page: the first frame showing the new row count (commit) or the label (branch).
      const seen = page.evaluate(
        ({ commit, label, rows }) =>
          new Promise<number>((resolve) => {
            const check = () => {
              const grid = document.querySelector('[role="grid"]')!;
              const done = commit
                ? Number(grid.getAttribute("aria-rowcount")) > rows
                : grid.querySelector(`[data-ref="${label}"]`) !== null;
              if (done) resolve(performance.timeOrigin + performance.now());
              else requestAnimationFrame(check);
            };
            requestAnimationFrame(check);
          }),
        { commit, label, rows: Number(await grid(page).getAttribute("aria-rowcount")) },
      );
      if (commit) {
        git("commit", "-q", "--allow-empty", "-m", `Refresh probe ${k}`);
      } else {
        git("branch", "-f", label, before.hash!);
      }
      const exited = Date.now();
      const shownAt = await seen;
      await page.waitForTimeout(300);
      const after = await anchor(page);
      expect(after.hash, "the top row is still the same commit").toBe(before.hash);
      results.push({ kind: commit ? "commit" : "branch", ms: shownAt - exited, jump: after.offset - before.offset });
      await page.waitForTimeout(700);
    }
    const ms = results.map((r) => r.ms);
    const line =
      `refresh r20k (chromium): ${results.map((r) => `${r.kind} ${r.ms.toFixed(0)} ms`).join(", ")}; ` +
      `median ${percentile(ms, 50).toFixed(0)} ms, max ${Math.max(...ms).toFixed(0)} ms; ` +
      `largest anchor offset change ${Math.max(...results.map((r) => Math.abs(r.jump))).toFixed(1)} px`;
    console.log(line);
    info.annotations.push({ type: "perf", description: line });
    expect.soft(Math.max(...ms), "slowest refresh, ms").toBeLessThan(1000);
    for (const r of results) expect.soft(Math.abs(r.jump), "anchor offset change, px").toBeLessThan(0.5);
  } finally {
    await stopDevserver(server);
  }
});
