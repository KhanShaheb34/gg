// Pull requests against the real core, with the devserver's fake `gh` (never the real one,
// never the network; the browser is never started: URL opens are recorded). The `prs`
// fixture's remote is github.com for the app (a local bare repository underneath). Markers on
// branches and worktrees, the menus, and the side panel's Pull request tab: the form (create
// with the push first, a failure kept in the tab, a suggestion that failed), a branch that has
// one already, the signed-out and missing-gh fallback to the compare page, the branch a menu
// asked for and the way back, what is typed kept, the tab showing after a push (and when it
// does not), the pull request's own view, the shortcut, and the panel at its least width.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

const FIELDS = "number,title,url,isDraft,headRefName,baseRefName,headRepositoryOwner,isCrossRepository";
const SEARCH = "feature/search-indexing-across-every-workspace-package";
const SHORTCUT = /^Pull request · (#\d+ (open|draft)|none yet) \((⌘⇧P|Ctrl\+Shift\+P)\)$/;

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const worktrees = (page: Page) => page.getByRole("grid", { name: "Worktrees" });
const label = (page: Page, name: string) => grid(page).locator(`[data-ref="${name}"]`).first();
const chip = (page: Page, n: number) => grid(page).locator(`button.pr-chip[data-pr="${n}"]`);
const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });
const notice = (page: Page, text: string | RegExp) => page.locator("[data-notices] .toast", { hasText: text });
const dialog = (page: Page) => page.getByRole("dialog");
const tab = (page: Page, id: "commit" | "pr" | "worktrees") => page.getByTestId(`side-tab-${id}`);
/** The Pull request tab's body. */
const panel = (page: Page) => page.getByTestId("pr-panel");
const button = (page: Page, name: string | RegExp) => panel(page).getByRole("button", { name, exact: typeof name === "string" });

function pr(number: number, head: string, title: string, draft = false) {
  return {
    number,
    title,
    url: `https://github.com/octo/cat/pull/${number}`,
    isDraft: draft,
    headRefName: head,
    baseRefName: "main",
    headRepositoryOwner: { id: "X", login: "octo" },
    isCrossRepository: false,
  };
}

/** gh answers the list with `prs`, and the default branch `main`. */
function listing(gg: GG, prs: object[]) {
  gg.gh.reply("pr-list", 0, JSON.stringify(prs));
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
}

/** The user's refresh (the toolbar's): the pull requests load with it. */
async function refresh(page: Page) {
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
}

async function menuOn(page: Page, name: string, itemName: string | RegExp) {
  await label(page, name).locator(".name").click({ button: "right" });
  await item(page, itemName).click();
}

/** Opens the `prs` fixture with the pull requests listed (`prs`), loaded. */
async function openListed(gg: GG, prs: object[] = []) {
  listing(gg, prs);
  await gg.page.goto("/");
  await open(gg.page, gg.repoPath("prs"));
  await refresh(gg.page);
  await expect.poll(() => prsStatus(gg.page)).toBe("ready");
}

/** The pull requests' state of the active tab's repository, as its snapshot has it. */
function prsStatus(page: Page): Promise<string | undefined> {
  return page.evaluate(async () => {
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    return app.activeRepoId === null ? undefined : findRepoView(app.activeRepoId)?.snapshot.prs.status;
  });
}

test("open pull requests are marked on their branches and worktrees; a marker opens its page and selects nothing", async ({ gg }) => {
  const { page } = gg;
  listing(gg, [pr(12, "feature/login", "Add the login form"), pr(13, SEARCH, "Index the search", true)]);
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);

  await expect(chip(page, 12)).toBeVisible();
  await expect(chip(page, 12)).toHaveText("#12");
  await expect(chip(page, 12)).toHaveAttribute("title", "#12 Add the login form\nmain ← feature/login");
  await expect(chip(page, 13)).toHaveClass(/draft/);
  await expect(chip(page, 13)).toHaveAttribute("title", `#13 Index the search\nmain ← ${SEARCH}\nDraft`);
  // The marker sits right after its branch's label.
  const after = await label(page, "feature/login").evaluate((el) => (el.nextElementSibling as HTMLElement).dataset.pr);
  expect(after).toBe("12");

  // The worktree on feature/login shows it too, and says so to assistive technology.
  await tab(page, "worktrees").click();
  const wt = worktrees(page).locator('[role="row"][data-path$="login"]');
  await expect(wt.locator("button.pr-chip")).toHaveText("#12");
  await expect(wt).toHaveAttribute("aria-label", /pull request #12/);

  // Clicked: the page opens (recorded, never started), the row is not selected.
  const selected = await grid(page).locator('[role="row"][aria-selected="true"]').count();
  await chip(page, 12).click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/12"))).toBe(true);
  expect(await grid(page).locator('[role="row"][aria-selected="true"]').count()).toBe(selected);
  await expect(page.getByRole("region", { name: /commit details/i })).toHaveCount(0);

  // Every gh command named the repository.
  for (const call of gg.gh.calls()) expect(call).toContain("--repo");
  expect(gg.gh.calls()[0]).toEqual(["pr", "list", "--repo", "octo/cat", "--state", "open", "--limit", "1000", "--json", FIELDS]);

  // The menus: Open pull request #12 on the branch and its worktree.
  await label(page, "feature/login").locator(".name").click({ button: "right" });
  await expect(item(page, "Open pull request #12")).toBeVisible();
  await page.keyboard.press("Escape");
  await wt.click({ button: "right" });
  await expect(item(page, "Open pull request #12")).toBeVisible();
  await page.keyboard.press("Escape");
});

test("create: the menu shows the Pull request tab for the branch at once, suggests from the commits, pushes first as its button says, then shows the pull request", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  gg.gh.slow("pr-list-head", 1);
  gg.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/21\n");
  gg.gh.slow("pr-create", 2);

  await menuOn(page, "fix/typo", "Create pull request…");
  // No dialog: the side panel's Pull request tab, about fix/typo (not the current branch),
  // the focus in the title.
  await expect(dialog(page)).toHaveCount(0);
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pr-branch")).toContainText("fix/typo");
  await expect(panel(page).getByTestId("pr-title")).toBeFocused();
  // At once, saying what it waits for.
  await expect(panel(page).getByTestId("pr-waiting")).toContainText("Asking gh for the default branch");
  await expect(button(page, "Create PR")).toBeDisabled();
  // Typed while it waits: kept when the suggestion comes.
  await panel(page).getByTestId("pr-body").fill("Typo in the first line.");
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(panel(page).getByTestId("pr-body")).toHaveValue("Typo in the first line.");
  await expect(panel(page).getByTestId("pr-base")).toHaveValue("main");
  // The repository it goes into, named; where the branch is pushed first, said.
  await expect(panel(page).getByTestId("pr-repo")).toHaveText("octo/cat");
  await expect(panel(page).getByTestId("push-note")).toHaveText("Pushes first to origin/fix/typo");
  const create = button(page, "Push and create PR");
  await expect(create).toBeEnabled();
  await panel(page).getByTestId("pr-draft").check();
  // gh lists it from now on, as GitHub would.
  gg.gh.reply("pr-list", 0, JSON.stringify([pr(21, "fix/typo", "Fix a typo in the README", true)]));
  gg.gh.reply("pr-list-head", 0, JSON.stringify([pr(21, "fix/typo", "Fix a typo in the README", true)]));
  await create.click();
  // The busy indicator keeps its width when the step appears in it.
  const busy = page.getByTestId("busy");
  await expect(busy).toBeVisible();
  const before = (await busy.boundingBox())!.width;

  // Waiting on the request: the fields read-only, the step said.
  await expect(panel(page).getByTestId("pr-step")).toContainText("Creating the pull request…");
  await expect(button(page, "Creating…")).toBeDisabled();
  await expect(panel(page).getByTestId("pr-title")).toHaveAttribute("readonly", "");
  // The busy indicator says the step too, once the operation has run for a second (op-progress).
  await expect(page.getByTestId("busy")).toContainText("Creating the pull request…");
  expect((await busy.boundingBox())!.width).toBeCloseTo(before, 0); // to the pixel: layout may differ in the last bits

  // Made: the tab shows it; the notice offers Open and Copy link.
  const view = panel(page).getByTestId("pr-view");
  await expect(view).toBeVisible();
  await expect(view.getByTestId("pr-number")).toHaveText("#21");
  await expect(view.getByTestId("pr-state")).toHaveText("Draft");
  await expect(view.getByTestId("pr-view-title")).toHaveText("Fix a typo in the README");
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request · #21 draft/);
  const done = notice(page, "Opened draft pull request #21 of fix/typo into main");
  await expect(done).toBeVisible();
  await expect(done.getByRole("button", { name: "Open", exact: true })).toBeVisible();
  await expect(done.getByRole("button", { name: "Copy link" })).toBeVisible();
  // Pushed to its namesake first, then gh with the description on stdin.
  expect(gg.git("prs", "rev-parse", "refs/remotes/origin/fix/typo").trim()).toBe(gg.git("prs", "rev-parse", "fix/typo").trim());
  const calls = gg.gh.calls();
  const made = calls.findIndex((c) => c[0] === "pr" && c[1] === "create");
  expect(calls[made]).toEqual([
    "pr", "create", "--repo", "octo/cat", "--head", "fix/typo", "--base", "main",
    "--title", "Fix a typo in the README", "--body-file", "-", "--draft",
  ]);
  expect(gg.gh.stdin(made)).toBe("Typo in the first line.");
  await done.getByRole("button", { name: "Open", exact: true }).click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/21"))).toBe(true);
  // The list reloaded by the core has it: still shown, from the list now; its marker in the graph.
  await expect(chip(page, 21)).toBeVisible();
  await expect(view.getByTestId("pr-number")).toHaveText("#21");
  // The form was forgotten with it: nothing typed comes back.
  gg.gh.reply("pr-list", 0, "[]");
  gg.gh.reply("pr-list-head", 0, "[]");
});

test("a branch with one already: the tab shows it, the focus on Open; Refresh asks again; a failure stays in the tab with Show log and the compare page", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);

  gg.gh.reply("pr-list-head", 0, JSON.stringify([pr(20, "docs/readme", "Explain the parser")]));
  await menuOn(page, "docs/readme", "Create pull request…");
  const view = panel(page).getByTestId("pr-view");
  await expect(view.getByTestId("pr-number")).toHaveText("#20");
  await expect(view.getByTestId("pr-view-title")).toHaveText("Explain the parser");
  // The title field that had the focus is gone: the focus is on Open, so Enter opens it.
  await expect(view.getByRole("button", { name: "Open in browser" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/20"))).toBe(true);

  // Closed on GitHub meanwhile: Refresh asks gh again, and the form shows. On the remote
  // already: nothing to push, the button says just that. gh fails: the tab keeps it.
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("pr-create", 1, "", "HTTP 502: Bad Gateway (https://api.github.com/graphql)\n");
  await page.getByTestId("pr-refresh").click();
  const create = button(page, "Create PR");
  await expect(create).toBeEnabled();
  await expect(panel(page).getByTestId("push-note")).toHaveCount(0);
  await create.click();
  const error = panel(page).getByTestId("pr-error");
  await expect(error).toContainText("HTTP 502: Bad Gateway");
  await expect(error.getByRole("button", { name: "Open the compare page" })).toBeVisible();
  await expect(error.getByRole("button", { name: "Show log" })).toBeVisible();
  await expect(create).toBeEnabled();
  await expect(page.locator("[data-notices] .toast", { hasText: "502" })).toHaveCount(0);
  await error.getByRole("button", { name: "Open the compare page" }).click();
  // With the form's title and description: nothing typed is lost.
  const page_ = "https://github.com/octo/cat/compare/main...docs/readme?expand=1&title=Explain%20the%20parser&body=-%20Explain%20the%20parser";
  await expect.poll(() => gg.launches().some((l) => l.includes(page_))).toBe(true);
  // Show log opens the command log at gh's entry.
  await error.getByRole("button", { name: "Show log" }).click();
  await expect(page.getByRole("region", { name: /command log/i })).toBeVisible();
});

test("gh signed out: said once, plainly; the tab says how to sign in, pushes and opens GitHub's compare page", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-list", 4, "", "To get started with GitHub CLI, please run:  gh auth login\n");
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);

  const said = notice(page, "The GitHub CLI (gh) is not signed in");
  await expect(said).toBeVisible();
  await expect(said).not.toHaveClass(/error/);
  await menuOn(page, "fix/typo", "Create pull request…");
  const box = panel(page).getByTestId("pr-no-gh");
  await expect(box).toContainText("gh is not signed in — run `gh auth login` in a terminal.");
  await expect(box).toContainText("fix/typo is pushed to origin/fix/typo, then GitHub’s compare page opens");
  // No form: nothing typed there would be used.
  await expect(panel(page).getByTestId("pr-title")).toHaveCount(0);
  await button(page, "Push and open the compare page").click();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...fix/typo?expand=1")))
    .toBe(true);
  expect(gg.git("prs", "rev-parse", "refs/remotes/origin/fix/typo").trim()).toBe(gg.git("prs", "rev-parse", "fix/typo").trim());
  expect(gg.gh.calls().filter((c) => c[1] === "create")).toEqual([]);
  // On the remote now: the tab just opens the compare page.
  await menuOn(page, "docs/readme", "Create pull request…");
  await expect(page.getByTestId("pr-branch")).toContainText("docs/readme");
  await button(page, "Open the compare page").click();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...docs/readme?expand=1")))
    .toBe(true);
  // Dismissed, it does not come back with the next refresh.
  await said.getByRole("button", { name: /dismiss|close/i }).click();
  await refresh(page);
  await page.waitForTimeout(500);
  await expect(notice(page, "not signed in")).toHaveCount(0);
});

test("gh not installed: the tab says so and how to fix it, pushes and then opens GitHub's compare page", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await expect(notice(page, "Pull requests need the GitHub CLI (gh)")).toBeVisible();

  await menuOn(page, "fix/typo", "Create pull request…");
  const box = panel(page).getByTestId("pr-no-gh");
  await expect(box).toContainText("GitHub CLI (gh) is not installed.");
  await expect(box).toContainText("Install it (cli.github.com) and sign in, then refresh");
  await expect(box).toContainText("fix/typo is pushed to origin/fix/typo, then GitHub’s compare page opens");
  const go = button(page, "Push and open the compare page");
  await expect(go).toBeEnabled();
  await go.click();
  await expect(notice(page, "Pushed fix/typo to origin/fix/typo. Finish on GitHub:")).toBeVisible();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...fix/typo?expand=1")))
    .toBe(true);
  expect(gg.git("prs", "rev-parse", "refs/remotes/origin/fix/typo").trim()).toBe(gg.git("prs", "rev-parse", "fix/typo").trim());
  expect(gg.gh.calls()).toEqual([]);
});

test("gh installed after it was missing: the tab asks again when it shows, and shows the real form", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await expect(notice(page, "Pull requests need the GitHub CLI (gh)")).toBeVisible();
  // Installed within the minute between loads: the tab's own refresh finds it.
  gg.gh.install();
  listing(gg, []);
  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(panel(page).getByTestId("pr-title")).toBeVisible();
  await expect(panel(page).getByTestId("pr-no-gh")).toHaveCount(0);
  await expect(button(page, "Push and create PR")).toBeVisible();
});

test("the compare page the tab promised is what happens, even if gh is installed meanwhile", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await expect(notice(page, "Pull requests need the GitHub CLI (gh)")).toBeVisible();
  await menuOn(page, "fix/typo", "Create pull request…");
  const go = button(page, "Push and open the compare page");
  await expect(go).toBeEnabled();
  // gh appears while the tab shows the compare page (no form): it must not be used.
  gg.gh.install();
  gg.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/99\n");
  await go.click();
  await expect(notice(page, "Pushed fix/typo to origin/fix/typo. Finish on GitHub:")).toBeVisible();
  expect(gg.gh.calls().filter((c) => c[1] === "create")).toEqual([]);
});

test("a suggestion that failed offers Try again, and the form can then be sent", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  // The core cannot answer the first time.
  let failed = false;
  await page.route("**/invoke", (route) => {
    if (!failed && (route.request().postDataJSON() as { cmd: string } | null)?.cmd === "pr_suggest") {
      failed = true;
      return route.fulfill({ json: { err: { code: "io", message: "gh did not answer in time.", detail: null } } });
    }
    return route.continue();
  });
  await menuOn(page, "fix/typo", "Create pull request…");
  const note = panel(page).getByTestId("pr-suggest-failed");
  await expect(note).toContainText("gh did not answer in time.");
  await expect(button(page, "Create PR")).toBeDisabled();
  await note.getByRole("button", { name: "Try again" }).click();
  await expect(note).toHaveCount(0);
  await expect(button(page, "Push and create PR")).toBeEnabled();
  await page.unrouteAll({ behavior: "wait" });
});

test("the button names where the branch really goes: remote.pushDefault sends it to the fork", async ({ gg }) => {
  const { page } = gg;
  listing(gg, []);
  const dir = gg.repoPath("prs");
  const fork = path.join(path.dirname(dir), "prs-fork.git");
  gg.git("prs", "init", "-q", "--bare", "-b", "main", fork);
  gg.git("prs", "remote", "add", "fork", "https://github.com/ada/cat.git");
  gg.git("prs", "config", `url.${fork}.insteadOf`, "https://github.com/ada/cat.git");
  gg.git("prs", "config", "remote.pushDefault", "fork");
  await page.goto("/");
  await open(page, dir);
  await refresh(page);

  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(panel(page).getByTestId("pr-repo")).toHaveText("octo/cat");
  await expect(panel(page).getByTestId("push-note")).toHaveText("Pushes first to fork/fix/typo");
  await expect(panel(page).getByTestId("push-note")).toHaveAttribute("title", "fix/typo is not on fork/fix/typo yet: it is pushed there first");
  // Only the base repository's branches are offered.
  const options = await panel(page).getByTestId("pr-base").locator("option").allTextContents();
  expect(options).toContain("main");
  expect(options.some((o) => o.startsWith("origin/") || o.startsWith("fork/"))).toBe(false);
  await expect(button(page, "Push and create PR")).toBeEnabled();
});

test("several branches with pull requests on one commit: the ones folded into +k are named in its tooltip; a marker click leaves the focus", async ({ gg }) => {
  const { page } = gg;
  for (const name of ["pr/one", "pr/two", "pr/three"]) gg.git("prs", "branch", name, "fix/typo");
  listing(gg, [pr(31, "fix/typo", "Typo"), pr(32, "pr/one", "One"), pr(33, "pr/two", "Two", true), pr(34, "pr/three", "Three")]);
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  const row = grid(page).locator('[role="row"]', { has: page.locator('[data-ref="fix/typo"]') });
  const more = row.locator('[data-slot="more"]');
  await expect(more).toBeVisible();
  await expect(row.locator("button.pr-chip:not([hidden])").first()).toBeVisible();
  await expect.poll(async () => ((await more.getAttribute("title")) ?? "").match(/pull request #\d+/g)?.length ?? 0).toBe(2);
  // The markers shown are not cut off by the cell.
  for (const chipEl of await row.locator("button.pr-chip:not([hidden])").all()) {
    const box = (await chipEl.boundingBox())!;
    const cell = (await row.locator(".desc").boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(cell.x + cell.width + 0.5);
  }
  // A click opens it and leaves the focus where it was (the grid).
  await grid(page).focus();
  await row.locator("button.pr-chip:not([hidden])").first().click();
  await expect(grid(page)).toBeFocused();
});

test("the branch a menu asked for: named at the top, back to the current branch with its button; the worktree menu asks too", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  await menuOn(page, "fix/typo", "Create pull request…");
  const line = page.getByTestId("pr-branch");
  await expect(line).toContainText("fix/typo");
  await expect(line.getByRole("button", { name: "Back to the current branch's pull request" })).toHaveAttribute(
    "title",
    "Back to the current branch's pull request (main)",
  );
  await line.getByTestId("pr-branch-return").click();
  await expect(line).toHaveCount(0);
  // main is the default branch: nothing to open a pull request into, said plainly.
  const none = panel(page).getByTestId("pr-none");
  await expect(none).toHaveAttribute("data-why", "default");
  await expect(none).toContainText("main is the repository’s default branch: there is nothing to open a pull request into.");
  await expect(panel(page).getByTestId("pr-title")).toHaveCount(0);

  // The worktree on feature/login has a pull request? Not here: its menu offers to make one, for its branch.
  await tab(page, "worktrees").click();
  await worktrees(page).locator('[role="row"][data-path$="login"]').click({ button: "right" });
  await item(page, "Create pull request…").click();
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(line).toContainText("feature/login");
  await expect(panel(page).getByTestId("pr-title")).toBeFocused();
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Login");

  // Checked out here, it is the current branch: no line.
  gg.git("prs", "worktree", "remove", "--force", path.join(path.dirname(gg.repoPath("prs")), "prs.worktrees", "login"));
  gg.git("prs", "switch", "-q", "feature/login");
  await refresh(page);
  await expect(page.getByTestId("active-worktree")).toContainText("feature/login");
  await expect(line).toHaveCount(0);
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Login");
});

test("what is typed is kept: another tab, the panel hidden, another repository's tab, and the branch asked for again", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  await menuOn(page, "fix/typo", "Create pull request…");
  const title = panel(page).getByTestId("pr-title");
  const body = panel(page).getByTestId("pr-body");
  await expect(title).toHaveValue("Fix a typo in the README");
  await title.fill("Fix the README's first line");
  await body.fill("It said teh.");
  await panel(page).getByTestId("pr-draft").check();

  // Another tab, and back.
  await tab(page, "commit").click();
  await expect(panel(page)).toHaveCount(0);
  await tab(page, "pr").click();
  await expect(title).toHaveValue("Fix the README's first line");
  await expect(body).toHaveValue("It said teh.");
  await expect(panel(page).getByTestId("pr-draft")).toBeChecked();
  // Shown again by its tab, it leaves the focus where it is.
  await expect(title).not.toBeFocused();

  // The panel hidden and shown again: on the same tab, with the same text.
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(page.locator('[data-region="side-panel"]')).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+b");
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(title).toHaveValue("Fix the README's first line");

  // Another repository's tab in between.
  await open(page, gg.repoPath("small"));
  await page.getByRole("tab", { name: "prs", exact: true }).click();
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(title).toHaveValue("Fix the README's first line");
  await expect(body).toHaveValue("It said teh.");

  // Back to the current branch, and the menu asks for fix/typo again: the same form.
  await page.getByTestId("pr-branch-return").click();
  await expect(title).toHaveCount(0);
  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(title).toHaveValue("Fix the README's first line");
  await expect(body).toHaveValue("It said teh.");
});

test("after a push: the tab shows for the branch pushed, without taking the focus; not for the default branch, one with a pull request, nor with the setting off", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg, [pr(20, "docs/readme", "Explain the parser")]);
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");

  // A branch that has a pull request, one commit ahead of its upstream: nothing changes.
  const ahead = gg.git("prs", "commit-tree", "docs/readme^{tree}", "-p", "docs/readme", "-m", "Explain the lexer").trim();
  gg.git("prs", "branch", "-f", "docs/readme", ahead);
  await refresh(page);
  // The graph has the new commit (a menu opened while the rows change closes).
  await expect(grid(page).locator('[role="row"]', { hasText: "Explain the lexer" })).toHaveCount(1);
  await menuOn(page, "docs/readme", /^Push/);
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, /docs\/readme/)).toBeVisible();
  await page.waitForTimeout(400);
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");

  // The default branch: nothing changes.
  await menuOn(page, "main", /^Push/);
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, /main/)).toBeVisible();
  await page.waitForTimeout(400);
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");

  // A branch with none: the tab shows, about it, the focus left where it was (the graph).
  await grid(page).focus();
  await menuOn(page, "fix/typo", "Push…");
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pr-branch")).toContainText("fix/typo");
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(panel(page).getByTestId("pr-title")).not.toBeFocused();
  await expect(panel(page).getByTestId("push-note")).toHaveCount(0);

  // Hidden, with the setting off: a push leaves it hidden.
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const box = page.getByTestId("settings-pr-tab");
  await expect(box).toBeChecked();
  await box.uncheck();
  await expect.poll(() => page.evaluate(async () => (await import("/src/lib/state/app.svelte.ts")).app.settings.openPrTabAfterPush)).toBe(false);
  await page.keyboard.press("Escape");
  await page.keyboard.press("ControlOrMeta+b");
  await expect(page.locator('[data-region="side-panel"]')).toHaveCount(0);
  gg.git("prs", "branch", "spike", "fix/typo");
  await refresh(page);
  await expect(label(page, "spike")).toBeVisible();
  await menuOn(page, "spike", "Push…");
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, /spike/)).toBeVisible();
  await page.waitForTimeout(400);
  await expect(page.locator('[data-region="side-panel"]')).toHaveCount(0);
});

test("after Commit & Push on a new branch: the tab shows, about the current branch", async ({ gg }) => {
  const { page } = gg;
  // A machine without an identity refuses the commit.
  gg.git("prs", "config", "user.name", "Ada Lovelace");
  gg.git("prs", "config", "user.email", "ada@example.com");
  gg.git("prs", "switch", "-q", "-c", "feature/notes");
  await openListed(gg);
  // A change to commit.
  const dir = gg.repoPath("prs");
  writeFileSync(path.join(dir, "notes.md"), "Notes\n");
  await tab(page, "commit").click();
  await page.getByTestId("commit-message").fill("Add notes");
  const push = page.getByTestId("commit-push-button");
  await expect(push).toBeEnabled();
  await push.click();
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pr-branch")).toHaveCount(0);
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Add notes");

});

test("gh missing: pull requests are not listed, and a push leaves the side panel as it is", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await expect(notice(page, "Pull requests need the GitHub CLI (gh)")).toBeVisible();
  await menuOn(page, "fix/typo", "Push…");
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, /Pushed fix\/typo/)).toBeVisible();
  await page.waitForTimeout(400);
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
});

test("a pull request's view: state, number, the title in full, base ← head, Open in browser, Copy link, Refresh, and as of when once stale", async ({ gg }) => {
  const { page } = gg;
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.clock.install();
  gg.git("prs", "switch", "-q", "fix/typo");
  const long = "Fix the typo in the README's first line, which said teh where it meant the, and the one in the parser's error message";
  await openListed(gg, [pr(31, "fix/typo", long), pr(32, "docs/readme", "Explain the parser", true)]);

  // ⌘⇧P (Ctrl+Shift+P): the tab for the current branch, the focus on Open.
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  const view = panel(page).getByTestId("pr-view");
  await expect(view.getByTestId("pr-state")).toHaveText("Open");
  await expect(view.getByTestId("pr-number")).toHaveText("#31");
  await expect(view.getByTestId("pr-view-title")).toHaveText(long);
  await expect(view.getByTestId("pr-flow")).toHaveText(/main\s*←\s*fix\/typo/);
  await expect(view.getByRole("button", { name: "Open in browser" })).toBeFocused();
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request · #31 open \((⌘⇧P|Ctrl\+Shift\+P)\)$/);
  // The title wraps, never cut.
  const t = view.getByTestId("pr-view-title");
  expect(await t.evaluate((el) => el.scrollWidth <= el.clientWidth && getComputedStyle(el).textOverflow !== "ellipsis")).toBe(true);
  expect((await t.boundingBox())!.height).toBeGreaterThan(30);
  await expect(view.getByTestId("pr-stale")).toHaveCount(0);

  await view.getByRole("button", { name: "Open in browser" }).click();
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/31"))).toBe(true);
  await view.getByRole("button", { name: "Copy link" }).click();
  await expect(notice(page, "Copied the link.")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("https://github.com/octo/cat/pull/31");

  // Refresh: the toolbar's refresh of the pull requests, busy while it runs.
  const reload = page.getByTestId("pr-refresh");
  await reload.click();
  await expect(reload).toHaveAttribute("aria-busy", "true");
  await expect(reload).toHaveAttribute("aria-busy", "false");

  // A draft: hollow and quiet, as its marker.
  gg.git("prs", "switch", "-q", "docs/readme");
  await refresh(page);
  await expect(page.getByTestId("active-worktree")).toContainText("docs/readme");
  await expect(view.getByTestId("pr-number")).toHaveText("#32");
  await expect(view.getByTestId("pr-state")).toHaveText("Draft");
  await expect(view.getByTestId("pr-state")).toHaveClass(/draft/);

  // Older than 10 minutes: as of when.
  await page.clock.fastForward("11:00");
  await expect(view.getByTestId("pr-stale")).toHaveText(/^as of /);
  // The checks and comments' room is left empty for now.
  await expect(view.locator(".rest")).toBeEmpty();
});

test("a remote not on GitHub, HEAD detached, no remote: the tab says so plainly, no form", async ({ gg }) => {
  const { page } = gg;
  gg.git("prs", "remote", "set-url", "origin", "https://gitlab.com/octo/cat.git");
  // gh has no credentials for that host: it is not GitHub.
  gg.gh.reply("auth-token", 1, "", "no oauth token found for gitlab.com\n");
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await tab(page, "pr").click();
  const none = panel(page).getByTestId("pr-none");
  await expect(none).toHaveAttribute("data-why", "not_github");
  await expect(none).toContainText("This remote is not on GitHub: pull requests are made on GitHub only.");
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request \((⌘⇧P|Ctrl\+Shift\+P)\)$/);

  gg.git("prs", "switch", "-q", "--detach", "main");
  await refresh(page);
  await expect(none).toHaveAttribute("data-why", "detached");
  await expect(none).toContainText("HEAD is detached: there is no branch to open a pull request from.");

  gg.git("prs", "switch", "-q", "fix/typo");
  gg.git("prs", "remote", "remove", "origin");
  await refresh(page);
  await expect(none).toHaveAttribute("data-why", "no_remote");
  await expect(none).toContainText("This repository has no remote.");
  await expect(panel(page).getByTestId("pr-title")).toHaveCount(0);
  expect(gg.gh.calls().filter((c) => c[0] === "pr")).toEqual([]);
});

test("the tab strip: Commit, Pull request, Worktrees, in that order, by the arrow keys too", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  const strip = page.getByRole("tablist", { name: "Side panel" });
  await expect(strip.getByRole("tab")).toHaveCount(3);
  expect(await strip.getByRole("tab").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label")))).toEqual([
    "Commit",
    "Pull request",
    "Worktrees",
  ]);
  await expect(tab(page, "pr")).toHaveAttribute("title", SHORTCUT);
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request · none yet \((⌘⇧P|Ctrl\+Shift\+P)\)$/);
  await expect(tab(page, "pr").locator("svg")).toHaveAttribute("width", "14");
  await tab(page, "commit").focus();
  await page.keyboard.press("ArrowRight");
  await expect(tab(page, "pr")).toBeFocused();
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#side-tabpanel")).toHaveAttribute("aria-labelledby", "side-tab-pr");
  // Its own button in the header: Refresh.
  await expect(page.getByTestId("pr-refresh")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pr-refresh")).toHaveCount(0);
});

test("at the panel's least width the form fits: one line naming a long branch, every field and the button inside the panel", async ({ gg }) => {
  const { page } = gg;
  const long = "feature/rework-the-parser-error-recovery-for-unterminated-strings";
  gg.git("prs", "branch", long, "fix/typo");
  await page.setViewportSize({ width: 900, height: 560 });
  await openListed(gg);
  await menuOn(page, long, "Create pull request…");
  await expect(panel(page).getByTestId("pr-title")).not.toHaveValue("");
  const side = page.locator('[data-region="side-panel"]');
  const s = (await side.boundingBox())!;
  expect(Math.round(s.width)).toBe(240);
  // The branch on one line, cut in the middle.
  const line = page.getByTestId("pr-branch");
  expect((await line.boundingBox())!.height).toBeLessThanOrEqual(31);
  // Everything up to the button fits in the panel and the window, side by side with nothing.
  for (const el of [
    panel(page).getByTestId("pr-title"),
    panel(page).getByTestId("pr-body"),
    panel(page).getByTestId("pr-base"),
    panel(page).getByTestId("pr-draft"),
    panel(page).getByTestId("push-note"),
    button(page, "Push and create PR"),
  ]) {
    await expect(el).toBeInViewport({ ratio: 1 });
    const b = (await el.boundingBox())!;
    expect(b.x).toBeGreaterThanOrEqual(s.x + 11);
    expect(b.x + b.width).toBeLessThanOrEqual(s.x + s.width - 11);
  }
  // Nothing scrolls sideways.
  expect(await panel(page).evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  // The description takes the height there is, and is never shorter than a few lines.
  expect((await panel(page).getByTestId("pr-body").boundingBox())!.height).toBeGreaterThanOrEqual(95);

  // A pull request's view fits too.
  gg.gh.reply("pr-list-head", 0, JSON.stringify([pr(44, long, "Rework the parser's error recovery")]));
  await page.getByTestId("pr-refresh").click();
  const view = panel(page).getByTestId("pr-view");
  await expect(view).toBeVisible();
  for (const el of [view.getByRole("button", { name: "Open in browser" }), view.getByRole("button", { name: "Copy link" }), view.getByTestId("pr-flow")]) {
    const b = (await el.boundingBox())!;
    expect(b.x + b.width).toBeLessThanOrEqual(s.x + s.width - 11);
  }
  const buttons = await view.locator(".actions button").evaluateAll((els) => els.map((e) => e.getBoundingClientRect().top));
  expect(new Set(buttons.map((y) => Math.round(y))).size).toBe(1);
});

test("in a narrow window the shortcut shows the tab over the graph, and Escape hides it", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 780, height: 600 });
  gg.git("prs", "switch", "-q", "fix/typo");
  await openListed(gg);
  const side = page.locator('[data-region="side-panel"]');
  await expect(side).toHaveCount(0);
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");
  await expect(side).toHaveClass(/overlay/);
  await expect(tab(page, "pr")).toHaveAttribute("aria-selected", "true");
  await expect(panel(page).getByTestId("pr-title")).toBeFocused();
  await expect(panel(page).getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(button(page, "Push and create PR")).toBeInViewport({ ratio: 1 });
  await page.keyboard.press("Escape");
  await expect(side).toHaveCount(0);
});

test("a suggestion for another branch that comes late never fills the form shown", async ({ gg }) => {
  const { page } = gg;
  await openListed(gg);
  // fix/typo's suggestion, kept.
  await menuOn(page, "fix/typo", "Create pull request…");
  const title = panel(page).getByTestId("pr-title");
  await expect(title).toHaveValue("Fix a typo in the README");
  // docs/readme's is held back by the core.
  let release: () => void = () => {};
  const held = new Promise<void>((r) => (release = r));
  let asked = false;
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd: string; args: { branch?: string } } | null;
    if (body?.cmd === "pr_suggest" && body.args.branch === "docs/readme") {
      asked = true;
      await held;
    }
    return route.continue();
  });
  await menuOn(page, "docs/readme", "Create pull request…");
  await expect.poll(() => asked).toBe(true);
  // Back to fix/typo (its suggestion kept) before docs/readme's answer comes.
  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(page.getByTestId("pr-branch")).toContainText("fix/typo");
  await expect(title).toHaveValue("Fix a typo in the README");
  release();
  await page.waitForTimeout(800);
  await expect(page.getByTestId("pr-branch")).toContainText("fix/typo");
  await expect(title).toHaveValue("Fix a typo in the README");
  await expect(panel(page).getByTestId("pr-body")).toHaveValue("- Fix a typo in the README");
  await page.unrouteAll({ behavior: "wait" });
  // docs/readme's, kept when it came, is there when its form shows.
  await menuOn(page, "docs/readme", "Create pull request…");
  await expect(title).toHaveValue("Explain the parser");
});
