// Cloning: the start page's Clone repository…, the tab bar's + menu (⌘T), the Clone dialog's
// two ways (a link; your repositories through the fake gh, and its states), where a clone goes
// (Settings → Cloning, kept across a relaunch), progress and Stop, a failure, and a repository
// gg already has. Clones come from local bare repositories (`<home>/remotes/<owner>/<name>.git`,
// the devserver's clone source), never the network; gh is the recording fake.

import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, openFolder, test, type GG } from "./fixtures";

const DEFAULT = process.platform === "darwin" ? "Developer" : "Projects";
const mod = process.platform === "darwin" ? "Meta" : "Control";

/** Makes `<home>/remotes/<owner>/<name>.git`, a bare copy of fixture repository `from`. */
function remote(gg: GG, owner: string, name: string, from = "small"): string {
  const bare = path.join(gg.home, "remotes", owner, `${name}.git`);
  gg.gitAt(gg.home, "init", "-q", "--bare", "-b", "main", bare);
  gg.git(from, "push", "-q", bare, "HEAD:refs/heads/main");
  return bare;
}

/** A remote of 300 files of random text (a pack of about a megabyte), for a clone that takes a while. */
function bigRemote(gg: GG, owner: string, name: string): void {
  const dir = gg.folder(`${name}-src`);
  gg.gitAt(dir, "init", "-q", "-b", "main");
  for (let i = 0; i < 300; i++) writeFileSync(path.join(dir, `f${i}.txt`), randomBytes(1500).toString("base64"));
  gg.gitAt(dir, "add", ".");
  gg.gitAt(dir, "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "-m", "Bulk");
  const bare = path.join(gg.home, "remotes", owner, `${name}.git`);
  gg.gitAt(gg.home, "init", "-q", "--bare", "-b", "main", bare);
  gg.gitAt(dir, "push", "-q", bare, "HEAD:refs/heads/main");
}

function reposAnswer(nodes: object[], total = nodes.length) {
  return JSON.stringify({ data: { viewer: { login: "octo", repositories: { totalCount: total, nodes } } } });
}

const REPOS = [
  { name: "cat", owner: { login: "octo" }, isPrivate: true, isArchived: false, isFork: false, description: "Purrs on command", pushedAt: new Date(Date.now() - 2 * 3600_000).toISOString() },
  { name: "dog", owner: { login: "hub" }, isPrivate: false, isArchived: false, isFork: true, description: "Fetches things", pushedAt: new Date(Date.now() - 3 * 86_400_000).toISOString() },
  { name: "owl", owner: { login: "octo" }, isPrivate: false, isArchived: true, isFork: false, description: null, pushedAt: new Date(Date.now() - 200 * 86_400_000).toISOString() },
];

const dialog = (page: Page) => page.getByRole("dialog", { name: "Clone a repository" });
const cloneButton = (page: Page) => dialog(page).getByRole("button", { name: "Clone", exact: true });

async function openDialog(page: Page) {
  await page.goto("/");
  await page.getByTestId("welcome-clone").click();
  await expect(dialog(page)).toBeVisible();
}

test("the start page shows gg's own icon and offers Clone repository…", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const icon = page.getByTestId("welcome").locator("img.app-icon");
  await expect(icon).toBeVisible();
  // The bundle's artwork itself, drawn from its vector.
  const src = (await icon.getAttribute("src"))!;
  const artwork = readFileSync(path.join(import.meta.dirname, "..", "src-tauri", "icons", "src", "app-icon-flat.svg"), "utf8");
  const shown = src.startsWith("data:") ? Buffer.from(src.split(",")[1], "base64").toString("utf8") : await (await page.request.get(src)).text();
  expect(shown).toBe(artwork);
  expect(await icon.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  const clone = page.getByTestId("welcome-clone");
  await expect(clone).toHaveText(/Clone repository…/);
  await clone.click();
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
  // Its shortcut, and the shortcuts sheet lists it.
  await page.keyboard.press(`${mod}+Shift+O`);
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet).toContainText("Clone a repository");
  await expect(sheet).toContainText("New tab");
});

test("a link: read as it is typed, cloned into the default folder (made for it), opened in a tab and kept in recents", async ({ gg }) => {
  const { page } = gg;
  remote(gg, "octo", "cat");
  await openDialog(page);
  await dialog(page).getByRole("radio", { name: "Link" }).click();
  const link = dialog(page).getByTestId("clone-link");
  await expect(link).toBeFocused();
  await link.fill("https://github.com/octo/cat");
  await expect(dialog(page).getByTestId("clone-link-hint")).toHaveText("octo/cat on github.com");
  await expect(dialog(page).getByTestId("clone-folder")).toHaveValue("cat");
  await expect(dialog(page).getByTestId("clone-parent")).toHaveValue(`~/${DEFAULT}`);
  // ~/Developer is not there yet: the clone makes it.
  await expect(dialog(page).getByTestId("clone-hint")).toHaveText(`→ ~/${DEFAULT}/cat · ~/${DEFAULT} is made for it`);
  expect(existsSync(path.join(gg.home, DEFAULT))).toBe(false);
  await link.press("Enter");
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "cat", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
  expect(gg.gitAt(path.join(gg.home, DEFAULT, "cat"), "rev-parse", "--is-inside-work-tree").trim()).toBe("true");
  // In recents: the + menu lists it, in a tab.
  await page.getByTestId("new-tab").click();
  await expect(page.getByTestId("new-tab-recent")).toContainText("cat");
  await expect(page.getByTestId("new-tab-recent")).toContainText("In a tab");
});

test("hostile or unclonable links are refused before anything runs; a taken folder is said and another name works", async ({ gg }) => {
  const { page } = gg;
  remote(gg, "octo", "cat");
  await openDialog(page);
  await dialog(page).getByRole("radio", { name: "Link" }).click();
  const link = dialog(page).getByTestId("clone-link");
  const hint = dialog(page).getByTestId("clone-link-hint");
  for (const [text, said] of [
    ["ext::sh -c touch% /tmp/x", "no spaces"],
    ["ext::sh", "transport::"],
    ["--upload-pack=touch", "cannot start with -"],
    ["file:///etc", "not from a folder"],
    ["https://octo:secret@github.com/octo/cat.git", "password"],
  ]) {
    await link.fill(text);
    await expect(hint).toContainText(said);
    await expect(cloneButton(page)).toBeDisabled();
  }
  // A folder that is there and not empty.
  mkdirSync(path.join(gg.home, DEFAULT, "cat"), { recursive: true });
  writeFileSync(path.join(gg.home, DEFAULT, "cat", "notes.txt"), "mine");
  await link.fill("git@github.com:octo/cat.git");
  await expect(dialog(page).getByTestId("clone-hint")).toContainText("There is a folder named cat in");
  await expect(dialog(page).getByTestId("clone-hint")).toContainText("not empty");
  await expect(cloneButton(page)).toBeDisabled();
  await dialog(page).getByTestId("clone-folder").fill("../cat");
  await expect(dialog(page).getByTestId("clone-hint")).toContainText("no / or \\");
  await dialog(page).getByTestId("clone-folder").fill("cat-2");
  await expect(dialog(page).getByTestId("clone-hint")).toHaveText(`→ ~/${DEFAULT}/cat-2`);
  await cloneButton(page).click();
  await expect(page.getByRole("tab", { name: "cat-2", exact: true })).toHaveAttribute("aria-selected", "true");
  expect(readdirSync(path.join(gg.home, DEFAULT, "cat"))).toEqual(["notes.txt"]);
});

test("your repositories: listed through gh, filtered and chosen with the keyboard, kept for the session", async ({ gg }) => {
  const { page } = gg;
  remote(gg, "hub", "dog");
  gg.gh.reply("gql-repos", 0, reposAnswer(REPOS, 240));
  await openDialog(page);
  const list = dialog(page).getByTestId("clone-repos");
  await expect(list.getByRole("option")).toHaveCount(3);
  await expect(list.getByRole("option").first()).toContainText("octo/cat");
  await expect(list.getByRole("option").first().getByRole("img", { name: "Private" })).toBeVisible();
  await expect(list.getByRole("option").first()).toContainText("Purrs on command");
  await expect(list.getByRole("option").first()).toContainText("2 h ago");
  await expect(list.getByRole("option").nth(1)).toContainText("fork");
  await expect(list.getByRole("option").nth(2)).toContainText("archived");
  await expect(dialog(page).getByTestId("clone-caption")).toHaveText("Your 3 most recently pushed repositories, of 240. For another, paste its link.");
  const filter = dialog(page).getByTestId("clone-filter");
  await expect(filter).toBeFocused();
  await expect(dialog(page).getByTestId("clone-folder")).toHaveValue("cat");
  await filter.press("ArrowDown");
  await expect(list.getByRole("option").nth(1)).toHaveAttribute("aria-selected", "true");
  await expect(dialog(page).getByTestId("clone-folder")).toHaveValue("dog");
  await filter.fill("fetch");
  await expect(list.getByRole("option")).toHaveCount(1);
  await filter.fill("zebra");
  await expect(dialog(page).getByTestId("clone-none")).toContainText("None of them match ‘zebra’");
  await filter.fill("dog");
  await expect(list.getByRole("option")).toHaveCount(1);
  await filter.press("Enter");
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "dog", exact: true })).toHaveAttribute("aria-selected", "true");
  const asked = gg.gh.calls().filter((c) => c.some((a) => a.includes("GgRepos")));
  expect(asked).toHaveLength(1);
  expect(asked[0]).toEqual(expect.arrayContaining(["--hostname", "github.com", "first=100"]));
  // Open again: the list kept, gh not asked again; the way chosen last.
  await page.keyboard.press(`${mod}+Shift+O`);
  await expect(dialog(page).getByTestId("clone-repos").getByRole("option")).toHaveCount(3);
  expect(gg.gh.calls().filter((c) => c.some((a) => a.includes("GgRepos")))).toHaveLength(1);
  // Reload asks again.
  await dialog(page).getByTestId("clone-reload").click();
  await expect.poll(() => gg.gh.calls().filter((c) => c.some((a) => a.includes("GgRepos"))).length).toBe(2);
});

test("a repository gg already has is switched to or opened, not cloned again", async ({ gg }) => {
  const { page } = gg;
  gg.git("small", "remote", "add", "github", "git@github.com:octo/cat.git");
  gg.git("branchy", "remote", "add", "github", "https://github.com/hub/dog.git");
  gg.gh.reply("gql-repos", 0, reposAnswer(REPOS));
  await page.goto("/");
  // branchy is only a recent one; small is open.
  page.once("dialog", (d) => d.accept(gg.repoPath("branchy")));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: "branchy", exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "branchy", exact: true }).hover();
  await page.keyboard.press(`${mod}+W`);
  await expect(page.getByRole("tab")).toHaveCount(0);
  page.once("dialog", (d) => d.accept(gg.repoPath("small")));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: "small", exact: true })).toHaveAttribute("aria-selected", "true");

  await page.keyboard.press(`${mod}+Shift+O`);
  const list = dialog(page).getByTestId("clone-repos");
  await expect(list.getByRole("option").first().getByTestId("clone-have")).toHaveText("Open");
  await expect(list.getByRole("option").nth(1).getByTestId("clone-have")).toHaveText("Cloned");
  await expect(dialog(page).getByTestId("clone-known")).toContainText("octo/cat is open in gg");
  await expect(dialog(page).getByRole("button", { name: "Switch to it" })).toBeEnabled();
  // The link way finds it too.
  await dialog(page).getByRole("radio", { name: "Link" }).click();
  await dialog(page).getByTestId("clone-link").fill("https://github.com/hub/dog.git");
  await expect(dialog(page).getByTestId("clone-known")).toContainText("hub/dog is one of your recent repositories");
  await dialog(page).getByRole("button", { name: "Open it" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "branchy", exact: true })).toHaveAttribute("aria-selected", "true");
  expect(existsSync(path.join(gg.home, DEFAULT))).toBe(false);
});

test("gh missing, signed out, rate limited, offline: said as the Pull requests page says it, and a link still clones", async ({ gg }) => {
  const { page } = gg;
  remote(gg, "octo", "cat");
  gg.gh.reply("gql-repos", 1, "", "You are not logged into any GitHub hosts. To log in, run: gh auth login");
  await openDialog(page);
  const state = dialog(page).getByTestId("clone-list-state");
  await expect(state).toHaveAttribute("data-state", "signed_out");
  await expect(state).toContainText("gh is not signed in");
  await expect(state).toContainText("A link still clones");
  for (const [said, problem, lead] of [
    ["GraphQL: API rate limit exceeded for user ID 1.", "rate_limited", "GitHub’s rate limit was reached"],
    ["error connecting to api.github.com\ncheck your internet connection", "offline", "GitHub could not be reached"],
  ]) {
    gg.gh.reply("gql-repos", 1, "", said);
    await dialog(page).getByTestId("clone-list-retry").click();
    await expect(state).toHaveAttribute("data-state", problem);
    await expect(state).toContainText(lead);
  }
  gg.gh.uninstall();
  await dialog(page).getByTestId("clone-list-retry").click();
  await expect(state).toHaveAttribute("data-state", "no_gh");
  await expect(state).toContainText("The GitHub CLI (gh) was not found");
  await expect(state.getByRole("button", { name: "Get the GitHub CLI" })).toBeVisible();
  // After Try again, the filter has the focus.
  await expect(dialog(page).getByTestId("clone-filter")).toBeFocused();
  // owner/name without gh: github.com over https, with git alone.
  await state.getByRole("button", { name: "Clone by its link" }).click();
  await dialog(page).getByTestId("clone-link").fill("octo/cat");
  await expect(dialog(page).getByTestId("clone-link-hint")).toContainText("octo/cat on github.com");
  await cloneButton(page).click();
  await expect(page.getByRole("tab", { name: "cat", exact: true })).toHaveAttribute("aria-selected", "true");
});

test("progress as git says it, and Stop takes away what the clone made", async ({ gg }) => {
  const { page } = gg;
  bigRemote(gg, "octo", "big");
  // The source answers a third of its pack, then waits.
  writeFileSync(path.join(gg.home, "tools", "clone-hold"), "300000");
  await openDialog(page);
  await dialog(page).getByRole("radio", { name: "Link" }).click();
  await dialog(page).getByTestId("clone-link").fill("octo/big");
  await expect(dialog(page).getByTestId("clone-hint")).toContainText(`~/${DEFAULT}/big`);
  await cloneButton(page).click();
  const progress = dialog(page).getByTestId("clone-progress");
  await expect(progress).toContainText("Receiving objects");
  await expect(progress).toContainText("%");
  await expect(dialog(page).getByTestId("clone-link")).not.toBeEditable();
  expect(existsSync(path.join(gg.home, DEFAULT, "big", ".git"))).toBe(true);
  await dialog(page).getByRole("button", { name: "Stop" }).click();
  await expect(dialog(page).getByTestId("clone-stopped")).toHaveText("Stopped. What the clone had made is gone.");
  expect(existsSync(path.join(gg.home, DEFAULT))).toBe(false);
  // Everything as it was, for another try.
  await expect(dialog(page).getByTestId("clone-link")).toHaveValue("octo/big");
  await expect(cloneButton(page)).toBeEnabled();
});

test("a failure says what happened and leaves the dialog as it was", async ({ gg }) => {
  const { page } = gg;
  await openDialog(page);
  await dialog(page).getByRole("radio", { name: "Link" }).click();
  await dialog(page).getByTestId("clone-link").fill("https://github.com/octo/nothing.git");
  await cloneButton(page).click();
  const failure = dialog(page).getByTestId("clone-failure");
  await expect(failure).toContainText("No repository at octo/nothing");
  await expect(failure).toContainText("does not exist, or your account cannot see it");
  await expect(failure.getByText("git’s output")).toBeVisible();
  await expect(dialog(page).getByTestId("clone-link")).toHaveValue("https://github.com/octo/nothing.git");
  await expect(dialog(page).getByTestId("clone-folder")).toHaveValue("nothing");
  expect(existsSync(path.join(gg.home, DEFAULT))).toBe(false);
  // The focus stayed in the dialog: Esc closes it.
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
});

test("the + menu: ⌘T, the recent repositories filtered, Open folder…, both ways to clone; Esc gives the focus back", async ({ gg }) => {
  const { page } = gg;
  remote(gg, "octo", "cat");
  await page.goto("/");
  page.once("dialog", (d) => d.accept(gg.repoPath("small")));
  await openFolder(page);
  page.once("dialog", (d) => d.accept(gg.repoPath("branchy")));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: "branchy", exact: true })).toHaveAttribute("aria-selected", "true");
  const tab = page.getByRole("tab", { name: "branchy", exact: true });
  await tab.focus();
  await page.keyboard.press(`${mod}+T`);
  const menu = page.getByTestId("new-tab-menu");
  await expect(menu).toBeVisible();
  await expect(page.getByTestId("new-tab-filter")).toBeFocused();
  await expect(menu.getByRole("option")).toHaveText([/Open folder…/, /branchy/, /small/, /Clone from a link…/, /Clone one of your repositories…/]);
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(tab).toBeFocused();
  // Filtered; ↓ and Enter open a recent one (its tab).
  await page.getByTestId("new-tab").click();
  await page.getByTestId("new-tab-filter").fill("sma");
  await expect(menu.getByRole("option")).toHaveText([/small/, /Clone from a link…/, /Clone one of your repositories…/]);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("tab", { name: "small", exact: true })).toHaveAttribute("aria-selected", "true");
  // A link pasted becomes the clone row's: Enter opens the dialog with it.
  await page.getByTestId("new-tab").click();
  await page.getByTestId("new-tab-filter").fill("git@github.com:octo/cat.git");
  await expect(menu.getByRole("option")).toHaveText([/Clone git@github.com:octo\/cat.git/]);
  await page.keyboard.press("Enter");
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).getByTestId("clone-link")).toHaveValue("git@github.com:octo/cat.git");
  await expect(dialog(page).getByTestId("clone-folder")).toHaveValue("cat");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("new-tab")).toBeFocused();
  // Clone one of your repositories…
  gg.gh.reply("gql-repos", 0, reposAnswer(REPOS));
  await page.getByTestId("new-tab").click();
  await page.getByTestId("new-tab-mine").click();
  await expect(dialog(page).getByTestId("clone-repos").getByRole("option")).toHaveCount(3);
  await page.keyboard.press("Escape");
  // ⌘O still goes straight to the folder picker.
  page.once("dialog", (d) => d.dismiss());
  await page.keyboard.press(`${mod}+O`);
  await expect(page.getByTestId("new-tab-menu")).toHaveCount(0);
});

test("Settings → Cloning: the default location, chosen, kept across a relaunch, used by the dialog, and reset", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await page.getByTestId("open-settings").click();
  const field = page.getByTestId("settings-clone-dir");
  await expect(field).toHaveValue(`~/${DEFAULT}`);
  await expect(page.getByTestId("settings-clone-dir-note")).toHaveText(`The default: ${DEFAULT} in your home folder.`);
  const code = path.join(gg.home, "code");
  page.once("dialog", (d) => {
    expect(d.message()).toBe("Choose where clones go");
    void d.accept(code);
  });
  await page.getByTestId("settings-clone-dir-choose").click();
  await expect(field).toHaveValue("~/code");
  await expect(page.getByTestId("settings-clone-dir-reset")).toHaveText(`Use the default, ~/${DEFAULT}`);
  // A relative folder is refused, and not kept.
  await field.fill("code");
  await field.blur();
  await expect(page.getByTestId("settings").getByRole("alert")).toContainText("full path");
  await field.fill("~/code");
  await field.blur();
  await expect(page.getByTestId("settings-clone-dir-reset")).toBeVisible();
  // Nothing was made for it.
  expect(existsSync(code)).toBe(false);

  await gg.relaunch();
  await page.getByTestId("welcome-clone").click();
  await expect(dialog(page).getByTestId("clone-parent")).toHaveValue("~/code");
  await page.keyboard.press("Escape");
  await page.getByTestId("open-settings").click();
  await expect(page.getByTestId("settings-clone-dir")).toHaveValue("~/code");
  await page.getByTestId("settings-clone-dir-reset").click();
  await expect(page.getByTestId("settings-clone-dir")).toHaveValue(`~/${DEFAULT}`);
  await expect(page.getByTestId("settings-clone-dir-note")).toHaveText(`The default: ${DEFAULT} in your home folder.`);
});
