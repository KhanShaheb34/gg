// The commit graph against real repositories: rows, labels, lanes, scrolling, selection and
// live refresh.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { BIG_COMMITS, bigSubject, closeButton, expect, test } from "./fixtures";

test.use({ timezoneId: "UTC" });

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
/** The graph's date format, in UTC (the page's time zone here). */
function utcDate(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
}

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const row = (page: Page, index: number) => grid(page).locator(`[role="row"][data-index="${index}"]`);
const subject = (r: Locator) => r.locator(".subject");

/** Scrolls the graph to `top` px. */
async function scrollTo(page: Page, top: number) {
  await grid(page).evaluate((el, y) => {
    el.scrollTop = y;
  }, top);
}

/** The subject of the first row fully below the header, and the graph's scrollTop. */
function firstVisible(page: Page) {
  return grid(page).evaluate((el) => {
    const header = el.querySelector<HTMLElement>("[data-header]")!;
    const top = header.getBoundingClientRect().bottom;
    const rows = [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]')]
      .filter((r) => r.getBoundingClientRect().top >= top - 0.5)
      .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    return { subject: rows[0]?.querySelector(".subject")?.textContent ?? null, scrollTop: el.scrollTop };
  });
}

/** Rows in the viewport that are still skeletons (not yet loaded). */
function visibleSkeletons(page: Page) {
  return grid(page).evaluate((el) => {
    const box = el.getBoundingClientRect();
    return [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]')].filter((r) => {
      const b = r.getBoundingClientRect();
      const inView = b.bottom > box.top && b.top < box.bottom;
      return inView && (r.getAttribute("aria-busy") === "true" || !r.querySelector(".subject")?.textContent);
    }).length;
  });
}

test("rows show subject, author, date and short hash as in git log", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));

  const log = gg
    .git("small", "log", "--all", "--format=%H%x09%at%x09%s", "-3")
    .trim()
    .split("\n")
    .map((line) => line.split("\t"));
  for (const [i, [hash, at, subj]] of log.entries()) {
    const r = row(page, i);
    await expect(subject(r)).toHaveText(subj);
    await expect(r.locator(".author")).toHaveText("Ada Lovelace");
    await expect(r.locator(".date")).toHaveText(utcDate(Number(at)));
    await expect(r.locator(".hash")).toHaveText(hash.slice(0, 8));
  }
});

test("tooltips are built when hovered: full hash, date, author and a cut-off subject", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const [hash, at] = gg.git("small", "log", "-1", "--format=%H%x09%at").trim().split("\t");
  const r = row(page, 0);
  await expect(subject(r)).not.toBeEmpty();
  // Nothing is prepared for rows nobody points at.
  for (const cell of [".hash", ".date", ".author", ".subject"]) {
    await expect(r.locator(cell)).not.toHaveAttribute("title", /./);
  }
  await r.locator(".hash").hover();
  await expect(r.locator(".hash")).toHaveAttribute("title", hash);
  await r.locator(".date").hover();
  await expect(r.locator(".date")).toHaveAttribute("title", new RegExp(`^\\w+day, ${utcDate(Number(at))}:00 `));
  await r.locator(".author").hover();
  await expect(r.locator(".author")).toHaveAttribute("title", "Ada Lovelace <ada@example.com>");
  // A subject that fits gets no tooltip; one that is cut off gets its full text.
  await subject(r).hover();
  await expect(subject(r)).not.toHaveAttribute("title", /./);
  const handle = page.getByRole("separator", { name: "Resize Author column" });
  const box = (await handle.boundingBox())!;
  await page.mouse.move(box.x + 4, box.y + 10);
  await page.mouse.down();
  await page.mouse.move(box.x - 900, box.y + 10, { steps: 4 });
  await page.mouse.up();
  await subject(r).hover();
  await expect(subject(r)).toHaveAttribute("title", "Add part 19");
});

test("labels: main on its commit, local and remote at the same commit as one pill", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));

  const mainHash = gg.git("small", "rev-parse", "main").trim();
  const mainRow = row(page, 0);
  await expect(mainRow.locator(".hash")).toHaveText(mainHash.slice(0, 8));
  await expect(mainRow.locator('[data-ref="main"]')).toBeVisible();
  await expect(mainRow.locator('[data-ref="main"]')).toHaveAttribute("data-current", "true");

  // fix/typo was pushed with its upstream: one pill naming the remote too.
  const fix = grid(page).locator('[data-ref="fix/typo"]');
  await expect(fix).toHaveCount(1);
  await expect(fix).toContainText("fix/typo");
  await expect(fix).toContainText("origin");
  // origin/main is two commits behind, so it is a pill of its own.
  await expect(grid(page).locator('[data-ref="origin/main"]')).toHaveCount(1);
});

test("the graph canvas draws lanes", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  await expect
    .poll(() =>
      page.getByTestId("graph-canvas").evaluate((c: HTMLCanvasElement) => {
        const data = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
        let painted = 0;
        for (let i = 3; i < data.length; i += 4) if (data[i] > 0) painted++;
        return painted;
      }),
    )
    .toBeGreaterThan(100);
});

test("scrolling a long history to the bottom shows the oldest commit, never empty rows", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));

  await scrollTo(page, 1e9);
  await expect(subject(row(page, BIG_COMMITS - 1))).toHaveText(bigSubject(1));
  await page.waitForTimeout(500); // asserting that nothing is left unloaded afterwards
  expect(await visibleSkeletons(page)).toBe(0);

  await scrollTo(page, 1500 * 26);
  await page.waitForTimeout(500);
  expect(await visibleSkeletons(page)).toBe(0);
});

test("selection: click, arrows, page keys, escape", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));

  await row(page, 2).click();
  await expect(row(page, 2)).toHaveAttribute("aria-selected", "true");
  await expect(grid(page)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(row(page, 3)).toHaveAttribute("aria-selected", "true");
  await expect(row(page, 2)).toHaveAttribute("aria-selected", "false");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await expect(row(page, 1)).toHaveAttribute("aria-selected", "true");

  // PageDown moves by a viewport and keeps the selection in view.
  await page.keyboard.press("PageDown");
  const selected = grid(page).locator('[role="row"][aria-selected="true"]');
  await expect(selected).toHaveCount(1);
  const index = Number(await selected.getAttribute("data-index"));
  expect(index).toBeGreaterThan(10);
  await expect(selected).toBeInViewport();

  await page.keyboard.press("End");
  await expect(row(page, BIG_COMMITS - 1)).toHaveAttribute("aria-selected", "true");
  await expect(row(page, BIG_COMMITS - 1)).toBeInViewport();
  await page.keyboard.press("Home");
  await expect(row(page, 0)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(grid(page).locator('[role="row"][aria-selected="true"]')).toHaveCount(0);
});

test("a commit made outside the app appears at the top without moving the view", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");

  gg.git("small", "commit", "-q", "--allow-empty", "-m", "Made outside the app");
  await expect(subject(row(page, 0))).toHaveText("Made outside the app", { timeout: 2000 });
  expect((await firstVisible(page)).scrollTop).toBe(0);
});

test("a refresh keeps the first visible commit in place when scrolled down", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await scrollTo(page, 1500 * 26 + 7);
  await expect.poll(async () => (await firstVisible(page)).subject).toMatch(/^Big commit \d+$/);
  const before = await firstVisible(page);

  gg.git("big", "commit", "-q", "--allow-empty", "-m", "Made outside the app");
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 2), { timeout: 2000 });
  await expect.poll(async () => (await firstVisible(page)).subject).toBe(before.subject);
  expect((await firstVisible(page)).scrollTop).toBe(before.scrollTop + 26);
});

test("uncommitted changes show as a row after the window regains focus", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");

  writeFileSync(path.join(gg.repoPath("small"), "src", "part1.txt"), "changed\n");
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(subject(row(page, 0))).toHaveText(/^Uncommitted changes/);
  await expect(row(page, 0).locator(".hash")).toHaveText("");
  await expect(subject(row(page, 1))).toHaveText("Add part 19");
});

test("an empty repository says so", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("empty"));
  await expect(page.getByText("No commits yet")).toBeVisible();
});

test("switching tabs keeps each graph's scroll and selection without fetching rows again", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await scrollTo(page, 800 * 26);
  await expect(subject(row(page, 800))).toHaveText(bigSubject(BIG_COMMITS - 800));
  await row(page, 805).click();
  const before = await firstVisible(page);

  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");

  let rowRequests = 0;
  page.on("request", (r) => {
    if (r.method() === "POST" && r.postData()?.includes('"get_rows"')) rowRequests++;
  });
  await page.getByRole("tab", { name: "big" }).click();
  await expect(row(page, 805)).toHaveAttribute("aria-selected", "true");
  expect(await firstVisible(page)).toEqual(before);
  await page.waitForTimeout(300); // asserting that no request follows
  expect(rowRequests).toBe(0);
});

test("clicking the short hash copies the full hash", async ({ gg, context }) => {
  const { page } = gg;
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hash = gg.git("small", "rev-parse", "HEAD").trim();
  await row(page, 0).locator(".hash").click();
  await expect(page.getByRole("status").filter({ hasText: "Copied" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hash);
});

test("column widths can be dragged, persist, and reset on double-click", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const author = page.getByRole("columnheader", { name: "Author" });
  const handle = page.getByRole("separator", { name: "Resize Author column" });
  const start = (await author.boundingBox())!.width;

  const box = (await handle.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - 60, box.y + box.height / 2, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => (await author.boundingBox())!.width).toBeCloseTo(start + 60, 0);

  await page.reload();
  await expect.poll(async () => (await author.boundingBox())!.width).toBeCloseTo(start + 60, 0);

  await handle.dblclick();
  await expect.poll(async () => (await author.boundingBox())!.width).toBeCloseTo(start, 0);
});

test("recent repositories show paths under the home folder with ~", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await closeButton(page, "small").click();
  const recent = page.getByRole("list", { name: "Recent repositories" }).getByRole("listitem");
  await expect(recent).toContainText("~/repos");
});

test("the performance probe records scroll frames when enabled", async ({ gg }) => {
  const { page } = gg;
  await page.addInitScript(() => {
    (window as unknown as { __GG_PROBE__: object }).__GG_PROBE__ = {};
  });
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await grid(page).evaluate(async (el) => {
    for (let i = 1; i <= 30; i++) {
      el.scrollTop = i * 400;
      await new Promise((r) => requestAnimationFrame(r));
    }
  });
  const probe = await page.evaluate(
    () => (window as unknown as { __GG_PROBE__: { frames: number[]; skeletonMaxMs: number } }).__GG_PROBE__,
  );
  expect(probe.frames.length).toBeGreaterThan(10);
  expect(probe.frames.every((ms) => ms > 0)).toBe(true);
  expect(typeof probe.skeletonMaxMs).toBe("number");
});

test("the lane canvas follows the Graph column's width and never covers the description", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  const canvas = page.getByTestId("graph-canvas");
  const header = page.getByRole("columnheader", { name: "Graph" });
  const handle = page.getByRole("separator", { name: "Resize Graph column" });

  const check = async () => {
    const column = (await header.boundingBox())!;
    await expect.poll(async () => (await canvas.boundingBox())!.width).toBeCloseTo(column.width, 0);
    const lanes = (await canvas.boundingBox())!;
    const desc = (await row(page, 0).locator(".desc").boundingBox())!;
    expect(lanes.x + lanes.width).toBeLessThanOrEqual(desc.x + 0.5);
  };
  const drag = async (dx: number) => {
    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2, { steps: 4 });
    await page.mouse.up();
  };

  const start = (await header.boundingBox())!.width;
  await drag(-30);
  await expect.poll(async () => (await header.boundingBox())!.width).toBeCloseTo(start - 30, 0);
  await check();
  await drag(90);
  await expect.poll(async () => (await header.boundingBox())!.width).toBeCloseTo(start + 60, 0);
  await check();
});

test("each rendered row has its own id and its real row index", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small")); // fewer rows than the pool: some slots are unused
  await expect(subject(row(page, 0))).not.toBeEmpty();
  const audit = () =>
    grid(page).evaluate((el) => {
      const rows = [...el.querySelectorAll<HTMLElement>('[role="rowgroup"] > [role="row"]')];
      const shown = rows.filter((r) => !r.hidden);
      const ids = rows.map((r) => r.id).filter(Boolean);
      return {
        duplicateIds: ids.length - new Set(ids).size,
        hiddenWithAttrs: rows.filter((r) => r.hidden && (r.id || r.hasAttribute("aria-rowindex"))).length,
        wrongIndex: shown.filter((r) => Number(r.getAttribute("aria-rowindex")) !== Number(r.dataset.index) + 2).length,
        shown: shown.length,
      };
    });
  expect(await audit()).toEqual({ duplicateIds: 0, hiddenWithAttrs: 0, wrongIndex: 0, shown: 30 });
});

test("rows and lanes move as one layer the size of the rendered rows, not of the whole history", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await scrollTo(page, 1500 * 26);
  await expect(subject(row(page, 1500))).not.toBeEmpty();
  const layout = await grid(page).evaluate((el) => {
    const group = el.querySelector<HTMLElement>('[role="rowgroup"]')!;
    return {
      groupHeight: group.getBoundingClientRect().height,
      viewport: el.clientHeight,
      scrollHeight: el.scrollHeight,
      canvasInGroup: group.contains(el.querySelector('[data-testid="graph-canvas"]')),
      willChange: getComputedStyle(group).willChange,
    };
  });
  expect(layout.scrollHeight).toBeGreaterThan(BIG_COMMITS * 26);
  expect(layout.groupHeight).toBeLessThan(3 * layout.viewport);
  expect(layout.canvasInGroup).toBe(true);
  expect(layout.willChange).toContain("transform");
});
