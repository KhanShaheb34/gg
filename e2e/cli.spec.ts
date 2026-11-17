// The `gg` terminal command, installed from the welcome screen into the test's own home
// folder (the devserver looks for it there only, and asks a stand-in shell what `gg` is).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

const dialog = (page: Page) => page.getByRole("dialog", { name: "Install the ‘gg’ command" });

/** Opens the dialog the way the app menu does (a browser page has no app menu). */
async function openFromMenu(page: Page): Promise<void> {
  await page.evaluate(async () => {
    const { openCliDialog } = await import("/src/lib/state/cli.svelte.ts");
    await openCliDialog();
  });
}

test("installs the command from the welcome screen and removes it again", async ({ gg }) => {
  const { page, home } = gg;
  const launcher = path.join(home, "bin", "gg");
  await page.goto("/");
  const hint = page.getByTestId("cli-hint");
  await expect(hint).toContainText("Open a repository from a terminal with gg.");
  await hint.getByRole("button", { name: "Install the command…" }).click();

  const box = dialog(page);
  await expect(box).toBeVisible();
  const choices = box.getByRole("radio");
  await expect(choices).toHaveCount(2);
  await expect(box.getByRole("radio", { name: "~/.local/bin" })).toBeChecked();
  await expect(box.getByTestId("cli-written")).toContainText("~/.local/bin/gg");
  await expect(box.getByTestId("cli-shadowed")).toHaveCount(0);

  await box.getByRole("radio", { name: "~/bin" }).check();
  await expect(box.getByTestId("cli-written")).toContainText("Writes one small script, ~/bin/gg, and changes nothing else.");
  await box.getByRole("button", { name: "Install", exact: true }).click();
  await expect(box).toHaveCount(0);
  await expect(page.getByText("Installed the gg command: wrote ~/bin/gg.")).toBeVisible();
  expect(readFileSync(launcher, "utf8")).toContain("# gg launcher, installed by gg ");
  await expect(hint).toHaveText("gg command installed in ~/bin · Remove");

  await openFromMenu(page);
  await expect(box.getByTestId("cli-installed")).toHaveText("Installed at ~/bin/gg.");
  await expect(box.getByRole("radio", { name: "~/bin" })).toBeChecked();
  await box.getByRole("button", { name: "Remove" }).click();
  await expect(box).toHaveCount(0);
  await expect(page.getByText("Removed the gg command: ~/bin/gg.")).toBeVisible();
  expect(existsSync(launcher)).toBe(false);
  await expect(hint).toContainText("Install the command…");
});

test("the welcome screen removes an installed command (Linux and Windows have no app menu)", async ({ gg }) => {
  const { page, home } = gg;
  await page.goto("/");
  const hint = page.getByTestId("cli-hint");
  await hint.getByRole("button", { name: "Install the command…" }).click();
  await dialog(page).getByRole("button", { name: "Install", exact: true }).click();
  await expect(hint).toHaveText("gg command installed in ~/.local/bin · Remove");
  await hint.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByText("Removed the gg command: ~/.local/bin/gg.")).toBeVisible();
  expect(existsSync(path.join(home, ".local", "bin", "gg"))).toBe(false);
  await expect(hint).toContainText("Open a repository from a terminal with gg.");
  // Still installed when the page loads again.
  await hint.getByRole("button", { name: "Install the command…" }).click();
  await dialog(page).getByRole("button", { name: "Install", exact: true }).click();
  await page.reload();
  await expect(hint).toHaveText("gg command installed in ~/.local/bin · Remove");
});

test("leaves a gg it did not write alone and says what it is", async ({ gg }) => {
  const { page, home } = gg;
  const theirs = path.join(home, "bin", "gg");
  writeFileSync(theirs, "#!/bin/sh\necho theirs\n", { mode: 0o755 });
  await page.goto("/");
  await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
  const box = dialog(page);
  await box.getByRole("radio", { name: "~/bin" }).check();
  await box.getByRole("button", { name: "Install", exact: true }).click();
  await expect(box.getByRole("alert")).toContainText("that gg did not write: a script run by /bin/sh");
  await expect(box).toBeVisible();
  expect(readFileSync(theirs, "utf8")).toBe("#!/bin/sh\necho theirs\n");
  // Another folder still works from the same dialog.
  await box.getByRole("radio", { name: "~/.local/bin" }).check();
  await box.getByRole("button", { name: "Install", exact: true }).click();
  await expect(box).toHaveCount(0);
  expect(existsSync(path.join(home, ".local", "bin", "gg"))).toBe(true);
});

test.describe("with a shell that has its own gg", () => {
  test.use({ ggShellSetup: "alias gg='git log --graph'" });

  test("warns that the shell's gg wins, and installs all the same", async ({ gg }) => {
    const { page, home } = gg;
    await page.goto("/");
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    await expect(box.getByTestId("cli-shadowed")).toHaveText(
      "Your shell already defines gg as an alias for git log --graph; that wins over this command until you remove it.",
    );
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    expect(existsSync(path.join(home, ".local", "bin", "gg"))).toBe(true);
  });
});

test.describe("with many folders on the PATH", () => {
  test.use({ ggCliFolders: 4 });

  test("offers them in a list", async ({ gg }) => {
    const { page, home } = gg;
    await page.goto("/");
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    const list = box.getByRole("combobox", { name: "Folder (on your PATH)" });
    await expect(list).toHaveValue(path.join(home, ".local", "bin"));
    await expect(list.locator("option")).toHaveText(["~/.local/bin", "~/bin", "~/tools/bin1", "~/tools/bin2", "~/tools/bin3", "~/tools/bin4"]);
    await list.selectOption({ label: "~/tools/bin3" });
    await expect(box.getByTestId("cli-written")).toContainText("~/tools/bin3/gg");
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    expect(existsSync(path.join(home, "tools", "bin3", "gg"))).toBe(true);
  });
});
