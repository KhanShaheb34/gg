// Screenshots for the visual check, written to docs/perf/screens/. Run on demand:
//   GG_SCREENS=1 pnpm e2e screens
import { chmodSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import type { OpOutcome, OpRequest } from "../src/lib/types";
import { expect, makeRepo, test } from "./fixtures";

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
    await page.getByRole("menuitem", { name: "Merge main into main…" }).hover();
    await shot("menu-branch-current");
    await page.keyboard.press("Escape");

    // Another branch, from the keyboard (the active item is the focus).
    await grid.locator('[data-ref="feature/ui"]').first().click({ button: "right" });
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await park(page);
    await shot("menu-branch");
    await page.keyboard.press("Escape");

    // A remote branch, near the right edge of the window.
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
