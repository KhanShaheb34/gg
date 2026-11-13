import type { Page } from "@playwright/test";
import { closeButton, expect, test } from "./fixtures";

/** Opens `path` through the "Open repository" button, answering the browser's path prompt. */
async function openThroughPicker(page: Page, path: string) {
  page.once("dialog", (dialog) => dialog.accept(path));
  await page.getByRole("button", { name: "Open repository" }).first().click();
}

const tabs = (page: Page) => page.getByRole("tablist", { name: "Open repositories" }).getByRole("tab");

test("shows the welcome screen when nothing is open", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await expect(page.getByTestId("welcome")).toBeVisible();
  await expect(page.getByTestId("welcome").getByRole("button", { name: "Open repository" })).toBeVisible();
  await expect(tabs(page)).toHaveCount(0);
  await expect(page.getByTestId("repo-view")).toHaveCount(0);
});

test("opens a repository, restores it on reload and keeps it in recents", async ({ gg }) => {
  const { page } = gg;
  const small = gg.repoPath("small");
  await page.goto("/");

  await openThroughPicker(page, small);
  await expect(tabs(page)).toHaveText(["small"]);
  await expect(tabs(page).first()).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("repo-view")).toBeVisible();
  await expect(page.getByTestId("welcome")).toHaveCount(0);

  await page.reload();
  await expect(tabs(page)).toHaveText(["small"]);
  await expect(page.getByTestId("repo-view")).toBeVisible();

  await closeButton(page, "small").click();
  await expect(tabs(page)).toHaveCount(0);
  const recents = page.getByRole("list", { name: "Recent repositories" });
  await expect(recents.getByRole("listitem")).toHaveCount(1);
  await expect(recents).toContainText("small");

  await recents.getByRole("button", { name: /^Open small/ }).click();
  await expect(tabs(page)).toHaveText(["small"]);
  await expect(page.getByTestId("repo-view")).toBeVisible();

  await closeButton(page, "small").click();
  await recents.getByRole("button", { name: "Remove small from recents" }).click();
  await expect(page.getByRole("list", { name: "Recent repositories" })).toHaveCount(0);
  await page.reload();
  await expect(page.getByTestId("welcome")).toBeVisible();
  await expect(page.getByRole("list", { name: "Recent repositories" })).toHaveCount(0);
});

test("opening the same repository twice keeps one tab", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await openThroughPicker(page, gg.repoPath("small"));
  await expect(tabs(page)).toHaveCount(1);
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath("small")));
  await page.getByRole("button", { name: "Open repository" }).click();
  await expect(tabs(page)).toHaveCount(1);
});

test("an empty repository opens as a tab", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await openThroughPicker(page, gg.repoPath("empty"));
  await expect(tabs(page)).toHaveText(["empty"]);
  await expect(page.getByTestId("repo-view")).toBeVisible();
});

test("opening a folder that is not a repository shows an error and no tab", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await openThroughPicker(page, gg.folder("plain"));
  await expect(page.getByRole("alert")).toContainText("not a git repository");
  // The core's text is isolated, so a path cannot reorder the rest of the toast.
  await expect(page.getByRole("alert").locator(".message > bdi")).toContainText("not a git repository");
  await expect(tabs(page)).toHaveCount(0);
  await expect(page.getByTestId("welcome")).toBeVisible();
});

test("keyboard: arrows move between tabs, the shortcut closes one", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await openThroughPicker(page, gg.repoPath("small"));
  await expect(tabs(page)).toHaveCount(1);
  page.once("dialog", (dialog) => dialog.accept(gg.repoPath("empty")));
  await page.getByRole("button", { name: "Open repository" }).click();
  await expect(tabs(page)).toHaveText(["small", "empty"]);
  await expect(tabs(page).nth(1)).toHaveAttribute("aria-selected", "true");

  await tabs(page).nth(1).focus();
  await page.keyboard.press("ArrowLeft");
  await expect(tabs(page).nth(0)).toHaveAttribute("aria-selected", "true");
  await expect(tabs(page).nth(0)).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(tabs(page).nth(1)).toHaveAttribute("aria-selected", "true");

  // Ctrl+Tab from a focused tab takes the focus along to the tab it selects.
  await tabs(page).nth(1).focus();
  await page.keyboard.press("Control+Tab");
  await expect(tabs(page).nth(0)).toHaveAttribute("aria-selected", "true");
  await expect(tabs(page).nth(0)).toBeFocused();

  // Cmd+9 is the last tab.
  await page.keyboard.press("ControlOrMeta+9");
  await expect(tabs(page).nth(1)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ControlOrMeta+1");
  await expect(tabs(page).nth(0)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ControlOrMeta+w");
  await expect(tabs(page)).toHaveText(["empty"]);
});

test("the tab list holds only tabs: close buttons are left to the mouse and the shortcut", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await openThroughPicker(page, gg.repoPath("small"));
  const list = page.getByRole("tablist", { name: "Open repositories" });
  const snapshot = await list.ariaSnapshot();
  expect(snapshot).not.toContain("button");
  expect(snapshot).toContain('tab "small"');
  // Still there for the mouse.
  await page.getByRole("tab", { name: "small" }).hover();
  await expect(closeButton(page, "small")).toBeVisible();
  // Delete closes the focused tab, as the ARIA tabs pattern has it, and the tab says so.
  await expect(page.getByRole("tab", { name: "small" })).toHaveAttribute("aria-keyshortcuts", "Delete");
  await page.getByRole("tab", { name: "small" }).focus();
  await page.keyboard.press("Delete");
  await expect(tabs(page)).toHaveCount(0);
});

test("the theme toggle switches and persists the theme; system follows the OS", async ({ gg }) => {
  const { page } = gg;
  const html = page.locator("html");
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  const toggle = page.getByRole("button", { name: /^Theme:/ });

  await expect(toggle).toHaveAccessibleName("Theme: System");
  await expect(html).toHaveAttribute("data-theme", "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(html).toHaveAttribute("data-theme", "dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect(html).toHaveAttribute("data-theme", "light");

  await toggle.click();
  await expect(toggle).toHaveAccessibleName("Theme: Light");
  await toggle.click();
  await expect(toggle).toHaveAccessibleName("Theme: Dark");
  await expect(html).toHaveAttribute("data-theme", "dark");
  await expect(html).toHaveCSS("color-scheme", "dark");

  await page.reload();
  await expect(html).toHaveAttribute("data-theme", "dark");
  await expect(toggle).toHaveAccessibleName("Theme: Dark");

  await toggle.click();
  await expect(toggle).toHaveAccessibleName("Theme: System");
  await expect(html).toHaveAttribute("data-theme", "light");
  await expect(html).toHaveCSS("color-scheme", "light");
});
