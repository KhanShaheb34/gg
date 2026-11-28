// Screenshots for the visual check, written to .screens/ (git-ignored; the README's shots in
// docs/screens/ are picked from them by hand). Run on demand:
//   GG_SCREENS=1 pnpm e2e screens
import { chmodSync, utimesSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, makeRepo, openFolder, otherClone, runOp, test } from "./fixtures";
import { answer, comment, review, run, threads, view } from "./prDetailsData";

const OUT = path.resolve(import.meta.dirname, "..", ".screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });

/** The sizes every menu and dialog is shot at: the default window and the smallest one. */
const SIZES = [
  ["", 1280, 800],
  ["small-", 900, 560],
] as const;

/** Somewhere nothing reacts to the pointer: the empty middle of the tab bar. */
const park = (page: Page) => page.mouse.move(1000, 20);

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toBeVisible();
}

/** The side panel's Worktrees tab (the panel shows on its Commit tab at first). */
const showWorktrees = (page: Page) => page.getByTestId("side-tab-worktrees").click();

for (const theme of ["light", "dark"] as const) {
  test(`welcome and open tab, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    const parent = path.dirname(gg.repoPath("small"));
    const others = ["payments-api", "design-system"].map((name) =>
      makeRepo(name, (git) => git("init", "-q", "-b", "main"), parent),
    );

    await page.goto("/");
    for (const dir of [...others, gg.repoPath("empty"), gg.repoPath("small")]) await open(page, dir);
    await page.mouse.move(640, 400);
    await page.waitForTimeout(150); // let hover styles settle
    await page.screenshot({ path: path.join(OUT, `tab-${theme}.png`) });

    for (let i = 0; i < 4; i++) await page.keyboard.press("ControlOrMeta+w");
    await expect(page.getByTestId("welcome")).toBeVisible();
    await page.mouse.move(640, 790);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `welcome-${theme}.png`) });
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`graph, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("small"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await grid.locator('[data-index="3"]').click();
    await page.mouse.move(900, 700);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `graph-${theme}.png`) });

    await open(page, gg.repoPath("branchy"));
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await showWorktrees(page);
    await park(page);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `graph-branchy-${theme}.png`) });
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`toolbar, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("branchy"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await page.mouse.move(700, 60);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `toolbar-${theme}.png`) });

    // View open, tags hidden: the button in the accent colour.
    await page.getByTestId("view-options").click();
    const view = page.getByRole("dialog", { name: "View", exact: true });
    await view.getByRole("checkbox", { name: "Tags", exact: true }).click();
    await page.mouse.move(1000, 60);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `toolbar-view-${theme}.png`) });
    await view.getByRole("checkbox", { name: "Tags", exact: true }).click();
    await page.keyboard.press("Escape");

    // The branch filter open, filtered to "feature", one branch ticked.
    await page.getByRole("button", { name: /^Branches:/ }).click();
    const dialog = page.getByRole("dialog", { name: "Branch filter" });
    await dialog.getByRole("option", { name: "feature/ui", exact: true }).click();
    await expect(grid.locator('[data-index="0"] .subject')).toHaveText("Row hover states");
    await page.keyboard.type("feat");
    await page.mouse.move(1000, 60); // off the list and the grid
    await page.waitForTimeout(250);
    await page.screenshot({ path: path.join(OUT, `toolbar-filter-${theme}.png`) });
    await dialog.getByRole("option", { name: "Show all", exact: true }).click();
    await page.keyboard.press("Escape");

    // Search active, a few hits, the second one selected.
    await page.keyboard.press("ControlOrMeta+f");
    await page.keyboard.type("plugin");
    await expect(page.getByTestId("search-count")).toHaveText(/^1 of \d+$/);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("search-count")).toHaveText(/^2 of \d+$/);
    await page.mouse.move(700, 60);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `toolbar-search-${theme}.png`) });
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`details panel, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("details"));
    const grid = page.getByRole("grid", { name: "Commits" });
    const panel = page.getByRole("region", { name: "Commit details" });
    const shot = async (name: string) => {
      await expect(panel.getByTestId("details-files-count")).toBeVisible();
      await page.mouse.move(1000, 60); // empty toolbar space, off every button
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
    };

    // A normal commit: added, modified, renamed and binary files, a message with a body.
    await grid.locator('[role="row"]', { hasText: "Rework peaches routes" }).click();
    await shot("details");

    // A merge commit: two parents, files against the first.
    await grid.locator('[role="row"]', { hasText: "Merge branch 'side'" }).click();
    await shot("details-merge");

    // A long message and many files, as a flat list.
    await grid.locator('[role="row"]', { hasText: "Generate the module scaffolding" }).click();
    await shot("details-long");
    await panel.getByRole("button", { name: "List" }).click();
    await shot("details-list");
    await panel.getByRole("button", { name: "Tree" }).click();
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`first run, empty, loading and error states, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    // For the loading shot: "small" stays loading with no rows (no events reach the page,
    // and its snapshots say so); the empty repository needs neither.
    const small = gg.repoPath("small");
    await page.route("**/events", (route) => route.abort());
    await page.route("**/invoke", async (route) => {
      const body = route.request().postDataJSON() as { cmd?: string } | null;
      if (body?.cmd !== "open_repo" && body?.cmd !== "get_snapshot") return route.continue();
      const response = await route.fetch();
      const json = (await response.json()) as { ok?: { rootPath: string; totalRows: number; loading: boolean } };
      if (json.ok?.rootPath === small) Object.assign(json.ok, { totalRows: 0, loading: true });
      await route.fulfill({ response, json });
    });
    await page.goto("/");
    await expect(page.getByTestId("welcome")).toBeVisible();
    await page.mouse.move(640, 790);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `welcome-first-run-${theme}.png`) });

    // A folder that is not a repository: the error toast, over the welcome screen.
    page.once("dialog", (dialog) => dialog.accept(gg.folder("plain")));
    await openFolder(page);
    await expect(page.getByRole("alert")).toBeVisible();
    await park(page);
    await page.waitForTimeout(250); // the toast's entrance
    await page.screenshot({ path: path.join(OUT, `toast-${theme}.png`) });
    await page.getByRole("button", { name: "Dismiss notification" }).click();

    // A repository without commits.
    await open(page, gg.repoPath("empty"));
    await expect(page.getByText("No commits yet")).toBeVisible();
    await page.mouse.move(640, 790);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `empty-${theme}.png`) });

    // Loading: the first rows held back (see the routes above), so the skeleton rows show.
    await open(page, small);
    await expect(page.locator('[role="row"][aria-busy="true"]:not([hidden])').first()).toBeVisible();
    await page.mouse.move(640, 790);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `loading-${theme}.png`) });
  });
}

// The smallest window (900×560) and keyboard focus / hover, both themes.
for (const theme of ["light", "dark"] as const) {
  test(`small window, focus and hover, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width: 900, height: 560 });
    await page.emulateMedia({ colorScheme: theme });
    const parent = path.dirname(gg.repoPath("small"));
    makeRepo("payments-api", (git) => git("init", "-q", "-b", "main"), parent);
    await page.goto("/");
    await open(page, gg.repoPath("small"));
    await page.keyboard.press("ControlOrMeta+w");
    await expect(page.getByTestId("welcome")).toBeVisible();
    await park(page);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `small-welcome-${theme}.png`) });

    await open(page, gg.repoPath("branchy"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await park(page);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `small-graph-${theme}.png`) });

    await page.getByRole("button", { name: /^Branches:/ }).click();
    await page.keyboard.type("feat");
    await park(page);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `small-filter-${theme}.png`) });
    await page.keyboard.press("Escape");

    await page.keyboard.press("ControlOrMeta+f");
    await page.keyboard.type("plugin");
    await expect(page.getByTestId("search-count")).toHaveText(/^1 of \d+$/);
    await park(page);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `small-search-${theme}.png`) });
    await page.keyboard.press("Escape");

    await grid.locator('[role="row"]', { hasText: "Sandbox file access" }).click();
    await expect(page.getByRole("region", { name: "Commit details" }).getByTestId("details-files-count")).toBeVisible();
    await park(page);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `small-details-${theme}.png`) });

    // Keyboard focus: the grid's selected row, then a toolbar button.
    await grid.focus();
    await page.keyboard.press("ArrowDown");
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `focus-row-${theme}.png`) });
    await page.getByRole("button", { name: "Refresh" }).focus();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Tab");
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `focus-button-${theme}.png`) });

    // Hover: a row and a tab.
    // A row that is not the selected one, so the hover tint is what shows.
    const hovered = grid.locator('[role="row"][aria-selected="false"]', { hasText: "Resizable columns" });
    await hovered.hover();
    await expect(hovered).toHaveAttribute("aria-selected", "false");
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `hover-row-${theme}.png`) });
    await page.getByRole("tab", { name: "branchy" }).hover();
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `hover-tab-${theme}.png`) });
  });
}


// Menus, the confirmation dialog, the busy indicator and the notices of operations.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`menus, dialogs and operations, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    const dir = gg.repoPath("branchy");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
    };
    // The default window: the side panel on its Worktrees tab, as in the graph's shot.
    if (width >= 1000) await showWorktrees(page);

    // A commit's menu with the reset submenu open, the hard reset under the pointer. Opened at
    // the start of the subject, so the menu covers the details' author line (an email address).
    await grid.locator('[role="row"]', { hasText: "Plugin sandboxing" }).locator(".subject").click({ button: "right", position: { x: 2, y: 8 } });
    await page.getByRole("menuitem", { name: /^Reset main to here$/ }).hover();
    await page.getByRole("menuitem", { name: "Hard: discard all changes" }).hover();
    await shot("menu-commit");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");

    // The current branch: a disabled item with its reason.
    await grid.locator('[data-ref="main"]').first().click({ button: "right" });
    await page.getByRole("menuitem", { name: "Merge into the current branch…" }).hover();
    await shot("menu-branch-current");
    await page.keyboard.press("Escape");

    // Another branch, from the keyboard (the active item is the focus).
    await grid.locator('[data-ref="feature/ui"]').first().click({ button: "right" });
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await park(page);
    await shot("menu-branch");
    await page.keyboard.press("Escape");

    // A remote branch's own label.
    await grid.locator('[data-ref="origin/main"]').first().click({ button: "right" });
    await shot("menu-remote");
    await page.keyboard.press("Escape");

    // The confirmation the core asks for: a hard reset with uncommitted changes.
    writeFileSync(path.join(dir, "files/f1.txt"), "changed\n");
    writeFileSync(path.join(dir, "files/f2.txt"), "changed\n");
    void runOp(page, { op: "reset", hash: gg.git("branchy", "rev-parse", "HEAD~2").trim(), mode: "hard" });
    await expect(page.getByRole("alertdialog")).toBeVisible();
    await park(page);
    await shot("dialog-confirm");
    await page.keyboard.press("Escape");

    // A failed checkout (a changed file it would overwrite): what is in the way, and the log.
    gg.git("branchy", "checkout", "--", ".");
    const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
    writeFileSync(path.join(dir, differing), "in the way\n");
    const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
    expect(failed.status).toBe("failed");
    const alert = page.getByRole("alert").filter({ hasText: failed.message });
    await expect(alert.getByRole("button", { name: "Show log" })).toBeVisible();
    await park(page);
    await shot("notice-failed");
    await alert.getByRole("button", { name: "Dismiss notification" }).click();

    // A merge whose notice offers to push, and the busy indicator with one queued.
    gg.git("branchy", "checkout", "--", ".");
    gg.git("branchy", "clean", "-fdq");
    // The graph has caught up with the clean-up before the operations start (none refresh
    // the view beside an operation).
    await expect(grid.locator('[role="row"]', { hasText: "Uncommitted changes" })).toHaveCount(0);
    const hook = path.join(dir, ".git/hooks/post-checkout");
    writeFileSync(hook, "#!/bin/sh\nsleep 2\n");
    chmodSync(hook, 0o755);
    const first = runOp(page, { op: "checkout_branch", branch: "feature/ui", fastForward: false });
    const second = runOp(page, { op: "checkout_branch", branch: "main", fastForward: false });
    await expect(page.getByTestId("busy")).toContainText("+1");
    await park(page);
    await shot("busy");
    await Promise.all([first, second]);
    // The merge commit is made by the core's git: give it the fixtures' author.
    gg.git("branchy", "config", "user.name", "Ada Lovelace");
    gg.git("branchy", "config", "user.email", "ada@example.com");
    await runOp(page, {
      op: "merge",
      target: { kind: "branch", name: "feature/ui" },
      noFf: true,
      squash: false,
      noCommit: false,
    });
    await page.mouse.move(1000, 760);
    await page.getByRole("status").filter({ hasText: /feature\/ui into main/ }).hover();
    await shot("notice-done");
  });
}

// The dialogs of commit and tag actions.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`commit and tag dialogs, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("branchy"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const dialog = page.locator('[role="dialog"], [role="alertdialog"]');
    const shot = async (name: string) => {
      await park(page);
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);
    };
    const pick = async (subject: string, name: string, parent?: RegExp) => {
      // Scrolled to where the row is (the details panel takes the bottom of the window).
      const row = grid.locator('[role="row"]', { hasText: subject }).first();
      for (let top = 0; top < 60 * 26 && !(await row.isVisible()); top += 150) {
        await grid.evaluate((el, t) => (el.scrollTop = t), top);
        await page.waitForTimeout(60);
      }
      await row.locator(".subject").click({ button: "right" });
      if (parent) await page.getByRole("menuitem", { name: parent }).hover();
      await page.getByRole("menuitem", { name, exact: true }).click();
      await expect(dialog).toBeVisible();
    };

    await pick("Plugin sandboxing", "Create branch here…");
    await page.keyboard.type("feature/sandbox");
    await shot("dialog-create-branch");

    await pick("Plugin sandboxing", "Create branch here…");
    await page.keyboard.type("feature/sand box");
    await shot("dialog-create-branch-invalid");

    // A name the core refuses: the tag exists.
    await pick("Plugin sandboxing", "Create tag here…");
    await page.keyboard.type("v1.0");
    await page.keyboard.press("Enter");
    await expect(dialog.getByText("already exists")).toBeVisible();
    await shot("dialog-create-tag-taken");

    await pick("Plugin sandboxing", "Create tag here…");
    await page.keyboard.type("v1.1-beta");
    await dialog.getByRole("textbox", { name: "Message" }).fill("First beta of 1.1\n\nPlugin sandboxing lands.");
    await shot("dialog-create-tag");

    await pick("Merge branch 'release/1.0'", "Cherry-pick…");
    await expect(dialog.locator(".choice").nth(1)).toContainText("Bump version to 1.0.0");
    await shot("dialog-cherry-pick-merge");

    await pick("Theme tokens", "Cherry-pick…");
    await shot("dialog-cherry-pick");

    await pick("Fix flaky test", "Revert…");
    await shot("dialog-revert");

    await pick("Update licence year", "Hard: discard all changes", /^Reset main to here$/);
    await shot("dialog-reset-hard");

    await pick("Update licence year", "Mixed: keep changes, unstaged", /^Reset main to here$/);
    await shot("dialog-reset");

    await grid.evaluate((el) => (el.scrollTop = 0));
    await grid.locator('[data-ref="v1.0"]').first().click({ button: "right" });
    await page.getByRole("menuitem", { name: "Delete…" }).click();
    await shot("dialog-delete-tag");

    await grid.locator('[data-ref="v1.0"]').first().click({ button: "right" });
    await page.getByRole("menuitem", { name: "Push…" }).click();
    await shot("dialog-push-tag");
  });
}

// The dialogs of branch actions and the staging routine's notices.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`branch dialogs and the staging routine, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    const dir = gg.repoPath("routine");
    const other = otherClone(dir);
    // A branch only the remote has, for the remote branch dialogs.
    gg.gitAt(other, "switch", "-q", "-c", "release/2.0", "origin/main");
    gg.gitAt(other, "commit", "-q", "--allow-empty", "-m", "Start release 2.0");
    gg.gitAt(other, "push", "-q", "origin", "release/2.0");
    gg.git("routine", "fetch", "-q", "origin");
    await page.goto("/");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const dialog = page.locator('[role="dialog"], [role="alertdialog"]');
    const label = (name: string, part = ".name") => grid.locator(`[data-ref="${name}"]`).first().locator(part);
    const pick = async (name: string, item: string, part = ".name") => {
      await label(name, part).click({ button: "right" });
      await page.getByRole("menuitem", { name: item, exact: true }).click();
      await expect(dialog).toBeVisible();
    };
    const snap = async (name: string) => {
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
    };
    const shot = async (name: string) => {
      await park(page);
      await snap(name);
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);
    };

    await pick("staging", "Check out");
    await shot("dialog-fast-forward");
    await pick("feature/login", "Merge feature/login into main…");
    await shot("dialog-merge");
    await pick("feature/login", "Rebase main onto feature/login…");
    await shot("dialog-rebase");
    await pick("feature/login", "Rename…");
    await shot("dialog-rename-branch");
    await pick("feature/login", "Delete…");
    await shot("dialog-delete-branch");
    await pick("feature/login", "Push…");
    await shot("dialog-push");
    await pick("feature/login", "Push…");
    await dialog.getByRole("checkbox", { name: /Force push/ }).check();
    await shot("dialog-push-force");
    await pick("feature/login", "Check out as a local branch…", ".remotes");
    await shot("dialog-checkout-remote-existing");
    await pick("origin/release/2.0", "Check out as a local branch…");
    await shot("dialog-checkout-remote");
    await pick("origin/release/2.0", "Fetch into a local branch…");
    await shot("dialog-fetch-into-local");

    // The routine: staging fast-forwarded, feature/login merged: the notice offers to push.
    await pick("staging", "Check out");
    await dialog.getByRole("button", { name: "Yes", exact: true }).click();
    await expect(grid.locator('[data-ref="staging"]').first()).toHaveClass(/current/);
    await pick("feature/login", "Merge feature/login into staging…");
    await dialog.getByRole("button", { name: "Merge", exact: true }).click();
    const merged = page.getByRole("status").filter({ hasText: "Merged feature/login into staging" });
    await merged.hover();
    await snap("notice-merge-push");
    await merged.getByRole("button", { name: "Dismiss notification" }).click();

    // Meanwhile the remote moved: the push is rejected, with what to do next.
    gg.gitAt(other, "switch", "-q", "staging");
    gg.gitAt(other, "pull", "-q", "--ff-only");
    gg.gitAt(other, "commit", "-q", "--allow-empty", "-m", "Hotfix three");
    gg.gitAt(other, "push", "-q", "origin", "staging");
    await pick("staging", "Push 3 commits…");
    await dialog.getByRole("button", { name: "Push", exact: true }).click();
    const rejected = page.getByRole("alert").filter({ hasText: "Push rejected: the remote has new commits" });
    await expect(rejected).toBeVisible();
    await rejected.hover();
    await snap("notice-push-rejected");

    // Pull: diverged, and git is not told how: the choice.
    await rejected.getByRole("button", { name: "Pull", exact: true }).click();
    await expect(dialog).toBeVisible();
    await shot("dialog-pull");
  });
}

// A menu taller than the smallest window: it scrolls inside, the last item active.
for (const theme of ["light", "dark"] as const) {
  test(`tall menu in a small window, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width: 900, height: 560 });
    await page.emulateMedia({ colorScheme: theme });
    for (let i = 1; i <= 40; i++) gg.git("small", "tag", `release-${String(i).padStart(2, "0")}`, "HEAD");
    await page.goto("/");
    await open(page, gg.repoPath("small"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await grid.locator('[role="row"]', { hasText: "Add part 19" }).locator(".ref.more").click({ button: "right" });
    await page.keyboard.press("End");
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `menu-tall-${theme}.png`) });
  });
}

// The conflict banner in each of its states, and the dialogs of continue and of
// stashes.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`conflict banner and stash dialogs, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    const dir = gg.repoPath("conflicts");
    const git = (...args: string[]) => {
      try {
        gg.git("conflicts", ...args);
      } catch {
        // A merge or rebase stopped on conflicts.
      }
    };
    const banner = page.getByTestId("conflict-banner");
    const dialog = page.locator('[role="dialog"], [role="alertdialog"]');
    const snap = async (name: string) => {
      await park(page);
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
    };
    const resolve = (file: string) => writeFileSync(path.join(dir, file), "resolved\n");
    await page.goto("/");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();

    // A merge with two conflicted files, one of them resolved.
    git("merge", "feature/x");
    await expect(banner).toContainText("2 files have conflicts");
    await snap("banner-merge");
    resolve("app.txt");
    await banner.hover({ position: { x: 30, y: 10 } });
    await expect(banner).toContainText("1 of 2 files resolved");
    await snap("banner-merge-partly");
    await banner.getByRole("button", { name: "Continue anyway…" }).click();
    await expect(dialog).toBeVisible();
    await snap("dialog-continue-anyway");
    await page.keyboard.press("Escape");
    // Resolved: ready, and Continue's question.
    resolve("style.css");
    await page.waitForTimeout(1100);
    await page.mouse.move(0, 300);
    await banner.hover({ position: { x: 30, y: 10 } });
    await expect(banner).toContainText("All conflicts are resolved");
    await snap("banner-merge-resolved");
    await banner.getByRole("button", { name: "Continue…" }).click();
    await expect(dialog).toBeVisible();
    await snap("dialog-continue");
    await page.keyboard.press("Escape");
    await banner.getByRole("button", { name: "Abort…" }).click();
    await expect(dialog).toBeVisible();
    await snap("dialog-abort");
    await page.keyboard.press("Escape");
    gg.git("conflicts", "merge", "--abort");
    await expect(banner).toHaveCount(0);

    // Many files: the list scrolls.
    git("merge", "wide");
    await expect(banner).toContainText("12 files have conflicts");
    await snap("banner-many");
    gg.git("conflicts", "merge", "--abort");
    await expect(banner).toHaveCount(0);

    // Nothing conflicted: a merge stopped before its commit.
    git("merge", "--no-commit", "--no-ff", "topic~4");
    await expect(banner).toContainText("ready to continue");
    await snap("banner-ready");
    gg.git("conflicts", "merge", "--abort");
    await expect(banner).toHaveCount(0);

    // A rebase stopped at step 2 of 5.
    gg.git("conflicts", "switch", "-q", "topic");
    git("rebase", "main");
    await expect(banner).toContainText("step 2 of 5");
    await snap("banner-rebase");
    gg.git("conflicts", "rebase", "--abort");
    gg.git("conflicts", "switch", "-q", "main");
    await expect(banner).toHaveCount(0);

    // Conflicts a stash left: the quieter banner.
    writeFileSync(path.join(dir, "app.txt"), "stashed change\n");
    gg.git("conflicts", "stash", "push", "-q", "--message=Try a new layout");
    writeFileSync(path.join(dir, "app.txt"), "committed change\n");
    gg.git("conflicts", "commit", "-qam", "Change the app");
    git("stash", "pop");
    await expect(banner).toContainText("1 file has conflicts");
    await snap("banner-stash");
    gg.git("conflicts", "checkout", "-q", "HEAD", "--", "app.txt");
    await expect(banner).toHaveCount(0);

    // Stash dialogs: save (from the uncommitted row), create a branch from a stash.
    writeFileSync(path.join(dir, "app.txt"), "work in progress\n");
    writeFileSync(path.join(dir, "notes.txt"), "untracked notes\n");
    await page.getByRole("button", { name: "Refresh" }).click();
    await grid.evaluate((el) => (el.scrollTop = 0));
    const uncommitted = grid.locator('[data-index="0"] .subject');
    await expect(uncommitted).toHaveText(/^Uncommitted changes/);
    await uncommitted.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Stash changes…" }).click();
    await dialog.getByLabel("Message (optional)").fill("Try a new layout");
    await snap("dialog-stash-save");
    await page.keyboard.press("Escape");
    await grid.locator('[data-ref="stash@{0}"]').first().click({ button: "right" });
    await page.getByRole("menuitem", { name: "Create branch from stash…" }).click();
    await page.keyboard.type("layout-trial");
    await snap("dialog-stash-branch");
    await page.keyboard.press("Escape");
  });
}

// The command log: closed, open, a failed operation reached from its notice, the
// background reads, at 1280×800 and 900×560.
for (const theme of ["light", "dark"] as const) {
  for (const size of [
    { name: "", width: 1280, height: 800 },
    { name: "small-", width: 900, height: 560 },
  ]) {
    test(`command log, ${size.width}x${size.height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width: size.width, height: size.height });
      await page.emulateMedia({ colorScheme: theme });
      const dir = gg.repoPath("branchy");
      gg.git("branchy", "config", "user.name", "Ada Lovelace");
      gg.git("branchy", "config", "user.email", "ada@example.com");
      await page.goto("/");
      await open(page, dir);
      const grid = page.getByRole("grid", { name: "Commits" });
      await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
      const shot = async (name: string) => {
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${size.name}${name}-${theme}.png`) });
      };
      const panel = page.getByRole("region", { name: "Command log" });

      await runOp(page, { op: "merge", target: { kind: "branch", name: "feature/ui" }, noFf: true, squash: false, noCommit: false });
      await runOp(page, { op: "create_branch", name: "release/1.1", at: gg.git("branchy", "rev-parse", "HEAD").trim(), checkout: false });
      const dismiss = page.locator("[data-notices]").getByRole("button", { name: "Dismiss notification" });
      while ((await dismiss.count()) > 0) await dismiss.first().click();
      await grid.locator('[role="row"]', { hasText: "Plugin sandboxing" }).click();
      await park(page);
      await shot("log-closed");

      await page.getByRole("button", { name: "Command log", exact: true }).click();
      await expect(panel.locator(".op")).toHaveCount(2);
      const merge = panel.locator(".cmd", { hasText: "git merge" });
      await merge.getByRole("button", { name: /git merge/ }).click();
      await park(page);
      await shot("log-open");

      // The time column in the Relative and System date formats.
      for (const [name, label] of [["relative", "Relative"], ["locale", "System"], ["standard", "Standard"]]) {
        await page.getByTestId("open-settings").click();
        await page.getByTestId("settings-dates").getByRole("radio", { name: label }).click();
        await page.keyboard.press("Escape");
        if (name === "standard") break;
        await park(page);
        await shot(`log-time-${name}`);
      }

      // A failed checkout: its notice's Show log goes to it, highlighted, its output open.
      const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
      writeFileSync(path.join(dir, differing), "in the way\n");
      const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
      await page.getByRole("alert").filter({ hasText: failed.message }).getByRole("button", { name: "Show log" }).click();
      await expect(panel.locator(".op.target")).toBeVisible();
      await park(page);
      await shot("log-failed");

      await panel.getByRole("checkbox", { name: "Background reads" }).check();
      await expect(panel.locator(".cmd.read").first()).toBeVisible();
      while ((await dismiss.count()) > 0) await dismiss.first().click();
      await park(page);
      await shot("log-reads");
      await panel.getByRole("checkbox", { name: "Background reads" }).uncheck();
      await page.getByRole("button", { name: "Command log", exact: true }).click();
    });
  }
}

// The smallest window, the many-files banner, the details panel as tall as
// it goes: the graph keeps its rows.
for (const theme of ["light", "dark"] as const) {
  test(`small window, banner and tall details, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width: 900, height: 560 });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("conflicts"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    try {
      gg.git("conflicts", "merge", "wide");
    } catch {
      // Stopped on conflicts.
    }
    await expect(page.getByTestId("conflict-banner")).toContainText("12 files have conflicts");
    await grid.locator('[role="row"]', { hasText: "Earlier work 39" }).click();
    const details = page.getByTestId("commit-details");
    const splitter = details.getByRole("separator", { name: "Resize details panel" });
    await expect(details).toBeVisible();
    await page.waitForTimeout(400); // the panel has taken its place: grab its edge, not the text
    const box = (await splitter.boundingBox())!;
    await page.mouse.move(box.x + 100, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 100, 0, { steps: 4 });
    await page.mouse.up();
    await park(page);
    await page.waitForTimeout(250);
    await page.screenshot({ path: path.join(OUT, `small-banner-details-${theme}.png`) });
    // The details panel's height goes back to its default for the next test run.
    await page.evaluate(() => localStorage.removeItem("gg.details.v1"));
  });
}

// The menus and confirmations not shot above, the remaining notice kinds, and the menus,
// dialogs, notices and busy state at 900×560.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`remaining menus, confirmations and notices, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto("/");
    const dir = gg.repoPath("branchy");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
    };
    const confirmation = page.getByRole("alertdialog");

    // A tag's menu.
    await grid.locator('[data-ref="v1.0"]').first().click({ button: "right" });
    await park(page);
    await shot("menu-tag");
    await page.keyboard.press("Escape");

    // The uncommitted row's menu, and a stash's.
    writeFileSync(path.join(dir, "files/f1.txt"), "changed\n");
    gg.git("branchy", "stash", "push", "-q", "-m", "Keep the draft");
    writeFileSync(path.join(dir, "files/f2.txt"), "changed\n");
    const uncommitted = grid.locator('[role="row"]', { hasText: "Uncommitted changes" });
    await expect(uncommitted).toHaveCount(1, { timeout: 5000 }).catch(async () => {
      await page.keyboard.press("ControlOrMeta+R");
      await expect(uncommitted).toHaveCount(1);
    });
    await uncommitted.locator(".subject").click({ button: "right" });
    await park(page);
    await shot("menu-uncommitted");
    await page.keyboard.press("Escape");
    const stash = grid.locator('[role="row"]', { hasText: "Keep the draft" });
    await stash.locator(".subject").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Drop…" }).hover();
    await shot("menu-stash");
    await page.keyboard.press("Escape");

    // The core's red confirmations: dropping a stash, deleting a branch not merged, deleting
    // a branch on the remote, force pushing.
    const stashHash = gg.git("branchy", "rev-parse", "stash@{0}").trim();
    void runOp(page, { op: "stash_drop", stash: stashHash });
    await expect(confirmation).toBeVisible();
    await park(page);
    await shot("dialog-drop-stash");
    await page.keyboard.press("Escape");
    await expect(confirmation).toHaveCount(0);

    void runOp(page, { op: "delete_branch", branch: "feature/ui" }); // 2 commits its upstream lacks
    await expect(confirmation).toBeVisible();
    await park(page);
    await shot("dialog-delete-unmerged");
    await page.keyboard.press("Escape");
    await expect(confirmation).toHaveCount(0);

    void runOp(page, { op: "delete_remote_branch", remote: "origin", branch: "hotfix/crash" });
    await expect(confirmation).toBeVisible();
    await park(page);
    await shot("dialog-delete-remote");
    await page.keyboard.press("Escape");
    await expect(confirmation).toHaveCount(0);

    // Force push: the remote has a commit the local branch has not.
    const other = `${dir}-elsewhere`;
    const originUrl = path.resolve(dir, gg.git("branchy", "remote", "get-url", "origin").trim());
    gg.gitAt(path.dirname(dir), "clone", "-q", originUrl, other);
    gg.gitAt(other, "-c", "user.name=Grace Hopper", "-c", "user.email=grace@example.com", "commit", "-q", "--allow-empty", "-m", "Pushed from elsewhere");
    gg.gitAt(other, "push", "-q", "origin", "main");
    gg.git("branchy", "fetch", "-q", "origin");
    void runOp(page, { op: "push", branch: "main", remote: "origin", to: null, force: true, });
    await expect(confirmation).toBeVisible();
    await park(page);
    await shot("dialog-force-push");
    await page.keyboard.press("Escape");
    await expect(confirmation).toHaveCount(0);

    // An information notice (a hash copied).
    await grid.locator('[data-index="3"] .hash').click();
    await expect(page.getByRole("status").filter({ hasText: "Copied" })).toBeVisible();
    await park(page);
    await shot("notice-info");

    // At 900×560: a commit's menu, a dialog, a failed operation's notice, the busy state
    // (taken once, in the run at the large size).
    if (prefix !== "") return;
    await page.setViewportSize({ width: 900, height: 560 });
    await page.keyboard.press("Escape");
    await grid.locator('[data-index="5"] .subject').click({ button: "right" });
    await page.getByRole("menuitem", { name: /^Reset main to here$/ }).hover();
    await shot("small-menu-commit");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await grid.locator('[data-ref="feature/ui"]').first().click({ button: "right" });
    await page.getByRole("menuitem", { name: /^Push/ }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await park(page);
    await shot("small-dialog-push");
    await page.keyboard.press("Escape");

    const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
    writeFileSync(path.join(dir, differing), "in the way\n");
    const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
    expect(failed.status).toBe("failed");
    await park(page);
    await shot("small-notice-failed");
    await page.getByRole("alert").filter({ hasText: failed.message }).getByRole("button", { name: "Dismiss notification" }).click();
    gg.git("branchy", "checkout", "--", ".");
    gg.git("branchy", "clean", "-fdq");
    const hook = path.join(dir, ".git/hooks/post-checkout");
    writeFileSync(hook, "#!/bin/sh\nsleep 2\n");
    chmodSync(hook, 0o755);
    const first = runOp(page, { op: "checkout_branch", branch: "feature/ui", fastForward: false });
    const second = runOp(page, { op: "checkout_branch", branch: "main", fastForward: false });
    await expect(page.getByTestId("busy")).toContainText("+1");
    await park(page);
    await shot("small-busy");
    await Promise.all([first, second]);
  });

  // A continue that comes out empty: the notice that offers to skip.
  test(`skip notice, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    const dir = gg.repoPath("conflicts");
    await open(page, dir);
    const hash = gg.git("conflicts", "rev-parse", "feature/x").trim();
    expect((await runOp(page, { op: "cherry_pick", hash, mainline: null, noCommit: false })).status).toBe("stopped");
    writeFileSync(path.join(dir, "app.txt"), "app from main\n");
    writeFileSync(path.join(dir, "style.css"), "body { color: navy; }\n");
    const failed = await runOp(page, { op: "continue", evenWithMarkers: false });
    expect(failed.status).toBe("failed");
    await park(page);
    await page.waitForTimeout(200);
    // The banner offers Skip too.
    await expect(page.getByTestId("conflict-banner").getByRole("button", { name: "Skip this commit" })).toBeVisible();
    await page.screenshot({ path: path.join(OUT, `${prefix}notice-skip-${theme}.png`) });
  });
}

// The side panel's worktree list and the worktree dialogs. In the small window (900 px) the
// panel is docked at its least width, shown by default.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
    test(`worktrees, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto("/");
      const repo = gg.repoPath("worktrees");
      await open(page, repo);
      const grid = page.getByRole("grid", { name: "Commits" });
      await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
      await showWorktrees(page);
      const list = page.getByRole("grid", { name: "Worktrees" });
      const row = (folder: string) => list.locator(`[role="row"][data-path$="/${folder}"]`);
      const shot = async (name: string) => {
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
      };
      await expect(row("feature-login").locator(".changes")).toHaveText("2");
      await expect(row("hotfix").locator(".changes")).toBeVisible();

      // The list as it opens: the main worktree active, the others in every state.
      await park(page);
      await shot("worktrees");

      // One selected (its HEAD selected in the graph), another under the pointer.
      await row("feature-login").click();
      await row("fix-typo").hover();
      await shot("worktrees-selected");

      // A worktree's menu.
      await row("release").click({ button: "right" });
      await page.getByRole("menuitem", { name: /^Locked/ }).hover();
      await shot("menu-worktree");
      await page.keyboard.press("Escape");

      // Create: a new branch, its folder suggested.
      await page.getByRole("button", { name: "Add worktree" }).click();
      const create = page.getByRole("dialog", { name: "Create worktree" });
      await create.getByLabel("New branch name").fill("feature/payments");
      await expect(create.getByTestId("worktree-location")).toHaveValue("~/repos/worktrees.worktrees/feature-payments");
      await park(page);
      await shot("dialog-create-worktree");
      // A folder another worktree has.
      await create.getByTestId("worktree-location").fill("~/repos/worktrees.worktrees/docs/site");
      await shot("dialog-create-worktree-invalid");
      await create.getByRole("button", { name: "Cancel" }).click();

      // From a branch checked out elsewhere.
      await grid.locator('.ref[data-ref="fix/typo"]').click({ button: "right" });
      await page.getByRole("menuitem", { name: "Create worktree from fix/typo…" }).click();
      await park(page);
      await shot("dialog-create-worktree-taken");
      await page.getByRole("dialog", { name: "Create worktree" }).getByRole("button", { name: "Cancel" }).click();

      // Delete, plain, then the core's red confirmation listing the work.
      await row("feature-login").click({ button: "right" });
      await page.getByRole("menuitem", { name: "Delete…" }).click();
      const del = page.getByRole("dialog", { name: "Delete worktree feature-login" });
      await del.getByRole("checkbox", { name: /Also delete branch/ }).check();
      await park(page);
      await shot("dialog-delete-worktree");
      await del.getByRole("checkbox", { name: /Also delete branch/ }).uncheck();
      await del.getByRole("button", { name: "Delete worktree" }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await shot("dialog-delete-worktree-red");
      await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();

      // Ignored files alone: listed in the same dialog, in the ordinary style.
      await row("docs").click({ button: "right" });
      await page.getByRole("menuitem", { name: "Delete…" }).click();
      const docs = page.getByRole("dialog", { name: "Delete worktree docs" });
      await docs.getByRole("button", { name: "Delete worktree" }).click();
      await expect(docs.getByTestId("dangers")).toBeVisible();
      await park(page);
      await shot("dialog-delete-worktree-ignored");
      await docs.getByRole("button", { name: "Cancel" }).click();

      // Prune: the ordinary confirmation naming the entry.
      await page.getByRole("button", { name: "Worktree actions" }).click();
      await page.getByRole("menuitem", { name: "Prune stale entries…" }).click();
      await expect(page.getByRole("dialog", { name: "Prune stale worktree entries" })).toBeVisible();
      await park(page);
      await shot("dialog-prune-worktrees");
      await page.keyboard.press("Escape");

      // Another worktree made active: the header and the graph follow.
      await row("feature-login").dblclick();
      await expect(list.locator('[data-active="true"]')).toHaveAttribute("data-path", /feature-login$/);
      await expect(grid.locator('[role="row"].uncommitted')).toHaveCount(1);
      await park(page);
      await shot("worktrees-switched");
    });
  }

for (const theme of ["light", "dark"] as const) {
  test(`worktrees, one and many, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    const parent = path.dirname(gg.repoPath("small"));
    // One worktree: the panel says what worktrees are for.
    const solo = makeRepo(
      "solo",
      (git, write) => {
        git("init", "-q", "-b", "main");
        write("README.md", "solo\n");
        git("add", "-A");
        git("commit", "-q", "-m", "Start");
      },
      parent,
    );
    await open(page, solo);
    await showWorktrees(page);
    await expect(page.getByTestId("worktrees-intro")).toBeVisible();
    await park(page);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `worktrees-one-${theme}.png`) });

    // Twenty, some with long branch names, in the narrowest panel.
    const many = makeRepo(
      "service-with-a-long-repository-name",
      (git, write, dir) => {
        git("init", "-q", "-b", "main");
        write("README.md", "many\n");
        git("add", "-A");
        git("commit", "-q", "-m", "Start");
        for (let i = 1; i <= 20; i++) {
          const name = i % 4 === 0 ? `feature/a-rather-long-branch-name-for-change-${i}` : `topic-${i}`;
          git("worktree", "add", "-q", "-b", name, path.join(path.dirname(dir), "many.worktrees", name.replace(/\//g, "-")));
        }
      },
      parent,
    );
    await page.evaluate(() => localStorage.setItem("gg.side.v1", JSON.stringify({ width: 240 })));
    await page.reload();
    await open(page, many);
    await showWorktrees(page);
    await expect(page.getByRole("grid", { name: "Worktrees" }).getByRole("row")).toHaveCount(21);
    await park(page);
    await page.waitForTimeout(1000);
    await page.screenshot({ path: path.join(OUT, `worktrees-many-${theme}.png`) });
    await page.evaluate(() => localStorage.removeItem("gg.side.v1"));
  });
}

// The side panel: its one toolbar button with the count of files to commit, its Commit and
// Worktrees icon tabs (the dot of files to commit), the tab strip at the panel's least width, another
// worktree's commit panel (the line naming it), and a tab focused from the keyboard.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
    test(`side panel tabs, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto("/");
      await open(page, gg.repoPath("commit"));
      const shot = async (name: string, clip?: { x: number; y: number; width: number; height: number }) => {
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`), clip });
      };
      const side = page.locator('[data-region="side-panel"]');
      await expect(page.getByTestId("commit-panel").getByRole("option")).toHaveCount(6);
      await park(page);
      await shot("side-commit");
      // The toolbar's right end and the panel's header, closer.
      const head = (await page.locator(".repo-head").boundingBox())!;
      const s = (await side.boundingBox())!;
      await shot("side-toolbar", { x: Math.max(0, s.x - 260), y: head.y, width: width - Math.max(0, s.x - 260), height: head.height + 60 });

      await page.getByTestId("side-tab-worktrees").click();
      await park(page);
      await shot("side-worktrees");

      // Another worktree's commit panel, on the Commit tab.
      await page.getByRole("grid", { name: "Worktrees" }).locator('[role="row"][data-path$="/topic"] .changes').click();
      await expect(page.getByTestId("commit-worktree")).toBeVisible();
      await park(page);
      await shot("side-commit-elsewhere");

      // A tab focused from the keyboard, the panel hidden then shown on it.
      await page.getByTestId("side-tab-commit").focus();
      await page.keyboard.press("ArrowRight");
      await shot("side-tab-focus", { x: s.x, y: s.y, width: s.width, height: 80 });

      // The least width: 240 px, many files to commit and many worktrees.
      for (let i = 0; i < 120; i++) writeFileSync(path.join(gg.repoPath("commit"), `new-${i}.txt`), "new\n");
      await page.evaluate(() => localStorage.setItem("gg.side.v1", JSON.stringify({ width: 240 })));
      await page.reload();
      await open(page, gg.repoPath("commit"));
      await expect(side).toBeVisible();
      await expect(page.getByTestId("side-tab-commit")).toHaveAttribute("title", /12\d changed files/);
      await park(page);
      await shot("side-least-commit");
      await page.getByTestId("side-tab-worktrees").click();
      await park(page);
      await shot("side-least-worktrees");
      await page.evaluate(() => localStorage.removeItem("gg.side.v1"));
    });
  }

// The commit panel: the files of the active worktree with a message; some unticked with what
// stops a commit said; another worktree offered to be made active; a merge with a conflict
// left (the file that cannot be ticked, the merge's note); a hook's refusal in a notice.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
    test(`commit panel, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto("/");
      const repo = gg.repoPath("commit");
      await open(page, repo);
      const shot = async (name: string) => {
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
      };
      const files = page.getByRole("listbox", { name: "Changed files" });
      await page.keyboard.press("ControlOrMeta+k");
      await expect(files.getByRole("option")).toHaveCount(6);
      await page.getByTestId("commit-message").fill("Rework the checkout summary\n\nThe line items show their price breakdown.");
      await park(page);
      await shot("commit");

      // Two unticked, the message gone: what stops a commit is said under the buttons.
      await files.locator('[data-path="todo.md"]').click();
      await files.locator('[data-path="old.txt"]').click();
      await page.getByTestId("commit-message").fill("");
      await files.locator('[data-path="src/app.ts"]').hover();
      await shot("commit-subset");

      // A failing hook: the notice with git's output, the message kept.
      writeFileSync(
        path.join(repo, ".git", "hooks", "pre-commit"),
        "#!/bin/sh\necho 'lint: src/app.ts:1: missing semicolon' >&2\nexit 1\n",
      );
      chmodSync(path.join(repo, ".git", "hooks", "pre-commit"), 0o755);
      await page.getByTestId("commit-message").fill("Rework the checkout summary");
      await page.getByTestId("commit-button").click();
      const failed = page.locator("[data-notices] .toast", { hasText: "refused the commit" });
      await expect(failed.getByRole("button", { name: "Show log" })).toBeVisible();
      await park(page);
      await shot("commit-hook-failed");
      await failed.getByRole("button", { name: /Dismiss/ }).click();

      // Another worktree: its files, the line naming it, and the offer to make it active.
      await showWorktrees(page);
      const topic = page.getByRole("grid", { name: "Worktrees" }).locator('[role="row"][data-path$="/topic"]');
      await topic.click({ button: "right" });
      await page.getByRole("menuitem", { name: "Commit…" }).click();
      await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
      await park(page);
      await shot("commit-elsewhere");
    });

    test(`commit panel concluding a merge, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto("/");
      await open(page, gg.repoPath("worktrees"));
      await showWorktrees(page);
      const hotfix = page.getByRole("grid", { name: "Worktrees" }).locator('[role="row"][data-path$="/hotfix"]');
      await hotfix.dblclick();
      await expect(page.getByTestId("active-worktree")).toContainText("hotfix");
      await page.keyboard.press("ControlOrMeta+k");
      await expect(page.getByTestId("commit-merge")).toBeVisible();
      await park(page);
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${prefix}commit-merge-${theme}.png`) });
    });
  }

const LONG = "feature/rework-the-parser-error-recovery-for-unterminated-strings";

// Pull requests: markers beside labels with ahead/behind counts and long names, on
// worktree rows (the Worktrees tab), stale ones; the branch menu; the side panel's Pull request tab loading,
// filled, waiting on the push, failed, for a branch that has one already, and for a long one.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
    test(`pull requests, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.clock.install();
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      const repo = gg.repoPath("prs");
      gg.git("prs", "branch", LONG, "fix/typo");
      // A row of several branches, each with a pull request: two shown, the rest in "+k".
      for (const name of ["hotfix/parser", "release/2026-11", "spike/lexer"]) gg.git("prs", "branch", name, "docs/readme");
      // The worktree's branch one ahead of its upstream: a count beside the marker.
      const wt = path.join(path.dirname(repo), "prs.worktrees", "login");
      writeFileSync(path.join(wt, "remember.txt"), "remember me\n");
      gg.gitAt(wt, "add", "-A");
      gg.gitAt(wt, "commit", "-q", "-m", "Remember the user");
      const pr = (number: number, head: string, title: string, draft = false) => ({
        number,
        title,
        url: `https://github.com/octo/cat/pull/${number}`,
        isDraft: draft,
        headRefName: head,
        baseRefName: "main",
        headRepositoryOwner: { login: "octo" },
        isCrossRepository: false,
      });
      gg.gh.reply(
        "pr-list",
        0,
        JSON.stringify([
          pr(1284, "feature/login", "Add the login form"),
          pr(1291, "feature/search-indexing-across-every-workspace-package", "Index the search across packages", true),
          pr(1295, "docs/readme", "Explain the parser"),
          pr(1296, "hotfix/parser", "Fix the parser crash", true),
          pr(1297, "release/2026-11", "Release November"),
          pr(1298, "spike/lexer", "Try a new lexer", true),
        ]),
      );
      gg.gh.reply("pr-list-head", 0, "[]");
      gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
      await page.goto("/");
      await open(page, repo);
      const grid = page.getByRole("grid", { name: "Commits" });
      await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
      await page.getByRole("button", { name: "Refresh" }).click();
      await expect(grid.locator(`button.pr-chip[data-pr="1284"]`).first()).toBeVisible();
      const shot = async (name: string) => {
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
      };
      if (width < 1000) await page.getByTestId("side-toggle").click();
      // The default window: the Worktrees tab, the marker beside the worktree's branch too.
      else await showWorktrees(page);
      await park(page);
      await shot("prs-markers");

      // A marker under the pointer, its tooltip's text in the title (not drawn by the browser).
      await grid.locator(`button.pr-chip[data-pr="1284"]`).first().hover();
      await shot("prs-marker-hover");

      // The list is older than 10 minutes: the markers dim.
      await page.clock.fastForward("11:00");
      await park(page);
      await shot("prs-stale");

      // The branch menu with Create pull request…
      await grid.locator('[data-ref="fix/typo"] .name').first().click({ button: "right" });
      await page.getByRole("menuitem", { name: "Create pull request…" }).hover();
      await shot("prs-menu");

      // The Pull request tab: waiting on gh, then filled in.
      gg.gh.slow("pr-list-head", 3);
      await page.getByRole("menuitem", { name: "Create pull request…" }).click();
      const tab = page.getByTestId("pr-panel");
      await expect(tab.getByTestId("pr-waiting")).toBeVisible();
      await shot("prs-tab-loading");
      await page.clock.runFor(3500);
      await expect(tab.getByTestId("pr-title")).toHaveValue("Typo");
      await shot("prs-tab");

      // Waiting on the push and gh.
      gg.gh.reply("pr-create", 1, "", "HTTP 502: Bad Gateway (https://api.github.com/graphql)\n");
      gg.gh.slow("pr-create", 4);
      await tab.getByRole("button", { name: "Push and create PR" }).click();
      await expect(tab.getByTestId("pr-step")).toContainText("Creating the pull request…");
      await shot("prs-tab-pending");
      await page.clock.runFor(4500);
      await expect(tab.getByTestId("pr-error")).toBeVisible();
      // The push was made before gh failed: once the repository is read again, the button
      // only creates.
      await page.clock.runFor(1000);
      await expect(tab.getByRole("button", { name: "Create PR", exact: true })).toBeVisible();
      await shot("prs-tab-failed");

      // A branch that has one already.
      gg.gh.slow("pr-list-head", 0);
      gg.gh.reply("pr-list-head", 0, JSON.stringify([pr(1302, "fix/typo", "Fix the typo in how the parser reads a file")]));
      await page.getByTestId("pr-refresh").click();
      await expect(tab.getByTestId("pr-view")).toBeVisible();
      await shot("prs-tab-existing");

      // A long branch name, the tab about it.
      gg.gh.reply("pr-list-head", 0, "[]");
      await grid.locator(`[data-ref="${LONG}"] .name`).first().click({ button: "right" });
      await page.getByRole("menuitem", { name: "Create pull request…" }).click();
      await expect(tab.getByTestId("pr-title")).not.toHaveValue("");
      await shot("prs-tab-long");
    });
  }

// The Pull request tab for the current branch: the create form for a branch with none, then an
// open pull request's merge status, checks and comments. For the README (the default window)
// and the site (the small one); only fixture names (octo/cat, ada, grace, a bot).
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`pull request tab, create and details, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    const repo = gg.repoPath("prs");
    gg.git("prs", "switch", "-q", "fix/typo");
    const pr = (number: number, head: string, title: string) => ({
      number,
      title,
      url: `https://github.com/octo/cat/pull/${number}`,
      isDraft: false,
      headRefName: head,
      baseRefName: "main",
      headRepositoryOwner: { login: "octo" },
      isCrossRepository: false,
    });
    const others = [pr(1284, "feature/login", "Add the login form"), pr(1295, "docs/readme", "Explain the parser")];
    gg.gh.reply("pr-list", 0, JSON.stringify(others));
    gg.gh.reply("pr-list-head", 0, "[]");
    gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
    await page.goto("/");
    await open(page, repo);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect(grid.locator(`button.pr-chip[data-pr="1284"]`).first()).toBeVisible();
    const shot = async (name: string) => {
      await park(page);
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
    };

    // No pull request yet: the form, filled in from the branch's commit.
    await grid.focus();
    await page.keyboard.press("ControlOrMeta+Shift+P");
    const tab = page.getByTestId("pr-panel");
    await expect(tab.getByTestId("pr-title")).toHaveValue("Typo");
    await expect(tab.getByTestId("pr-body")).not.toHaveValue("");
    await shot("pr-tab-create");

    // The branch pushed and its pull request opened: status, checks, comments.
    gg.git("prs", "push", "-q", "-u", "origin", "fix/typo");
    const mine = pr(1302, "fix/typo", "Fix a typo in the README");
    gg.gh.reply("pr-list", 0, JSON.stringify([...others, mine]));
    gg.gh.reply("pr-list-head", 0, JSON.stringify([mine]));
    answer(
      gg,
      1302,
      view(1302, {
        review: "APPROVED",
        checks: [
          run("macos-latest", "passing", 11 * 60_000),
          run("ubuntu-latest", "passing", 9 * 60_000),
          run("windows-latest", "passing", 17 * 60_000),
          run("lint", "passing", 60_000),
        ],
        comments: [
          comment("1", "ada", "Thanks! While you are in there, the **Install** section has the same typo twice.", 95),
          comment("2", "docs-preview", "Preview of the README for `fix/typo` is ready: no broken links found.", 90),
        ],
        reviews: [review("R1", "grace", "Approved: both typos are fixed now.", "APPROVED", 20)],
      }),
      threads(
        [
          {
            path: "README.md",
            line: 3,
            comments: [
              { id: "c1", login: "grace", body: "Should this say `pnpm`, as the build section does?", minutesAgo: 60 },
              { id: "c2", login: "ada", body: "Yes, changed in the second commit.", minutesAgo: 40 },
            ],
          },
        ],
        ["docs-preview"],
      ),
    );
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect(tab.getByTestId("pr-view")).toBeVisible();
    await expect(tab.getByTestId("pr-merge-status")).toHaveText("Ready to merge");
    await expect(tab.getByTestId("pr-comment").first()).toBeVisible();
    // The checks opened in the default window; in the small one they stay summed up, so the
    // comments show below them.
    if (prefix === "") {
      await tab.getByTestId("pr-checks-toggle").click();
      await expect(tab.getByTestId("pr-check")).toHaveCount(4);
    }
    await shot("pr-tab-details");

    // A reply being written in the code comment's thread, beside Resolve (the README's).
    if (prefix === "") {
      const thread = tab.locator('[data-testid="pr-comment"][data-kind="thread"]', { hasText: "README.md" });
      const field = thread.getByTestId("pr-reply-box-field");
      await field.fill("Both say `pnpm` now: the install step and the build section.");
      await thread.getByTestId("pr-reply-box-send").scrollIntoViewIfNeeded();
      await shot("pr-tab-reply");
    }
  });
}

// The Commit tab's "Committed on branch": two files to commit, and below the message the
// files the branch's commits change since it left origin/main, opened.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
  test(`committed on branch, ${width}x${height}, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: theme });
    const repo = gg.repoPath("branchfiles");
    writeFileSync(path.join(repo, "src/report.ts"), "export const a = 1;\nexport const b = 20;\nexport const c = 30;\nexport const d = 4;\n");
    writeFileSync(path.join(repo, "src/chart.ts"), "export const chart = {};\n");
    await page.goto("/");
    await open(page, repo);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("listbox", { name: "Changed files" }).getByRole("option")).toHaveCount(2);
    await page.getByTestId("commit-message").fill("Chart the report's totals");
    const toggle = page.getByTestId("branch-files-toggle");
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await toggle.click();
    await expect(page.getByTestId("branch-file")).toHaveCount(5);
    await park(page);
    await page.waitForTimeout(250);
    await page.screenshot({ path: path.join(OUT, `${prefix}committed-on-branch-${theme}.png`) });
  });
}

// Settings and keyboard shortcuts: the sheet at its top, a refused command with the
// one beside it running, the lower sections, the reset confirmation; the shortcuts sheet; the
// busy indicator with git's progress; a stale index.lock's notice.
for (const [prefix, width, height] of SIZES)
  for (const theme of ["light", "dark"] as const) {
    test(`settings and shortcuts, ${width}x${height}, ${theme}`, async ({ gg }) => {
      const { page } = gg;
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme });
      const repo = gg.repoPath("small");
      await page.goto("/");
      await open(page, repo);
      const grid = page.getByRole("grid", { name: "Commits" });
      await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
      const shot = async (name: string) => {
        await park(page);
        await page.waitForTimeout(250);
        await page.screenshot({ path: path.join(OUT, `${prefix}${name}-${theme}.png`) });
      };
      const settings = page.getByTestId("settings");
      await page.keyboard.press("ControlOrMeta+,");
      await expect(settings).toBeVisible();
      await shot("settings");

      await settings.getByTestId("settings-editor").fill("code --goto={path}");
      await settings.getByTestId("settings-terminal").fill("kitty --directory {path}");
      await expect(settings.getByRole("alert").filter({ hasText: "separate word" })).toBeVisible();
      await expect(settings.getByTestId("settings-terminal-runs")).toBeVisible();
      await settings.getByRole("button", { name: "Tools" }).click();
      await page.waitForTimeout(400);
      await shot("settings-tools");

      await settings.getByTestId("settings-pattern").fill("../{repo}-trees/{branch}");
      await expect(settings.getByTestId("settings-pattern-preview")).toContainText("small-trees");
      await settings.getByRole("button", { name: "About" }).click();
      await page.waitForTimeout(400);
      await shot("settings-about");

      await settings.getByTestId("settings-reset").click();
      await expect(page.getByRole("dialog", { name: "Reset all settings?" })).toBeVisible();
      await shot("settings-reset");
      await page.getByRole("dialog", { name: "Reset all settings?" }).getByRole("button", { name: "Cancel" }).click();
      await page.keyboard.press("Escape");
      await expect(settings).toHaveCount(0);

      await grid.focus();
      await page.keyboard.press("?");
      await expect(page.getByTestId("shortcuts")).toBeVisible();
      await shot("shortcuts");
      await page.keyboard.press("Escape");

      // A push that takes a while: its progress line in the busy indicator.
      const hook = path.join(repo, ".git", "hooks", "pre-push");
      writeFileSync(hook, "#!/bin/sh\necho 'Checking 42 files before the push…' >&2\nsleep 3\n");
      chmodSync(hook, 0o755);
      const push = runOp(page, { op: "push", branch: "main", remote: "origin", to: null, force: false });
      await expect(page.getByTestId("busy")).toContainText("Checking 42 files", { timeout: 4000 });
      await shot("busy-progress");
      await push;

      // A stale lock in the way.
      const lock = path.join(repo, ".git", "index.lock");
      writeFileSync(lock, "");
      const old = new Date(Date.now() - 3 * 3600_000);
      utimesSync(lock, old, old);
      await grid.locator('[data-index="2"]').click({ button: "right" });
      await page.getByRole("menuitem", { name: "Check out this commit" }).click();
      await expect(page.getByRole("button", { name: "Remove the lock file…" })).toBeVisible();
      await shot("lock-stale");
    });
  }

for (const theme of ["light", "dark"] as const) {
  test(`branch picker, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    const repo = gg.repoPath("branchy");
    // A branch checked out in a second worktree, as the picker marks it.
    gg.git("branchy", "worktree", "add", "-q", path.join(path.dirname(repo), "branchy-release"), "release/1.0");
    await open(page, repo);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const picker = page.getByTestId("branch-picker");
    const field = picker.getByRole("combobox");
    const shot = async (name: string) => {
      await page.waitForTimeout(200); // the picker's entrance
      await page.screenshot({ path: path.join(OUT, `picker-${name}-${theme}.png`) });
    };

    await page.keyboard.press("ControlOrMeta+p");
    await expect(field).toBeFocused();
    await shot("empty");
    await field.fill("fea");
    await picker.locator('[role="option"]').nth(1).hover();
    await shot("filter");
    await field.fill("feature/export");
    await shot("create");
    await field.fill("has space");
    await shot("refused");
    await field.fill("feature/export");
    await picker.locator('[role="option"][data-kind="createFrom"]').click();
    await field.fill("de");
    await shot("from");
  });
}

// The sync control in the toolbar, just before Fetch: behind (as the user's main was), diverged, the
// counts hovered and waiting while an action runs; at the default size and the narrowest
// window the app allows, with a long branch name.
for (const theme of ["light", "dark"] as const) {
  test(`branch sync, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    const repo = gg.repoPath("routine");
    const other = otherClone(repo);
    gg.gitAt(other, "switch", "-q", "main");
    for (let i = 1; i <= 9; i++) gg.gitAt(other, "commit", "-q", "--allow-empty", "-m", `Remote change ${i}`);
    gg.gitAt(other, "push", "-q", "origin", "main");
    gg.git("routine", "fetch", "-q", "origin");
    await page.goto("/");
    await open(page, repo);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const pull = page.getByTestId("sync-pull");
    await expect(pull).toHaveText("9");
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `sync-${name}-${theme}.png`), clip: { x: 0, y: 0, width: page.viewportSize()!.width, height: 120 } });
    };
    await park(page);
    await shot("behind");
    await pull.hover();
    await shot("behind-hover");

    gg.git("routine", "commit", "-q", "--allow-empty", "-m", "Local one");
    gg.git("routine", "commit", "-q", "--allow-empty", "-m", "Local two");
    await expect(page.getByTestId("sync-push")).toHaveText("2");
    await park(page);
    await shot("diverged");

    // A pull held by a slow hook: both counts wait, the running pull shows at the right.
    const hook = path.join(repo, ".git", "hooks", "post-merge");
    writeFileSync(hook, "#!/bin/sh\nsleep 2\n");
    chmodSync(hook, 0o755);
    gg.git("routine", "config", "pull.rebase", "false");
    await pull.click();
    await expect(page.getByTestId("busy")).toBeVisible();
    await park(page);
    await shot("running");
    await expect(pull).toHaveCount(0);
    await page.getByRole("status").getByRole("button", { name: "Dismiss notification" }).first().click();

    await page.setViewportSize({ width: 800, height: 560 });
    gg.git("routine", "reset", "-q", "--hard", "HEAD~1"); // the merge undone: diverged again
    gg.git("routine", "branch", "-q", "-m", "main", "feature/a-rather-long-branch-name-for-the-header");
    gg.git("routine", "branch", "-q", "--set-upstream-to", "origin/main");
    gg.git("routine", "commit", "-q", "--allow-empty", "-m", "Local three");
    await expect(page.getByTestId("active-worktree")).toContainText("feature/");
    await park(page);
    await shot("narrow");
  });
}

// The header (actions) and the status bar (the folder, when it was last fetched): at the
// default size, then the narrowest window the app allows on a worktree with a long path,
// the side panel shown and hidden, with the path's menu and the View popover open.
for (const theme of ["light", "dark"] as const) {
  test(`status bar, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    const repo = gg.repoPath("small");
    gg.git("small", "fetch", "-q", "origin");
    // A clone deep in the home folder, as an agent's worktree folder is.
    const origin = path.join(path.dirname(repo), "small-origin.git");
    const deep = path.join(gg.home, "Code", "experiments", "payments-service", ".agents", "worktrees");
    const long = makeRepo("agent-a883f2c1e9b7d4a6-the-status-bar-and-a-header-for-actions-only-at-the-narrowest-width", (git) => {
      git("clone", "-q", origin, ".");
      git("fetch", "-q", "origin");
    }, deep);
    await page.goto("/");
    await open(page, repo);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    await expect(page.getByTestId("status-fetched")).toHaveText("Fetched just now");
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `statusbar-${name}-${theme}.png`) });
    };
    await park(page);
    await shot("normal");
    await page.getByTestId("status-path").click();
    await shot("menu");
    await page.keyboard.press("Escape");
    await page.getByTestId("view-options").click();
    await shot("view");
    await page.keyboard.press("Escape");

    await page.setViewportSize({ width: 800, height: 560 });
    await open(page, long);
    await expect(page.getByTestId("status-path")).toContainText("narrowest-width");
    await park(page);
    await shot("long-nopanel");
    // Narrow, the side panel shows over the graph, down to the status bar.
    await page.keyboard.press("ControlOrMeta+b");
    await park(page);
    await shot("long-panel");
  });
}
