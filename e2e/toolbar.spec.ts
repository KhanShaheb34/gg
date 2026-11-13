// The toolbar above the graph against real repositories: branch filter, remote branches and
// tags toggles, search, fetch and refresh.
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { BIG_COMMITS, bigSubject, expect, makeRepo, test } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const row = (page: Page, index: number) => grid(page).locator(`[role="row"][data-index="${index}"]`);
const subject = (r: Locator) => r.locator(".subject");
const filterButton = (page: Page) => page.getByRole("button", { name: /^Branches:/ });
const filterDialog = (page: Page) => page.getByRole("dialog", { name: "Branch filter" });
const option = (page: Page, name: string) =>
  filterDialog(page).getByRole("option", { name, exact: true });
const searchBox = (page: Page) => page.getByRole("searchbox", { name: "Search commits" });
const searchCount = (page: Page) => page.getByTestId("search-count");
const selectedRow = (page: Page) => grid(page).locator('[role="row"][aria-selected="true"]');

/** Subjects of all rows, top to bottom (the history must fit on screen). */
function subjects(page: Page) {
  return grid(page).evaluate((el) =>
    [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]')]
      .sort((a, b) => Number(a.dataset.index) - Number(b.dataset.index))
      .map((r) => r.querySelector(".subject")?.textContent ?? ""),
  );
}

/** Subjects git lists for `revs`, newest first, as the graph orders them. */
function gitSubjects(gg: { git(name: string, ...args: string[]): string }, repo: string, ...revs: string[]) {
  return gg.git(repo, "log", "--format=%s", "--date-order", ...revs).trim().split("\n");
}

test("selecting only one branch leaves exactly its commits; Show all restores them", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const all = gitSubjects(gg, "small", "--all");
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(all.length + 1));
  await expect(filterButton(page)).toHaveText(/Show all/);

  await filterButton(page).click();
  await expect(filterDialog(page)).toBeVisible();
  await expect(filterDialog(page).getByRole("combobox", { name: "Filter branches" })).toBeFocused();
  await option(page, "feature/login").click();
  await expect(option(page, "feature/login")).toHaveAttribute("aria-selected", "true");
  await expect(filterButton(page)).toHaveText(/1 branch/);

  const feature = gitSubjects(gg, "small", "feature/login");
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(feature.length + 1));
  await expect.poll(() => subjects(page)).toEqual(feature);

  await option(page, "Show all").click();
  await expect(filterButton(page)).toHaveText(/Show all/);
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(all.length + 1));
  await expect.poll(() => subjects(page)).toEqual(all);

  await page.keyboard.press("Escape");
  await expect(filterDialog(page)).toHaveCount(0);
  await expect(filterButton(page)).toBeFocused();
});

test("the branch filter works from the keyboard", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await filterButton(page).focus();
  await page.keyboard.press("Enter");
  const input = filterDialog(page).getByRole("combobox", { name: "Filter branches" });
  await expect(input).toBeFocused();
  await page.keyboard.type("typo");
  await expect(filterDialog(page).getByRole("option")).toHaveText(["Show all", "fix/typo", "origin/fix/typo"]);
  // Typing makes the first matching branch the active one; arrows move it.
  await expect(option(page, "fix/typo")).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowDown");
  await expect(option(page, "origin/fix/typo")).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowUp");
  await expect(option(page, "fix/typo")).toHaveAttribute("data-active", "true");
  await expect(input).toHaveAttribute("aria-activedescendant", /.+/);
  // Sections are labelled groups; Home and End go to the first and last option.
  await expect(filterDialog(page).getByRole("group", { name: "Local branches", exact: true }).getByRole("option")).toHaveText(["fix/typo"]);
  await expect(filterDialog(page).getByRole("group", { name: "Remote branches", exact: true }).getByRole("option")).toHaveText([
    "origin/fix/typo",
  ]);
  await page.keyboard.press("End");
  await expect(option(page, "origin/fix/typo")).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Home");
  await expect(option(page, "Show all")).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(option(page, "fix/typo")).toHaveAttribute("aria-selected", "true");
  await expect(filterButton(page)).toHaveText(/1 branch/);
  await expect.poll(() => subjects(page)).toEqual(gitSubjects(gg, "small", "fix/typo"));

  // Outside clicks close it too.
  await page.mouse.click(700, 500);
  await expect(filterDialog(page)).toHaveCount(0);
});

test("the branch filter stays quick with thousands of branches", async ({ gg }) => {
  const { page } = gg;
  const dir = makeRepo("many", (git, write) => {
    git("init", "-q", "-b", "main");
    git("commit", "-q", "--allow-empty", "-m", "Only commit");
    const head = git("rev-parse", "HEAD").trim();
    const lines = Array.from({ length: 2500 }, (_, i) => `${head} refs/heads/topic/b-${String(i).padStart(4, "0")}`);
    write(".git/packed-refs", `# pack-refs with: peeled fully-peeled \n${lines.join("\n")}\n`);
  });
  await page.goto("/");
  await open(page, dir);
  await expect(subject(row(page, 0))).toHaveText("Only commit");

  await filterButton(page).click();
  const options = filterDialog(page).getByRole("option");
  await expect(options.first()).toBeVisible();
  expect(await options.count()).toBeLessThan(60); // only what is in view is rendered

  await page.keyboard.type("b-2499");
  await expect(options).toHaveText(["Show all", "topic/b-2499"]);
  await filterDialog(page).getByRole("combobox", { name: "Filter branches" }).fill("");
  // The last branch can be reached by scrolling the list.
  const input = filterDialog(page).getByRole("combobox", { name: "Filter branches" });
  await page.keyboard.press("ArrowDown");
  await expect(input).toHaveAttribute("aria-activedescendant", /.+/);
  await filterDialog(page)
    .getByRole("listbox")
    .evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
  await expect(option(page, "topic/b-2499")).toBeVisible();
  // The active option scrolled out of the rendered rows: the field no longer points at it.
  await expect(input).not.toHaveAttribute("aria-activedescendant", /.+/);
  // End goes to the last one, and points at it.
  await page.keyboard.press("End");
  await expect(option(page, "topic/b-2499")).toHaveAttribute("data-active", "true");
  const active = (await input.getAttribute("aria-activedescendant"))!;
  await expect(page.locator(`[id="${active}"]`)).toHaveText("topic/b-2499");
});

test("unticking Remote branches removes origin from every label", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  const labels = grid(page).locator("[data-ref]");
  await expect(labels.filter({ hasText: "origin" }).first()).toBeVisible();

  const remotes = page.getByRole("checkbox", { name: "Remote branches" });
  await expect(remotes).toBeChecked();
  await remotes.click();
  await expect(remotes).not.toBeChecked();
  await expect(labels.filter({ hasText: "origin" })).toHaveCount(0);
  await expect(labels.filter({ hasText: "develop" })).toHaveCount(1);

  // Tags go the same way.
  const tags = page.getByRole("checkbox", { name: "Tags" });
  await expect(grid(page).locator('[data-ref="v1.0"]')).toHaveCount(1);
  await tags.click();
  await expect(grid(page).locator('[data-ref="v1.0"]')).toHaveCount(0);
});

test("search: n of m, Enter and Shift+Enter move between hits, Esc closes and clears", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(BIG_COMMITS + 1));

  await page.keyboard.press("ControlOrMeta+f");
  await expect(searchBox(page)).toBeFocused();
  await page.keyboard.type("Big commit 15");
  // 15, 150-159 and 1500-1599; the newest (1599) is the first row hit.
  await expect(searchCount(page)).toHaveText("1 of 111");
  await expect(subject(selectedRow(page))).toHaveText(bigSubject(1599));
  await expect(selectedRow(page)).toBeInViewport();
  await expect(subject(selectedRow(page)).locator("mark").first()).toHaveText("Big commit 15");

  await page.keyboard.press("Enter");
  await expect(searchCount(page)).toHaveText("2 of 111");
  await expect(subject(selectedRow(page))).toHaveText(bigSubject(1598));
  await expect(selectedRow(page)).toBeInViewport();
  await expect(searchBox(page)).toBeFocused();

  await page.keyboard.press("Shift+Enter");
  await page.keyboard.press("Shift+Enter");
  await expect(searchCount(page)).toHaveText("111 of 111");
  await expect(subject(selectedRow(page))).toHaveText(bigSubject(15));
  await expect(selectedRow(page)).toBeInViewport();
  // Every hit row carries a marker; other rows do not.
  await expect(selectedRow(page)).toHaveAttribute("data-hit", "true");

  await page.keyboard.press("Escape");
  await expect(searchBox(page)).toHaveCount(0);
  await expect(grid(page).locator("mark:not(:empty)")).toHaveCount(0);
  await expect(grid(page).locator('[data-hit="true"]')).toHaveCount(0);
  await expect(subject(selectedRow(page))).toHaveText(bigSubject(15));
});

test("search marks what matched: the subject, the author or the hash", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("LOVELACE");
  await expect(searchCount(page)).toHaveText("1 of 30");
  await expect(row(page, 0).locator(".author mark")).toHaveText("Lovelace");
  await expect(row(page, 0).locator(".subject mark")).toHaveText("");

  const hash = gg.git("small", "rev-parse", "HEAD~3").trim();
  await searchBox(page).fill(hash.slice(0, 6));
  await expect(searchCount(page)).toHaveText("1 of 1");
  await expect(selectedRow(page).locator(".hash mark")).toHaveText(hash.slice(0, 6));
  await expect(selectedRow(page).locator(".hash")).toHaveText(hash.slice(0, 8));
});

test("search finds nothing: says so", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await page.getByRole("button", { name: "Search commits" }).click();
  await expect(searchBox(page)).toBeFocused();
  await page.keyboard.type("zzzz-nothing");
  await expect(searchCount(page)).toHaveText("No results");
});

test("fetch brings a commit pushed to origin from elsewhere: origin/main moves", async ({ gg }) => {
  const { page } = gg;
  const small = gg.repoPath("small");
  await page.goto("/");
  await open(page, small);
  const originMain = grid(page).locator('[role="row"]', { has: page.locator('[data-ref="origin/main"]') });
  await expect(subject(originMain)).toHaveText("Merge branch 'feature/login'");

  const origin = path.join(path.dirname(small), "small-origin.git");
  makeRepo("pusher", (git) => {
    git("clone", "-q", origin, ".");
    git("commit", "-q", "--allow-empty", "-m", "Pushed from elsewhere");
    git("push", "-q", "origin", "HEAD:main");
  });

  const fetch = page.getByRole("button", { name: "Fetch" });
  await fetch.click();
  await expect(subject(originMain)).toHaveText("Pushed from elsewhere", { timeout: 10_000 });
  await expect(fetch).toHaveAttribute("data-state", "idle");
  await fetch.hover();
  await expect(fetch).toHaveAttribute("title", /Last fetched just now/);
});

test("a fetch from a broken remote shows the warning with git's message", async ({ gg }) => {
  const { page } = gg;
  gg.git("small", "remote", "set-url", "origin", "/nonexistent/gg-nowhere.git");
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const fetch = page.getByRole("button", { name: "Fetch" });
  await fetch.click();
  await expect(fetch).toHaveAttribute("data-state", "error", { timeout: 10_000 });
  await expect(fetch.locator(".warn-dot")).toBeVisible();
  await expect(fetch).toHaveAttribute("title", /does not appear to be a git repository/);
  await expect(page.getByRole("alert").filter({ hasText: "Fetch failed" })).toContainText(
    "does not appear to be a git repository",
  );
});

test("Cmd/Ctrl+R refreshes without reloading the page", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await page.evaluate(() => ((window as unknown as { __marker: number }).__marker = 42));
  await page.keyboard.press("ControlOrMeta+r"); // also on the welcome screen
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Add part 19");

  let refreshes = 0;
  page.on("request", (r) => {
    if (r.method() === "POST" && r.postData()?.includes('"refresh"')) refreshes++;
  });
  await grid(page).click();
  await page.keyboard.press("ControlOrMeta+r");
  await expect.poll(() => refreshes).toBe(1);
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => (window as unknown as { __marker?: number }).__marker)).toBe(42);

  // The button shows the refresh running, then takes the next one.
  const button = page.getByRole("button", { name: "Refresh" });
  await expect(button).toHaveAttribute("aria-busy", "false");
  await button.click();
  await expect(button).toHaveAttribute("aria-busy", "true");
  await expect.poll(() => refreshes).toBe(2);
  await expect(button).toHaveAttribute("aria-busy", "false");
});

test("the branch filter is remembered: across a page reload and after the tab is closed", async ({ gg }) => {
  const { page } = gg;
  const small = gg.repoPath("small");
  const feature = gitSubjects(gg, "small", "feature/login");
  await page.goto("/");
  await open(page, small);
  await filterButton(page).click();
  await option(page, "feature/login").click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(feature.length + 1));
  await page.keyboard.press("Escape");

  await page.reload();
  await expect(filterButton(page)).toHaveText(/1 branch/);
  await expect.poll(() => subjects(page)).toEqual(feature);

  // Closed, the core forgets the repository; reopened, the filter comes back from storage.
  await page.getByRole("button", { name: "Close small" }).click();
  await open(page, small);
  await expect(filterButton(page)).toHaveText(/1 branch/);
  await expect.poll(() => subjects(page)).toEqual(feature);
});

test("changing the filter keeps the selected commit selected and in view", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const target = gg.git("small", "log", "-1", "--format=%s", "feature/login~2").trim();
  const r = grid(page).locator('[role="row"]', { has: page.getByText(target, { exact: true }) });
  await r.click();
  await expect(r).toHaveAttribute("aria-selected", "true");

  await filterButton(page).click();
  await option(page, "feature/login").click();
  await expect(grid(page)).toHaveAttribute("aria-rowcount", String(gitSubjects(gg, "small", "feature/login").length + 1));
  await expect(subject(selectedRow(page))).toHaveText(target);
  await expect(selectedRow(page)).toBeInViewport();
});

test("filtering to one branch and back never moves the Description column", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  await expect(subject(row(page, 0))).not.toBeEmpty();
  const description = page.getByRole("columnheader", { name: "Description" });
  const graphHeader = page.getByRole("columnheader", { name: "Graph" });
  const x = (await description.boundingBox())!.x;

  await filterButton(page).click();
  await option(page, "feature/search").click(); // a single lane
  await expect(subject(row(page, 0))).toHaveText("Search keyboard shortcuts");
  expect((await description.boundingBox())!.x).toBe(x);
  await expect(graphHeader.locator(".hlabel")).toBeVisible();

  await option(page, "Show all").click();
  await expect(subject(row(page, 0))).toHaveText("Row hover states");
  expect((await description.boundingBox())!.x).toBe(x);
});

test("a filtered repository opens with a Graph column wide enough for its header", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await filterButton(page).click();
  await option(page, "fix/typo").click();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Close small" }).click();
  await open(page, gg.repoPath("small"));
  await expect(subject(row(page, 0))).toHaveText("Fix typo 4");
  const graph = (await page.getByRole("columnheader", { name: "Graph" }).boundingBox())!;
  expect(graph.width).toBeGreaterThanOrEqual(52);
  await expect(page.getByRole("columnheader", { name: "Graph" }).locator(".hlabel")).toBeVisible();
});
