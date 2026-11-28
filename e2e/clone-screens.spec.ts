// Screenshots of cloning for the visual check, at 2x, in both themes, written to .screens/
// (git-ignored), or GG_SCREENS_DIR: the start page with gg's icon, the Clone dialog's two ways,
// a clone in progress, a failure, the + menu and Settings → Cloning. Run on demand:
//   GG_SCREENS=1 pnpm e2e clone-screens
import { randomBytes } from "node:crypto";
import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, openFolder, test } from "./fixtures";

const OUT = process.env.GG_SCREENS_DIR ?? path.resolve(import.meta.dirname, "..", ".screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1100, height: 720 }, deviceScaleFactor: 2 });
test.setTimeout(120_000);

async function shoot(page: Page, name: string, theme: string) {
  await page.mouse.move(1090, 710);
  await page.waitForTimeout(300);
  await page.screenshot({ path: path.join(OUT, `clone-${name}-${theme}.png`), animations: "disabled" });
}

const day = 86_400_000;
const REPOS = [
  ["octo", "cat", true, false, "Purrs on command", 2 * 3600_000],
  ["octo", "website", false, false, "The site, its pages and its pictures", 20 * 3600_000],
  ["hub", "dog", false, true, "Fetches things", 3 * day],
  ["octo", "dotfiles", true, false, null, 9 * day],
  ["team-a", "payments-api", true, false, "Takes payments and sends receipts", 15 * day],
  ["team-a", "design-system", false, false, "Buttons, fields and the colours they share", 40 * day],
  ["octo", "owl", false, false, "Archived notes", 400 * day],
] as const;

function reply(gg: GG) {
  const nodes = REPOS.map(([owner, name, isPrivate, isFork, description, ago]) => ({
    name,
    owner: { login: owner },
    isPrivate,
    isArchived: name === "owl",
    isFork,
    description,
    pushedAt: new Date(Date.now() - ago).toISOString(),
  }));
  gg.gh.reply("gql-repos", 0, JSON.stringify({ data: { viewer: { login: "octo", repositories: { totalCount: 132, nodes } } } }));
}

for (const theme of ["light", "dark"] as const) {
  test(`cloning, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    reply(gg);
    // A remote that takes a while: held after part of its pack.
    const src = gg.folder("big-src");
    gg.gitAt(src, "init", "-q", "-b", "main");
    for (let i = 0; i < 300; i++) writeFileSync(path.join(src, `f${i}.txt`), randomBytes(1500).toString("base64"));
    gg.gitAt(src, "add", ".");
    gg.gitAt(src, "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "-m", "Bulk");
    const bare = path.join(gg.home, "remotes", "octo", "website.git");
    gg.gitAt(gg.home, "init", "-q", "--bare", "-b", "main", bare);
    gg.gitAt(src, "push", "-q", bare, "HEAD:refs/heads/main");

    await page.goto("/");
    // Recent repositories for the start page and the + menu.
    for (const name of ["branchy", "small"]) {
      page.once("dialog", (d) => d.accept(gg.repoPath(name)));
      await openFolder(page);
      await expect(page.getByRole("tab", { name, exact: true })).toBeVisible();
    }
    gg.git("small", "remote", "add", "github", "git@github.com:octo/cat.git");
    await page.keyboard.press("ControlOrMeta+w");
    await page.keyboard.press("ControlOrMeta+w");
    await expect(page.getByTestId("welcome")).toBeVisible();
    await shoot(page, "start", theme);

    const dialog = page.getByRole("dialog", { name: "Clone a repository" });
    await page.getByTestId("welcome-clone").click();
    await expect(dialog.getByTestId("clone-repos").getByRole("option")).toHaveCount(REPOS.length);
    await dialog.getByTestId("clone-filter").press("ArrowDown");
    await expect(dialog.getByTestId("clone-hint")).toContainText("website");
    await shoot(page, "yours", theme);
    await dialog.getByTestId("clone-filter").press("ArrowUp");
    await expect(dialog.getByTestId("clone-known")).toBeVisible();
    await shoot(page, "yours-known", theme);

    await dialog.getByRole("radio", { name: "Link" }).click();
    await dialog.getByTestId("clone-link").fill("https://github.com/octo/website");
    await expect(dialog.getByTestId("clone-hint")).toContainText("website");
    await shoot(page, "link", theme);

    writeFileSync(path.join(gg.home, "tools", "clone-hold"), "330000");
    await dialog.getByRole("button", { name: "Clone", exact: true }).click();
    await expect(dialog.getByTestId("clone-progress")).toContainText("%");
    await page.waitForTimeout(600);
    await shoot(page, "progress", theme);
    await dialog.getByRole("button", { name: "Stop" }).click();
    await expect(dialog.getByTestId("clone-stopped")).toBeVisible();
    rmSync(path.join(gg.home, "tools", "clone-hold"));

    await dialog.getByTestId("clone-link").fill("https://github.com/octo/nothing.git");
    await expect(dialog.getByTestId("clone-hint")).toContainText("nothing");
    await dialog.getByRole("button", { name: "Clone", exact: true }).click();
    await expect(dialog.getByTestId("clone-failure")).toBeVisible();
    await shoot(page, "failure", theme);
    await page.keyboard.press("Escape");

    gg.gh.reply("gql-repos", 1, "", "You are not logged into any GitHub hosts. To log in, run: gh auth login");
    await page.getByTestId("welcome-clone").click();
    await dialog.getByRole("radio", { name: "Your repositories" }).click();
    await dialog.getByTestId("clone-reload").click();
    await expect(dialog.getByTestId("clone-list-state")).toBeVisible();
    await shoot(page, "signed-out", theme);
    await page.keyboard.press("Escape");

    page.once("dialog", (d) => d.accept(gg.repoPath("branchy")));
    await openFolder(page);
    await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
    await page.getByTestId("new-tab").click();
    await expect(page.getByTestId("new-tab-menu")).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `clone-new-tab-${theme}.png`), animations: "disabled" });
    await page.getByTestId("new-tab-filter").fill("git@github.com:octo/website.git");
    await page.screenshot({ path: path.join(OUT, `clone-new-tab-link-${theme}.png`), animations: "disabled" });
    await page.keyboard.press("Escape");

    await page.getByTestId("open-settings").click();
    await page.getByRole("button", { name: "Cloning" }).click();
    await page.waitForTimeout(500);
    await shoot(page, "settings", theme);
  });
}
