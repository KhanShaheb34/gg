// Screenshots of Generate with AI in every state, both themes, the side panel at its least
// width (a 900 × 560 window), for the visual check; written to .screens/ (git-ignored). Run on
// demand: GG_SCREENS=1 pnpm e2e ai-screens
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, STATUS_BAR_H, test } from "./fixtures";

const OUT = path.resolve(import.meta.dirname, "..", ".screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
// As much taller as the status bar takes, so the panel has the room it had before it.
const H = 560 + STATUS_BAR_H;
test.use({ viewport: { width: 900, height: H }, deviceScaleFactor: 2 });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
}

/** The side panel and what lies over it (a popover may reach past its left edge). */
async function shoot(page: Page, name: string) {
  await page.mouse.move(5, 300);
  await page.waitForTimeout(200);
  const side = (await page.locator('[data-region="side-panel"]').boundingBox())!;
  const x = Math.max(0, side.x - 60);
  await page.screenshot({ path: path.join(OUT, `ai-${name}.png`), clip: { x, y: 0, width: 900 - x, height: H } });
}

async function setSettings(page: Page, patch: object) {
  await page.evaluate(async (p) => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    app.settings = await api.updateSettings(p);
  }, patch);
}

async function commitPanel(gg: GG, theme: string, seen = true) {
  const { page } = gg;
  await page.emulateMedia({ colorScheme: theme as "light" | "dark" });
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  if (seen) await setSettings(page, { aiNoticeSeen: ["claude", "codex"] });
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByTestId("commit-count")).toHaveText("6 of 6 files ticked");
}

for (const theme of ["light", "dark"] as const) {
  test(`generate with AI: the commit message's states, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    gg.ai.install("claude");
    await commitPanel(gg, theme);
    const button = page.getByTestId("commit-ai");
    await shoot(page, `idle-${theme}`);
    await button.hover();
    await page.waitForTimeout(900);
    await page.screenshot({ path: path.join(OUT, `ai-idle-hover-${theme}.png`), clip: { x: 560, y: 300, width: 340, height: 120 } });

    // Generating.
    gg.ai.reply("claude", 0, "Tidy the notes and the app\n\nThe roadmap moves to notes/roadmap.md, the old\nfile goes, and the app reads the new one.\n", "", 30_000);
    await page.getByTestId("commit-message").fill("wip");
    await button.click();
    await expect(button).toHaveAttribute("data-state", "running");
    await shoot(page, `generating-${theme}`);
    await page.getByTestId("commit-ai-cancel").click();
    await expect(button).toHaveAttribute("data-state", "ready");

    // Filled, with Undo.
    gg.ai.reply("claude", 0, "Tidy the notes and the app\n\nThe roadmap moves to notes/roadmap.md, the old\nfile goes, and the app reads the new one.\n");
    await button.click();
    await expect(page.getByTestId("commit-ai-undo")).toBeVisible();
    await shoot(page, `filled-${theme}`);

    // Failures.
    gg.ai.reply("claude", 1, "Not logged in · Please run /login\n");
    await button.click();
    await expect(page.getByTestId("commit-ai-problem")).toBeVisible();
    await shoot(page, `error-signed-out-${theme}`);
    gg.ai.reply("claude", 1, "You've hit your limit · resets 3pm\n");
    await button.click();
    await expect(page.getByTestId("commit-ai-problem")).toHaveAttribute("data-kind", "limited");
    await shoot(page, `error-limited-${theme}`);

    // Nothing ticked.
    await page.getByTestId("tick-all").click();
    await expect(button).toHaveAttribute("aria-disabled", "true");
    await shoot(page, `nothing-ticked-${theme}`);
  });

  test(`generate with AI: no tool, then the first-use notice, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await commitPanel(gg, theme, false);
    await page.getByTestId("commit-ai").click();
    await expect(page.getByTestId("commit-ai-popover")).toBeVisible();
    await shoot(page, `no-tool-${theme}`);
    gg.ai.install("claude");
    await page.getByTestId("ai-check-again").click();
    await expect(page.getByTestId("ai-notice-continue")).toBeVisible();
    await shoot(page, `first-use-${theme}`);
  });

  test(`generate with AI: the pull request form, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.emulateMedia({ colorScheme: theme });
    gg.gh.reply("pr-list", 0, "[]");
    gg.gh.reply("pr-list-head", 0, "[]");
    gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
    gg.ai.install("claude");
    gg.ai.reply(
      "claude",
      0,
      JSON.stringify({
        title: "Fix a typo in the README",
        body: "## Problem\nThe README's first line misspelt the project's name.\n\n## Solution\nSpell it right.",
        draft: false,
      }),
    );
    await page.goto("/");
    await open(page, gg.repoPath("prs"));
    await setSettings(page, { aiNoticeSeen: ["claude"] });
    await page.getByRole("grid", { name: "Commits" }).locator('[data-ref="fix/typo"]').first().locator(".name").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Create pull request…" }).click();
    const panel = page.getByTestId("pr-panel");
    await expect(panel.getByTestId("pr-title")).not.toHaveValue("");
    await panel.getByTestId("pr-body").fill("Typo.");
    await shoot(page, `pr-idle-${theme}`);
    await panel.getByTestId("pr-ai").click();
    await expect(panel.getByTestId("pr-ai-undo")).toBeVisible();
    await shoot(page, `pr-filled-${theme}`);
  });

  test(`generate with AI: settings, ${theme}`, async ({ gg }) => {
    const { page } = gg;
    await page.setViewportSize({ width: 1280, height: 800 + STATUS_BAR_H });
    await page.emulateMedia({ colorScheme: theme });
    gg.ai.install("claude");
    await page.goto("/");
    await open(page, gg.repoPath("commit"));
    await page.keyboard.press("ControlOrMeta+,");
    await page.getByRole("button", { name: "AI", exact: true }).click();
    await expect(page.getByTestId("settings-ai").locator('[data-tool="claude"]')).toContainText("0.0.1");
    await page.waitForTimeout(600);
    await page.screenshot({ path: path.join(OUT, `ai-settings-${theme}.png`) });
    await page.getByTestId("settings-ai-prompt-commit").locator("summary").click();
    await page.getByTestId("settings-ai-prompt-commit").scrollIntoViewIfNeeded();
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(OUT, `ai-settings-prompt-${theme}.png`) });
  });
}
