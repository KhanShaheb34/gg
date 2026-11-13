// Context menus, dialogs and the operation state against the real core: right-click and
// keyboard menus on commits and labels, copying, the confirmation the core asks for, the
// busy indicator, and closing a busy tab.
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { closeButton, expect, test } from "./fixtures";
import type { OpOutcome, OpRequest } from "../src/lib/types";

test.use({ permissions: ["clipboard-read", "clipboard-write"] });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const rowOf = (page: Page, subject: string) => grid(page).locator('[role="row"]', { hasText: subject });
const menus = (page: Page) => page.getByRole("menu");
const menu = (page: Page) => menus(page).first();
const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });

/** The names of the root menu's items, in order. */
async function itemNames(page: Page): Promise<string[]> {
  return menu(page).getByRole("menuitem").allTextContents().then((all) => all.map((t) => t.trim()));
}

/** Runs `request` through the app's own runOp (the module the page loaded), in the active tab. */
function runOp(page: Page, request: OpRequest): Promise<OpOutcome> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return ops.runOp(app.activeRepoId!, req);
  }, request);
}

test("right-clicking a commit selects it and shows the commit actions", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const row = rowOf(page, "Add part 3");
  await row.locator(".subject").click({ button: "right" });
  await expect(row).toHaveAttribute("aria-selected", "true");
  await expect(menu(page)).toBeVisible();
  expect(await itemNames(page)).toEqual([
    "Check out this commit",
    "Create branch here…",
    "Create tag here…",
    "Cherry-pick…",
    "Revert…",
    "Reset main to here",
    "Copy hash",
    "Copy message",
  ]);
  // The reset submenu opens on hover.
  await item(page, "Reset main to here").hover();
  await expect(menus(page)).toHaveCount(2);
  await expect(menus(page).nth(1).getByRole("menuitem")).toHaveText([
    "Soft: keep changes, staged",
    "Mixed: keep changes, unstaged",
    "Hard: discard all changes",
  ]);
  await page.keyboard.press("Escape");
  await expect(menus(page)).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(menus(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
});

test("branch, tag and remote labels have their own menus; the reason of a disabled item shows", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));

  // The current branch: checkout, delete, merge and rebase cannot apply.
  await grid(page).locator('[data-ref="main"]').click({ button: "right" });
  expect(await itemNames(page)).toEqual([
    "Check out",
    "Rename…",
    "Delete…",
    "Merge main into main…",
    "Rebase main onto main…",
    "Push 2 commits…",
    "Pull",
    "Copy name",
  ]);
  const checkout = item(page, "Check out");
  await expect(checkout).toHaveAttribute("aria-disabled", "true");
  await checkout.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Already checked out");
  await expect(checkout).toHaveAttribute("aria-describedby", "menu-why");
  // A disabled item cannot be picked.
  await checkout.click({ force: true });
  await expect(menu(page)).toBeVisible();
  await page.keyboard.press("Escape");

  // Another branch, merged into main.
  await grid(page).locator('[data-ref="feature/login"] .name').click({ button: "right" });
  await expect(item(page, "Merge feature/login into main…")).not.toHaveAttribute("aria-disabled", "true");
  await expect(item(page, "Pull")).toHaveAttribute("aria-disabled", "true");
  await page.keyboard.press("Escape");

  // A tag (low in the history: the details panel now takes the bottom of the window).
  await grid(page).evaluate((el) => (el.scrollTop = el.scrollHeight));
  await grid(page).locator('[data-ref="v1"]').click({ button: "right" });
  expect(await itemNames(page)).toEqual(["Push…", "Delete…", "Copy name"]);
  await page.keyboard.press("Escape");

  // The remote part of a combined label is the remote branch.
  await grid(page).evaluate((el) => (el.scrollTop = 0));
  await grid(page).locator('[data-ref="fix/typo"] .remotes').click({ button: "right" });
  expect(await itemNames(page)).toEqual([
    "Check out as a local branch…",
    "Fetch into a local branch…",
    "Merge origin/fix/typo into main…",
    "Delete from origin…",
    "Copy name",
  ]);
  await page.keyboard.press("Escape");

  // A remote branch label of its own.
  await grid(page).locator('[data-ref="origin/main"]').click({ button: "right" });
  await expect(item(page, "Delete from origin…")).toBeVisible();
  await page.keyboard.press("Escape");
});

test("the +k pill lists the hidden labels, each with its menu", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("labelled"));
  // Row 300 (newest first): scrolled to, a little below the top.
  await grid(page).evaluate((el) => (el.scrollTop = 300 * 26 - 120));
  const row = rowOf(page, "Big commit 300");
  await row.locator(".ref.more").click({ button: "right" });
  expect(await itemNames(page)).toEqual(["Tag x3", "Tag x4", "Tag x5"]);
  await item(page, "Tag x4").hover();
  await expect(menus(page).nth(1).getByRole("menuitem")).toHaveText(["Push…", "Delete…", "Copy name"]);
  await menus(page).nth(1).getByRole("menuitem", { name: "Copy name" }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("x4");
});

test("the keyboard opens, moves through and picks from the menu", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hash = gg.git("small", "rev-parse", "HEAD~3").trim(); // "Add part 17", below the merge
  await rowOf(page, "Add part 17").click();
  await grid(page).focus();

  await page.keyboard.press("Shift+F10");
  await expect(item(page, "Check out this commit")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(item(page, "Create branch here…")).toBeFocused();
  await page.keyboard.press("End");
  await expect(item(page, "Copy message")).toBeFocused();
  await page.keyboard.press("ArrowDown"); // wraps
  await expect(item(page, "Check out this commit")).toBeFocused();
  await page.keyboard.press("Home");
  await page.keyboard.type("re");
  await expect(item(page, "Revert…")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await expect(item(page, "Soft: keep changes, staged")).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(menus(page)).toHaveCount(1);
  await expect(item(page, "Reset main to here")).toBeFocused();

  await page.keyboard.type("copy h");
  await expect(item(page, "Copy hash")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(menus(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hash);
  await expect(page.getByRole("status").filter({ hasText: "Copied the hash" })).toBeVisible();
});

test("copy message copies the whole message; copy name the branch's name", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowOf(page, "Rework peaches routes").click({ button: "right" });
  await item(page, "Copy message").click();
  await expect(page.getByRole("status").filter({ hasText: "Copied the message" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "Rework peaches routes\n\nMove the old notes into docs and add b.py.\n    Indented line keeps   its spacing.",
  );

  await grid(page).locator('[data-ref="side"]').click({ button: "right" });
  await item(page, "Copy name").click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("side");
});

test("the menu closes on scroll, an outside click and the window losing focus, and stays inside the window", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  const first = grid(page).locator('[data-index="2"] .subject');

  await first.click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  const box = (await grid(page).boundingBox())!;
  await page.mouse.move(box.x + box.width - 40, box.y + box.height - 40);
  await page.mouse.wheel(0, 300);
  await expect(menus(page)).toHaveCount(0);
  await grid(page).evaluate((el) => (el.scrollTop = 0));

  await first.click({ button: "right" });
  await page.mouse.click(box.x + box.width - 40, box.y + 60);
  await expect(menus(page)).toHaveCount(0);

  await first.click({ button: "right" });
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await expect(menus(page)).toHaveCount(0);

  // Near the bottom-right corner it opens up and to the left of the pointer.
  const viewport = page.viewportSize()!;
  const area = (await grid(page).boundingBox())!; // above the details panel, now open
  await page.mouse.click(viewport.width - 10, area.y + area.height - 12, { button: "right" });
  await expect(menu(page)).toBeVisible();
  const shown = (await menu(page).boundingBox())!;
  expect(shown.x).toBeGreaterThanOrEqual(0);
  expect(shown.y).toBeGreaterThanOrEqual(0);
  expect(shown.x + shown.width).toBeLessThanOrEqual(viewport.width);
  expect(shown.y + shown.height).toBeLessThanOrEqual(viewport.height);
});

test("a dangerous request asks first: Cancel has the focus and runs nothing; confirming runs it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  writeFileSync(path.join(dir, "src/part1.txt"), "changed\n");
  const target = gg.git("small", "rev-parse", "HEAD~1").trim();
  await rowOf(page, "Add part 18").click();
  await grid(page).focus();

  void runOp(page, { op: "reset", hash: target, mode: "hard" });
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("heading")).toHaveText(/^Reset main to /);
  await expect(dialog.getByTestId("dangers")).toContainText("discards the uncommitted changes to 1 file");
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  // Enter on Cancel cancels; so does Escape.
  await page.keyboard.press("Enter");
  await expect(dialog).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");

  void runOp(page, { op: "reset", hash: target, mode: "hard" });
  await expect(dialog).toBeVisible();
  // Focus stays inside the dialog.
  for (let i = 0; i < 3; i++) await page.keyboard.press("Tab");
  expect(await dialog.evaluate((d) => d.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");

  const ran = runOp(page, { op: "reset", hash: target, mode: "hard" });
  await dialog.getByRole("button", { name: "Reset and discard" }).click();
  expect((await ran).status).toBe("done");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");
  expect(gg.git("small", "rev-parse", "HEAD").trim()).toBe(target);
  await expect(page.getByRole("status").filter({ hasText: /Reset main/ })).toBeVisible();
  await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText("Add part 18");
});

test("a failed operation shows git's text folded below its message, with a link to the log", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  // Checking out fix/typo would overwrite this uncommitted change.
  mkdirSync(path.join(dir, "docs"));
  writeFileSync(path.join(dir, "docs/typo1.md"), "mine\n");
  gg.git("small", "add", "docs/typo1.md");
  const outcome = await runOp(page, { op: "checkout_branch", branch: "fix/typo", fastForward: false });
  expect(outcome.status).toBe("failed");
  const notice = page.getByRole("alert").filter({ hasText: outcome.message });
  await expect(notice).toBeVisible();
  await expect(notice.locator(".output")).toHaveCount(0);
  await notice.getByRole("button", { name: "Git output" }).click();
  await expect(notice.locator(".output")).toContainText("docs/typo1.md");
  await expect(notice.getByRole("button", { name: "Show log" })).toBeVisible();
});

test("the toolbar shows the running operation and how many wait behind it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  // Each checkout runs this hook, which takes a while.
  const hook = path.join(dir, ".git/hooks/post-checkout");
  writeFileSync(hook, "#!/bin/sh\nsleep 1\n");
  chmodSync(hook, 0o755);

  const busy = page.getByTestId("busy");
  const first = runOp(page, { op: "checkout_branch", branch: "fix/typo", fastForward: false });
  const second = runOp(page, { op: "checkout_branch", branch: "main", fastForward: false });
  await expect(busy).toBeVisible();
  await expect(busy).toContainText("fix/typo");
  await expect(busy).toContainText("+1");
  // While it runs, menus open and their items stay enabled (requests queue).
  await grid(page).locator('[data-ref="feature/login"] .name').click({ button: "right" });
  await expect(item(page, "Check out")).not.toHaveAttribute("aria-disabled", "true");
  await page.keyboard.press("Escape");

  // Closing the tab now is refused, and the tab stays.
  await closeButton(page, "small").click();
  await expect(page.getByRole("status").filter({ hasText: "An operation is running" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "small" })).toBeVisible();

  expect((await first).status).toBe("done");
  expect((await second).status).toBe("done");
  await expect(busy).toHaveCount(0);
  expect(gg.git("small", "branch", "--show-current").trim()).toBe("main");
});

test("an operation that ends at once shows no busy indicator", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  // Watch for the indicator appearing at all while a quick operation runs.
  await page.evaluate(() => {
    const w = window as unknown as { busySeen: boolean };
    w.busySeen = false;
    new MutationObserver(() => {
      if (document.querySelector('[data-testid="busy"]')) w.busySeen = true;
    }).observe(document.body, { childList: true, subtree: true });
  });
  const hash = gg.git("small", "rev-parse", "HEAD").trim();
  expect((await runOp(page, { op: "create_tag", name: "quick", hash, message: null })).status).toBe("done");
  await page.waitForTimeout(300);
  expect(await page.evaluate(() => (window as unknown as { busySeen: boolean }).busySeen)).toBe(false);
});
