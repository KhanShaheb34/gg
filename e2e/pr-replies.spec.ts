// Replying, resolving and commenting from the side panel's Pull request tab, against the real
// core and the devserver's fake gh (never the real one, never the network). The `prs` fixture
// is on fix/typo, whose pull request is #31. The fake gh answers each write, and then (`then`)
// answers the details as GitHub would once it has it, so what shows after a write is what the
// tab loaded again. The core's floor between two loads is 0 ms here (--pr-details-every-ms).
import type { Locator, Page } from "@playwright/test";
import { expect, type GG, openFolder, test } from "./fixtures";
import { answer, comment, threads, view, type ThreadSpec } from "./prDetailsData";

test.use({ viewport: { width: 1280, height: 900 }, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const tab = (page: Page, id: "commit" | "pr" | "worktrees") => page.getByTestId(`side-tab-${id}`);
const thread = (page: Page, path: string) => page.locator('[data-testid="pr-comment"][data-kind="thread"]', { hasText: path });
const box = (scope: Locator | Page, id = "pr-reply-box") => ({
  field: scope.getByTestId(`${id}-field`),
  send: scope.getByTestId(`${id}-send`),
  error: scope.getByTestId(`${id}-error`),
});

/** ada's question on parser.ts with the owner's answer, and a resolved one on Toolbar.svelte. */
function parser(extra: ThreadSpec["comments"] = [], o: Partial<ThreadSpec> = {}): ThreadSpec {
  return {
    id: "PRRT_parser",
    path: "src/parser.ts",
    line: 42,
    comments: [
      { id: "c1", login: "ada", body: "This loses the position.", minutesAgo: 80 },
      { id: "c2", login: "KhanShaheb34", body: "Which one?", minutesAgo: 60 },
      ...extra,
    ],
    ...o,
  };
}
const toolbar = (o: Partial<ThreadSpec> = {}): ThreadSpec => ({
  id: "PRRT_toolbar",
  path: "src/Toolbar.svelte",
  line: 7,
  resolved: true,
  comments: [{ id: "c3", login: "grace", body: "Rename this.", minutesAgo: 70 }],
  ...o,
});
const V = (comments: object[] = [comment("1", "ada", "Thanks for this.", 90)]) => view(31, { comments, reviews: [] });

/** Opens the `prs` fixture on fix/typo, its pull request #31 shown in the Pull request tab. */
async function showPr(gg: GG, v: object = V(), t: object = threads([parser(), toolbar()])) {
  const { page } = gg;
  gg.git("prs", "switch", "-q", "fix/typo");
  const listed = { number: 31, title: "Fix a typo", url: "https://github.com/octo/cat/pull/31", isDraft: false, headRefName: "fix/typo", baseRefName: "main", headRepositoryOwner: { id: "X", login: "octo" }, isCrossRepository: false };
  gg.gh.reply("pr-list", 0, JSON.stringify([listed]));
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  answer(gg, 31, v, t);
  await page.goto("/");
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath("prs")));
  await openFolder(page);
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.locator("button.pr-chip[data-pr='31']").first()).toBeVisible();
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(page.getByTestId("pr-status")).toBeVisible();
}

/** The calls of the fake gh whose first arguments are `prefix`, with what each read on stdin. */
function writes(gg: GG, prefix: string[]): { argv: string[]; stdin: string }[] {
  return gg.gh
    .calls()
    .map((argv, i) => ({ argv, i }))
    .filter(({ argv }) => prefix.every((p, k) => argv[k] === p))
    .map(({ argv, i }) => ({ argv, stdin: gg.gh.stdin(i) }));
}
const mutations = (gg: GG) => writes(gg, ["api", "graphql"]).filter((c) => !c.argv.some((a) => a.startsWith("n=")));

test("a reply: a quiet line that grows, sent on Reply, then in the thread as gh has it, the box cleared", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  const t = thread(page, "parser.ts");
  const b = box(t);
  // One quiet line, no button, until it is used.
  await expect(b.field).toHaveAttribute("placeholder", "Reply…");
  const line = (await b.field.boundingBox())!.height;
  expect(line).toBeLessThanOrEqual(30);
  await expect(b.send).toHaveCount(0);
  await b.field.click();
  await expect(b.send).toBeVisible();
  await expect(b.send).toBeDisabled();
  await expect.poll(async () => (await b.field.boundingBox())!.height).toBeGreaterThan(line + 20);
  // Blank: still nothing to send.
  await b.field.fill("   \n ");
  await expect(b.send).toBeDisabled();
  // Enter is a new line; nothing is sent while typing.
  const text = "The column, `col`.\nSee $(the fix) and \"quotes\".";
  await b.field.fill("");
  await b.field.pressSequentially("The column, `col`.");
  await page.keyboard.press("Enter");
  await b.field.pressSequentially('See $(the fix) and "quotes".');
  await expect(b.field).toHaveValue(text);
  expect(mutations(gg)).toHaveLength(0);

  // GitHub has the reply once it is sent: the details then say so.
  gg.gh.slow("gql-reply", 1);
  gg.gh.then("gql-reply", { "api-graphql-31": JSON.stringify(threads([parser([{ id: "c9", login: "KhanShaheb34", body: text, minutesAgo: 0 }]), toolbar()])) });
  await b.send.click();
  // While it sends: read-only, said on the button; the comments stay as they were.
  await expect(b.send).toHaveText("Sending…");
  await expect(b.send).toBeDisabled();
  await expect(b.field).toHaveAttribute("readonly", "");
  await expect(page.getByTestId("pr-details-loading")).toHaveCount(0);
  await expect(page.getByTestId("pr-looking")).toHaveCount(0);
  await expect(t.getByTestId("pr-reply")).toHaveCount(1);
  // Then the reply, in the thread, and the box empty again.
  await expect(t.getByTestId("pr-reply")).toHaveCount(2);
  await expect(t.getByTestId("pr-reply").last()).toContainText("The column, col.");
  await expect(b.field).toHaveValue("");
  await expect(b.error).toHaveCount(0);

  // gh was asked once, with the text as data on its stdin.
  const sent = mutations(gg);
  expect(sent).toHaveLength(1);
  expect(sent[0].argv).toEqual(["api", "graphql", "--input", "-"]);
  const request = JSON.parse(sent[0].stdin);
  expect(request.query).toContain("addPullRequestReviewThreadReply");
  expect(request.variables).toEqual({ thread: "PRRT_parser", body: text });
  // No notice: the tab says how it went.
  await expect(page.locator("[data-notices] .toast")).toHaveCount(0);
});

test("⌘↩ in the box sends it; a comment on the conversation goes to gh pr comment and shows at the end", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  const b = box(page, "pr-comment-box");
  await expect(b.field).toHaveAttribute("placeholder", "Add a comment…");
  const body = "@ada thanks!\n- one\n- two";
  await b.field.fill(body);
  gg.gh.then("pr-comment", { "pr-view-31": JSON.stringify(V([comment("1", "ada", "Thanks for this.", 90), comment("2", "KhanShaheb34", body, 0)])) });
  await b.field.press("ControlOrMeta+Enter");
  const cards = page.getByTestId("pr-comment");
  await expect(cards.filter({ hasText: "@ada thanks!" })).toHaveCount(1);
  await expect(b.field).toHaveValue("");
  const sent = writes(gg, ["pr", "comment"]);
  expect(sent).toHaveLength(1);
  expect(sent[0].argv).toEqual(["pr", "comment", "31", "--repo", "octo/cat", "--body-file", "-"]);
  expect(sent[0].stdin).toBe(body);
  // The comment is the last one; the box under it.
  await expect(cards.last()).toContainText("@ada thanks!");
  // Plain Enter never sends.
  await b.field.fill("x");
  await b.field.press("Enter");
  await page.waitForTimeout(300);
  expect(writes(gg, ["pr", "comment"])).toHaveLength(1);
});

test("Resolve and Unresolve flip the thread as gh has it; a resolved thread is folded", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  const t = thread(page, "parser.ts");
  const resolve = t.getByTestId("pr-thread-resolve");
  await expect(resolve).toHaveAttribute("data-action", "resolve");
  await expect(resolve).toHaveText("Resolve");
  await expect(resolve).toHaveAccessibleName("Resolve conversation");
  gg.gh.then("gql-resolve", { "api-graphql-31": JSON.stringify(threads([parser([], { resolved: true }), toolbar()])) });
  await resolve.click();
  // Resolved: tagged, folded (its reply hidden), the box gone, Unresolve beside Show more.
  await expect(t.getByTestId("pr-comment-context")).toContainText("Resolved");
  await expect(t.getByTestId("pr-reply")).toHaveCount(0);
  await expect(t.getByTestId("pr-reply-box")).toHaveCount(0);
  await expect(resolve).toHaveAttribute("data-action", "unresolve");
  await expect(t.getByTestId("pr-comment-more")).toHaveText("Show more (1 reply)");
  const resolved = mutations(gg);
  expect(resolved.map((c) => c.argv[0] + " " + c.argv[1] + " " + c.argv[2])).toEqual(["api graphql -f"]);
  expect(resolved[0].argv[3]).toContain("resolveReviewThread(input:{threadId:$thread})");
  expect(resolved[0].argv.slice(4)).toEqual(["-f", "thread=PRRT_parser"]);
  // Shown whole: its replies and the box to reply in it, Unresolve beside the box.
  await t.getByTestId("pr-comment-more").click();
  await expect(t.getByTestId("pr-reply")).toHaveCount(1);
  await expect(t.getByTestId("pr-reply-box-field")).toBeVisible();
  await t.getByTestId("pr-comment-more").click();

  gg.gh.then("gql-unresolve", { "api-graphql-31": JSON.stringify(threads([parser(), toolbar()])) });
  await resolve.click();
  await expect(t.getByTestId("pr-comment-context")).not.toContainText("Resolved");
  await expect(t.getByTestId("pr-reply")).toHaveCount(1);
  await expect(resolve).toHaveAttribute("data-action", "resolve");
  const all = mutations(gg);
  expect(all).toHaveLength(2);
  expect(all[1].argv[3]).toContain("unresolveReviewThread");

  // The other, resolved from the start: folded, with Unresolve.
  await expect(thread(page, "Toolbar.svelte").getByTestId("pr-thread-resolve")).toHaveAttribute("data-action", "unresolve");
});

test("a failure keeps the draft and says why beside it, with Show log; sent again, it goes", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  const t = thread(page, "parser.ts");
  const b = box(t);
  gg.gh.reply("gql-reply", 1, "", "gh: Resource not accessible by personal access token\n");
  await b.field.fill("Done.");
  await b.send.click();
  await expect(b.error).toContainText("gh could not post the reply. gh: Resource not accessible by personal access token.");
  await expect(b.field).toHaveValue("Done.");
  await expect(t.getByTestId("pr-reply")).toHaveCount(1);
  await expect(page.locator("[data-notices] .toast")).toHaveCount(0);
  // Its command in the log.
  await b.error.getByRole("button", { name: "Show log" }).click();
  await expect(page.getByTestId("command-log")).toBeVisible();
  await page.keyboard.press("Escape");

  // Resolving that fails: said under the thread, nothing changed.
  gg.gh.reply("gql-resolve", 1, "", "gh: Could not resolve to a node with the global id of 'PRRT_parser'.\n");
  await t.getByTestId("pr-thread-resolve").click();
  await expect(t.getByTestId("pr-thread-resolve-error")).toContainText("gh could not resolve the conversation");
  await expect(t.getByTestId("pr-thread-resolve")).toHaveAttribute("data-action", "resolve");

  // Again, now that it works: sent, the error gone, the box empty.
  gg.gh.reply("gql-reply", 0, "{}");
  gg.gh.then("gql-reply", { "api-graphql-31": JSON.stringify(threads([parser([{ id: "c9", login: "KhanShaheb34", body: "Done.", minutesAgo: 0 }]), toolbar()])) });
  await b.send.click();
  await expect(t.getByTestId("pr-reply")).toHaveCount(2);
  await expect(b.error).toHaveCount(0);
  await expect(b.field).toHaveValue("");
});

test("drafts stay through the side panel's tabs and other commits, and nothing is sent by them", async ({ gg }) => {
  const { page } = gg;
  await showPr(gg);
  await box(thread(page, "parser.ts")).field.fill("Half a reply");
  await box(page, "pr-comment-box").field.fill("Half a comment");
  await tab(page, "commit").click();
  await expect(page.getByTestId("pr-view")).toHaveCount(0);
  await tab(page, "worktrees").click();
  // Another commit selected, then back.
  await grid(page).locator('[data-index="2"]').click();
  await expect(grid(page).locator('[data-index="2"]')).toHaveAttribute("aria-selected", "true");
  await tab(page, "pr").click();
  await expect(box(thread(page, "parser.ts")).field).toHaveValue("Half a reply");
  await expect(box(page, "pr-comment-box").field).toHaveValue("Half a comment");
  // Held open, as drafts: their buttons there.
  await expect(box(thread(page, "parser.ts")).send).toBeEnabled();
  expect(mutations(gg)).toHaveLength(0);
  expect(writes(gg, ["pr", "comment"])).toHaveLength(0);
  // Escape leaves the box, keeping the draft.
  await box(thread(page, "parser.ts")).field.focus();
  await page.keyboard.press("Escape");
  await expect(box(thread(page, "parser.ts")).field).not.toBeFocused();
  await expect(box(thread(page, "parser.ts")).field).toHaveValue("Half a reply");
  await expect(page.getByTestId("pr-view")).toBeVisible();
});

test("what the user may not do is not offered: no box, no Resolve, no comment box on a locked conversation", async ({ gg }) => {
  const { page } = gg;
  const t = threads(
    [
      parser([], { canReply: false, canResolve: false }),
      { id: "PRRT_reply", path: "src/reply-only.ts", line: 1, canResolve: false, comments: [{ id: "c5", login: "ada", body: "Hm.", minutesAgo: 30 }] },
      { id: "PRRT_resolve", path: "src/resolve-only.ts", line: 2, canReply: false, comments: [{ id: "c6", login: "ada", body: "Ok.", minutesAgo: 20 }] },
      toolbar({ canUnresolve: false, canReply: false }),
    ],
    [],
    { locked: true, permission: "READ" },
  );
  await showPr(gg, V(), t);
  const none = thread(page, "parser.ts");
  await expect(none.getByTestId("pr-reply-box")).toHaveCount(0);
  await expect(none.getByTestId("pr-thread-resolve")).toHaveCount(0);
  const replyOnly = thread(page, "reply-only.ts");
  await expect(replyOnly.getByTestId("pr-reply-box")).toHaveCount(1);
  await expect(replyOnly.getByTestId("pr-thread-resolve")).toHaveCount(0);
  const resolveOnly = thread(page, "resolve-only.ts");
  await expect(resolveOnly.getByTestId("pr-reply-box")).toHaveCount(0);
  await expect(resolveOnly.getByTestId("pr-thread-resolve")).toHaveAttribute("data-action", "resolve");
  const folded = thread(page, "Toolbar.svelte");
  await expect(folded.getByTestId("pr-thread-resolve")).toHaveCount(0);
  await expect(folded.getByTestId("pr-comment-more")).toHaveCount(0);
  await expect(page.getByTestId("pr-comment-box")).toHaveCount(0);

  // The conversation open again (or the user may write): the box is there.
  answer(gg, 31, V(), threads([parser()], [], { locked: true, permission: "MAINTAIN" }));
  await page.getByTestId("pr-refresh").click();
  await expect(page.getByTestId("pr-comment-box")).toHaveCount(1);
});

test("at the panel's least width the box, its button and Resolve fit, nothing scrolls sideways", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 800 });
  await showPr(gg);
  const side = page.locator('[data-region="side-panel"]');
  const s = (await side.boundingBox())!;
  expect(Math.round(s.width)).toBe(240);
  const t = thread(page, "parser.ts");
  await box(t).field.fill("A reply long enough to wrap onto a few lines in a panel this narrow, and then some more words.");
  for (const el of [box(t).field, box(t).send, t.getByTestId("pr-thread-resolve"), box(page, "pr-comment-box").field]) {
    await el.scrollIntoViewIfNeeded();
    const b = (await el.boundingBox())!;
    expect(b.x).toBeGreaterThanOrEqual(s.x);
    expect(b.x + b.width).toBeLessThanOrEqual(s.x + s.width + 0.5);
  }
  // The button and Resolve on one line.
  const send = (await box(t).send.boundingBox())!;
  const resolve = (await t.getByTestId("pr-thread-resolve").boundingBox())!;
  expect(Math.abs(send.y + send.height / 2 - (resolve.y + resolve.height / 2))).toBeLessThan(2);
  expect(await page.getByTestId("pr-view").evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
});
