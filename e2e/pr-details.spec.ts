// A pull request's checks and comments in the side panel's Pull request tab, against the real
// core and the devserver's fake gh (never the real one, never the network; the browser is
// never started: opens are recorded). The `prs` fixture is on fix/typo, whose pull request is
// #31 (docs/readme's #32). The core's floor between two loads of one pull request is made
// 0 ms here (--pr-details-every-ms) and the tab's poll 400 ms, so loads can be counted.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";
import { answer, comment, coderabbit, review, run, status, THREADS_22, threads, view, viewCalls } from "./prDetailsData";

test.use({ viewport: { width: 1280, height: 900 }, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const POLL_MS = 400;
const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const tab = (page: Page, id: "commit" | "pr" | "worktrees") => page.getByTestId(`side-tab-${id}`);
const side = (page: Page) => page.locator('[data-region="side-panel"]');
const cards = (page: Page) => page.getByTestId("pr-comment");
const checksToggle = (page: Page) => page.getByTestId("pr-checks-toggle");

function listed(number: number, head: string, title: string) {
  return { number, title, url: `https://github.com/octo/cat/pull/${number}`, isDraft: false, headRefName: head, baseRefName: "main", headRepositoryOwner: { id: "X", login: "octo" }, isCrossRepository: false };
}

/** The conversation: ada's comment, CodeRabbit's (captured), a review, a thread with a reply. */
function talk() {
  const rabbit = coderabbit();
  return {
    comments: [comment("1", "ada", "Should we keep the old wording? See [the guide](https://docs.example.com/guide) and [this](javascript:alert(1)).", 90), ...rabbit.comments],
    reviews: [...rabbit.reviews, review("R9", "grace", "Approved: thanks.", "APPROVED", 10)],
    threads: threads(
      [
        {
          path: "src/parser.ts",
          line: 42,
          comments: [
            { id: "c1", login: "ada", body: "This loses the position.", minutesAgo: 80 },
            { id: "c2", login: "KhanShaheb34", body: "Fixed.", minutesAgo: 60 },
          ],
        },
        {
          path: "src/lib/components/Toolbar.svelte",
          line: 274,
          resolved: true,
          comments: [
            { id: "c3", login: "coderabbitai", bot: true, body: "**Minor**\n\nOpen the active worktree's commit panel.\n\nMore\n\nlines\n\nhere.", minutesAgo: 70 },
            { id: "c4", login: "KhanShaheb34", body: "Done.", minutesAgo: 50 },
          ],
        },
      ],
      ["coderabbitai"],
    ),
  };
}

const PASSING = () =>
  view(31, {
    review: "APPROVED",
    checks: [run("macos-latest", "passing"), run("ubuntu-latest", "passing"), status("ci/legacy", "SUCCESS")],
    comments: talk().comments,
    reviews: talk().reviews,
  });
const RUNNING = () => view(31, { merge: "UNSTABLE", checks: [run("macos-latest", "running"), run("lint", "passing"), run("windows-latest", "queued")] });
const FAILING = () =>
  view(31, {
    merge: "BLOCKED",
    review: "CHANGES_REQUESTED",
    checks: [run("macos-latest", "passing"), run("ubuntu-latest", "failing"), run("windows-latest", "running"), run("deploy", "skipped", 0, "Deploy")],
  });

/** Opens the `prs` fixture on fix/typo (#31 listed, docs/readme's #32 too), the Pull request tab shown. */
async function showPr(gg: GG, v: object | null = PASSING(), t: object = talk().threads, poll = POLL_MS, loaded = true) {
  const { page } = gg;
  await page.addInitScript((ms) => ((window as { __GG_PR_POLL_MS__?: number }).__GG_PR_POLL_MS__ = ms), poll);
  gg.git("prs", "switch", "-q", "fix/typo");
  gg.gh.reply("pr-list", 0, JSON.stringify([listed(31, "fix/typo", "Fix a typo"), listed(32, "docs/readme", "Explain the parser")]));
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  if (v !== null) answer(gg, 31, v, t);
  await page.goto("/");
  const dir = gg.repoPath("prs");
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.locator("button.pr-chip[data-pr='31']").first()).toBeVisible();
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(page.getByTestId("pr-view")).toBeVisible();
  if (loaded) await expect(page.getByTestId("pr-status")).toBeVisible();
}

/** gh answers #31 with `v` (and `t`) from now on; the tab's Refresh asks again. */
async function reload(gg: GG, v: object, t: object = threads([])) {
  answer(gg, 31, v, t);
  await gg.page.getByTestId("pr-refresh").click();
}

test("the status line, the checks and the comments, as gh gives them; Refresh asks again", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  // gh asked with the pull request's number and the repository, and the threads' query.
  const calls = gg.gh.calls();
  expect(calls.find((c) => c[0] === "pr" && c[1] === "view")).toEqual(["pr", "view", "31", "--repo", "octo/cat", "--json", expect.stringContaining("statusCheckRollup")]);
  const api = calls.find((c) => c[0] === "api")!;
  expect(api.slice(0, 2)).toEqual(["api", "graphql"]);
  expect(api).toContain("o=octo");
  expect(api).toContain("r=cat");
  expect(api).toContain("n=31");

  // Status: ready, approved; not buttons.
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  await expect(page.getByTestId("pr-review")).toHaveText("Approved");
  await expect(page.getByTestId("pr-status").getByRole("button")).toHaveCount(0);
  // All passing: the checks closed, summed up.
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("pr-checks-summary")).toHaveText("3 passing");
  await expect(page.getByTestId("pr-check")).toHaveCount(0);
  await checksToggle(page).click();
  await expect(page.getByTestId("pr-check")).toHaveCount(3);
  await expect(page.getByTestId("pr-check").first()).toContainText("macos-latest");
  await expect(page.getByTestId("pr-check").first()).toContainText(/\d+m/);
  // The comments: oldest first, bots tagged, a thread with its place and reply.
  await expect(page.getByTestId("pr-comments-count")).toHaveText("6");
  await expect(cards(page)).toHaveCount(6);
  await expect(cards(page).first()).toContainText("coderabbitai");
  const bots = page.locator('[data-testid="pr-comment"][data-bot="true"]');
  await expect(bots).toHaveCount(3);
  await expect(bots.first().locator(".bot-tag")).toHaveText("bot");
  const thread = page.locator('[data-testid="pr-comment"][data-kind="thread"]', { hasText: "parser.ts" });
  await expect(thread.getByTestId("pr-comment-context")).toHaveText(/src\/parser\.ts:42/);
  await expect(thread.getByTestId("pr-reply")).toContainText("Fixed.");
  // The review with an empty summary (the owner's) is left out; grace's is there.
  await expect(page.locator('[data-testid="pr-comment"][data-kind="review"]')).toHaveCount(2);

  // Running: open at once, said in the status line; the user's own choice then wins.
  await reload(gg, RUNNING());
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Checks running");
  await expect(page.getByTestId("pr-checks-summary")).toHaveText("2 running · 1 passing");
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("pr-check").filter({ hasText: "windows-latest" }).locator(".icon")).toHaveAttribute("aria-label", "Queued");
  const before = viewCalls(gg, 31);
  // Refresh: asks again at once, busy while it does.
  await reload(gg, FAILING());
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Blocked");
  await expect(page.getByTestId("pr-review")).toHaveText("Changes requested");
  expect(viewCalls(gg, 31)).toBeGreaterThan(before);
  // Failing first, then running, then the rest.
  await expect(page.getByTestId("pr-check")).toHaveCount(4);
  const states = await page.getByTestId("pr-check").evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.state));
  expect(states).toEqual(["failing", "pending", "passing", "skipped"]);
  await expect(page.getByTestId("pr-checks-summary")).toHaveText("1 failing · 1 running · 1 passing · 1 skipped");
  // No comments now.
  await expect(page.getByTestId("pr-comments-none")).toHaveText("No comments yet.");
});

test("the tab's dot follows the checks, red, amber, green, and says so in its tooltip", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg, FAILING(), threads([]), 60_000);
  const dot = tab(page, "pr").getByTestId("side-tab-dot");
  await expect(dot).toHaveAttribute("data-tone", "danger");
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request · #31 open · checks failing \(/);
  await reload(gg, RUNNING());
  await expect(dot).toHaveAttribute("data-tone", "warning");
  await expect(tab(page, "pr")).toHaveAttribute("title", /· checks running \(/);
  await reload(gg, PASSING(), talk().threads);
  await expect(dot).toHaveAttribute("data-tone", "success");
  await expect(tab(page, "pr")).toHaveAttribute("title", /· checks passing \(/);
  // Shown while another tab is.
  await tab(page, "commit").click();
  await expect(dot).toHaveAttribute("data-tone", "success");
  // No checks: no dot.
  await tab(page, "pr").click();
  await reload(gg, view(31));
  await expect(page.getByTestId("pr-checks-summary")).toHaveText("No checks");
  await expect(dot).toHaveCount(0);
});

test("while a check runs the tab asks again, and stops when all finish, when it is hidden and when the window is not focused", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg, RUNNING(), threads([]));
  // Asked again and again while it runs.
  const start = viewCalls(gg, 31);
  await expect.poll(() => viewCalls(gg, 31), { timeout: 10_000 }).toBeGreaterThanOrEqual(start + 3);
  // All finished: no more.
  answer(gg, 31, PASSING(), talk().threads);
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  const settled = viewCalls(gg, 31);
  await page.waitForTimeout(POLL_MS * 4);
  expect(viewCalls(gg, 31)).toBe(settled);
  // Nor for any other change of the repository (a file written: the status, a new snapshot).
  writeFileSync(path.join(gg.repoPath("prs"), "touched.txt"), "x\n");
  await expect(page.getByTestId("side-tab-commit").getByTestId("side-tab-dot")).toBeVisible();
  await page.waitForTimeout(POLL_MS * 2);
  expect(viewCalls(gg, 31)).toBe(settled);

  // Running again: another tab shown, no more.
  await reload(gg, RUNNING());
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Checks running");
  await tab(page, "commit").click();
  await page.waitForTimeout(POLL_MS);
  const hidden = viewCalls(gg, 31);
  await page.waitForTimeout(POLL_MS * 4);
  expect(viewCalls(gg, 31)).toBe(hidden);
  // Back on it: at once, and again.
  await tab(page, "pr").click();
  await expect.poll(() => viewCalls(gg, 31), { timeout: 10_000 }).toBeGreaterThanOrEqual(hidden + 2);

  // The window loses the focus: no more; back, asked again.
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.waitForTimeout(POLL_MS);
  const blurred = viewCalls(gg, 31);
  await page.waitForTimeout(POLL_MS * 4);
  expect(viewCalls(gg, 31)).toBe(blurred);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect.poll(() => viewCalls(gg, 31), { timeout: 10_000 }).toBeGreaterThan(blurred);

  // The side panel hidden: none either.
  await page.getByRole("button", { name: "Hide the side panel" }).click();
  await expect(side(page)).toHaveCount(0);
  await page.waitForTimeout(POLL_MS);
  const gone = viewCalls(gg, 31);
  await page.waitForTimeout(POLL_MS * 4);
  expect(viewCalls(gg, 31)).toBe(gone);
});

test("the filter: all, people, bots; long and resolved comments shortened with Show more; links open only https", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  const filter = page.getByTestId("pr-comments-filter");
  await expect(filter).toHaveAttribute("role", "radiogroup");
  await filter.getByRole("radio", { name: "Humans" }).click();
  await expect(cards(page)).toHaveCount(4); // ada's, grace's, and both threads (each has a person's reply)
  await expect(page.locator('[data-testid="pr-comment"][data-kind="comment"][data-bot="true"]')).toHaveCount(0);
  await filter.getByRole("radio", { name: "Bots" }).click();
  await expect(cards(page)).toHaveCount(3);
  // From the keyboard: the arrows move the choice.
  await filter.getByRole("radio", { name: "Bots" }).focus();
  await page.keyboard.press("ArrowLeft");
  await expect(filter.getByRole("radio", { name: "Humans" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowLeft");
  await expect(cards(page)).toHaveCount(6);

  // CodeRabbit's long comment: shortened, then whole, then shortened again.
  const long = page.locator('[data-testid="pr-comment"][data-kind="comment"]', { hasText: "Review limit reached" });
  const body = long.getByTestId("pr-comment-body");
  const more = long.getByTestId("pr-comment-more");
  await expect(more).toHaveText("Show more");
  await expect(more).toHaveAttribute("aria-expanded", "false");
  const short = (await body.boundingBox())!.height;
  expect(short).toBeLessThan(260);
  await more.click();
  await expect(more).toHaveText("Show less");
  await expect(more).toHaveAttribute("aria-expanded", "true");
  expect((await body.boundingBox())!.height).toBeGreaterThan(short);
  // Its disclosures are closed, and open from the keyboard.
  const walkthrough = long.locator("details", { hasText: "Walkthrough" }).first();
  await expect(walkthrough).not.toHaveAttribute("open", "");
  await walkthrough.locator("summary").first().focus();
  await page.keyboard.press("Enter");
  await expect(walkthrough).toHaveAttribute("open", "");
  await more.click();
  await expect(more).toHaveText("Show more");
  // Nothing of it is fetched or run.
  expect(await long.locator("img, script, iframe, style, a[href]").count()).toBe(0);

  // The resolved thread: its first lines, its reply hidden; Show more shows both.
  const resolved = page.locator('[data-testid="pr-comment"][data-kind="thread"]', { hasText: "Toolbar.svelte" });
  await expect(resolved.getByTestId("pr-comment-context")).toContainText("Resolved");
  await expect(resolved.getByTestId("pr-reply")).toHaveCount(0);
  await resolved.getByTestId("pr-comment-more").press("Enter");
  await expect(resolved.getByTestId("pr-reply")).toContainText("Done.");

  // Links: an https one opens in the browser (click or Enter), with its address in its tooltip;
  // a javascript: one is only text.
  const ada = page.locator('[data-testid="pr-comment"]', { hasText: "keep the old wording" });
  const guide = ada.getByRole("link", { name: "the guide" });
  await expect(guide).toHaveAttribute("title", "https://docs.example.com/guide");
  await guide.click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://docs.example.com/guide"))).toBe(true);
  const opened = gg.launches().length;
  await expect(ada.getByRole("link")).toHaveCount(1);
  await ada.getByText(/and this\./).click();
  await guide.focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => gg.launches().length).toBe(opened + 1);
  expect(gg.launches().every((l) => !l.some((a) => a.startsWith("javascript:")))).toBe(true);
  // A comment's page opens on GitHub.
  await ada.getByTestId("pr-comment-open").click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/31#issuecomment-1"))).toBe(true);
  // A check's page too.
  await checksToggle(page).press("Enter");
  await page.getByTestId("pr-check-open").first().click();
  await expect.poll(() => gg.launches().some((l) => l.some((a) => a.startsWith("https://github.com/octo/cat/actions/runs/1/")))).toBe(true);
});

test("an answer that comes late for another branch's pull request never shows on this one", async ({ gg }) => {
  const { page } = gg;
  answer(gg, 32, view(32, { checks: [run("docs", "passing")], comments: [comment("9", "ada", "About the docs.", 5)] }), threads([]));
  // #31's first answer is held back by the core.
  let release: () => void = () => {};
  const held = new Promise<void>((r) => (release = r));
  let asked = false;
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd: string; args: { number?: number } } | null;
    if (body?.cmd === "pr_details" && body.args.number === 31 && !asked) {
      asked = true;
      await held;
    }
    return route.continue();
  });
  gg.git("prs", "switch", "-q", "fix/typo");
  gg.gh.reply("pr-list", 0, JSON.stringify([listed(31, "fix/typo", "Fix a typo"), listed(32, "docs/readme", "Explain the parser")]));
  answer(gg, 31, FAILING(), threads([]));
  await page.goto("/");
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath("prs")));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.locator("button.pr-chip[data-pr='31']").first()).toBeVisible();
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(page.getByTestId("pr-details-loading")).toBeVisible();
  await expect.poll(() => asked).toBe(true);
  // The branch changes before #31's answer comes: #32's is shown.
  gg.git("prs", "switch", "-q", "docs/readme");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByTestId("pr-number")).toHaveText("#32");
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  await expect(cards(page)).toHaveText([/About the docs\./]);
  release();
  await page.waitForTimeout(600);
  await expect(page.getByTestId("pr-number")).toHaveText("#32");
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  await expect(page.getByTestId("pr-check")).toHaveCount(0);
  await expect(page.getByTestId("pr-checks-summary")).toHaveText("1 passing");
  await page.unrouteAll({ behavior: "wait" });
});

test("a load that fails keeps what was shown and says so once, in the tab, with Retry; the first one too", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-view-31", 1, "", "GraphQL: API rate limit exceeded for user ID 1.\n");
  await showPr(gg, null, talk().threads, 60_000, false);
  await expect(page.getByTestId("pr-details-failed")).toHaveText(/Could not load: GraphQL: API rate limit exceeded/);
  await expect(page.locator("[data-notices] .toast")).toHaveCount(0);
  // Retry, now that it works.
  answer(gg, 31, FAILING(), threads([]));
  await page.getByTestId("pr-details-retry").click();
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Blocked");
  await expect(page.getByTestId("pr-details-failed")).toHaveCount(0);
  // A reload that fails: the checks stay, the line says so.
  gg.gh.reply("pr-view-31", 1, "", "gh: Bad credentials\n");
  await page.getByTestId("pr-refresh").click();
  await expect(page.getByTestId("pr-details-failed")).toHaveText(/Could not refresh: gh: Bad credentials/);
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Blocked");
  await expect(page.getByTestId("pr-check")).toHaveCount(4);
  await expect(page.locator("[data-notices] .toast")).toHaveCount(0);
  // The threads alone failing: the rest shown, said in the comments.
  answer(gg, 31, PASSING(), talk().threads);
  gg.gh.reply("api-graphql-31", 1, "", "gh: Something went wrong while executing your query.\n");
  await page.getByTestId("pr-refresh").click();
  await expect(page.getByTestId("pr-threads-error")).toContainText("Code comments could not be loaded");
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  await expect(page.locator('[data-testid="pr-comment"][data-kind="thread"]')).toHaveCount(0);
});

test("at the panel's least width everything fits: the status, the checks, the filter and the comments, nothing scrolls sideways", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 800 });
  const v = PASSING() as ReturnType<typeof view>;
  v.statusCheckRollup = [run("windows-latest-with-a-very-long-job-name-indeed", "failing"), run("ubuntu-latest", "running")];
  v.mergeStateStatus = "BLOCKED";
  v.reviewDecision = "CHANGES_REQUESTED";
  await showPr(gg, v, talk().threads, 60_000);
  const s = (await side(page).boundingBox())!;
  expect(Math.round(s.width)).toBe(240);
  const inside = async (el: ReturnType<Page["locator"]>) => {
    const b = (await el.boundingBox())!;
    expect(b.x, await el.evaluate((e) => e.outerHTML.slice(0, 80))).toBeGreaterThanOrEqual(s.x);
    expect(b.x + b.width, await el.evaluate((e) => e.outerHTML.slice(0, 80))).toBeLessThanOrEqual(s.x + s.width + 0.5);
  };
  for (const el of [page.getByTestId("pr-merge-status"), page.getByTestId("pr-review"), checksToggle(page), page.getByTestId("pr-comments-filter")]) await inside(el);
  for (const row of await page.getByTestId("pr-check").all()) await inside(row);
  for (const card of await cards(page).all()) await inside(card);
  expect(await page.getByTestId("pr-view").evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  // Every check row on one line.
  for (const row of await page.getByTestId("pr-check").all()) expect((await row.boundingBox())!.height).toBeLessThanOrEqual(27);

  // In a narrow window, over the graph: the same.
  await page.getByRole("button", { name: "Hide the side panel" }).click();
  await page.setViewportSize({ width: 760, height: 700 });
  await page.getByRole("button", { name: "Side panel" }).click();
  await expect(side(page)).toHaveClass(/overlay/);
  await tab(page, "pr").click();
  await expect(page.getByTestId("pr-merge-status")).toBeVisible();
  expect(await page.getByTestId("pr-view").evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
});

test("from the keyboard: the checks' toggle, the filter, Show more and the links, in order", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg, PASSING(), talk().threads, 60_000);
  await page.getByRole("button", { name: "Copy link" }).focus();
  await page.keyboard.press("Tab");
  await expect(checksToggle(page)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("Space");
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("pr-comments-filter").getByRole("radio", { name: "All" })).toBeFocused();
  // Then the first comment's page button, and on through the comments.
  await page.keyboard.press("Tab");
  await expect(cards(page).first().getByTestId("pr-comment-open")).toBeFocused();
  // The checks keep the user's choice through a reload.
  await page.getByTestId("pr-refresh").click();
  await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "false");
  await checksToggle(page).click();
  await reload(gg, PASSING(), talk().threads);
  await expect(checksToggle(page)).toHaveAttribute("aria-expanded", "true");
});

test("a deployment bot's comment: no badge repeating its word, its button an ordinary link, nothing loaded", async ({ gg }) => {
  const { page } = gg;
  const body = [
    "| Project | Deployment | Updated (UTC) |",
    "| :--- | :----- | :------ |",
    "| [web](https://bot.example/web) | ![Ready](https://bot.example/ready.svg) [Ready](https://bot.example/web/inspect) | Nov 25, 2026 |",
    "",
    '<a href="https://bot.example/review"><img src="https://bot.example/review.svg" alt="Request Review"></a>',
    "",
    "Built with ![a cat](https://bot.example/cat.png) and ![](https://bot.example/x.png).",
  ].join("\n");
  await showPr(gg, view(31, { comments: [comment("7", "deploybot", body, 5)], reviews: [] }), threads([], ["deploybot"]), 60_000);
  const card = cards(page).first();
  await expect(card).toContainText("Ready");
  await expect(card).not.toContainText("[image");
  await expect(card.locator("td", { hasText: "Ready" })).toHaveText("Ready");
  // Images that are not dropped: their alt text, muted, named as not loaded.
  await expect(card.getByRole("img", { name: "Image, not loaded: a cat" })).toHaveText("a cat");
  await expect(card.getByRole("img", { name: "Image, not loaded", exact: true })).toHaveText("image");
  const button = card.getByRole("link", { name: "Request Review" });
  await expect(button).toHaveAttribute("title", "https://bot.example/review");
  await button.click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://bot.example/review"))).toBe(true);
  expect(await card.locator("img, a[href]").count()).toBe(0);
});

test("the captured threads answer reads too (a resolved thread with the owner's reply)", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg, view(31, { comments: [], reviews: [] }), THREADS_22, 60_000);
  const t = page.locator('[data-testid="pr-comment"][data-kind="thread"]');
  await expect(t).toHaveCount(1);
  await expect(t.getByTestId("pr-comment-context")).toContainText("Toolbar.svelte:274");
  await expect(t.getByTestId("pr-comment-context")).toContainText("Resolved");
  await expect(t).toHaveAttribute("data-bot", "true");
  await t.getByTestId("pr-comment-more").click();
  await expect(t.getByTestId("pr-reply")).toContainText("KhanShaheb34");
});
