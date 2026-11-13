// Screenshots for the visual check, written to docs/perf/screens/. Run on demand:
//   GG_SCREENS=1 pnpm e2e screens
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, makeRepo, test } from "./fixtures";

const OUT = path.resolve(import.meta.dirname, "..", "docs", "perf", "screens");

test.skip(!process.env.GG_SCREENS, "set GG_SCREENS=1 to take screenshots");
test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });

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
    await page.mouse.move(900, 790);
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
    await page.mouse.move(900, 790);
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, `toolbar-${theme}.png`) });

    // The branch filter open, filtered to "feature", one branch ticked.
    await page.getByRole("button", { name: /^Branches:/ }).click();
    const dialog = page.getByRole("dialog", { name: "Branch filter" });
    await dialog.getByRole("option", { name: "feature/ui", exact: true }).click();
    await expect(grid.locator('[data-index="0"] .subject')).toHaveText("Row hover states");
    await page.keyboard.type("feat");
    await page.waitForTimeout(250);
    await page.screenshot({ path: path.join(OUT, `toolbar-filter-${theme}.png`) });
    await dialog.getByRole("option", { name: "Show All", exact: true }).click();
    await page.keyboard.press("Escape");

    // Search active, a few hits, the second one selected.
    await page.keyboard.press("ControlOrMeta+f");
    await page.keyboard.type("plugin");
    await expect(page.getByTestId("search-count")).toHaveText(/^1 of \d+$/);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("search-count")).toHaveText(/^2 of \d+$/);
    await page.mouse.move(900, 790);
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(OUT, `toolbar-search-${theme}.png`) });
  });
}
