// The Pull requests page across repositories, against the real core and the devserver's fake gh
// (never the real one, never the network): its groups from one query, the states it says
// (gh missing, signed out, rate limit, offline, empty, failed), the list kept between runs, a
// pull request's own page (its checks and comments, a reply from it), going to the repository
// it is open in and checking out its branch there, cloning one that is not (from a local
// folder), the keyboard, a narrow window and hundreds of rows.
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";
import { MIX, inboxCalls, replyInbox, type Node } from "./inboxData";
import { answer, comment, threads, view } from "./prDetailsData";

test.use({ viewport: { width: 1180, height: 820 }, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const button = (page: Page) => page.getByTestId("inbox-button");
const rows = (page: Page) => page.getByTestId("inbox-pr");
const row = (page: Page, key: string) => page.locator(`[data-testid="inbox-pr"][data-key="${key}"]`);
const list = (page: Page) => page.getByRole("listbox", { name: "Pull requests" });

/** Opens the page from the tab bar, with no repository open. */
async function openPage(gg: GG) {
  await gg.page.goto("/");
  await expect(gg.page.getByTestId("welcome")).toBeVisible();
  await button(gg.page).click();
  await expect(gg.page.getByTestId("inbox")).toBeVisible();
}

/** Opens fixture repository `name` in a tab. */
async function openRepo(gg: GG, name: string) {
  const { page } = gg;
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath(name)));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

test("the groups come from one gh query, each pull request once, with no repository open", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  await openPage(gg);
  await expect(rows(page)).toHaveCount(7);
  const groups = page.getByTestId("inbox-group");
  await expect(groups).toHaveText([/Your pull requests\s*2/, /Waiting for your review\s*3/, /Mentioned or assigned\s*2/]);
  // In order, newest first in each group.
  await expect(rows(page).locator(".title")).toHaveText([
    "Fix a typo in the README",
    "Add the login form",
    "Speed up the search index",
    "Explain the parser in the docs",
    "Read the settings file once",
    "Rename the toolbar's buttons",
    "Tidy the release notes",
  ]);
  const typo = row(page, "github.com/octo/cat#31");
  await expect(typo.locator(".repo")).toHaveText("octo/cat");
  await expect(typo.locator(".number")).toHaveText("#31");
  // Its state in its icon's colour and words: passing checks green.
  await expect(typo.getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "success");
  await expect(typo.getByTestId("inbox-state-icon")).toHaveAttribute("title", "Checks passing");
  await expect(typo.getByTestId("inbox-review")).toHaveAttribute("data-review", "approved");
  // Each row starts with its state: open, or a draft.
  await expect(typo.getByTestId("inbox-state-icon")).toHaveAttribute("data-state", "open");
  const draft = row(page, "github.com/octo/cat#29");
  await expect(draft.getByTestId("inbox-state-icon")).toHaveAttribute("data-state", "draft");
  await expect(draft.getByTestId("inbox-state-icon")).toHaveAttribute("title", "Draft");
  // Titles start at one edge, whatever the row says before them.
  const lefts = await rows(page).locator(".title").evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().left)));
  expect(new Set(lefts).size).toBe(1);
  await expect(draft.getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "muted");
  const failing = row(page, "github.com/hub/dog#42");
  await expect(failing.getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "danger");
  await expect(failing.getByTestId("inbox-state-icon")).toHaveAttribute("title", "Checks failing · changes requested");
  await expect(failing.getByTestId("inbox-review")).toContainText("Changes requested");
  await expect(failing.locator(".by")).toHaveText("ada");
  await expect(row(page, "github.com/hub/owl#7").getByTestId("inbox-state-icon")).toHaveAttribute("data-tone", "neutral");
  // No separate checks badge: the icon says it.
  await expect(page.getByTestId("inbox-checks")).toHaveCount(0);
  await expect(row(page, "github.com/hub/dog#12").locator(".why")).toHaveText("Mentioned");
  await expect(row(page, "github.com/hub/owl#5").locator(".why")).toHaveText("Assigned");
  // The first load is the starting point: nothing new.
  await expect(page.locator('[data-testid="inbox-pr"][data-activity="none"]')).toHaveCount(7);
  await expect(page.getByTestId("inbox-unread")).toHaveCount(0);
  await expect(page.getByTestId("inbox-accounts")).toHaveText("@octo");
  // One query for the list, for github.com; then one for the details of the rows shown; nothing else of gh.
  const calls = inboxCalls(gg);
  expect(calls).toHaveLength(1);
  expect(calls[0].slice(-2)).toEqual(["-F", "first=100"]);
  await expect.poll(() => gg.gh.calls().length).toBe(2);
  expect(gg.gh.calls()[1].join(" ")).toContain("GgInboxDetails");
  // The page reads as a page: no repository tab is selected, the welcome page is behind it.
  await expect(button(page)).toHaveAttribute("aria-current", "page");
  // An icon alone, named by its label and tooltip.
  await expect(button(page)).toHaveText("");
  await expect(button(page)).toHaveAttribute("aria-label", "Pull requests");
  await expect(button(page)).toHaveAttribute("title", /^Pull requests \((⌘⇧I|Ctrl\+Shift\+I)\)$/);
  await expect(page.getByTestId("welcome")).toHaveCount(0);
  await button(page).click();
  await expect(page.getByTestId("welcome")).toBeVisible();
});

test("says why there is no list: gh missing, signed out, the rate limit, offline, a failure, nothing open", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await openPage(gg);
  const state = page.getByTestId("inbox-state");
  await expect(state).toHaveAttribute("data-state", "no_gh");
  await expect(state).toContainText("The GitHub CLI (gh) was not found");
  await expect(state.getByRole("button", { name: "Get the GitHub CLI" })).toBeVisible();
  gg.gh.install();

  const cases: [string, string, RegExp][] = [
    ["signed_out", "You are not logged into any GitHub hosts. To log in, run: gh auth login", /gh is not signed in to github\.com/],
    ["rate_limited", "GraphQL: API rate limit exceeded for user ID 1.", /rate limit was reached/],
    ["offline", "error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com", /could not be reached/],
    ["failed", "HTTP 502: Bad gateway", /Could not load them: HTTP 502: Bad gateway/],
  ];
  for (const [kind, said, text] of cases) {
    gg.gh.reply("gql-inbox", 1, "", said);
    await page.getByTestId("inbox-retry").click();
    await expect(state).toHaveAttribute("data-state", kind);
    await expect(state).toContainText(text);
  }
  replyInbox(gg, {});
  await page.getByTestId("inbox-retry").click();
  await expect(state).toHaveAttribute("data-state", "empty");
  await expect(state).toContainText("No open pull requests need you");
});

test("the list shows at once from what gg kept, a failed reload keeps it as of then, with a note", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  await openPage(gg);
  await expect(rows(page)).toHaveCount(7);
  // Next run: gh is slow and then fails; the list is there before it answers.
  gg.gh.slow("gql-inbox", 3);
  gg.gh.reply("gql-inbox", 1, "", "error connecting to api.github.com");
  await gg.relaunch();
  await button(page).click();
  await expect(rows(page)).toHaveCount(7, { timeout: 1500 });
  await expect(page.getByTestId("inbox-refresh")).toHaveAttribute("aria-busy", "true");
  await expect(page.getByTestId("inbox-host-note")).toContainText("github.com could not be reached", { timeout: 10_000 });
  await expect(page.getByTestId("inbox-stale")).toHaveText(/^as of \d\d:\d\d$/);
  await expect(rows(page)).toHaveCount(7);
});

test("new and updated since last opened; opening one in gg clears its mark", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, { review: [{ number: 42, repo: "hub/dog", comments: 1 }] });
  await openPage(gg);
  await expect(rows(page)).toHaveCount(1);
  // More comments on #42 (someone else's), and #43 is new.
  replyInbox(gg, { review: [{ number: 42, repo: "hub/dog", comments: 3 }, { number: 43, repo: "hub/dog" }] });
  answer(gg, 42, view(42, { comments: [comment("1", "ada", "Ping", 5)], reviews: [] }));
  await page.getByTestId("inbox-refresh").click();
  await expect(row(page, "github.com/hub/dog#42")).toHaveAttribute("data-activity", "updated");
  await expect(row(page, "github.com/hub/dog#43")).toHaveAttribute("data-activity", "new");
  await expect(page.getByTestId("inbox-button-dot")).toBeVisible();
  await expect(button(page)).toHaveAttribute("aria-label", "Pull requests, 2 with something new");
  // One dot for either, on the state icon; the title bolder.
  await expect(row(page, "github.com/hub/dog#42").getByTestId("inbox-unread")).toBeVisible();
  await expect(row(page, "github.com/hub/dog#43").getByTestId("inbox-unread")).toBeVisible();
  await row(page, "github.com/hub/dog#42").click();
  await expect(page.getByTestId("inbox-pr-page")).toBeVisible();
  await page.getByTestId("inbox-back").click();
  await expect(row(page, "github.com/hub/dog#42")).toHaveAttribute("data-activity", "none");
  await expect(row(page, "github.com/hub/dog#42").getByTestId("inbox-unread")).toHaveCount(0);
  // Kept: the next load agrees.
  await page.getByTestId("inbox-refresh").click();
  await expect(page.getByTestId("inbox-refresh")).toHaveAttribute("aria-busy", "false");
  await expect(row(page, "github.com/hub/dog#42")).toHaveAttribute("data-activity", "none");
  await expect(row(page, "github.com/hub/dog#43")).toHaveAttribute("data-activity", "new");
});

test("the keyboard: the shortcut opens the page, arrows and Enter open a pull request, Escape goes back", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  answer(gg, 42, view(42, { comments: [], reviews: [] }));
  await page.goto("/");
  await expect(page.getByTestId("welcome")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+Shift+I");
  await expect(rows(page)).toHaveCount(7);
  await list(page).focus();
  // Focus selects the first; ↓ twice is the third.
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(row(page, "github.com/hub/dog#42")).toHaveAttribute("aria-selected", "true");
  await expect(list(page)).toHaveAttribute("aria-activedescendant", "inbox-item-2");
  await page.keyboard.press("End");
  await expect(row(page, "github.com/hub/owl#5")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("inbox-pr-page")).toBeVisible();
  await expect(page.getByTestId("pr-view-title")).toHaveText("Speed up the search index");
  await expect(page.getByTestId("inbox-pr-repo")).toHaveText("hub/dog");
  await expect(page.getByTestId("pr-status")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("inbox-pr-page")).toHaveCount(0);
  // Back where it was: the list has the focus, the same one selected.
  await expect(list(page)).toBeFocused();
  await expect(row(page, "github.com/hub/dog#42")).toHaveAttribute("aria-selected", "true");
  // The shortcut again: back to where the window was.
  await page.keyboard.press("ControlOrMeta+Shift+I");
  await expect(page.getByTestId("welcome")).toBeVisible();
  // Listed on the shortcuts sheet.
  await page.keyboard.press("?");
  await expect(page.getByRole("dialog")).toContainText("The Pull requests page");
});

test("a pull request's page shows its checks and comments and takes a reply, never through a repository's side panel", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  const parser = {
    id: "PRRT_parser",
    path: "src/parser.ts",
    line: 42,
    comments: [{ id: "c1", login: "ada", body: "This loses the position.", minutesAgo: 80 }],
  };
  answer(gg, 42, view(42, { comments: [comment("1", "ada", "Thanks for this.", 90)], reviews: [] }), threads([parser]));
  await page.goto("/");
  await openRepo(gg, "prs");
  await button(page).click();
  await row(page, "github.com/hub/dog#42").click();
  const prPage = page.getByTestId("inbox-pr-page");
  await expect(prPage.getByTestId("pr-view-title")).toHaveText("Speed up the search index");
  await expect(prPage.getByTestId("pr-flow")).toContainText("main");
  await expect(prPage.locator('[data-testid="pr-comment"]')).toHaveCount(2);
  // Asked of gh by the repository's name; no repository tab shows meanwhile.
  const viewCall = gg.gh.calls().find((c) => c[0] === "pr" && c[1] === "view")!;
  expect(viewCall.slice(0, 5)).toEqual(["pr", "view", "42", "--repo", "hub/dog"]);
  await expect(page.locator('[data-region="side-panel"]')).toHaveCount(0);

  // A reply, as the side panel sends one: on gh's stdin.
  gg.gh.reply("gql-reply", 0, JSON.stringify({ data: { addPullRequestReviewThreadReply: { comment: { id: "c9", url: "https://github.com/hub/dog/pull/42#discussion_rc9" } } } }));
  const replied = threads([{ ...parser, comments: [...parser.comments, { id: "c9", login: "octo", body: "Fixed: it says the line now.", minutesAgo: 0 }] }]);
  gg.gh.then("gql-reply", { "api-graphql-42": JSON.stringify(replied) });
  const thread = prPage.locator('[data-testid="pr-comment"][data-kind="thread"]');
  await thread.getByTestId("pr-reply-box-field").click();
  await thread.getByTestId("pr-reply-box-field").fill("Fixed: it says the line now.");
  await thread.getByTestId("pr-reply-box-send").click();
  await expect(thread).toContainText("Fixed: it says the line now.");
  await expect(thread.getByTestId("pr-reply-box-field")).toHaveValue("");
  const sent = gg.gh.calls().findIndex((c) => c.join(" ") === "api graphql --input -");
  expect(JSON.parse(gg.gh.stdin(sent)).variables).toEqual({ thread: "PRRT_parser", body: "Fixed: it says the line now." });
  // Open in browser: its page on GitHub.
  await prPage.getByTestId("pr-open").click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/hub/dog/pull/42"))).toBe(true);
});

test("go to the repository it is open in, and check out its branch there with gg's own checkout", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  answer(gg, 33, view(33, { comments: [], reviews: [] }));
  answer(gg, 29, view(29, { comments: [], reviews: [] }));
  await page.goto("/");
  await openRepo(gg, "prs");
  await button(page).click();
  // #33's branch, docs/readme, is a local branch of prs: Check out.
  await row(page, "github.com/octo/cat#33").click();
  const checkout = page.getByTestId("inbox-checkout");
  await expect(checkout).toHaveAttribute("data-plan", "local");
  await expect(checkout).toHaveText("Check out");
  await expect(page.getByTestId("inbox-go-to-repo")).toBeVisible();
  await checkout.click();
  // The repository's tab shows, on that branch.
  await expect(page.getByTestId("repo-view")).toBeVisible();
  await expect(page.getByTestId("active-worktree")).toContainText("docs/readme");
  expect(gg.git("prs", "branch", "--show-current").trim()).toBe("docs/readme");

  // Back on the page: the pull request it was on, as it was left.
  await button(page).click();
  await expect(page.getByTestId("pr-view-title")).toHaveText("Read the settings file once");
  // #29's branch is checked out in another worktree: that one is made active instead.
  await page.getByTestId("inbox-back").click();
  await row(page, "github.com/octo/cat#29").click();
  await expect(checkout).toHaveAttribute("data-plan", "worktree");
  // Go to repository: its tab, nothing changed.
  await page.getByTestId("inbox-go-to-repo").click();
  await expect(page.getByTestId("repo-view")).toBeVisible();
  await expect(page.getByTestId("active-worktree")).toContainText("docs/readme");
  // A repository not open here offers to clone it instead.
  await button(page).click();
  await page.keyboard.press("Escape");
  await row(page, "github.com/hub/dog#42").click();
  await expect(page.getByTestId("inbox-clone")).toBeVisible();
  await expect(page.getByTestId("inbox-go-to-repo")).toHaveCount(0);
});

test("clone and open: into a new folder of the folder chosen, from a local remote, then in its own tab", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  answer(gg, 42, view(42, { comments: [], reviews: [] }));
  // hub/dog's "remote": a bare repository in the clone source (never the network).
  const remotes = path.join(gg.home, "remotes", "hub");
  gg.gitAt(gg.home, "init", "-q", "--bare", "-b", "main", path.join(remotes, "dog.git"));
  gg.git("small", "push", "-q", path.join(remotes, "dog.git"), "HEAD:refs/heads/main");
  const into = gg.folder("clones");
  await openPage(gg);
  await row(page, "github.com/hub/dog#42").click();
  await page.getByTestId("inbox-clone").click();
  const dialog = page.getByRole("dialog", { name: "Clone hub/dog" });
  await dialog.getByTestId("clone-parent").fill(into);
  await expect(dialog.getByTestId("clone-hint")).toContainText(`~/${path.relative(gg.home, path.join(into, "dog"))}`);
  await dialog.getByRole("button", { name: "Clone and open" }).click();
  await expect(dialog).toHaveCount(0);
  // Its own tab, active.
  await expect(page.getByRole("tab", { name: "dog" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
  expect(gg.gitAt(path.join(into, "dog"), "rev-parse", "--is-inside-work-tree").trim()).toBe("true");
  // Again: the folder is there already, said in the dialog, nothing touched. The page shows
  // the pull request it was left on.
  await button(page).click();
  await expect(page.getByTestId("inbox-pr-page")).toBeVisible();
  await expect(page.getByTestId("inbox-go-to-repo")).toHaveCount(0);
  await page.getByTestId("inbox-clone").click();
  await dialog.getByTestId("clone-parent").fill(into);
  await dialog.getByRole("button", { name: "Clone and open" }).click();
  await expect(dialog.getByTestId("clone-hint")).toContainText("already");
});

test("a narrow window keeps each row on one line", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 520, height: 700 });
  replyInbox(gg, MIX);
  await openPage(gg);
  await expect(rows(page)).toHaveCount(7);
  await expect(button(page)).toHaveText("");
  const box = await row(page, "github.com/hub/dog#42").boundingBox();
  expect(box!.height).toBe(56);
  expect(box!.x + box!.width).toBeLessThanOrEqual(520);
  // The review says less; the list never scrolls sideways.
  await expect(row(page, "github.com/hub/dog#42").locator(".short")).toBeVisible();
  const sideways = await list(page).evaluate((el) => el.scrollWidth > el.clientWidth);
  expect(sideways).toBe(false);
});

test("hundreds of pull requests: only those in view are drawn, and the end is reached", async ({ gg }) => {
  const { page } = gg;
  const many: Node[] = Array.from({ length: 300 }, (_, i) => ({ number: i + 1, repo: "hub/dog", minutesAgo: i }));
  replyInbox(gg, { review: many.slice(0, 100), mentioned: many.slice(100, 200), assigned: many.slice(200) });
  await openPage(gg);
  // The first group's heading is drawn; the next one's is far below, not yet.
  await expect(page.getByTestId("inbox-group")).toHaveCount(1);
  await expect(page.getByTestId("inbox-group")).toHaveAttribute("data-group", "review");
  const drawn = await rows(page).count();
  expect(drawn).toBeGreaterThan(5);
  expect(drawn).toBeLessThan(60);
  await list(page).focus();
  await page.keyboard.press("End");
  await expect(row(page, "github.com/hub/dog#300")).toBeVisible();
  await expect(row(page, "github.com/hub/dog#300")).toHaveAttribute("aria-selected", "true");
  expect(await rows(page).count()).toBeLessThan(60);
});

test("gh is asked when the page opens, not again within the floor, and on Refresh; never while it is away", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  await openPage(gg);
  await expect(rows(page)).toHaveCount(7);
  expect(inboxCalls(gg)).toHaveLength(1);
  // Away and back within 30 s: what is there shows, gh is not asked.
  await button(page).click();
  await button(page).click();
  await expect(rows(page)).toHaveCount(7);
  await page.waitForTimeout(300);
  expect(inboxCalls(gg)).toHaveLength(1);
  await page.getByTestId("inbox-refresh").click();
  await expect.poll(() => inboxCalls(gg).length).toBe(2);
  await expect(page.getByTestId("inbox-refresh")).toHaveAttribute("aria-busy", "false");
  // ⌘R on the page refreshes the page.
  await page.keyboard.press("ControlOrMeta+R");
  await expect.poll(() => inboxCalls(gg).length).toBe(3);
  await button(page).click();
  await page.waitForTimeout(500);
  expect(inboxCalls(gg)).toHaveLength(3);
});
