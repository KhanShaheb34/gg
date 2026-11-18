// The settings sheet and the keyboard shortcuts sheet, end to end against the real
// core: every field saves as it changes and survives a relaunch, the core's refusals show
// inline without losing the text, Test and the links are recorded launches (nothing is
// started), and the keys the sheet lists do what it says.
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const sheet = (page: Page) => page.getByTestId("settings");
const stored = (gg: GG) => {
  const file = path.join(gg.home, "config", "state.json");
  return existsSync(file) ? (JSON.parse(readFileSync(file, "utf8")) as { settings?: Record<string, unknown> }).settings ?? {} : {};
};

test("settings open from the keys and the gear, save each field as it changes, and survive a relaunch", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));

  await page.keyboard.press("ControlOrMeta+,");
  await expect(sheet(page)).toBeVisible();
  await expect(sheet(page).getByRole("heading", { name: "Settings" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);
  await page.getByTestId("open-settings").click();
  await expect(sheet(page)).toBeVisible();

  // Theme: the window follows at once.
  await sheet(page).getByTestId("settings-theme").getByRole("radio", { name: "Dark" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  // Dates: the example and the graph change at once; saved.
  const date = grid(page).locator('[data-index="0"] .date');
  await expect(date).toHaveText(/^\d{1,2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}$/);
  await sheet(page).getByTestId("settings-dates").getByRole("radio", { name: "ISO 8601" }).click();
  await expect(sheet(page).getByTestId("settings-date-example")).toContainText(/\d{4}-\d{2}-\d{2} \d{2}:\d{2}/);
  await expect(date).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  await expect.poll(() => stored(gg).dateFormat).toBe("iso");

  // Background fetch.
  await sheet(page).getByTestId("settings-fetch").getByRole("radio", { name: "15 min" }).click();
  await expect.poll(() => stored(gg).fetchIntervalSecs).toBe(900);

  // The editor: refused as typed, the text kept, nothing saved; then a good one, saved.
  const editor = sheet(page).getByTestId("settings-editor");
  await editor.fill("code --goto={path}");
  await expect(sheet(page).getByRole("alert").filter({ hasText: "Write {path} as a separate word." })).toBeVisible();
  await expect(editor).toHaveValue("code --goto={path}");
  await expect(editor).toHaveAttribute("aria-invalid", "true");
  expect(stored(gg).editorCommand ?? null).toBeNull();
  await editor.fill("code -w");
  await expect(sheet(page).getByRole("status").filter({ hasText: /^Saved$/ }).first()).toBeVisible();
  await expect.poll(() => stored(gg).editorCommand).toBe("code -w");
  // What runs, on the open repository's folder.
  await expect(sheet(page).getByTestId("settings-editor-runs")).toHaveText(/code -w ~\/repos\/small/);

  // The worktree location: refused inside the repository, then saved, with its preview.
  const where = sheet(page).getByTestId("settings-pattern");
  await where.fill("wt/{branch}");
  await expect(sheet(page).getByRole("alert").filter({ hasText: "outside the repository" })).toBeVisible();
  await where.fill("../{repo}-trees/{branch}");
  await expect(sheet(page).getByTestId("settings-pattern-preview")).toContainText("feature/login");
  await expect(sheet(page).getByTestId("settings-pattern-preview")).toContainText("~/repos/small-trees/feature-login");
  await expect.poll(() => stored(gg).worktreePattern).toBe("../{repo}-trees/{branch}");

  // The settings file, where it is.
  await expect(sheet(page).getByTestId("settings-file")).toHaveText("~/config/state.json");
  await expect(sheet(page).getByTestId("settings-version")).toHaveText(/^Version \d+\.\d+\.\d+/);

  // Quit and start again: all of it is there.
  await gg.relaunch();
  await expect(grid(page).locator('[data-index="0"] .date')).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.keyboard.press("ControlOrMeta+,");
  await expect(sheet(page).getByTestId("settings-dates").getByRole("radio", { name: "ISO 8601" })).toHaveAttribute("aria-checked", "true");
  await expect(sheet(page).getByTestId("settings-fetch").getByRole("radio", { name: "15 min" })).toHaveAttribute("aria-checked", "true");
  await expect(sheet(page).getByTestId("settings-editor")).toHaveValue("code -w");
  await expect(sheet(page).getByTestId("settings-pattern")).toHaveValue("../{repo}-trees/{branch}");

  // Reset to defaults, confirmed: back as a new install has them; the tabs stay.
  await sheet(page).getByTestId("settings-reset").click();
  const confirm = page.getByRole("dialog", { name: "Reset all settings?" });
  await confirm.getByRole("button", { name: "Reset settings" }).click();
  await expect(sheet(page).getByTestId("settings-editor")).toHaveValue("");
  await expect(sheet(page).getByTestId("settings-pattern")).toHaveValue("../{repo}.worktrees/{branch}");
  await expect(sheet(page).getByTestId("settings-dates").getByRole("radio", { name: "Standard" })).toHaveAttribute("aria-checked", "true");
  await expect.poll(() => stored(gg)).toEqual({
    theme: "system",
    fetchIntervalSecs: 300,
    editorCommand: null,
    terminalCommand: null,
    worktreePattern: "../{repo}.worktrees/{branch}",
    dateFormat: "standard",
  });
  await expect(page.getByRole("tab", { name: "small" })).toBeVisible();
});

test("Test launches the commands on the open worktree, and the links and the settings file open; all recorded, nothing started", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  // Without a repository, there is nothing to try a command on.
  await page.keyboard.press("ControlOrMeta+,");
  await expect(sheet(page).getByTestId("settings-terminal-test")).toBeDisabled();
  await page.keyboard.press("Escape");

  const dir = gg.repoPath("small");
  await open(page, dir);
  await page.keyboard.press("ControlOrMeta+,");
  await sheet(page).getByTestId("settings-terminal").fill("kitty --directory {path}");
  await sheet(page).getByTestId("settings-terminal-test").click();
  await expect.poll(() => gg.launches().some((l) => l.join(" ") === `kitty --directory ${dir}`)).toBe(true);
  await expect(sheet(page).getByTestId("settings-terminal-tested")).toContainText("Started");
  // A command the core refuses is not tried.
  await sheet(page).getByTestId("settings-terminal").fill("bash -lc {path}");
  await expect(sheet(page).getByRole("alert").filter({ hasText: "cannot follow -lc" })).toBeVisible();
  await expect(sheet(page).getByTestId("settings-terminal-test")).toBeDisabled();

  const before = gg.launches().length;
  await sheet(page).getByRole("button", { name: "gg.shakirul.com" }).click();
  await sheet(page).getByRole("button", { name: "Source code" }).click();
  await expect.poll(() => gg.launches().length).toBe(before + 2);
  const opened = gg.launches().slice(before).map((l) => l.at(-1));
  expect(opened).toEqual(["https://gg.shakirul.com", "https://github.com/KhanShaheb34/gg"]);
  await sheet(page).getByRole("button", { name: /Reveal in Finder|Show in File Explorer|Open containing folder/ }).click();
  await expect.poll(() => gg.launches().length).toBe(before + 3);

  // The gg command: its dialog, from here.
  await sheet(page).getByTestId("settings-cli-install").click();
  await expect(page.getByRole("dialog", { name: "Install the ‘gg’ command" })).toBeVisible();
});

test("the keyboard shortcuts sheet lists every group, opens with ? but not while typing, and the worktree keys switch worktree", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);

  await grid(page).focus();
  await page.keyboard.press("?");
  const shortcuts = page.getByTestId("shortcuts");
  await expect(shortcuts).toBeVisible();
  for (const group of ["Repositories", "Graph", "Actions", "Panels", "Worktrees"]) {
    await expect(shortcuts.getByRole("heading", { name: group })).toBeVisible();
  }
  await expect(shortcuts.getByText("Make worktree 1 to 9 active")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(shortcuts).toHaveCount(0);

  // In the search field, ? is a character.
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("?");
  await expect(shortcuts).toHaveCount(0);
  await page.keyboard.press("Escape");

  // ⌘⌥2 (Ctrl+Shift+2 elsewhere): the second worktree as the panel lists it.
  const second = page.getByRole("grid", { name: "Worktrees" }).locator('[role="row"]').nth(1);
  const target = await second.getAttribute("data-path");
  await grid(page).focus();
  await page.keyboard.press(process.platform === "darwin" ? "Meta+Alt+Digit2" : "Control+Shift+Digit2");
  await expect(page.getByTestId("active-worktree")).toHaveAttribute("title", new RegExp(path.basename(target!)));
  await page.keyboard.press(process.platform === "darwin" ? "Meta+Alt+Digit1" : "Control+Shift+Digit1");
  await expect(page.getByTestId("active-worktree")).toHaveAttribute("title", new RegExp(`${path.basename(repo)}\\b`));
});

// A dialog over the settings sheet (Reset, Install gg) gives the focus
// back to the control that opened it when it closes, and nothing behind the sheet (the tab
// bar, the repository) can be reached with Tab while the sheet is open.
test("a dialog over the settings gives the focus back, and Tab never leaves the sheet", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await page.getByTestId("open-settings").click();
  await expect(sheet(page)).toBeVisible();

  for (const [control, dialog, cancel] of [
    ["settings-reset", "Reset all settings?", "Cancel"],
    ["settings-cli-install", /gg.*command/i, "Cancel"],
  ] as const) {
    const opener = sheet(page).getByTestId(control);
    await opener.click();
    const box = page.getByRole("dialog", { name: dialog });
    await expect(box).toBeVisible();
    await box.getByRole("button", { name: cancel }).click();
    await expect(box).toHaveCount(0);
    await expect(opener).toBeFocused();
  }

  // Around the whole sheet with Tab: never on anything behind it.
  const inSheet = async () => page.evaluate(() => !!document.activeElement?.closest('[data-testid="settings"]'));
  for (let i = 0; i < 60; i++) {
    await page.keyboard.press("Tab");
    expect(await inSheet(), `Tab ${i + 1}`).toBe(true);
  }
  // The window behind takes no input at all while the sheet is open.
  await expect(page.locator('[data-testid="app-root"] > .behind').first()).toHaveAttribute("inert", "");
});

// A Test button that cannot be pressed says why, in words on the
// sheet that the button is described by (not only in a tooltip).
test("a disabled Test button says why, visibly", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await page.getByTestId("open-settings").click();
  for (const kind of ["editor", "terminal"]) {
    const button = sheet(page).getByTestId(`settings-${kind}-test`);
    await expect(button).toBeDisabled();
    const why = sheet(page).getByTestId(`settings-${kind}-test-why`);
    await expect(why).toBeVisible();
    await expect(why).toHaveText("Open a repository to try it on.");
    const described = (await button.getAttribute("aria-describedby")) ?? "";
    expect(described.split(" ")).toContain(await why.getAttribute("id"));
  }
  // A repository open: pressable, and nothing said.
  await page.keyboard.press("Escape");
  await open(page, gg.repoPath("small"));
  await page.getByTestId("open-settings").click();
  await expect(sheet(page).getByTestId("settings-editor-test")).toBeEnabled();
  await expect(sheet(page).getByTestId("settings-editor-test-why")).toHaveCount(0);
  // A command the core refuses: its own reason first, then what it means for Test.
  await sheet(page).getByTestId("settings-editor").fill("code x{path}");
  await sheet(page).getByTestId("settings-editor").blur();
  const note = sheet(page).locator("#settings-editor-note p");
  await expect(note.first()).toHaveAttribute("role", "alert");
  await expect(note.first()).toContainText("{path}");
  await expect(note.nth(1)).toHaveText("It was not saved, so Test waits until it is fixed.");
});
