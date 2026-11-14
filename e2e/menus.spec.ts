// Context menus, dialogs and the operation state against the real core: right-click and
// keyboard menus on commits and labels, copying, the confirmation the core asks for, the
// busy indicator, and closing a busy tab.
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { closeButton, expect, runOp, test } from "./fixtures";

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
/** The generation of the rows the active tab shows. */
function generation(page: Page): Promise<number> {
  return page.evaluate(async () => {
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return findRepoView(app.activeRepoId!)!.snapshot.generation;
  });
}

const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });

/** The names of the root menu's items, in order. */
async function itemNames(page: Page): Promise<string[]> {
  return menu(page).getByRole("menuitem").allTextContents().then((all) => all.map((t) => t.trim()));
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
    "Merge into the current branch…",
    "Rebase the current branch onto this…",
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

test("a menu taller than the window scrolls inside it, and the keyboard keeps the active item in view", async ({
  gg,
}) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 560 });
  for (let i = 1; i <= 40; i++) gg.git("small", "tag", `release-${String(i).padStart(2, "0")}`, "HEAD");
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await rowOf(page, "Add part 19").locator(".ref.more").click({ button: "right" });
  const m = menu(page);
  await expect(m).toBeVisible();
  const box = (await m.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(560);
  expect(await m.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);

  await page.keyboard.press("End");
  const last = m.getByRole("menuitem").last();
  await expect(last).toBeFocused();
  await expect(last).toBeInViewport();
  const inside = await last.evaluate((el) => {
    const a = el.getBoundingClientRect();
    const b = el.parentElement!.getBoundingClientRect();
    return a.top >= b.top && a.bottom <= b.bottom;
  });
  expect(inside).toBe(true);
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

test("the keyboard menu acts on the selected commit, by its hash, even when its index points elsewhere (I6, R42)", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hash = gg.git("small", "rev-parse", "HEAD~3").trim(); // "Add part 17"
  await rowOf(page, "Add part 17").click();
  await grid(page).focus();
  // The index of the selection drifts for a moment (rows replaced underneath): it points at
  // another commit while the selection is still this one.
  await page.evaluate(async () => {
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const view = findRepoView(app.activeRepoId!)!;
    view.selectedIndex = view.selectedIndex! + 2;
  });
  await page.keyboard.press("Shift+F10");
  await page.keyboard.type("copy h");
  await page.keyboard.press("Enter");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hash);
});

test("the keyboard reaches label menus: the row's menu lists its labels, remote halves included", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await rowOf(page, "Fix typo 4").click();
  await grid(page).focus();
  await page.keyboard.press("Shift+F10");
  await page.keyboard.press("End");
  await expect(item(page, "Branches")).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(menus(page).nth(1).getByRole("menuitem")).toHaveText(["Branch fix/typo", "Remote branch origin/fix/typo"]);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await expect(menus(page).nth(2).getByRole("menuitem").first()).toHaveText("Check out as a local branch…");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(menus(page)).toHaveCount(0);

  // A row without labels has no such item.
  await rowOf(page, "Add part 18").locator(".subject").click({ button: "right" });
  expect(await itemNames(page)).not.toContain("Branches");
});

test("the +k pill lists the remote halves of the hidden labels too", async ({ gg }) => {
  const { page } = gg;
  const names = ["fix/a", "fix/b", "fix/c", "fix/d", "fix/e"];
  for (const name of names) gg.git("small", "branch", name, "fix/typo");
  gg.git("small", "push", "-q", "origin", ...names);
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await rowOf(page, "Fix typo 4").locator(".ref.more").click({ button: "right" });
  const listed = await itemNames(page);
  const hidden = listed.filter((n) => n.startsWith("Branch ")).map((n) => n.slice("Branch ".length));
  expect(hidden.length).toBeGreaterThan(0);
  for (const name of hidden) expect(listed).toContain(`Remote branch origin/${name}`);
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

test("the menu stays open while the graph refreshes, and closes when an operation starts or ends", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await rowOf(page, "Add part 18").locator(".subject").click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  gg.git("small", "commit", "-q", "--allow-empty", "-m", "Made elsewhere"); // the graph reloads
  await expect(rowOf(page, "Made elsewhere")).toBeVisible();
  // Once the menu has looked at the new rows, it is still open.
  await expect
    .poll(async () => (await menu(page).getAttribute("data-checked-generation")) === String(await generation(page)))
    .toBe(true);
  await expect(menus(page)).toHaveCount(1);
  await page.keyboard.press("Escape");

  // An operation that starts and ends closes it (R26).
  await rowOf(page, "Add part 18").locator(".subject").click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  await runOp(page, { op: "checkout_commit", hash: gg.git("small", "rev-parse", "HEAD~1").trim() });
  await expect(menus(page)).toHaveCount(0);
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
  await expect(dialog).toBeVisible();
  await page.waitForTimeout(450); // the red button takes no click at once (see Dialog)
  await dialog.getByRole("button", { name: "Reset and discard" }).click();
  expect((await ran).status).toBe("done");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");
  expect(gg.git("small", "rev-parse", "HEAD").trim()).toBe(target);
  await expect(page.getByRole("status").filter({ hasText: /Reset main/ })).toBeVisible();
  await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText("Add part 18");
});

test("a confirmation is described by its message; Enter on a button inside a dialog presses that button", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  writeFileSync(path.join(dir, "src/part1.txt"), "changed\n");
  void runOp(page, { op: "reset", hash: gg.git("small", "rev-parse", "HEAD~1").trim(), mode: "hard" });
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  const described = await dialog.evaluate((d) => {
    const ids = (d.getAttribute("aria-describedby") ?? "").split(" ").filter(Boolean);
    return ids.map((id) => document.getElementById(id)?.textContent ?? "").join(" ");
  });
  expect(described).toContain("discards the uncommitted changes to 1 file");

  // A button of the dialog's own content (as a "Show more" would be): Enter presses it, and
  // does not confirm.
  await dialog.evaluate((d) => {
    const b = document.createElement("button");
    b.textContent = "Show more";
    b.onclick = () => (b.dataset.pressed = "yes");
    d.querySelector('[data-testid="dangers"]')!.append(b);
    b.focus();
  });
  await page.waitForTimeout(450); // past the red button's first moments, so Enter could confirm
  await page.keyboard.press("Enter");
  await expect(dialog.getByRole("button", { name: "Show more" })).toHaveAttribute("data-pressed", "yes");
  await expect(dialog).toBeVisible();
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");
  await page.keyboard.press("Escape");
});

test("a quick second click does not confirm the confirmation the core asks for next", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  writeFileSync(path.join(dir, "src/part1.txt"), "changed\n");
  const target = gg.git("small", "rev-parse", "HEAD~1").trim();
  // The core asks once more, as when a force push's remote moved meanwhile: the first
  // confirmed request reaches it unconfirmed.
  let reasked = false;
  await page.route("**/invoke", (route) => {
    const body = route.request().postDataJSON() as { cmd: string; args: { confirmed?: string[] } } | null;
    if (body?.cmd === "run_op" && !reasked && (body.args.confirmed?.length ?? 0) > 0) {
      reasked = true;
      return route.continue({ postData: JSON.stringify({ ...body, args: { ...body.args, confirmed: [] } }) });
    }
    return route.continue();
  });

  const ran = runOp(page, { op: "reset", hash: target, mode: "hard" });
  const go = page.getByRole("alertdialog").getByRole("button", { name: "Reset and discard" });
  await expect(go).toBeVisible();
  await page.waitForTimeout(450);
  const box = (await go.boundingBox())!;
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.click(x, y);
  // The second confirmation opens where the first was; a click right away does nothing.
  await expect.poll(() => reasked).toBe(true);
  await expect(go).toBeVisible();
  await page.mouse.click(x, y);
  await page.waitForTimeout(150);
  await expect(go).toBeVisible();
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");

  // A deliberate click, a moment later, goes ahead.
  await page.waitForTimeout(400);
  await go.click();
  expect((await ran).status).toBe("done");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");
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
  // Showing the log keeps the notice: it is what the log is read against.
  await notice.getByRole("button", { name: "Show log" }).click();
  await expect(notice).toBeVisible();
});

test("a notice stays while keyboard focus is on it, and leaves after it moves away", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hash = gg.git("small", "rev-parse", "HEAD").trim();
  await runOp(page, { op: "create_tag", name: "held", hash, message: null });
  const notice = page.getByRole("status").filter({ hasText: "Created tag held" });
  await notice.getByRole("button", { name: "Dismiss notification" }).focus();
  await page.mouse.move(10, 400); // not hovering it
  await page.waitForTimeout(4_500); // past an info notice's 4 s
  await expect(notice).toBeVisible();
  await grid(page).focus();
  await expect(notice).toHaveCount(0, { timeout: 6_000 });
});

test("the toolbar shows the running operation and how many wait behind it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  // Each checkout runs this hook, which takes a while.
  const hook = path.join(dir, ".git/hooks/post-checkout");
  writeFileSync(hook, "#!/bin/sh\nsleep 2\n");
  chmodSync(hook, 0o755);

  const busy = page.getByTestId("busy");
  // Both in the same instant: the core queues them in the order they were sent (R28).
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
  const started = Date.now();
  expect((await runOp(page, { op: "create_tag", name: "quick", hash, message: null })).status).toBe("done");
  const took = Date.now() - started;
  await page.waitForTimeout(300);
  // The indicator waits 150 ms (R23): an operation that took less never shows it. (On a
  // loaded machine even this one can take longer, and then showing it is right.)
  const seen = await page.evaluate(() => (window as unknown as { busySeen: boolean }).busySeen);
  if (took < 140) expect(seen).toBe(false);
});

test("the command log taking in new commands does not close an open menu", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  // The log open with the background reads, so a refresh adds entries and it follows them.
  await page.getByRole("button", { name: "Command log", exact: true }).click();
  const log = page.getByRole("region", { name: "Command log" });
  await log.getByRole("checkbox", { name: "Background reads" }).check();
  await expect(log.locator(".row").first()).toBeVisible();
  const before = await log.locator(".row").count();

  await grid(page).locator('[data-index="4"] .subject').click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  await page.waitForTimeout(400);
  gg.git("big", "commit", "-q", "--allow-empty", "-m", "Made outside the app");
  await expect.poll(() => log.locator(".row").count(), { timeout: 5000 }).toBeGreaterThan(before);
  await page.waitForTimeout(500);
  await expect(menu(page)).toBeVisible();
});

test("a menu stays when the app itself scrolls the graph a moment later (keeping a row in view)", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await grid(page).locator('[data-index="6"] .subject').click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  // What the graph does when a panel opening below it (late, on a busy machine) would hide
  // the selected row: it scrolls by itself. No wheel, no click, no key from the user.
  await page.waitForTimeout(500);
  await grid(page).evaluate((el) => (el.scrollTop += 52));
  await page.waitForTimeout(300);
  await expect(menu(page)).toBeVisible();
  // The user's own scrolling still closes it.
  const box = (await grid(page).boundingBox())!;
  await page.mouse.move(box.x + box.width - 40, box.y + box.height - 40);
  await page.mouse.wheel(0, 300);
  await expect(menus(page)).toHaveCount(0);
});

test("a menu acts on the label the user right-clicked, even if the row's data moved on before it was drawn", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  // Defensive (R43): the app does not reach this state by itself (a row's element and its
  // data change together, in one frame); the test fakes it, a drawn row still showing
  // develop's commit and label while the data under it is another row's, to check that the
  // guard acts on what the element shows.
  const shown = await page.evaluate(() => {
    const develop = document.querySelector<HTMLElement>('.ref[data-ref="develop"]')!;
    const developRow = develop.closest<HTMLElement>('[role="row"]')!;
    const other = [...document.querySelectorAll<HTMLElement>('[role="row"][data-index]')].find(
      (r) => r !== developRow && r.querySelector('.ref[data-slot="0"]:not([hidden])') && !r.hidden,
    )!;
    const label = other.querySelector<HTMLElement>('.ref[data-slot="0"]')!;
    const was = label.dataset.ref;
    label.dataset.ref = "develop";
    label.dataset.doctored = "true";
    other.dataset.hash = developRow.dataset.hash;
    return was;
  });
  expect(shown).not.toBe("develop");
  await grid(page).locator('.ref[data-doctored="true"]').click({ button: "right" });
  await expect(item(page, "Copy name")).toBeVisible();
  await item(page, "Copy name").click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("develop");
});
