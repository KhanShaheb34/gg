// The commit graph against real repositories: rows, labels, lanes, scrolling, selection and
// live refresh.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { BIG_COMMITS, bigSubject, closeButton, expect, test, viewOption } from "./fixtures";

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
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
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
  // Squeezed by the Commit column: Author and Date give their room back to the description;
  // Commit does not.
  const handle = page.getByRole("separator", { name: "Resize Commit column" });
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
  // No worktree mark on branches not checked out in another worktree.
  await expect(grid(page).locator(".ref .wt:visible")).toHaveCount(0);
  // A branch checked out in another worktree gets one.
  gg.git("small", "worktree", "add", "-q", "../small-wt", "fix/typo");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(grid(page).locator('[data-ref="fix/typo"] .wt')).toBeVisible();
  await expect(grid(page).locator(".ref .wt:visible")).toHaveCount(1);

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

test("a row clicked while it is still loading is selected once it has loaded", async ({ gg }) => {
  const { page } = gg;
  let release: () => void = () => {};
  const held = new Promise<void>((r) => (release = r));
  await page.route("**/invoke", async (route) => {
    if ((route.request().postDataJSON() as { cmd: string } | null)?.cmd === "get_rows") await held;
    return route.continue();
  });
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(row(page, 2)).toHaveAttribute("aria-busy", "true");
  await row(page, 2).click();
  release();
  await expect(row(page, 2)).toHaveAttribute("aria-busy", "false");
  await expect(row(page, 2)).toHaveAttribute("aria-selected", "true");
  await expect(grid(page)).toBeFocused();
  await page.unrouteAll({ behavior: "wait" });
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

test("a tab in the background picks up working-tree changes when it is shown after the window comes back", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");
  await open(page, gg.repoPath("details"));
  await expect(page.getByRole("tab", { name: /details/ })).toHaveAttribute("aria-selected", "true");

  writeFileSync(path.join(gg.repoPath("small"), "src", "part1.txt"), "changed\n");
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await page.getByRole("tab", { name: /small/ }).click();
  await expect(subject(row(page, 0))).toHaveText(/^Uncommitted changes/);
});

test("an empty repository says so, and the controls that need commits are off", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("empty"));
  await expect(page.getByText("No commits yet")).toBeVisible();
  await expect(page.getByText("Commits made on main show up here.")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Branches:/ })).toBeDisabled();
  await expect(page.getByTestId("view-options")).toBeDisabled();
  await expect(page.getByTestId("view-options")).toHaveAttribute("title", /no commits/i);
  await expect(page.getByRole("button", { name: "Search commits" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Search commits" })).toHaveAttribute("title", /no commits/i);
  await expect(page.getByRole("button", { name: "Refresh" })).toBeEnabled();
  // Cmd+F opens no search box either.
  await page.keyboard.press("ControlOrMeta+f");
  await expect(page.getByRole("searchbox")).toHaveCount(0);
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

test("while the first rows load, one viewport of skeletons shows and the grid does not scroll", async ({ gg }) => {
  const { page } = gg;
  // The repository stays "loading, no rows yet": no events, and snapshots say so.
  await page.route("**/events", (route) => route.abort());
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd?: string } | null;
    if (body?.cmd !== "open_repo" && body?.cmd !== "get_snapshot") return route.continue();
    const response = await route.fetch();
    const json = (await response.json()) as { ok?: { totalRows: number; loading: boolean } };
    if (json.ok) Object.assign(json.ok, { totalRows: 0, loading: true });
    await route.fulfill({ response, json });
  });
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const g = grid(page);
  await expect(g.locator('[role="row"][aria-busy="true"]:not([hidden])').first()).toBeVisible();
  const state = await g.evaluate((el) => ({
    skeletons: el.querySelectorAll('[role="row"][aria-busy="true"]:not([hidden])').length,
    rowArea: el.clientHeight - el.querySelector<HTMLElement>("[data-header]")!.offsetHeight,
    scrollable: el.scrollHeight - el.clientHeight,
  }));
  expect(state.skeletons).toBeGreaterThan(0);
  expect(state.skeletons).toBeLessThanOrEqual(Math.floor(state.rowArea / 26));
  expect(state.scrollable).toBeLessThanOrEqual(0);
  // The description's bar starts where the text will, at the cell's left, as the others do.
  const bar = await g
    .locator('[role="row"][aria-busy="true"]:not([hidden]) .desc')
    .first()
    .evaluate((el) => getComputedStyle(el, "::after").order);
  expect(bar).toBe("-1");
});

test("the lane canvas is as tall as the rows the viewport needs, and shrinks with it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  const canvas = page.getByTestId("graph-canvas");
  const tall = (await canvas.boundingBox())!.height;
  await row(page, 2).click(); // the details panel opens below the graph
  await expect(page.getByRole("region", { name: "Commit details" })).toBeVisible();
  await expect.poll(async () => (await canvas.boundingBox())!.height).toBeLessThan(tall);
});

test("aria-activedescendant names only a row that is rendered", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await row(page, 2).click();
  const target = () =>
    grid(page).evaluate((el) => {
      const id = el.getAttribute("aria-activedescendant");
      if (id === null) return "none";
      const r = document.getElementById(id);
      return r && !r.hidden ? "rendered" : "missing";
    });
  await expect.poll(target).toBe("rendered");
  await scrollTo(page, 1500 * 26);
  await expect(subject(row(page, 1500))).not.toBeEmpty();
  await expect.poll(target).toBe("none");
  await scrollTo(page, 0);
  await expect.poll(target).toBe("rendered");
});

test("lanes stay level with their rows after scrolling: each commit's dot is at its row's centre", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big")); // linear: every commit is a dot on lane 0
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  for (const top of [1500 * 26 + 7, 333 * 26 + 19, 41]) {
    await scrollTo(page, top);
    // The rows there loaded (on a slow CI runner a page of rows can take longer than a fixed
    // wait; how fast they come is the scroll tests' and the perf runs' to say, not this one's).
    await expect.poll(() => visibleSkeletons(page)).toBe(0);
    await page.waitForTimeout(200); // the lanes are drawn for them
    const misses = await grid(page).evaluate((el) => {
      const c = el.querySelector<HTMLCanvasElement>('[data-testid="graph-canvas"]')!;
      const ratio = c.width / c.getBoundingClientRect().width;
      const data = c.getContext("2d")!.getImageData(0, 0, c.width, c.height);
      const alpha = (x: number, y: number) => {
        const px = Math.round(x * ratio);
        const py = Math.round(y * ratio);
        return py < 0 || py >= c.height ? -1 : data.data[(py * c.width + px) * 4 + 3];
      };
      const box = c.getBoundingClientRect();
      const header = el.querySelector<HTMLElement>("[data-header]")!.getBoundingClientRect().bottom;
      const out: string[] = [];
      for (const r of el.querySelectorAll<HTMLElement>('[role="row"][data-index]:not([hidden])')) {
        const b = r.getBoundingClientRect();
        if (b.top < header || b.bottom > el.getBoundingClientRect().bottom) continue;
        const mid = b.top + b.height / 2 - box.top;
        // Lane 0's dot: 3 px right of the line, inside the dot (radius 4) but off the line.
        const x = 6 + 8 + 3;
        if (alpha(x, mid) === 0 || alpha(x, mid - 9) > 0 || alpha(x, mid + 9) > 0) out.push(r.dataset.index!);
      }
      return out;
    });
    expect(misses).toEqual([]);
  }
});

test("a row's tooltip goes when its commit changes under a still pointer", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  const hash = gg.git("small", "rev-parse", "HEAD").trim();
  const cell = row(page, 0).locator(".hash");
  await cell.hover();
  await expect(cell).toHaveAttribute("title", hash);
  gg.git("small", "commit", "-q", "--allow-empty", "-m", "Commit under the pointer");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(subject(row(page, 0))).toHaveText("Commit under the pointer");
  await expect(cell).not.toHaveAttribute("title", hash);
});

test("scrolling creates no elements: ref labels use fixed slots, with +k for the rest", async ({ gg }) => {
  // 1,200 scroll positions, a frame each: 20 s at 60 frames a second, more where a CI
  // machine's headless browser draws fewer.
  test.setTimeout(120_000);
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("labelled"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", "601");
  await expect(subject(row(page, 0))).not.toBeEmpty();
  const added = await grid(page).evaluate(async (el) => {
    const group = el.querySelector('[role="rowgroup"]')!;
    let elements = 0;
    const observer = new MutationObserver((records) => {
      for (const r of records) for (const n of r.addedNodes) if (n.nodeType === Node.ELEMENT_NODE) elements++;
    });
    observer.observe(group, { childList: true, subtree: true });
    for (let top = 0; top <= 600 * 26; top += 13) {
      el.scrollTop = top;
      await new Promise((r) => requestAnimationFrame(r));
    }
    await new Promise((r) => setTimeout(r, 300));
    observer.disconnect();
    return elements;
  });
  expect(added).toBe(0);

  // Commit 300 has five labels: two shown, and "+3" naming the others.
  await scrollTo(page, (600 - 300) * 26 - 100);
  const busy = row(page, 300);
  await expect(busy.locator(".subject")).toHaveText(bigSubject(300));
  await expect(busy.locator(".ref:not([hidden])")).toHaveCount(3);
  const more = busy.locator(".ref.more");
  await expect(more).toHaveText("+3");
  // The hidden labels, each described as a shown one is.
  await expect(more).toHaveAttribute("title", "Tag x3\nTag x4\nTag x5");
});

test("a drag of more than a screen a frame fills the rows now and then, and all of them once it stops", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  await page.waitForTimeout(300); // rows 0..440 are loaded ahead
  const seen = await grid(page).evaluate(async (el) => {
    const busy = () => {
      const box = el.getBoundingClientRect();
      return [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]:not([hidden])')].filter((r) => {
        const b = r.getBoundingClientRect();
        return b.bottom > box.top + 28 && b.top < box.bottom && r.getAttribute("aria-busy") === "true";
      }).length;
    };
    // Like a scrollbar drag over the loaded rows: down and up again, a screen and a half
    // per frame, for 60 frames.
    const screen = el.clientHeight;
    const frames: boolean[] = [];
    for (let k = 1; k <= 60; k++) {
      const step = k % 20;
      el.scrollTop = (k % 40 < 20 ? step : 20 - step) * 1.5 * screen;
      await new Promise((r) => requestAnimationFrame(r));
      await new Promise((r) => requestAnimationFrame(r));
      frames.push(busy() === 0);
    }
    // The frames until every row is filled: the order matters, not how long a loaded
    // machine takes (the 5 s bound only keeps a broken view from hanging the test).
    const stopped = performance.now();
    while (busy() > 0 && performance.now() - stopped < 5000) await new Promise((r) => requestAnimationFrame(r));
    return { filled: frames.filter(Boolean).length, after: busy() };
  });
  // Some frames of the drag were filled (before, none were until it stopped); then all are.
  // How many were held depends on the frame rate, so it is not asserted.
  expect(seen.filled).toBeGreaterThan(0);
  expect(seen.after).toBe(0);
});

test("scrolling a few px a frame, as a trackpad does, fills every row in every frame", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  await page.waitForTimeout(300);
  const busyFrames = await grid(page).evaluate(async (el) => {
    let frames = 0;
    for (let k = 1; k <= 120; k++) {
      el.scrollTop = k * 13;
      await new Promise((r) => requestAnimationFrame(r));
      const box = el.getBoundingClientRect();
      const busy = [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]:not([hidden])')].some((r) => {
        const b = r.getBoundingClientRect();
        return b.bottom > box.top + 28 && b.top < box.bottom && r.getAttribute("aria-busy") === "true";
      });
      if (busy) frames++;
    }
    return frames;
  });
  expect(busyFrames).toBe(0);
});

test("stashes: one row each with a stash label; the Stashes toggle hides them and is remembered", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("stashes"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  const [newest, older] = gg
    .git("stashes", "stash", "list", "--format=%H%x09%s")
    .trim()
    .split("\n")
    .map((l) => l.split("\t"));
  expect(older[1]).toBe("On main: Keep the parser draft");
  for (const [i, [hash, subj]] of [newest, older].entries()) {
    const r = row(page, i);
    await expect(subject(r)).toHaveText(subj);
    await expect(r.locator(".hash")).toHaveText(hash.slice(0, 8));
    const label = r.locator(`[data-ref="stash@{${i}}"]`);
    await expect(label).toBeVisible();
    await expect(label).toHaveClass(/\bstash\b/);
    await expect(label).toHaveAttribute("title", `Stash stash@{${i}}`);
  }
  await expect(subject(row(page, 2))).toHaveText(bigSubject(300));
  // Its own colour, not its lane's, in both themes: unlike a branch label.
  const colours = () =>
    page.evaluate(() => {
      const ink = (sel: string) => getComputedStyle(document.querySelector(sel)!).borderColor;
      return { stash: ink('[data-ref="stash@{1}"]'), branch: ink('[data-ref="main"]') };
    });
  const light = await colours();
  expect(light.stash).not.toBe(light.branch);
  await page.evaluate(() => (document.documentElement.dataset.theme = "dark"));
  const dark = await colours();
  expect(dark.stash).not.toBe(dark.branch);
  expect(dark.stash).not.toBe(light.stash);
  await page.evaluate(() => (document.documentElement.dataset.theme = "light"));

  const toggle = await viewOption(page, "Stashes");
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 1));
  await expect(grid(page).locator(".ref.stash:visible")).toHaveCount(0);
  await expect(subject(row(page, 0))).toHaveText(bigSubject(300));

  await page.reload();
  await expect(await viewOption(page, "Stashes")).not.toBeChecked();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 1));
  await (await viewOption(page, "Stashes")).click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  await expect(row(page, 1).locator('[data-ref="stash@{1}"]')).toBeVisible();
});

test("hiding and showing stashes keeps the selected commit and the first visible commit in place", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("stashes"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  await scrollTo(page, 120 * 26 + 7);
  await expect.poll(async () => (await firstVisible(page)).subject).toMatch(/^Big commit \d+$/);
  const before = await firstVisible(page);
  const target = grid(page).locator('[role="row"]', { has: page.getByText(bigSubject(170), { exact: true }) });
  await target.click();
  await expect(target).toHaveAttribute("aria-selected", "true");

  const toggle = await viewOption(page, "Stashes");
  await toggle.click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 1));
  await expect.poll(async () => (await firstVisible(page)).subject).toBe(before.subject);
  expect((await firstVisible(page)).scrollTop).toBe(before.scrollTop - 2 * 26);
  const selected = grid(page).locator('[role="row"][aria-selected="true"]');
  await expect(subject(selected)).toHaveText(bigSubject(170));

  await toggle.click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  await expect.poll(async () => (await firstVisible(page)).subject).toBe(before.subject);
  expect((await firstVisible(page)).scrollTop).toBe(before.scrollTop);
  await expect(subject(selected)).toHaveText(bigSubject(170));
  await expect(selected).toBeInViewport();
});

test("a stash saved outside the app appears at the top without moving the view", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");
  writeFileSync(path.join(gg.repoPath("small"), "src", "part1.txt"), "changed\n");
  gg.git("small", "stash", "push", "-q", "--message=Saved elsewhere");
  await expect(subject(row(page, 0))).toHaveText("On main: Saved elsewhere", { timeout: 2000 });
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();
  expect((await firstVisible(page)).scrollTop).toBe(0);
});

test("with a branch selection, the stashes on its commits show, and the Stashes toggle still works", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("stashes"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  await page.getByRole("button", { name: /^Branches:/ }).click();
  await page.getByRole("dialog", { name: "Branch filter" }).getByRole("option", { name: "main", exact: true }).click();
  await page.keyboard.press("Escape");
  // Both stashes are on commits of main.
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();
  const toggle = await viewOption(page, "Stashes");
  await expect(toggle).toBeEnabled();
  await toggle.click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 1));
  await toggle.click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(300 + 2 + 1));
});
