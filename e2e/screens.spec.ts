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
