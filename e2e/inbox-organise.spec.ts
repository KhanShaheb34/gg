// The Pull requests page, organised: the list painted before its details come, and those read
// only for the rows that show; its scroll bars at the window's edge; grouping by role,
// repository or status, kept; the page keeping its place; pull requests not updated for a
// month apart and folded; pins; a pull request's files. Against the real core and the
// devserver's fake gh (never the real one, never the network).
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, openFolder, test } from "./fixtures";
import { detailsCalls, idOf, inboxCalls, replyDetails, replyFiles, replyInbox, type Node } from "./inboxData";
import { answer, view } from "./prDetailsData";

test.use({ viewport: { width: 1180, height: 820 }, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const DAY = 24 * 60;
const button = (page: Page) => page.getByTestId("inbox-button");
const rows = (page: Page) => page.getByTestId("inbox-pr");
const row = (page: Page, key: string) => page.locator(`[data-testid="inbox-pr"][data-key="${key}"]`);
const group = (page: Page, id: string) => page.locator(`[data-testid="inbox-group"][data-group="${id}"]`);
const list = (page: Page) => page.getByRole("listbox", { name: "Pull requests" });
const groupBy = (page: Page, label: string) => page.getByTestId("inbox-group-by").getByRole("radio", { name: label });

async function openPage(gg: GG) {
  await gg.page.goto("/");
  await expect(gg.page.getByTestId("welcome")).toBeVisible();
  await button(gg.page).click();
  await expect(gg.page.getByTestId("inbox")).toBeVisible();
}

/** Two recent, two a month and more old, in two repositories and a third with only old ones. */
const AGED: { mine: Node[]; review: Node[]; mentioned: Node[] } = {
  mine: [
    { number: 1, title: "Recent of mine", author: "octo", minutesAgo: 60 },
    { number: 2, title: "Old of mine", author: "octo", minutesAgo: 45 * DAY },
  ],
  review: [
    { number: 3, repo: "hub/dog", title: "Recent to review", checks: "FAILURE", review: "CHANGES_REQUESTED", minutesAgo: 120 },
    { number: 4, repo: "hub/dog", title: "Old to review", minutesAgo: 31 * DAY },
  ],
  mentioned: [{ number: 5, repo: "hub/owl", title: "Old mention", minutesAgo: 90 * DAY }],
};

test("the list paints before its details, which are read only for the rows that show", async ({ gg }) => {
  const { page } = gg;
  const lean = (n: Node): Node => ({ ...n, lean: true });
  replyInbox(gg, { mine: AGED.mine.map(lean), review: AGED.review.map(lean), mentioned: AGED.mentioned.map(lean) });
  // As GitHub answers: the ones asked about (the recent two) only.
  replyDetails(gg, [AGED.mine[0], AGED.review[0]]);
  gg.gh.slow("gql-inbox-details", 2);
  await openPage(gg);
  // The rows, before the details: no checks yet.
  await expect(row(page, "github.com/hub/dog#3")).toBeVisible();
  await expect(row(page, "github.com/hub/dog#3").getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "neutral");
  // Then the details fill in.
  await expect(row(page, "github.com/hub/dog#3").getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "danger", { timeout: 10_000 });
  await expect(row(page, "github.com/hub/dog#3").getByTestId("inbox-review")).toHaveAttribute("data-review", "changes_requested");
  // Asked for the two recent ones only: the old ones are folded away.
  expect(inboxCalls(gg)).toHaveLength(1);
  const asked = detailsCalls(gg);
  expect(asked).toHaveLength(1);
  expect(asked[0].sort()).toEqual([idOf({ number: 1 }), idOf({ number: 3, repo: "hub/dog" })].sort());
  // Unfolded, the old ones' details are read then.
  gg.gh.fast("gql-inbox-details");
  replyDetails(gg, [AGED.mine[1], AGED.review[1], AGED.mentioned[0]]);
  await group(page, "older").click();
  await expect.poll(() => detailsCalls(gg).length).toBe(2);
  expect(detailsCalls(gg)[1].sort()).toEqual([idOf({ number: 2 }), idOf({ number: 4, repo: "hub/dog" }), idOf({ number: 5, repo: "hub/owl" })].sort());
});

test("the list's and the pull request page's scroll bars are at the window's edge; what they hold is centred", async ({ gg }) => {
  const { page } = gg;
  const many: Node[] = Array.from({ length: 40 }, (_, i) => ({ number: i + 1, repo: "hub/dog", minutesAgo: i }));
  replyInbox(gg, { review: many });
  answer(gg, 1, view(1, { comments: [], reviews: [] }));
  await openPage(gg);
  const width = page.viewportSize()!.width;
  const box = (await list(page).boundingBox())!;
  expect(Math.round(box.x)).toBe(0);
  expect(Math.round(box.x + box.width)).toBe(width);
  expect(await list(page).evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
  // The rows, centred: as far from either edge.
  const first = (await rows(page).first().boundingBox())!;
  expect(Math.abs(first.x - (width - first.x - first.width))).toBeLessThanOrEqual(2);
  expect(first.width).toBeLessThan(width);
  await rows(page).first().click();
  const scroller = page.getByTestId("inbox-pr-scroll");
  const s = (await scroller.boundingBox())!;
  expect(Math.round(s.x)).toBe(0);
  expect(Math.round(s.x + s.width)).toBe(width);
});

test("old pull requests (by their last update) go to one folded section at the end; by repository, behind Show N older", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, AGED);
  await openPage(gg);
  // By role: the recent ones in their groups; the three old ones folded at the end.
  await expect(rows(page)).toHaveCount(2);
  await expect(group(page, "older")).toHaveAttribute("data-open", "false");
  await expect(group(page, "older")).toContainText("Older than a month");
  await expect(group(page, "older")).toContainText("3");
  await group(page, "older").click();
  await expect(rows(page)).toHaveCount(5);
  await expect(rows(page).locator(".title")).toHaveText(["Recent of mine", "Recent to review", "Old to review", "Old of mine", "Old mention"]);
  // By repository: hub/dog's old one behind Show 1 older; octo/cat's too; hub/owl (only old) behind one more row.
  await groupBy(page, "Repository").click();
  // Newest first: octo/cat (an hour ago), then hub/dog (two).
  await expect(page.getByTestId("inbox-group")).toHaveText([/octo\/cat\s*2/, /hub\/dog\s*2/]);
  await expect(page.getByTestId("inbox-more")).toHaveText(["Show 1 older one", "Show 1 older one", "Show 1 more repository (only older ones)"]);
  await expect(rows(page)).toHaveCount(2);
  await page.getByTestId("inbox-more").first().click();
  await expect(rows(page)).toHaveCount(3);
  await page.locator('[data-testid="inbox-more"][data-id="repos"]').click();
  await expect(group(page, "repo:github.com/hub/owl")).toBeVisible();
  await expect(row(page, "github.com/hub/owl#5")).toBeVisible();
  // By status: what needs attention (failing checks, changes requested) first; old ones folded again.
  await groupBy(page, "Status").click();
  await expect(page.getByTestId("inbox-group").first()).toContainText("Needs attention");
  await expect(rows(page).first()).toHaveAttribute("data-key", "github.com/hub/dog#3");
});

test("the grouping is kept, in gg's config, across runs", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, AGED);
  await openPage(gg);
  await groupBy(page, "Repository").click();
  await expect(groupBy(page, "Repository")).toHaveAttribute("aria-checked", "true");
  // Kept in gg's own config folder (never a repository's).
  const file = path.join(gg.home, "config", "pr-inbox.json");
  await expect.poll(() => (existsSync(file) ? JSON.parse(readFileSync(file, "utf8")).groupBy : null)).toBe("repository");
  await gg.relaunch();
  await button(page).click();
  await expect(groupBy(page, "Repository")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("inbox-group").first()).toContainText("octo/cat");
});

test("the page keeps its place: a pull request left for its repository is there on return, and Escape goes back to the list where it was", async ({ gg }) => {
  const { page } = gg;
  const many: Node[] = Array.from({ length: 40 }, (_, i) => ({ number: i + 1, minutesAgo: i, head: "docs/readme" }));
  replyInbox(gg, { review: many });
  answer(gg, 25, view(25, { comments: [], reviews: [] }));
  await page.goto("/");
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath("prs")));
  await openFolder(page);
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await button(page).click();
  await list(page).evaluate((el) => (el.scrollTop = 900));
  await row(page, "github.com/octo/cat#25").click();
  await expect(page.getByTestId("pr-view-title")).toHaveText("Change 25");
  await page.getByTestId("inbox-go-to-repo").click();
  await expect(page.getByTestId("repo-view")).toBeVisible();
  // Back: the same pull request's page.
  await button(page).click();
  await expect(page.getByTestId("inbox-pr-page")).toBeVisible();
  await expect(page.getByTestId("pr-view-title")).toHaveText("Change 25");
  // Escape: the list, scrolled as it was, the same one selected.
  await page.keyboard.press("Escape");
  await expect(list(page)).toBeFocused();
  expect(await list(page).evaluate((el) => el.scrollTop)).toBeGreaterThan(800);
  await expect(row(page, "github.com/octo/cat#25")).toHaveAttribute("aria-selected", "true");
});

test("pins: from the row, the keyboard, the menu and the pull request's page; first in every grouping; kept across runs, a merged one too", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, AGED);
  await openPage(gg);
  // The row's pin (on hover): to the Pinned section, not repeated below.
  await row(page, "github.com/hub/dog#3").hover();
  await row(page, "github.com/hub/dog#3").getByTestId("inbox-pin").click();
  await expect(group(page, "pinned")).toContainText("1");
  await expect(rows(page).first()).toHaveAttribute("data-key", "github.com/hub/dog#3");
  await expect(row(page, "github.com/hub/dog#3")).toHaveCount(1);
  // From its page: the old #2 is pinned, shown first whatever its age.
  await group(page, "older").click();
  await row(page, "github.com/octo/cat#2").click();
  await page.getByTestId("inbox-pr-pin").click();
  await expect(page.getByTestId("inbox-pr-pin")).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Escape");
  await expect(group(page, "pinned")).toContainText("2");
  // The menu: unpin #3.
  await row(page, "github.com/hub/dog#3").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Unpin" }).click();
  await expect(group(page, "pinned")).toContainText("1");
  // P from the keyboard pins the one selected.
  await list(page).focus();
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowDown");
  const selected = await page.locator('[data-testid="inbox-pr"][aria-selected="true"]').getAttribute("data-key");
  await page.keyboard.press("p");
  await expect(group(page, "pinned")).toContainText("2");
  for (const by of ["Repository", "Status", "Role"]) {
    await groupBy(page, by).click();
    await expect(page.getByTestId("inbox-group").first()).toHaveAttribute("data-group", "pinned");
    // Pinned is no repository: its rows keep theirs, whatever the grouping.
    await expect(row(page, "github.com/octo/cat#2").locator(".repo")).toHaveText("octo/cat");
  }
  // Kept across runs; #2 merged since and no list has it: still pinned, as merged.
  const { mine, ...rest } = AGED;
  replyInbox(gg, { ...rest, mine: mine.filter((n) => n.number !== 2) });
  replyDetails(gg, [{ number: 2, state: "MERGED" }, ...rest.review]);
  await gg.relaunch();
  await button(page).click();
  await expect(group(page, "pinned")).toContainText("2");
  await expect(row(page, "github.com/octo/cat#2").getByTestId("inbox-state-icon")).toHaveAttribute("data-state", "merged", { timeout: 10_000 });
  expect(selected).not.toBeNull();
  await expect(row(page, selected!)).toBeVisible();
});

test("a pull request's files: as Committed on branch lists them, renames, and each state", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, { review: [{ number: 3, repo: "hub/dog" }, { number: 6, repo: "hub/dog" }, { number: 7, repo: "hub/dog" }, { number: 8, repo: "hub/dog" }] });
  for (const n of [3, 6, 7, 8]) answer(gg, n, view(n, { comments: [], reviews: [] }));
  replyFiles(gg, 3, [
    [
      { filename: "src/search/index.ts", status: "modified", additions: 12, deletions: 3, changes: 15, patch: "@@" },
      { filename: "docs/search.md", previous_filename: "docs/find.md", status: "renamed", additions: 0, deletions: 0, changes: 0 },
    ],
    [{ filename: "src/old.ts", status: "removed", additions: 0, deletions: 40, changes: 40, patch: "@@" }],
  ]);
  replyFiles(gg, 6, [[]]);
  replyFiles(gg, 7, [], 1, "HTTP 404: Not Found (https://api.github.com/repos/hub/dog/pulls/7/files)");
  replyFiles(gg, 8, [Array.from({ length: 120 }, (_, i) => ({ filename: `gen/f${i}.ts`, status: "added", additions: 1, deletions: 0, changes: 1, patch: "@@" }))]);
  await openPage(gg);
  await row(page, "github.com/hub/dog#3").click();
  const files = page.getByTestId("pr-files");
  await expect(files).toHaveAttribute("data-state", "listed");
  await expect(page.getByTestId("pr-files-count")).toHaveText("3");
  await expect(page.getByTestId("pr-file")).toHaveCount(3);
  const renamed = page.locator('[data-testid="pr-file"][data-path="docs/search.md"]');
  await expect(renamed).toHaveAttribute("data-status", "R");
  await expect(renamed).toContainText("← docs/find.md");
  await expect(page.locator('[data-testid="pr-file"][data-path="src/old.ts"]')).toHaveAttribute("data-status", "D");
  // Closes like Checks.
  await page.getByTestId("pr-files-toggle").click();
  await expect(page.getByTestId("pr-file")).toHaveCount(0);
  // Asked with every page, by the repository's name.
  const call = gg.gh.calls().find((c) => c[0] === "api" && c[1] === "--hostname")!;
  expect(call).toEqual(["api", "--hostname", "github.com", "--paginate", "repos/hub/dog/pulls/3/files?per_page=100"]);

  await page.getByTestId("inbox-back").click();
  await row(page, "github.com/hub/dog#6").click();
  await expect(page.getByTestId("pr-files-none")).toHaveText("No files changed.");
  await page.getByTestId("inbox-back").click();
  await row(page, "github.com/hub/dog#7").click();
  await expect(page.getByTestId("pr-files-failed")).toContainText("HTTP 404");
  // Very many: closed until opened.
  await page.getByTestId("inbox-back").click();
  await row(page, "github.com/hub/dog#8").click();
  await expect(page.getByTestId("pr-files-count")).toHaveText("120");
  await expect(page.getByTestId("pr-file")).toHaveCount(0);
  await page.getByTestId("pr-files-toggle").click();
  await expect(page.getByTestId("pr-file")).toHaveCount(120);
});
