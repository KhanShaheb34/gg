// The side panel's worktree list against the real core: what each row says, worktrees made
// in a terminal showing up, making one active, creating and deleting them (with the core's
// red confirmation when work would be lost, the ordinary one for pruning), the menus that
// reach the Create dialog, the "make it active" offer of a refusal, showing and hiding the
// panel (remembered; laid over the graph in a narrow window), and the keyboard.
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, openFolder, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

/** Opens `dir`; `list`: on the side panel's Worktrees tab (the panel shown, docked). */
async function open(page: Page, dir: string, list = true) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  if (list) await page.getByTestId("side-tab-worktrees").click();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const list = (page: Page) => page.getByRole("grid", { name: "Worktrees" });
const rows = (page: Page) => list(page).getByRole("row");
/** The row of the worktree in folder `folder` (its last path component). */
const row = (page: Page, folder: string) => list(page).locator(`[role="row"][data-path$="${path.sep}${folder}"]`);
const activeRow = (page: Page) => list(page).locator('[role="row"][data-active="true"]');
const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });
const notice = (page: Page, text: string | RegExp) => page.locator("[data-notices] .toast", { hasText: text });
/** The folder the fixture's linked worktrees are in. */
const linked = (repo: string) => path.join(path.dirname(repo), "worktrees.worktrees");

test("every worktree is listed with how it stands, the main one first and the active one marked", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("worktrees"));
  await expect(rows(page)).toHaveCount(8);
  await expect(rows(page).first()).toHaveAttribute("data-path", gg.repoPath("worktrees"));
  await expect(activeRow(page)).toHaveAttribute("data-path", gg.repoPath("worktrees"));
  await expect(activeRow(page)).toHaveAttribute("aria-label", /^main, worktrees, active, main worktree/);

  // Changes and ahead/behind; each folder by its name, the folder they are in said once.
  await expect(page.getByTestId("worktrees-folder")).toHaveText("in~/repos/worktrees.worktrees");
  const login = row(page, "feature-login");
  await expect(login.locator(".name")).toHaveText("feature/login");
  await expect(login.locator(".path")).toHaveText("feature-login");
  await expect(login.locator(".path")).toHaveAttribute("title", path.join(linked(gg.repoPath("worktrees")), "feature-login"));
  await expect(activeRow(page).locator(".path")).toHaveText("worktrees");
  await expect(login.locator(".changes")).toHaveText("2");
  await expect(login.locator(".changes")).toHaveAttribute("title", "Uncommitted: 1 changed, 1 new. Commit…");
  await expect(login.locator(".ab")).toHaveText("↑2 ↓1");
  // No upstream, a commit on no remote.
  await expect(row(page, "fix-typo").locator(".line.one .upstream")).toHaveText("1 unpushed");
  await expect(row(page, "docs").locator(".line.one .upstream")).toHaveText("no upstream");
  // Detached: the short hash, monospace.
  await expect(row(page, "detached").locator(".hash")).toHaveText(gg.git("worktrees", "rev-parse", "--short=8", "main~2").trim());
  // Locked, missing, a merge stopped there with its conflict.
  await expect(row(page, "release").locator(".line.two .badge")).toHaveText("locked");
  await expect(row(page, "release").locator(".badge")).toHaveAttribute("title", "Locked: on a USB drive");
  await expect(row(page, "old-spike").locator(".badge")).toHaveText("folder missing");
  await expect(row(page, "hotfix").locator(".badge")).toHaveText("merging");
  await expect(row(page, "hotfix").locator(".changes")).toHaveClass(/conflicts/);

  // The header names the active worktree's branch, which opens the branch picker; the status bar its folder.
  const header = page.getByTestId("active-worktree");
  await expect(header.locator(".wt-branch")).toHaveText("main");
  await expect(page.getByTestId("status-path")).toHaveText("~/repos/worktrees");
  await expect(header).toHaveAttribute("aria-label", /^Switch branch: main, in the active worktree ~\/repos\/worktrees, one of 8 worktrees/);
  // Rows hold no interactive element inside an option: a grid of rows, buttons in their own cells.
  await expect(list(page).locator('[role="option"]')).toHaveCount(0);
  await header.click();
  await expect(page.getByTestId("branch-picker")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("branch-picker")).toHaveCount(0);
  // With the panel hidden, the header counts the worktrees; the count shows them, the focus in the list.
  await page.keyboard.press("ControlOrMeta+b");
  await expect(list(page)).toHaveCount(0);
  await page.getByTestId("worktree-count").click();
  await expect(list(page)).toBeFocused();
});

test("a worktree added or removed in a terminal shows within a second", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  await expect(rows(page)).toHaveCount(8);
  gg.git("worktrees", "worktree", "add", "-q", "-b", "agent/task", path.join(linked(repo), "agent-task"));
  await expect(row(page, "agent-task")).toBeVisible({ timeout: 1000 });
  await expect(row(page, "agent-task").locator(".name")).toHaveText("agent/task");
  gg.git("worktrees", "worktree", "remove", path.join(linked(repo), "agent-task"));
  await expect(row(page, "agent-task")).toHaveCount(0, { timeout: 1000 });
});

test("a click shows the worktree's HEAD in the graph; making it active moves the uncommitted row there", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  const login = row(page, "feature-login");
  await login.click();
  await expect(login).toHaveAttribute("aria-selected", "true");
  const head = gg.git("worktrees", "rev-parse", "feature/login").trim();
  await expect(grid(page).locator('[role="row"][aria-selected="true"]')).toHaveAttribute("data-hash", head);

  // The main worktree is clean: no uncommitted row.
  await expect(grid(page).locator('[role="row"].uncommitted')).toHaveCount(0);
  await login.dblclick();
  await expect(activeRow(page)).toHaveAttribute("data-path", path.join(linked(repo), "feature-login"));
  await expect(page.getByTestId("active-worktree")).toContainText("feature/login");
  const uncommitted = grid(page).locator('[role="row"].uncommitted');
  await expect(uncommitted).toHaveCount(1);
  await expect(uncommitted.locator(".subject")).toHaveText("Uncommitted changes (2)");

  // The row's button makes another one active; the panel shows the switch at once.
  const main = rows(page).first();
  await main.hover();
  await expect(main.getByRole("button", { name: "Make active" })).toBeVisible();
  await main.getByRole("button", { name: "Make active" }).click();
  await expect(activeRow(page)).toHaveAttribute("data-path", repo);
  await expect(grid(page).locator('[role="row"].uncommitted')).toHaveCount(0);

  // A worktree whose folder is gone cannot be made active, and says why.
  await row(page, "old-spike").dblclick();
  await expect(notice(page, "old-spike cannot be made active. Its folder is missing.")).toBeVisible();
  await expect(activeRow(page)).toHaveAttribute("data-path", repo);
});

test("a worktree is created from the panel, checked out and made active, then deleted with its branch", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  await page.getByRole("button", { name: "Add worktree" }).click();
  const dialog = page.getByRole("dialog", { name: "Create worktree" });
  await expect(dialog.getByRole("radio", { name: /A new branch/ })).toBeChecked();
  await expect(dialog).toContainText("Starting at main.");
  const create = dialog.getByRole("button", { name: "Create worktree" });
  await expect(create).toBeDisabled();
  await dialog.getByLabel("New branch name").fill("feature/payments");
  // The location follows the pattern in the settings, home as ~.
  const location = dialog.getByTestId("worktree-location");
  await expect(location).toHaveValue("~/repos/worktrees.worktrees/feature-payments");
  // Inline checks: a name taken, a folder of a worktree.
  await dialog.getByLabel("New branch name").fill("fix/typo");
  await expect(dialog).toContainText("A branch named “fix/typo” already exists.");
  await expect(create).toBeDisabled();
  await dialog.getByLabel("New branch name").fill("feature/payments");
  await location.fill("~/repos/worktrees.worktrees/docs");
  await expect(dialog).toContainText("There is a worktree there already.");
  await location.fill("~/repos/worktrees.worktrees/docs/inner");
  await expect(dialog).toContainText("inside the worktree at ~/repos/worktrees.worktrees/docs");
  await location.fill("~/repos/worktrees.worktrees/payments");
  await dialog.getByRole("checkbox", { name: /Make it the active worktree/ }).check();
  // An agent adds a worktree from a terminal while the dialog is open: the one made here is
  // made active, not that one.
  const agents = path.join(linked(repo), "agent-task");
  gg.git("worktrees", "worktree", "add", "-q", "-b", "agent/task", agents);
  await create.click();
  await expect(dialog).toHaveCount(0);
  const made = path.join(linked(repo), "payments");
  await expect(row(page, "payments")).toBeVisible();
  await expect(row(page, "agent-task")).toBeVisible();
  await expect(activeRow(page)).toHaveAttribute("data-path", made);
  expect(gg.gitAt(made, "rev-parse", "--abbrev-ref", "HEAD").trim()).toBe("feature/payments");

  // Deleted: clean, its branch merged (at main): one plain dialog, and it goes with its branch;
  // the active one hands the activity to main first.
  await row(page, "payments").click({ button: "right" });
  await item(page, "Delete…").click();
  const del = page.getByRole("dialog", { name: "Delete worktree payments" });
  await expect(del).toContainText("It is the active worktree: the main worktree becomes active first.");
  await expect(del.getByRole("checkbox", { name: /Also delete branch feature\/payments/ })).not.toBeChecked();
  await del.getByRole("checkbox", { name: /Also delete branch/ }).check();
  await del.getByRole("button", { name: "Delete worktree" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(row(page, "payments")).toHaveCount(0);
  await expect(activeRow(page)).toHaveAttribute("data-path", repo);
  expect(existsSync(made)).toBe(false);
  expect(gg.git("worktrees", "branch", "--list", "feature/payments").trim()).toBe("");
});

test("the core's refusal of a folder shows under the location, the dialog kept open", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  // A folder with something in it.
  const full = path.join(path.dirname(repo), "full");
  mkdirSync(full, { recursive: true });
  writeFileSync(path.join(full, "keep.txt"), "mine\n");
  await page.getByRole("button", { name: "Add worktree" }).click();
  const dialog = page.getByRole("dialog", { name: "Create worktree" });
  await dialog.getByLabel("New branch name").fill("feature/full");
  await dialog.getByTestId("worktree-location").fill("~/repos/full");
  await dialog.getByRole("button", { name: "Create worktree" }).click();
  await expect(dialog).toContainText("is not empty");
  await expect(dialog.getByTestId("worktree-location")).toHaveAttribute("aria-invalid", "true");
  await expect(page.locator("[data-notices] .toast")).toHaveCount(0);
  // Another folder: created.
  await dialog.getByTestId("worktree-location").fill("~/repos/empty-enough");
  await dialog.getByRole("button", { name: "Create worktree" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(row(page, "empty-enough")).toBeVisible();
});

test("deleting a worktree with changes lists them and needs the red confirmation", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  const login = path.join(linked(repo), "feature-login");
  // Its changes, read in the background, are known first.
  await expect(row(page, "feature-login").locator(".changes")).toHaveText("2");
  await row(page, "feature-login").click({ button: "right" });
  await item(page, "Delete…").click();
  const del = page.getByRole("dialog", { name: "Delete worktree feature-login" });
  await expect(del.getByTestId("delete-work-note")).toHaveText(/It has 2 uncommitted changes/);
  // The plain dialog's button is not red.
  await expect(del.getByRole("button", { name: "Delete worktree" })).not.toHaveClass(/danger/);
  await del.getByRole("button", { name: "Delete worktree" }).click();
  const red = page.getByRole("alertdialog");
  await expect(red).toBeVisible();
  await expect(red.getByTestId("dangers")).toContainText("app.txt");
  await expect(red.getByTestId("dangers")).toContainText("login/notes.txt");
  await expect(red.getByRole("button", { name: "Delete worktree" })).toHaveClass(/danger/);
  // Cancel keeps everything.
  await red.getByRole("button", { name: "Cancel" }).click();
  await expect(red).toHaveCount(0);
  expect(existsSync(path.join(login, "login", "notes.txt"))).toBe(true);
  await expect(row(page, "feature-login")).toBeVisible();

  // Confirmed: gone, its work with it.
  await row(page, "feature-login").click({ button: "right" });
  await item(page, "Delete…").click();
  await page.getByRole("dialog", { name: "Delete worktree feature-login" }).getByRole("button", { name: "Delete worktree" }).click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await page.waitForTimeout(450); // a red button takes no click in its first moments
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete worktree" }).click();
  await expect(row(page, "feature-login")).toHaveCount(0);
  expect(existsSync(login)).toBe(false);
  // The branch stays.
  expect(gg.git("worktrees", "branch", "--list", "feature/login").trim()).toBe("feature/login");
});

test("ignored files alone and pruning are asked in the ordinary style, not red", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  await row(page, "docs").click({ button: "right" });
  await item(page, "Delete…").click();
  // One dialog: what the core found (ignored files git deletes) is listed in the same dialog.
  const ask = page.getByRole("dialog", { name: "Delete worktree docs" });
  await ask.getByRole("button", { name: "Delete worktree" }).click();
  await expect(ask.getByTestId("dangers")).toContainText("build/");
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(ask.locator('[data-severity="ordinary"]')).toHaveCount(1);
  await expect(ask.getByRole("button", { name: "Delete worktree" })).not.toHaveClass(/danger/);
  await ask.getByRole("button", { name: "Delete worktree" }).click();
  await expect(row(page, "docs")).toHaveCount(0);
  await expect(ask).toHaveCount(0);

  // Prune, from the panel's menu: the missing one is named, in the ordinary style.
  await page.getByRole("button", { name: "Worktree actions" }).click();
  await item(page, "Prune stale entries…").click();
  const prune = page.getByRole("dialog", { name: "Prune stale worktree entries" });
  await expect(prune).toContainText("old-spike");
  await expect(prune.getByRole("button", { name: "Prune" })).not.toHaveClass(/danger/);
  await prune.getByRole("button", { name: "Prune" }).click();
  await expect(row(page, "old-spike")).toHaveCount(0);
  await expect(notice(page, /old-spike/)).toBeVisible();
});

test("branch and commit menus open the Create dialog where they were asked", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  // A branch no worktree has: checked out as it is.
  await grid(page).locator('.ref[data-ref="patch"]').click({ button: "right" });
  await item(page, "Create worktree from patch…").click();
  let dialog = page.getByRole("dialog", { name: "Create worktree" });
  await expect(dialog.getByRole("radio", { name: /An existing branch/ })).toBeChecked();
  await expect(dialog.getByLabel("Branch", { exact: true })).toHaveValue("patch");
  await expect(dialog.getByTestId("worktree-location")).toHaveValue("~/repos/worktrees.worktrees/patch");
  await dialog.getByRole("button", { name: "Create worktree" }).click();
  await expect(row(page, "patch")).toBeVisible();
  await expect(activeRow(page)).toHaveAttribute("data-path", repo); // not made active

  // A branch checked out somewhere: a new branch at it, said why.
  await grid(page).locator('.ref[data-ref="fix/typo"]').click({ button: "right" });
  await item(page, "Create worktree from fix/typo…").click();
  dialog = page.getByRole("dialog", { name: "Create worktree" });
  await expect(dialog.getByTestId("taken-note")).toContainText("fix/typo is checked out in ~/repos/worktrees.worktrees/fix-typo");
  await expect(dialog.getByRole("radio", { name: /A new branch/ })).toBeChecked();
  await dialog.getByRole("button", { name: "Cancel" }).click();

  // A commit: a new branch at it, or the commit detached.
  await grid(page).locator('[role="row"]', { hasText: "Add a readme" }).click({ button: "right" });
  await item(page, "Create worktree here…").click();
  dialog = page.getByRole("dialog", { name: "Create worktree" });
  await dialog.getByRole("radio", { name: /This commit, detached/ }).check();
  await expect(dialog.getByTestId("worktree-location")).toHaveValue(/~\/repos\/worktrees\.worktrees\/[0-9a-f]{8}$/);
  await dialog.getByRole("button", { name: "Create worktree" }).click();
  await expect(dialog).toHaveCount(0);
  const readme = gg.git("worktrees", "rev-parse", "HEAD~1").trim();
  await expect(list(page).locator(".hash", { hasText: readme.slice(0, 8) })).toBeVisible();
});

test("a branch checked out in another worktree: the notice offers to make that one active", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  // Checking it out here is refused by git; the menu says so first, so ask the core directly.
  await page.evaluate(async () => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const id = app.activeRepoId!;
    const snap = findRepoView(id)!.snapshot;
    await ops.runOp(id, { op: "checkout_branch", branch: "fix/typo", fastForward: false, expectWorktree: snap.activeWorktree }).catch(() => {});
  });
  const offer = page.getByRole("button", { name: "Make that worktree active" });
  await expect(offer).toBeVisible();
  await offer.click();
  await expect(activeRow(page)).toHaveAttribute("data-path", path.join(linked(repo), "fix-typo"));
  await expect(page.getByTestId("active-worktree")).toContainText("fix/typo");
});

test("the panel is shown and hidden with Cmd+B, remembered per repository, and lies over the graph in a narrow window", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  const panel = page.locator('[data-region="side-panel"]');
  await expect(panel).toBeVisible();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(panel).toHaveCount(0);
  await expect(page.getByTestId("side-toggle")).toHaveAttribute("aria-pressed", "false");
  // Remembered for this repository (once the core has it: a reload would drop the request).
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const { api } = await import("/src/lib/ipc.ts");
        const { app } = await import("/src/lib/state/app.svelte.ts");
        return (await api.getRepoPrefs(app.activeRepoId!)).sidePanel;
      }),
    )
    .toBe(false);
  await page.reload();
  await open(page, repo, false);
  await expect(panel).toHaveCount(0);
  await page.getByTestId("side-toggle").click();
  await expect(panel).toBeVisible();
  await page.getByTestId("side-tab-worktrees").click();

  // Resized by its edge, within its limits; the width is kept.
  const edge = panel.getByRole("separator", { name: "Resize the side panel" });
  const box = (await edge.boundingBox())!;
  await page.mouse.move(box.x + 4, box.y + 200);
  await page.mouse.down();
  await page.mouse.move(box.x - 600, box.y + 200, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())!.width).toBe(480);

  // 900 px: still docked and shown, at its least width.
  await page.setViewportSize({ width: 900, height: 560 });
  await expect(panel).toBeVisible();
  await expect(panel).not.toHaveClass(/overlay/);
  await expect.poll(async () => (await panel.boundingBox())!.width).toBe(240);
  await expect(page.getByTestId("worktree-count")).toHaveCount(0);

  // Under 840 px: hidden until asked for, the header saying how many there are; then over the
  // graph (which keeps its width); a click on the graph or Escape hides it.
  await page.setViewportSize({ width: 820, height: 560 });
  await expect(panel).toHaveCount(0);
  await expect(page.getByTestId("worktree-count")).toHaveText("· 8\u00a0worktrees");
  const before = (await grid(page).boundingBox())!.width;
  await page.keyboard.press("ControlOrMeta+b");
  await expect(panel).toBeVisible();
  await expect(panel).toHaveClass(/overlay/);
  expect((await grid(page).boundingBox())!.width).toBeCloseTo(before, 0);
  await grid(page).locator('[data-index="2"] .subject').click();
  await expect(panel).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+b");
  await expect(panel).toBeVisible();
  await list(page).focus();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
});

test("over the graph, the panel starts below the conflict banner, whose buttons stay in reach", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 820, height: 560 });
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo, false);
  // A merge stopped in the main worktree: the banner shows.
  gg.git("worktrees", "checkout", "-q", "-b", "clash");
  writeFileSync(path.join(repo, "README.md"), "ours\n");
  gg.git("worktrees", "commit", "-qam", "Ours");
  gg.git("worktrees", "checkout", "-q", "main");
  writeFileSync(path.join(repo, "README.md"), "theirs\n");
  gg.git("worktrees", "commit", "-qam", "Theirs");
  try {
    gg.git("worktrees", "merge", "clash");
  } catch {
    // Stops on the conflict.
  }
  const abort = page.getByRole("button", { name: "Abort" }).first();
  await expect(abort).toBeVisible();
  await page.keyboard.press("ControlOrMeta+b");
  const panel = page.locator('[data-region="side-panel"]');
  await expect(panel).toBeVisible();
  const top = (await panel.boundingBox())!.y;
  const bannerBox = (await abort.boundingBox())!;
  expect(top).toBeGreaterThanOrEqual(bannerBox.y + bannerBox.height);
  await abort.click({ trial: true });
});

test("the delete dialog says up front what is stopped there, and the create dialog says a refusal about neither field above its buttons", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  // The other worktrees' states are read in the background after the open: once the merge
  // stopped there is known (on a loaded machine that takes a moment).
  await expect(row(page, "hotfix").locator(".badge")).toHaveText("merging");
  await row(page, "hotfix").click({ button: "right" });
  await item(page, "Delete…").click();
  const del = page.getByRole("dialog", { name: "Delete worktree hotfix" });
  await expect(del.getByTestId("delete-in-progress")).toHaveText("A merge is stopped there; deleting the worktree abandons it.");
  await del.getByRole("button", { name: "Cancel" }).click();
  await row(page, "detached").click({ button: "right" });
  await item(page, "Delete…").click();
  await expect(page.getByRole("dialog", { name: "Delete worktree detached" })).toContainText("HEAD there is detached at");
  await page.keyboard.press("Escape");

  // The branch chosen is deleted in a terminal meanwhile: not a problem of the location.
  await grid(page).locator('.ref[data-ref="patch"]').click({ button: "right" });
  await item(page, "Create worktree from patch…").click();
  const dialog = page.getByRole("dialog", { name: "Create worktree" });
  await expect(dialog.getByTestId("worktree-location")).toHaveValue("~/repos/worktrees.worktrees/patch");
  gg.git("worktrees", "branch", "-D", "patch");
  await dialog.getByRole("button", { name: "Create worktree" }).click();
  await expect(dialog.getByTestId("create-error")).toHaveText("Nothing was created. There is no local branch named “patch”.");
  await expect(dialog.locator(".field-hint.problem")).toHaveCount(0);
});

test("the toolbar is never cut off, down to the least window width", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("worktrees"));
  for (const width of [800, 900]) {
    await page.setViewportSize({ width, height: 560 });
    const buttons = page.locator(".repo-head .toolbar").locator("button");
    const count = await buttons.count();
    expect(count).toBeGreaterThan(6);
    for (let i = 0; i < count; i++) {
      const box = (await buttons.nth(i).boundingBox())!;
      expect(box.x, `${width}: control ${i}`).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width, `${width}: control ${i}`).toBeLessThanOrEqual(width);
    }
    // The branch keeps its room.
    const branch = page.getByTestId("active-worktree").locator(".wt-branch");
    expect(await branch.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
  }
});

test("squeezed ref labels keep what does not fit inside their own pill, never over the next one", async ({ gg }) => {
  const { page } = gg;
  // The main row: `main | origin`, `docs` with its folder mark, `+2`, in the narrow
  // description column a 900 px window leaves beside the side panel.
  await page.setViewportSize({ width: 900, height: 560 });
  await page.goto("/");
  await open(page, gg.repoPath("worktrees"));
  const row = grid(page).locator('[role="row"]', { has: page.locator('.ref[data-ref="main"]') });
  const spills = await row.locator(".ref:not([hidden])").evaluateAll((pills) =>
    pills
      .filter((p) => p.scrollWidth > p.clientWidth + 1 && getComputedStyle(p).overflowX === "visible")
      .map((p) => p.textContent?.trim()),
  );
  expect(spills).toEqual([]);
  // And the pills themselves never overlap.
  const boxes = await row.locator(".ref:not([hidden]), .pr-chip:not([hidden])").evaluateAll((els) =>
    els.map((e) => e.getBoundingClientRect()).map((r) => [r.left, r.right]),
  );
  for (let i = 1; i < boxes.length; i++) expect(boxes[i][0]).toBeGreaterThanOrEqual(boxes[i - 1][1] - 0.5);
});

test("at 900 px beside the docked panel the description keeps 280 px: Author gives way first, then Date", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 560 });
  await page.goto("/");
  await open(page, gg.repoPath("worktrees"));
  await expect(list(page)).toBeVisible();
  const header = page.locator("[data-header] [role=columnheader]");
  const width = async (name: string) => (await header.filter({ hasText: name }).boundingBox())!.width;
  await expect.poll(() => width("Description")).toBeGreaterThanOrEqual(279);
  // Author narrowed (from 150), Date as it was or narrowed less; the commit column kept.
  expect(await width("Author")).toBeLessThan(150);
  expect(await width("Commit")).toBe(84);
  // A wide window: every column as set again.
  await page.setViewportSize({ width: 1400, height: 800 });
  await expect.poll(() => width("Author")).toBe(150);
  expect(await width("Date")).toBe(140);
});

test("the keyboard reaches the panel: F6, arrows, Enter and the menu key", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  await grid(page).focus();
  // No notice: F6 goes from the graph to the panel, and on round to the graph.
  await page.keyboard.press("F6");
  await expect(list(page)).toBeFocused();
  await page.keyboard.press("F6");
  await expect(grid(page)).toBeFocused();
  await page.keyboard.press("Shift+F6");
  await expect(list(page)).toBeFocused();

  // Arrows move the selection (and show its HEAD); Enter makes it active.
  await page.keyboard.press("Home");
  await expect(rows(page).first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  await expect(rows(page).nth(1)).toHaveAttribute("aria-selected", "true");
  // The active descendant is the row's gridcell, which names the worktree.
  const cell = rows(page).nth(1).getByRole("gridcell").first();
  await expect(list(page)).toHaveAttribute("aria-activedescendant", (await cell.getAttribute("id")) ?? "(no id)");
  await expect(cell).toHaveAttribute("aria-label", /.+/);
  const target = (await rows(page).nth(1).getAttribute("data-path"))!;
  await page.keyboard.press("Enter");
  await expect(activeRow(page)).toHaveAttribute("data-path", target);

  // The menu key opens the selected row's menu, its first item active.
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ContextMenu");
  await expect(page.getByRole("menu")).toBeVisible();
  await expect(item(page, "Make active")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(list(page)).toBeFocused();

  // With a notice up, F6 from the graph goes to it first, then the panel.
  await page.getByRole("button", { name: "Worktree actions" }).click();
  await item(page, "Prune stale entries…").click();
  await page.getByRole("dialog", { name: "Prune stale worktree entries" }).getByRole("button", { name: "Prune" }).click();
  await expect(notice(page, /old-spike/)).toBeVisible();
  await grid(page).focus();
  await page.keyboard.press("F6");
  await expect(page.locator("[data-notices]").locator(":focus")).toHaveCount(1);
  await page.keyboard.press("F6");
  await expect(list(page)).toBeFocused();
});
