// Stashes (spec §5.2): saving one from the uncommitted-changes row, and a stash row's apply,
// pop, drop and create-branch, end to end against the real core.
import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, test } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const row = (page: Page, i: number) => grid(page).locator(`[data-index="${i}"]`);
const subject = (page: Page, i: number) => row(page, i).locator(".subject");
const item = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });
const dialog = (page: Page) => page.getByRole("dialog");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
/** A red button takes no click just after its dialog opens (see Dialog): wait that out. */
const armed = (page: Page) => page.waitForTimeout(450);

/** The fixture `small` with a change to a tracked file and, if asked, a new untracked one. */
function change(gg: GG, untracked = false): string {
  const dir = gg.repoPath("small");
  writeFileSync(path.join(dir, "src/part1.txt"), "changed for the stash\n");
  if (untracked) writeFileSync(path.join(dir, "scratch.txt"), "untracked notes\n");
  return dir;
}

/** Saves the uncommitted changes from their row's menu with `message`. */
async function stashChanges(page: Page, message: string, includeUntracked = false) {
  await expect(subject(page, 0)).toHaveText(/^Uncommitted changes/);
  await subject(page, 0).click({ button: "right" });
  await item(page, "Stash changes…").click();
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Stash changes");
  await d.getByLabel("Message (optional)").fill(message);
  if (includeUntracked) await d.getByRole("checkbox", { name: /Include untracked files/ }).check();
  await d.getByRole("button", { name: "Stash", exact: true }).click();
}

/** Opens the menu of the stash `stash@{n}` and picks `name`. */
async function pickOnStash(page: Page, n: number, name: string) {
  await grid(page).locator(`[data-ref="stash@{${n}}"]`).first().click({ button: "right" });
  await item(page, name).click();
}

const stashList = (gg: GG) => gg.git("small", "stash", "list", "--format=%s").trim();

test("save_apply_pop_drop_branch: save, apply, pop and create a branch from a stash", async ({ gg }) => {
  const { page } = gg;
  const dir = change(gg);
  await page.goto("/");
  await open(page, dir);

  // Save.
  await stashChanges(page, "Header copy");
  await expect(notice(page, /Stashed/)).toBeVisible();
  await expect(subject(page, 0)).toHaveText(/Header copy/);
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();
  expect(stashList(gg)).toBe("On main: Header copy");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");

  // Apply: the changes come back, the stash stays.
  await pickOnStash(page, 0, "Apply");
  await expect(notice(page, /Applied/)).toBeVisible();
  await expect(subject(page, 0)).toHaveText(/^Uncommitted changes/);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");
  expect(stashList(gg)).toBe("On main: Header copy");

  // Pop (after putting the tree back): the changes come back, the stash goes.
  gg.git("small", "checkout", "--", "src/part1.txt");
  await expect(subject(page, 0)).not.toHaveText(/^Uncommitted changes/);
  await pickOnStash(page, 0, "Pop");
  await expect(notice(page, /Popped|Applied/)).toBeVisible();
  await expect(grid(page).locator('[data-ref="stash@{0}"]')).toHaveCount(0);
  expect(stashList(gg)).toBe("");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");

  // Create a branch from a stash: made where the stash was made, checked out, stash gone.
  await stashChanges(page, "For a branch");
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();
  await pickOnStash(page, 0, "Create branch from stash…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Create branch from stash");
  await expect(d).toContainText("For a branch");
  await d.getByLabel("Branch name").fill("main");
  await expect(d.getByText("A branch named “main” already exists.")).toBeVisible();
  await expect(d.getByRole("button", { name: "Create branch" })).toBeDisabled();
  await d.getByLabel("Branch name").fill("from-stash");
  await d.getByRole("button", { name: "Create branch" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(grid(page).locator('[data-ref="from-stash"]')).toHaveClass(/\bcurrent\b/);
  expect(gg.git("small", "symbolic-ref", "--short", "HEAD").trim()).toBe("from-stash");
  expect(stashList(gg)).toBe("");
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");
});

test("drop_needs_confirmation", async ({ gg }) => {
  const { page } = gg;
  const dir = change(gg);
  gg.git("small", "stash", "push", "-q", "--message=Throwaway");
  await page.goto("/");
  await open(page, dir);
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();

  // Cancelled: nothing happens.
  await pickOnStash(page, 0, "Drop…");
  let confirm = page.getByRole("alertdialog");
  await expect(confirm).toContainText("Throwaway");
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(confirm).toHaveCount(0);
  expect(stashList(gg)).toBe("On main: Throwaway");

  // Confirmed: gone.
  await pickOnStash(page, 0, "Drop…");
  confirm = page.getByRole("alertdialog");
  await expect(confirm).toContainText("Throwaway");
  await armed(page);
  await confirm.getByRole("button", { name: "Drop stash" }).click();
  await expect(confirm).toHaveCount(0);
  await expect(notice(page, /Dropped/)).toBeVisible();
  await expect(grid(page).locator('[data-ref="stash@{0}"]')).toHaveCount(0);
  expect(stashList(gg)).toBe("");
});

test("stash_untracked_option: untracked files go into the stash only when asked", async ({ gg }) => {
  const { page } = gg;
  const dir = change(gg, true);
  await page.goto("/");
  await open(page, dir);

  await stashChanges(page, "Tracked only");
  await expect(row(page, 1).locator('[data-ref="stash@{0}"]')).toBeVisible();
  // The untracked file stayed: the uncommitted row is still there, for it alone.
  await expect(subject(page, 0)).toHaveText(/^Uncommitted changes/);
  expect(existsSync(path.join(dir, "scratch.txt"))).toBe(true);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("?? scratch.txt");

  await stashChanges(page, "With untracked", true);
  // Newest on top, even saved in the same second as the first (R33).
  await expect(row(page, 0).locator('[data-ref="stash@{0}"]')).toBeVisible();
  await expect(subject(page, 0)).toHaveText(/With untracked/);
  await expect(row(page, 1).locator('[data-ref="stash@{1}"]')).toBeVisible();
  await expect(subject(page, 1)).toHaveText(/Tracked only/);
  expect(existsSync(path.join(dir, "scratch.txt"))).toBe(false);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");
  expect(gg.git("small", "show", "--name-only", "--format=", "stash@{0}^3").trim()).toBe("scratch.txt");
});

// A stale index.lock (M5), here in the way of applying a stash: the failure names it, and
// "Remove the lock file…" removes it after a red confirmation; a fresh one is not offered.
test("a stale index.lock is named, and removed only through a red confirmation", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("small");
  const { utimesSync } = await import("node:fs");
  const lock = path.join(dir, ".git", "index.lock");
  writeFileSync(lock, "");
  const old = new Date(Date.now() - 2 * 3600_000);
  utimesSync(lock, old, old);
  await page.goto("/");
  await open(page, dir);
  await row(page, 1).click({ button: "right" });
  await item(page, "Check out this commit").click();
  const failed = page.getByRole("alert").filter({ hasText: "index.lock" });
  await expect(failed).toContainText("made 2 h ago");
  await failed.getByRole("button", { name: "Remove the lock file…" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm.getByTestId("dangers")).toContainText("another git process or an editor");
  expect(existsSync(lock)).toBe(true);
  await armed(page);
  await confirm.getByRole("button", { name: "Remove lock file" }).click();
  await expect(notice(page, "Removed the lock file")).toBeVisible();
  expect(existsSync(lock)).toBe(false);
  expect(existsSync(path.join(dir, ".git", "index"))).toBe(true);

  // A fresh one: named, never offered.
  writeFileSync(lock, "");
  await row(page, 1).click({ button: "right" });
  await item(page, "Check out this commit").click();
  const fresh = page.getByRole("alert").filter({ hasText: "Another git process is working" });
  await expect(fresh).toBeVisible();
  await expect(fresh.getByRole("button", { name: "Remove the lock file…" })).toHaveCount(0);
  expect(existsSync(lock)).toBe(true);
});
