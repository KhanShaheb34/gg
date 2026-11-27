// Screenshots of the Pull requests page for the visual check, at 2x, in both themes, written
// to .screens/ (git-ignored), or GG_SCREENS_DIR. Run on demand:
//   GG_SCREENS=1 pnpm e2e inbox-screens
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";
import { MIX, replyFiles, replyInbox } from "./inboxData";
import { answer, comment, review, run, threads, view } from "./prDetailsData";

const OUT = process.env.GG_SCREENS_DIR ?? path.resolve(import.meta.dirname, "..", ".screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1180, height: 760 }, deviceScaleFactor: 2, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

async function shoot(page: Page, name: string, theme: string) {
  await page.mouse.move(2, 750);
  await page.waitForTimeout(250);
  await page.screenshot({ path: path.join(OUT, `inbox-${name}-${theme}.png`), animations: "disabled" });
}

async function openRepo(gg: GG, name: string) {
  const { page } = gg;
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath(name)));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

for (const theme of ["light", "dark"] as const) {
  test(`the Pull requests page, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    // A first load, then one with activity: an update on #42, #44 new.
    replyInbox(gg, MIX);
    await page.goto("/");
    await openRepo(gg, "prs");
    await page.getByTestId("inbox-button").click();
    await expect(page.getByTestId("inbox-pr")).toHaveCount(7);
    replyInbox(gg, {
      ...MIX,
      review: [
        { ...MIX.review[0], comments: 4 },
        { number: 44, repo: "hub/dog", title: "Cache the parsed settings", author: "mia", checks: "PENDING", minutesAgo: 3 },
        ...MIX.review.slice(1),
      ],
    });
    await page.getByTestId("inbox-refresh").click();
    await expect(page.getByTestId("inbox-pr")).toHaveCount(8);
    await shoot(page, "list", theme);
    // Old ones (not updated for a month) folded at the end; one pinned.
    const DAY = 24 * 60;
    replyInbox(gg, {
      ...MIX,
      review: [{ ...MIX.review[0], comments: 4 }, ...MIX.review.slice(1)],
      mentioned: [...MIX.mentioned, { number: 9, repo: "hub/elk", title: "Move the docs to the new site", author: "grace", minutesAgo: 50 * DAY }],
      assigned: [...MIX.assigned, { number: 8, repo: "hub/dog", title: "Try the new parser behind a flag", author: "mia", minutesAgo: 40 * DAY, draft: true }],
    });
    await page.getByTestId("inbox-refresh").click();
    await expect(page.locator('[data-testid="inbox-group"][data-group="older"]')).toBeVisible();
    const typo = page.locator('[data-testid="inbox-pr"][data-key="github.com/octo/cat#31"]');
    await typo.hover();
    await typo.getByTestId("inbox-pin").click();
    await expect(page.locator('[data-testid="inbox-group"][data-group="pinned"]')).toBeVisible();
    await shoot(page, "list-pinned-older", theme);
    await page.locator('[data-testid="inbox-group"][data-group="older"]').click();
    await shoot(page, "list-older-open", theme);
    await page.getByTestId("inbox-group-by").getByRole("radio", { name: "Repository" }).click();
    await shoot(page, "list-by-repository", theme);
    await page.getByTestId("inbox-group-by").getByRole("radio", { name: "Status" }).click();
    await shoot(page, "list-by-status", theme);
    await page.getByTestId("inbox-group-by").getByRole("radio", { name: "Role" }).click();
    await page.locator('[data-testid="inbox-group"][data-group="older"]').click();

    // A row pointed at: Open in browser and the pin at its end.
    await page.locator('[data-testid="inbox-pr"][data-key="github.com/hub/owl#7"]').hover();
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `inbox-list-hover-${theme}.png`), animations: "disabled" });
    // A row selected from the keyboard.
    await page.getByRole("listbox", { name: "Pull requests" }).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await shoot(page, "list-keyboard", theme);

    // A pull request's page.
    const conv = threads([
      {
        id: "PRRT_index",
        path: "src/search/index.ts",
        line: 88,
        comments: [
          { id: "c1", login: "octo", body: "This rebuilds the whole index on each keystroke. Could it update only the changed file?", minutesAgo: 70 },
          { id: "c2", login: "ada", body: "Good point: it now updates the file's entries only.", minutesAgo: 40 },
        ],
      },
    ]);
    answer(
      gg,
      42,
      view(42, {
        merge: "BLOCKED",
        review: "CHANGES_REQUESTED",
        checks: [run("build", "passing"), run("test", "failing"), run("lint", "passing")],
        comments: [comment("1", "ada", "The index is rebuilt in the background now; numbers in the description.", 95)],
        reviews: [review("R1", "octo", "A few questions on the index.", "CHANGES_REQUESTED", 60)],
      }),
      conv,
    );
    replyFiles(gg, 42, [
      [
        { filename: "src/search/index.ts", status: "modified", additions: 48, deletions: 12, changes: 60, patch: "@@" },
        { filename: "src/search/cache.ts", status: "added", additions: 64, deletions: 0, changes: 64, patch: "@@" },
        { filename: "docs/search.md", previous_filename: "docs/find.md", status: "renamed", additions: 3, deletions: 1, changes: 4, patch: "@@" },
        { filename: "src/search/legacy.ts", status: "removed", additions: 0, deletions: 120, changes: 120, patch: "@@" },
      ],
    ]);
    await page.locator('[data-testid="inbox-pr"][data-key="github.com/hub/dog#42"]').click();
    await expect(page.getByTestId("pr-status")).toBeVisible();
    await expect(page.getByTestId("pr-file")).toHaveCount(4);
    await shoot(page, "pr-page", theme);

    // Its repository is open here: Go to repository and Check out.
    answer(gg, 33, view(33, { comments: [], reviews: [], checks: [run("build", "passing")] }));
    await page.getByTestId("inbox-back").click();
    await page.locator('[data-testid="inbox-pr"][data-key="github.com/octo/cat#33"]').click();
    await expect(page.getByTestId("inbox-checkout")).toBeVisible();
    await expect(page.getByTestId("pr-status")).toBeVisible();
    await shoot(page, "pr-page-open-repo", theme);

    // Clone and open.
    await page.getByTestId("inbox-back").click();
    await page.locator('[data-testid="inbox-pr"][data-key="github.com/hub/dog#42"]').click();
    await page.getByTestId("inbox-clone").click();
    await page.getByRole("dialog").getByTestId("clone-parent").fill(path.join(gg.home, "Dev"));
    await shoot(page, "clone", theme);
    await page.keyboard.press("Escape");

    // A narrow window.
    await page.getByTestId("inbox-back").click();
    await page.setViewportSize({ width: 480, height: 760 });
    await shoot(page, "narrow", theme);
    await page.setViewportSize({ width: 1180, height: 760 });
  });

  test(`the Pull requests page's states, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    gg.gh.uninstall();
    await page.goto("/");
    await page.getByTestId("inbox-button").click();
    const state = page.getByTestId("inbox-state");
    await expect(state).toHaveAttribute("data-state", "no_gh");
    await shoot(page, "state-no-gh", theme);
    gg.gh.install();
    const cases: [string, string][] = [
      ["signed_out", "You are not logged into any GitHub hosts. To log in, run: gh auth login"],
      ["rate_limited", "GraphQL: API rate limit exceeded for user ID 1."],
      ["offline", "error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com"],
      ["failed", "HTTP 502: Bad gateway"],
    ];
    for (const [kind, said] of cases) {
      gg.gh.reply("gql-inbox", 1, "", said);
      await page.getByTestId("inbox-retry").click();
      await expect(state).toHaveAttribute("data-state", kind);
      await shoot(page, `state-${kind.replace("_", "-")}`, theme);
    }
    replyInbox(gg, {});
    await page.getByTestId("inbox-retry").click();
    await expect(state).toHaveAttribute("data-state", "empty");
    await shoot(page, "state-empty", theme);
    // A list, then a reload that fails: the list stays, with a note.
    replyInbox(gg, MIX);
    await page.getByTestId("inbox-retry").click();
    await expect(page.getByTestId("inbox-pr")).toHaveCount(7);
    gg.gh.reply("gql-inbox", 1, "", "error connecting to api.github.com");
    await page.getByTestId("inbox-refresh").click();
    await expect(page.getByTestId("inbox-host-note")).toBeVisible();
    await shoot(page, "state-stale", theme);
  });
}
