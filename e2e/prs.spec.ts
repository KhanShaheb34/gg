// Pull requests against the real core, with the devserver's fake `gh` (never the real one,
// never the network; the browser is never started: URL opens are recorded). The `prs`
// fixture's remote is github.com for the app (a local bare repository underneath). Markers on
// branches and worktrees, the menus, the Create pull request dialog (create with the push
// first, an existing one, a failure kept in the dialog), and the signed-out fallback to the
// compare page.
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

const FIELDS = "number,title,url,isDraft,headRefName,baseRefName,headRepositoryOwner,isCrossRepository";
const SEARCH = "feature/search-indexing-across-every-workspace-package";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const worktrees = (page: Page) => page.getByRole("grid", { name: "Worktrees" });
const label = (page: Page, name: string) => grid(page).locator(`[data-ref="${name}"]`).first();
const chip = (page: Page, n: number) => grid(page).locator(`button.pr-chip[data-pr="${n}"]`);
const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });
const notice = (page: Page, text: string | RegExp) => page.locator("[data-notices] .toast", { hasText: text });
const dialog = (page: Page) => page.getByRole("dialog");

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

/** The user's refresh: the pull requests load with it. */
async function refresh(page: Page) {
  await page.getByRole("button", { name: "Refresh" }).click();
}

async function menuOn(page: Page, name: string, itemName: string | RegExp) {
  await label(page, name).locator(".name").click({ button: "right" });
  await item(page, itemName).click();
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

test("create: the dialog opens at once, suggests from the commits, pushes first as its button says, and leaves Open and Copy link", async ({ gg }) => {
  const { page } = gg;
  listing(gg, []);
  gg.gh.slow("pr-list-head", 1);
  gg.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/21\n");
  gg.gh.slow("pr-create", 2);
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);

  await menuOn(page, "fix/typo", "Create pull request…");
  // At once, saying what it waits for; Cancel works meanwhile.
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).getByTestId("pr-waiting")).toContainText("Asking gh for the default branch");
  await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeEnabled();
  // Typed while it waits: kept when the suggestion comes.
  await dialog(page).getByTestId("pr-body").fill("Typo in the first line.");
  await expect(dialog(page).getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(dialog(page).getByTestId("pr-body")).toHaveValue("Typo in the first line.");
  await expect(dialog(page).getByTestId("pr-base")).toHaveValue("main");
  // The repository it goes into, named (Task 9 review I3).
  await expect(dialog(page).getByTestId("pr-repo")).toHaveText("octo/cat");
  await expect(dialog(page).getByTestId("push-note")).toHaveText("not pushed to origin/fix/typo yet");
  const create = dialog(page).getByRole("button", { name: "Push fix/typo to origin/fix/typo and create pull request" });
  await expect(create).toBeEnabled();
  await dialog(page).getByTestId("pr-draft").check();
  await create.click();
  // Task 10 review minor 12: the busy indicator keeps its width when the step appears in it.
  const busy = page.getByTestId("busy");
  await expect(busy).toBeVisible();
  const before = (await busy.boundingBox())!.width;

  // Waiting on the request: Cancel off, the step said.
  await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeDisabled();
  await expect(dialog(page).getByTestId("pr-step")).toContainText("Creating the pull request…");
  await expect(dialog(page).getByRole("button", { name: "Creating…" })).toBeDisabled();
  // The busy indicator says the step too, once the operation has run for a second (op-progress).
  await expect(page.getByTestId("busy")).toContainText("Creating the pull request…");
  expect((await busy.boundingBox())!.width).toBe(before);

  await expect(dialog(page)).toHaveCount(0);
  const done = notice(page, "Opened draft pull request #21 of fix/typo into main");
  await expect(done).toBeVisible();
  await expect(done.getByRole("button", { name: "Open", exact: true })).toBeVisible();
  await expect(done.getByRole("button", { name: "Copy link" })).toBeVisible();
  // Pushed to its namesake first (R38), then gh with the description on stdin.
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
});

test("an existing pull request: the dialog says so and opens it; a failure stays in the dialog with Show log", async ({ gg }) => {
  const { page } = gg;
  listing(gg, []);
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);

  gg.gh.reply("pr-list-head", 0, JSON.stringify([pr(20, "docs/readme", "Explain the parser")]));
  await menuOn(page, "docs/readme", "Create pull request…");
  await expect(dialog(page).getByRole("heading")).toHaveText("Pull request #20 already exists");
  await expect(dialog(page).getByTestId("pr-existing")).toContainText("Explain the parser");
  await expect(dialog(page).getByRole("button", { name: "Copy link" })).toBeVisible();
  // The title field that had the focus is gone: the focus is on Open, so Enter opens it
  // (Task 9 review I4).
  await expect(dialog(page).getByRole("button", { name: "Open", exact: true })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(dialog(page)).toHaveCount(0);
  await expect.poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/pull/20"))).toBe(true);

  // On the remote already: nothing to push, the button says just that. gh fails: the dialog stays.
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("pr-create", 1, "", "HTTP 502: Bad Gateway (https://api.github.com/graphql)\n");
  await menuOn(page, "docs/readme", "Create pull request…");
  const create = dialog(page).getByRole("button", { name: "Create pull request", exact: true });
  await expect(create).toBeEnabled();
  await create.click();
  const error = dialog(page).getByTestId("pr-error");
  await expect(error).toContainText("HTTP 502: Bad Gateway");
  await expect(error.getByRole("button", { name: "Open the compare page" })).toBeVisible();
  await expect(error.getByRole("button", { name: "Show log" })).toBeVisible();
  await expect(dialog(page).getByRole("button", { name: "Cancel" })).toBeEnabled();
  await expect(page.locator("[data-notices] .toast", { hasText: "502" })).toHaveCount(0);
  await error.getByRole("button", { name: "Open the compare page" }).click();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...docs/readme?expand=1")))
    .toBe(true);
});

test("gh signed out: said once, plainly; the branch menu pushes and opens GitHub's compare page, through the dialog", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-list", 4, "", "To get started with GitHub CLI, please run:  gh auth login\n");
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);

  const said = notice(page, "The GitHub CLI (gh) is not signed in");
  await expect(said).toBeVisible();
  await expect(said).not.toHaveClass(/error/);
  // Task 9 review I5: as without gh, the branch is pushed first, and the menu says so.
  // Task 10 review minor 8: the menu promises no more than it knows; the dialog says it.
  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(dialog(page).getByTestId("pr-no-gh")).toContainText("fix/typo is pushed to origin/fix/typo, then GitHub’s compare page opens");
  await dialog(page).getByRole("button", { name: "Push fix/typo to origin/fix/typo and open the compare page" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...fix/typo?expand=1")))
    .toBe(true);
  expect(gg.git("prs", "rev-parse", "refs/remotes/origin/fix/typo").trim()).toBe(gg.git("prs", "rev-parse", "fix/typo").trim());
  expect(gg.gh.calls().filter((c) => c[1] === "create")).toEqual([]);
  // On the remote now: the menu just opens the compare page (through the dialog's button).
  await menuOn(page, "docs/readme", "Create pull request…");
  await dialog(page).getByRole("button", { name: "Open the compare page" }).click();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...docs/readme?expand=1")))
    .toBe(true);
  // Dismissed, it does not come back with the next refresh.
  await said.getByRole("button", { name: /dismiss|close/i }).click();
  await refresh(page);
  await page.waitForTimeout(500);
  await expect(notice(page, "not signed in")).toHaveCount(0);
});

test("gh not installed: the dialog says it pushes and then opens GitHub's compare page, and does so", async ({ gg }) => {
  const { page } = gg;
  gg.gh.uninstall();
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await refresh(page);
  await expect(notice(page, "Pull requests need the GitHub CLI (gh)")).toBeVisible();

  await menuOn(page, "fix/typo", "Create pull request…");
  await expect(dialog(page).getByTestId("pr-no-gh")).toContainText(
    "GitHub CLI (gh) is not installed: fix/typo is pushed to origin/fix/typo, then GitHub’s compare page opens",
  );
  const go = dialog(page).getByRole("button", { name: "Push fix/typo to origin/fix/typo and open the compare page" });
  await expect(go).toBeEnabled();
  await go.click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(notice(page, "Pushed fix/typo to origin/fix/typo. Finish on GitHub:")).toBeVisible();
  await expect
    .poll(() => gg.launches().some((l) => l.includes("https://github.com/octo/cat/compare/main...fix/typo?expand=1")))
    .toBe(true);
  expect(gg.git("prs", "rev-parse", "refs/remotes/origin/fix/typo").trim()).toBe(gg.git("prs", "rev-parse", "fix/typo").trim());
  expect(gg.gh.calls()).toEqual([]);
});

test("the button names where the branch really goes: remote.pushDefault sends it to the fork (Task 9 review I3)", async ({ gg }) => {
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
  await expect(dialog(page).getByTestId("pr-repo")).toHaveText("octo/cat");
  await expect(dialog(page).getByTestId("push-note")).toHaveText("not pushed to fork/fix/typo yet");
  // Only the base repository's branches are offered.
  const options = await dialog(page).getByTestId("pr-base").locator("option").allTextContents();
  expect(options).toContain("main");
  expect(options.some((o) => o.startsWith("origin/") || o.startsWith("fork/"))).toBe(false);
  await expect(dialog(page).getByRole("button", { name: "Push fix/typo to fork/fix/typo and create pull request" })).toBeEnabled();
});

test("several branches with pull requests on one commit: the ones folded into +k are named in its tooltip; a marker click leaves the focus (Task 9 review minor 5)", async ({ gg }) => {
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
