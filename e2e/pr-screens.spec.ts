// Screenshots of the Pull request tab's checks and comments for the visual check, at 2x, in
// both themes, written to .screens/ (git-ignored), or GG_SCREENS_DIR. Run on demand:
//   GG_SCREENS=1 pnpm e2e pr-screens
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { expect, type GG, openFolder, test } from "./fixtures";
import { answer, comment, coderabbit, review, run, status, threads, view } from "./prDetailsData";

const OUT = process.env.GG_SCREENS_DIR ?? path.resolve(import.meta.dirname, "..", ".screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const side = (page: Page) => page.locator('[data-region="side-panel"]');
const card = (page: Page, text: string, kind = "comment") => page.locator(`[data-testid="pr-comment"][data-kind="${kind}"]`).filter({ hasText: text }).first();

/** The conversation of the screenshots: a person's comment, CodeRabbit's (captured), a review, and two threads. */
function conversation() {
  const rabbit = coderabbit();
  return {
    comments: [
      comment("1", "ada", "Looks good overall. One question about the parser's error path: see the thread on `parser.ts`.\n\nAlso, should we keep the old **README** wording for the install step?", 95),
      ...rabbit.comments,
    ],
    reviews: [...rabbit.reviews, review("R9", "grace", "Approved: thanks for the quick fix.", "APPROVED", 20)],
    threads: threads(
      [
        {
          path: "src/parser.ts",
          line: 42,
          comments: [
            { id: "c1", login: "ada", body: "This `throw` loses the position. Could it say which line it stopped at?", minutesAgo: 80 },
            { id: "c2", login: "KhanShaheb34", body: "Good catch, it says the line and column now.", minutesAgo: 60 },
          ],
        },
        {
          path: "src/lib/components/Toolbar.svelte",
          line: 274,
          resolved: true,
          comments: [
            { id: "c3", login: "coderabbitai", bot: true, body: "**Minor** · **Functional Correctness**\n\n**Open the active worktree's commit panel when another worktree's commit panel is shown.**\n\nWhen the side panel shows the commit panel of a non-active worktree, the Commit button appears pressed. Clicking it hides that panel instead of opening the active worktree's commit panel.", minutesAgo: 70 },
            { id: "c4", login: "KhanShaheb34", body: "Fixed: the button now hides the panel only when it shows the active worktree's commit panel.", minutesAgo: 50 },
          ],
        },
      ],
      ["coderabbitai"],
    ),
  };
}

async function showPr(gg: GG, v: object, t: object) {
  const { page } = gg;
  gg.git("prs", "switch", "-q", "fix/typo");
  const list = [{ number: 31, title: "Fix a typo in the README's first line", url: "https://github.com/octo/cat/pull/31", isDraft: false, headRefName: "fix/typo", baseRefName: "main", headRepositoryOwner: { id: "X", login: "octo" }, isCrossRepository: false }];
  gg.gh.reply("pr-list", 0, JSON.stringify(list));
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  answer(gg, 31, v, t);
  await page.goto("/");
  const dir = gg.repoPath("prs");
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await page.getByRole("grid", { name: "Commits" }).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(page.getByTestId("pr-view")).toBeVisible();
}

/** Answers with `v` and `t` from now on, and refreshes. */
async function reload(gg: GG, v: object, t: object, ready: (page: Page) => Promise<void>) {
  answer(gg, 31, v, t);
  await gg.page.getByTestId("pr-refresh").click();
  await ready(gg.page);
}

async function shoot(target: Locator, name: string, theme: string) {
  await target.page().mouse.move(5, 895);
  await target.page().waitForTimeout(200);
  await target.screenshot({ path: path.join(OUT, `pr-${name}-${theme}.png`), animations: "disabled" });
}

for (const theme of ["light", "dark"] as const) {
  test(`pull request checks and comments, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    const talk = conversation();
    const passing = view(31, {
      review: "APPROVED",
      checks: [run("macos-latest", "passing", 11 * 60_000), run("ubuntu-latest", "passing", 9 * 60_000), run("windows-latest", "passing", 17 * 60_000), { ...run("CodeRabbit", "skipped"), conclusion: "NEUTRAL", workflowName: "" }],
      comments: talk.comments,
      reviews: talk.reviews,
    });
    await showPr(gg, passing, talk.threads);
    await expect(page.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
    await shoot(side(page), "passing", theme);

    // The filter: people, then bots.
    const filter = page.getByTestId("pr-comments-filter");
    await filter.getByRole("radio", { name: "Humans" }).click();
    await shoot(side(page), "humans", theme);
    await filter.getByRole("radio", { name: "Bots" }).click();
    await shoot(side(page), "bots", theme);
    await filter.getByRole("radio", { name: "All" }).click();

    // CodeRabbit's long comment, shortened, then in full.
    const long = card(page, "Explain Change");
    await long.scrollIntoViewIfNeeded();
    await shoot(long, "bot-collapsed", theme);
    await long.getByTestId("pr-comment-more").click();
    await long.locator("details").first().evaluate((d) => ((d as HTMLDetailsElement).open = true));
    await shoot(long, "bot-expanded", theme);
    await long.getByTestId("pr-comment-more").click();

    // The resolved thread, shortened, then open with its reply.
    const resolved = card(page, "Toolbar.svelte", "thread");
    await resolved.scrollIntoViewIfNeeded();
    await shoot(resolved, "resolved", theme);
    await resolved.getByTestId("pr-comment-more").click();
    await shoot(resolved, "resolved-open", theme);
    await resolved.getByTestId("pr-comment-more").click();

    // A thread to reply in: its quiet box and Resolve, then the box in use, then a failure.
    const open = card(page, "parser.ts", "thread");
    await open.scrollIntoViewIfNeeded();
    await shoot(open, "thread", theme);
    await open.getByTestId("pr-reply-box-field").fill("It says the line and column now: `parse error at 3:14`.\nThe test covers it too.");
    await shoot(open, "thread-reply", theme);
    gg.gh.reply("gql-reply", 1, "", "gh: Resource not accessible by personal access token\n");
    await open.getByTestId("pr-reply-box-send").click();
    await expect(open.getByTestId("pr-reply-box-error")).toBeVisible();
    await open.getByTestId("pr-reply-box-field").blur();
    await shoot(open, "thread-reply-failed", theme);
    await open.getByTestId("pr-reply-box-field").fill("");
    await open.getByTestId("pr-reply-box-field").blur();
    // The conversation's box, at its end.
    await page.getByTestId("pr-comment-box").scrollIntoViewIfNeeded();
    await shoot(side(page), "comments-end", theme);
    await page.getByTestId("pr-details").evaluate((el) => el.closest(".pr-view")!.scrollTo(0, 0));

    // Running.
    const running = view(31, {
      merge: "UNSTABLE",
      checks: [run("macos-latest", "running", 2 * 60_000), run("ubuntu-latest", "running", 3 * 60_000), run("windows-latest", "queued"), run("lint", "passing", 40_000)],
      comments: talk.comments.slice(0, 1),
    });
    await reload(gg, running, threads([]), async (p) => expect(p.getByTestId("pr-merge-status")).toHaveText("Checks running"));
    await shoot(side(page), "running", theme);

    // Failing, blocked, changes requested.
    const failing = view(31, {
      merge: "BLOCKED",
      review: "CHANGES_REQUESTED",
      checks: [
        run("macos-latest", "passing", 11 * 60_000),
        run("ubuntu-latest", "failing", 6 * 60_000),
        run("windows-latest", "running", 7 * 60_000),
        run("e2e (chromium)", "passing", 14 * 60_000, "End-to-end"),
        run("deploy-preview", "skipped", 0, "Deploy"),
        run("benchmarks", "cancelled", 60_000, "Perf"),
        status("ci/legacy-build", "SUCCESS"),
      ],
      comments: talk.comments.slice(0, 1),
      reviews: [review("R8", "grace", "The Windows path still uses the old separator; see CI.", "CHANGES_REQUESTED", 5)],
    });
    await reload(gg, failing, threads([]), async (p) => expect(p.getByTestId("pr-review")).toHaveText("Changes requested"));
    await shoot(side(page), "failing", theme);

    // No checks, no comments.
    await reload(gg, view(31, { review: "REVIEW_REQUIRED" }), threads([]), async (p) => expect(p.getByTestId("pr-checks-summary")).toHaveText("No checks"));
    await shoot(side(page), "empty", theme);

    // A reload that fails: what was shown stays, said in one line with Retry.
    await reload(gg, failing, threads([]), async (p) => expect(p.getByTestId("pr-review")).toHaveText("Changes requested"));
    gg.gh.reply("pr-view-31", 1, "", "GraphQL: API rate limit exceeded for user ID 1.\n");
    await page.getByTestId("pr-refresh").click();
    await expect(page.getByTestId("pr-details-failed")).toBeVisible();
    await shoot(side(page), "reload-failed", theme);
  });

  test(`pull request details loading, failed and at the least width, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    // The first load, held back: the placeholders.
    let release: () => void = () => {};
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/invoke", async (route) => {
      const body = route.request().postDataJSON() as { cmd?: string } | null;
      if (body?.cmd === "pr_details") await held;
      return route.continue();
    });
    gg.gh.reply("pr-view-31", 1, "", "GraphQL: Could not resolve to a PullRequest with the number of 31.\n");
    await showPr(gg, view(31), threads([]));
    gg.gh.reply("pr-view-31", 1, "", "GraphQL: Could not resolve to a PullRequest with the number of 31.\n");
    await expect(page.getByTestId("pr-details-loading")).toBeVisible();
    await shoot(side(page), "loading", theme);
    release();
    await expect(page.getByTestId("pr-details-failed")).toBeVisible();
    await shoot(side(page), "failed", theme);
    await page.unrouteAll({ behavior: "wait" });

    // The least width, docked beside the graph.
    const talk = conversation();
    answer(
      gg,
      31,
      view(31, {
        merge: "BLOCKED",
        review: "CHANGES_REQUESTED",
        checks: [run("ubuntu-latest", "failing", 6 * 60_000), run("windows-latest-with-a-long-name", "running", 7 * 60_000), run("macos-latest", "passing", 11 * 60_000)],
        comments: talk.comments,
        reviews: talk.reviews,
      }),
      talk.threads,
    );
    await page.setViewportSize({ width: 900, height: 900 });
    await page.getByTestId("pr-details-retry").click();
    await expect(page.getByTestId("pr-review")).toHaveText("Changes requested");
    expect(Math.round((await side(page).boundingBox())!.width)).toBe(240);
    await shoot(side(page), "narrow", theme);
    // A reply being written there, beside Resolve.
    const open = card(page, "parser.ts", "thread");
    await open.getByTestId("pr-reply-box-field").fill("It says the line and column now.");
    await open.scrollIntoViewIfNeeded();
    await shoot(open, "narrow-thread", theme);
  });
}
