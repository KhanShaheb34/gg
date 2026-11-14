// Screenshots for the visual check, written to docs/perf/screens/. Run on demand:
//   GG_SCREENS=1 pnpm e2e screens
import { chmodSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import type { OpOutcome, OpRequest } from "../src/lib/types";
import { expect, makeRepo, otherClone, test } from "./fixtures";

const OUT = path.resolve(import.meta.dirname, "..", "docs", "perf", "screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });

/** Somewhere nothing reacts to the pointer: the empty middle of the tab bar. */
const park = (page: Page) => page.mouse.move(1000, 20);

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toBeVisible();
}

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
    await page.getByRole("button", { name: "Open repository" }).first().click();
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

/** Runs `request` through the app's own runOp (the module the page loaded), in the active tab. */
function runOp(page: Page, request: OpRequest): Promise<OpOutcome> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return ops.runOp(app.activeRepoId!, req);
  }, request);
}

// Menus, the confirmation dialog, the busy indicator and the notices of operations.
for (const theme of ["light", "dark"] as const) {
  test(`menus, dialogs and operations, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    const dir = gg.repoPath("branchy");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
    };

    // A commit's menu with the reset submenu open, the hard reset under the pointer.
    await grid.locator('[role="row"]', { hasText: "Plugin sandboxing" }).locator(".subject").click({ button: "right" });
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

    // A failed checkout (a changed file it would overwrite): the notice with git's output unfolded.
    gg.git("branchy", "checkout", "--", ".");
    const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
    writeFileSync(path.join(dir, differing), "in the way\n");
    const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
    expect(failed.status).toBe("failed");
    const alert = page.getByRole("alert").filter({ hasText: failed.message });
    await alert.getByRole("button", { name: "Git output" }).click();
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

// The dialogs of commit and tag actions (Task 7).
for (const theme of ["light", "dark"] as const) {
  test(`commit and tag dialogs, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.goto("/");
    await open(page, gg.repoPath("branchy"));
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const dialog = page.locator('[role="dialog"], [role="alertdialog"]');
    const shot = async (name: string) => {
      await park(page);
      await page.waitForTimeout(250);
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
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

// The dialogs of branch actions and the staging routine's notices (Task 8).
for (const theme of ["light", "dark"] as const) {
  test(`branch dialogs and the staging routine, ${theme}`, async ({ gg }) => {
    const { page } = gg;
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
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
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
    const merged = page.getByRole("status").filter({ hasText: "Merged feature/login into staging." });
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
    const rejected = page.getByRole("alert").filter({ hasText: "The remote has commits you don't have" });
    await expect(rejected).toBeVisible();
    await rejected.hover();
    await snap("notice-push-rejected");

    // Pull: diverged, and git is not told how: the choice.
    await rejected.getByRole("button", { name: "Pull staging", exact: true }).click();
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

// The conflict banner (spec §5.6) in each of its states, and the dialogs of continue and of
// stashes.
for (const theme of ["light", "dark"] as const) {
  test(`conflict banner and stash dialogs, ${theme}`, async ({ gg }) => {
    const { page } = gg;
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
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
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

// The command log (Task 10): closed, open, a failed operation reached from its notice, the
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

// Task 9 review item 2: the smallest window, the many-files banner, the details panel as tall as
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
    const box = (await splitter.boundingBox())!;
    await page.mouse.move(box.x + 100, box.y + 4);
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
// dialogs, notices and busy state at 900×560 (Task 11).
for (const theme of ["light", "dark"] as const) {
  test(`remaining menus, confirmations and notices, both sizes, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto("/");
    const dir = gg.repoPath("branchy");
    await open(page, dir);
    const grid = page.getByRole("grid", { name: "Commits" });
    await expect(grid.locator('[data-index="0"] .subject')).not.toBeEmpty();
    const shot = async (name: string) => {
      await page.waitForTimeout(200);
      await page.screenshot({ path: path.join(OUT, `${name}-${theme}.png`) });
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

    void runOp(page, { op: "delete_branch", branch: "feature/search" });
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
    gg.gitAt(path.dirname(dir), "clone", "-q", gg.git("branchy", "remote", "get-url", "origin").trim(), other);
    gg.gitAt(other, "-c", "user.name=Grace Hopper", "-c", "user.email=grace@example.com", "commit", "-q", "--allow-empty", "-m", "Pushed from elsewhere");
    gg.gitAt(other, "push", "-q", "origin", "main");
    gg.git("branchy", "fetch", "-q", "origin");
    void runOp(page, { op: "push", branch: "main", remote: "origin", setUpstream: false, force: true, lease: null });
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

    // At 900×560: a commit's menu, a dialog, a failed operation's notice, the busy state.
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
  test(`skip notice, ${theme}`, async ({ gg }) => {
    const { page } = gg;
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
    await page.screenshot({ path: path.join(OUT, `notice-skip-${theme}.png`) });
  });
}
