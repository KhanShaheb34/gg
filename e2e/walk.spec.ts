// The worktree walk (src/lib/perf/worktreeWalkProbe.ts) in the browser build, at both sizes and in
// both themes: the very probe the release app runs with GG_PERF_SCROLL=walk3, driven the way
// scripts/walk.sh drives it there (its `walk-need` marks answered by changing files; its
// `walk-shot` marks screenshotted), on the repository scripts/walk-fixture.sh --github makes,
// with the devserver's fake `gh`. Run on demand:
//   GG_WALK=1 [GG_WALK_OUT=<folder for screenshots>] pnpm e2e walk
// One difference from the real app: the browser build has no second launch to hand a folder
// to, so the `gg` step's folder is opened through the page's Open button here (scripts/walk.sh
// runs the real `gg` command against the release app).
import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, realpathSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, openFolder, test } from "./fixtures";

test.skip(!process.env.GG_WALK, "set GG_WALK=1 to run the walk");

// The devserver appends the probe's marks here (it inherits this worker's environment).
const LOG = path.join(tmpdir(), `gg-walk-${process.pid}.log`);
process.env.GG_PERF_LOG = LOG;
const OUT = process.env.GG_WALK_OUT;

/** The lines of the log after `from` lines: [name, value]. */
function marks(from: number): [string, string][] {
  if (!existsSync(LOG)) return [];
  return readFileSync(LOG, "utf8")
    .split("\n")
    .filter((l) => l !== "")
    .slice(from)
    .map((l) => l.split("\t").slice(0, 2) as [string, string]);
}

for (const [size, width, height] of [
  ["1280x800", 1280, 800],
  ["900x560", 900, 560],
] as const)
  for (const theme of ["light", "dark"] as const) {
    test(`the worktree walk, ${size}, ${theme}`, async ({ gg }) => {
      test.setTimeout(240_000);
      const { page } = gg;
      await page.setViewportSize({ width, height });
      const parent = realpathSync(mkdtempSync(path.join(gg.home, "walk-")));
      const script = path.resolve(import.meta.dirname, "..", "scripts", "walk-fixture.sh");
      const work = execFileSync(script, ["--github", parent], {
        encoding: "utf8",
        env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
      }).trim();
      const W = path.dirname(work);
      gg.gh.reply("pr-list", 0, "[]");
      gg.gh.reply("pr-list-head", 0, "[]");
      gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
      gg.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/21\n");

      const start = marks(0).length;
      await page.addInitScript(
        (mode) => {
          const w = window as unknown as Record<string, unknown>;
          w.__GG_PROBE__ = {};
          w.__GG_PERF_SCROLL__ = mode;
        },
        theme === "dark" ? "walk3-dark" : "walk3",
      );
      await page.goto("/");
      page.once("dialog", (dialog) => dialog.accept(work));
      await openFolder(page);

      // The driver.
      let seen = start;
      const deadline = Date.now() + 220_000;
      for (;;) {
        const fresh = marks(seen);
        seen += fresh.length;
        for (const [name] of fresh) {
          if (name === "walk-need edit worktree walk-wt") {
            const wt = path.join(W, "work.worktrees", "walk-wt");
            appendFileSync(path.join(wt, "app.txt"), "changed in the worktree\n");
            appendFileSync(path.join(wt, "new.txt"), "new\n");
            appendFileSync(path.join(wt, "scratch.txt"), "scratch\n");
          } else if (name === "walk-need list pr 21") {
            gg.gh.reply(
              "pr-list",
              0,
              JSON.stringify([
                {
                  number: 21,
                  title: "Walk: a change in a worktree",
                  url: "https://github.com/octo/cat/pull/21",
                  isDraft: false,
                  headRefName: "walk/wt",
                  baseRefName: "main",
                  headRepositoryOwner: { id: "X", login: "octo" },
                  isCrossRepository: false,
                },
              ]),
            );
          } else if (name === "walk-need gg other") {
            page.once("dialog", (dialog) => dialog.accept(path.join(W, "other")));
            await openFolder(page);
          } else if (name.startsWith("walk-shot ") && OUT) {
            mkdirSync(OUT, { recursive: true });
            await page.screenshot({ path: path.join(OUT, `${size}-${theme}-${name.slice(10)}.png`) });
          }
        }
        if (fresh.some(([name]) => name === "scroll-done") || Date.now() > deadline) break;
        await page.waitForTimeout(100);
      }

      const all = marks(start);
      const report = all.filter(([n]) => n.startsWith("walk") && !n.startsWith("walk-need") && !n.startsWith("walk-shot"));
      console.log(report.map(([n, v]) => `${n}\t${v}`).join("\n"));
      const failed = report.filter(([n, v]) => n.startsWith("walk ") && v === "0").map(([n]) => n);
      expect(failed).toEqual([]);
      expect(all.find(([n]) => n === "walk-passed")?.[1]).toBe("12");
      expect(all.find(([n]) => n === "page-errors")?.[1]).toBe("0");
    });
  }
