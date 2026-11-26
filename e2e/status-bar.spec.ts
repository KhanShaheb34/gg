// The header holds actions and the status bar along the window's foot holds information: the
// active worktree's folder (copied or shown in the file manager from its menu) and when the
// repository was last fetched; View › Status bar hides it, remembered across a reload.
import { existsSync, utimesSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, makeRepo, test, viewPopover } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
}

const bar = (page: Page) => page.getByTestId("status-bar");
const pathButton = (page: Page) => page.getByTestId("status-path");
const fetched = (page: Page) => page.getByTestId("status-fetched");
const menu = (page: Page) => page.getByTestId("context-menu");
const statusBarBox = (page: Page) => viewPopover(page).getByRole("checkbox", { name: "Status bar", exact: true });

test("the header names neither the repository nor its folder; the status bar shows the folder", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  // No repository open: no status bar.
  await expect(page.getByTestId("welcome")).toBeVisible();
  await expect(bar(page)).toHaveCount(0);

  const small = gg.repoPath("small");
  await open(page, small);
  const head = page.locator(".repo-head");
  await expect(head.getByTestId("active-worktree")).toContainText("main");
  await expect(head).not.toContainText("small");
  await expect(head).not.toContainText("~/repos");
  await expect(head.getByRole("heading")).toHaveCount(0);

  // The bar runs the window's width, under the graph and the side panel, at its foot.
  await expect(bar(page)).toBeVisible();
  const box = (await bar(page).boundingBox())!;
  const viewport = page.viewportSize()!;
  expect(box.x).toBe(0);
  expect(box.width).toBe(viewport.width);
  expect(box.y + box.height).toBe(viewport.height);
  const side = (await page.locator('[data-region="side-panel"]').boundingBox())!;
  expect(side.y + side.height).toBeLessThanOrEqual(box.y);

  await expect(pathButton(page)).toHaveText("~/repos/small");
  await expect(pathButton(page)).toHaveAttribute("title", small);
});

test("the path's menu copies the path and shows the folder in the file manager", async ({ gg, context }) => {
  const { page } = gg;
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  const small = gg.repoPath("small");
  await open(page, small);

  await pathButton(page).click();
  await expect(menu(page).getByRole("menuitem")).toHaveText([/^Copy path\s*$/, /^(Reveal in Finder|Show in File Explorer|Open containing folder)\s*$/]);
  await expect(pathButton(page)).toHaveAttribute("aria-expanded", "true");
  // It opens upward, above the bar.
  expect((await menu(page).boundingBox())!.y + (await menu(page).boundingBox())!.height).toBeLessThanOrEqual(
    (await bar(page).boundingBox())!.y,
  );
  await menu(page).getByRole("menuitem", { name: "Copy path" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Copied the path." })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(small);

  // From the keyboard too; the file manager is asked to show the folder (nothing starts in tests).
  const before = gg.launches().length;
  await pathButton(page).focus();
  await page.keyboard.press("Enter");
  await expect(menu(page)).toBeVisible();
  await menu(page).getByRole("menuitem", { name: /Finder|Explorer|folder/ }).click();
  await expect.poll(() => gg.launches().length).toBe(before + 1);
  const launch = gg.launches().at(-1)!;
  expect(launch.some((arg) => arg === small || arg === path.dirname(small))).toBe(true);
});

/** Fixture `small` fetched two hours ago, as far as its FETCH_HEAD says. */
function fetchedTwoHoursAgo(gg: { repoPath(name: string): string; git(name: string, ...args: string[]): string }) {
  gg.git("small", "fetch", "-q", "origin");
  const head = path.join(gg.repoPath("small"), ".git", "FETCH_HEAD");
  const then = new Date(Date.now() - 2 * 3600_000 - 60_000);
  utimesSync(head, then, then);
}

test("the last fetch shows, gg's own takes its place, and the wording moves on as time passes", async ({ gg }) => {
  const { page } = gg;
  await page.clock.install();
  fetchedTwoHoursAgo(gg);
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(fetched(page)).toHaveText("Fetched 2 h ago");
  await expect(fetched(page)).toHaveAttribute("title", /^Last fetched \w+day, \d+ \w+ \d{4} \d\d:\d\d:\d\d \(UTC[+-]\d\d:\d\d\)$/);

  await page.getByRole("button", { name: "Fetch" }).click();
  await expect(fetched(page)).toHaveText("Fetched just now", { timeout: 10_000 });
  // Time passes: the wording follows without anything else happening.
  await page.clock.fastForward("03:10");
  await expect(fetched(page)).toHaveText("Fetched 3 min ago");
  await page.clock.fastForward("58:00");
  await expect(fetched(page)).toHaveText("Fetched 1 h ago");
});

test("a fetch in a terminal shows as it happens", async ({ gg }) => {
  const { page } = gg;
  fetchedTwoHoursAgo(gg);
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(fetched(page)).toHaveText("Fetched 2 h ago");
  gg.git("small", "fetch", "-q", "origin");
  await expect(fetched(page)).toHaveText("Fetched just now");
});

test("a fresh clone, never fetched since, says nothing about fetching", async ({ gg }) => {
  const { page } = gg;
  const origin = path.join(path.dirname(gg.repoPath("small")), "small-origin.git");
  const clone = makeRepo("fresh", (git) => git("clone", "-q", origin, "."));
  const head = path.join(clone, ".git", "FETCH_HEAD");
  // A clone writes no FETCH_HEAD.
  expect(existsSync(head)).toBe(false);
  await page.goto("/");
  await open(page, clone);
  await expect(pathButton(page)).toBeVisible();
  await expect(fetched(page)).toHaveCount(0);
});

test("View › Status bar hides it, and the choice survives a reload", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await expect(bar(page)).toBeVisible();

  await page.getByTestId("view-options").click();
  await expect(statusBarBox(page)).toBeChecked();
  await statusBarBox(page).click();
  await expect(bar(page)).toHaveCount(0);
  // Not a graph option: the View button does not read as hiding something.
  await expect(page.getByTestId("view-options")).not.toHaveClass(/hiding/);
  await page.keyboard.press("Escape");

  await page.reload();
  await expect(page.getByRole("tab", { name: "small", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("repo-view")).toBeVisible();
  await expect(bar(page)).toHaveCount(0);

  await page.getByTestId("view-options").click();
  await expect(statusBarBox(page)).not.toBeChecked();
  await statusBarBox(page).click();
  await expect(bar(page)).toBeVisible();
  await page.reload();
  await expect(page.getByTestId("repo-view")).toBeVisible();
  await expect(bar(page)).toBeVisible();
});
