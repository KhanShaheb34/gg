// The `gg` terminal command, installed from the welcome screen into the test's own home
// folder (the devserver looks for it there only, and asks a stand-in shell what `gg` is).
import { existsSync, lstatSync, mkdirSync, readFileSync, readlinkSync, symlinkSync, writeFileSync } from "node:fs";
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
  await expect(page.getByText("Installed the gg command. Wrote ~/bin/gg.")).toBeVisible();
  expect(readFileSync(launcher, "utf8")).toContain("# gg launcher, installed by gg ");
  await expect(hint).toHaveText("gg command installed in ~/bin · Remove");

  await openFromMenu(page);
  await expect(box.getByTestId("cli-installed")).toHaveText("Installed at ~/bin/gg.");
  await expect(box.getByRole("radio", { name: "~/bin" })).toBeChecked();
  await box.getByRole("button", { name: "Remove" }).click();
  await expect(box).toHaveCount(0);
  await expect(page.getByText("Removed the gg command. ~/bin/gg.")).toBeVisible();
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
  await expect(page.getByText("Removed the gg command. ~/.local/bin/gg.")).toBeVisible();
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

test("a gg command Homebrew linked into the app bundle is shown as Homebrew's and left alone", async ({ gg }) => {
  const { page, home } = gg;
  // As a cask's `binary` makes it: a link in the prefix's bin (a Caskroom beside it) to the
  // launcher inside gg.app. Here the test's home folder stands for the prefix.
  const resources = path.join(home, "Applications", "gg.app", "Contents", "Resources");
  mkdirSync(resources, { recursive: true });
  mkdirSync(path.join(home, "Caskroom", "gg"), { recursive: true });
  const bundled = path.join(resources, "gg");
  writeFileSync(bundled, "#!/bin/sh\n# gg launcher, installed by gg 0.3.1\n", { mode: 0o755 });
  const link = path.join(home, "bin", "gg");
  symlinkSync(bundled, link);

  await page.goto("/");
  const hint = page.getByTestId("cli-hint");
  await expect(hint).toHaveText("gg command installed by Homebrew in ~/bin");
  await expect(hint.getByRole("button")).toHaveCount(0);
  // The app menu's "Install ‘gg’ Command…": nothing to install, only where it is.
  await openFromMenu(page);
  await expect(page.getByText("The gg command is already installed by Homebrew at ~/bin/gg.")).toBeVisible();
  await expect(dialog(page)).toHaveCount(0);
  // Settings: where and whose, and no Install or Remove.
  await page.keyboard.press("ControlOrMeta+,");
  const settings = page.getByTestId("settings");
  await expect(settings.getByTestId("settings-cli-state")).toHaveText("Installed by Homebrew at ~/bin/gg");
  await expect(settings.getByTestId("settings-cli-link")).toHaveText(
    "Homebrew made it, a link to the launcher inside gg.app, and removes it when you uninstall gg with Homebrew; gg leaves it alone.",
  );
  await expect(settings.getByTestId("settings-cli-install")).toHaveCount(0);
  await expect(settings.getByTestId("settings-cli-remove")).toHaveCount(0);
  expect(lstatSync(link).isSymbolicLink()).toBe(true);
  expect(readlinkSync(link)).toBe(bundled);
});

test.describe("with a shell that has its own gg", () => {
  test.use({ ggShellSetup: "alias gg='git log --graph'" });

  test("warns that the shell's gg wins, and installs all the same", async ({ gg }) => {
    const { page, home } = gg;
    await page.goto("/");
    await shellAnswered(page);
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    await expect(box.getByTestId("cli-shadowed")).toHaveText(
      "Your shell already defines gg as an alias for git log --graph; that wins over this command until you remove it.",
    );
    // A shell that is neither zsh nor bash: nothing is offered to turn it off.
    await expect(box.getByTestId("cli-turn-off")).toHaveCount(0);
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    expect(existsSync(path.join(home, ".local", "bin", "gg"))).toBe(true);
  });
});

/** gg's line, as the core writes it after a file that ends with a line break. */
const LINE = "unalias gg 2>/dev/null || :  # added by gg: lets the gg command run; gg Settings > Command line takes it out\n";
/** How long the shell's first answer may take when many tests run at once. */
const SHELL_WAIT = 15_000;

/**
 * Waits until the core has the stand-in shell's answer, then loads the page again, so the
 * page starts from it: an answer that came before the page listened for it (the devserver's
 * events reach only a page already listening) is not waited for in vain.
 */
async function shellAnswered(page: Page): Promise<void> {
  await expect
    .poll(
      () =>
        page.evaluate(async () => {
          const { api } = await import("/src/lib/ipc.ts");
          return (await api.getAppState()).cli.shadowedBy;
        }),
      { timeout: SHELL_WAIT },
    )
    .not.toBeNull();
  await page.reload();
}
/** A start-up file of the test's home folder, with a byte that is not UTF-8. */
const ZSHRC = Buffer.from("export EDITOR=vi\n# caf\xe9\n", "latin1");

test.describe("with zsh and an alias called gg (Oh My Zsh's)", () => {
  // The stand-in zsh makes the alias, then reads ~/.zshrc as zsh would: a line there that
  // turns it off works, and the shell, asked again, says so.
  test.use({
    ggShellName: "zsh",
    ggShellSetup: `alias gg='git gui citool'\nif [ -f "$HOME/.zshrc" ]; then . "$HOME/.zshrc"; fi`,
  });

  test("leaves ~/.zshrc alone unless the box is ticked", async ({ gg }) => {
    const { page, home } = gg;
    const rc = path.join(home, ".zshrc");
    writeFileSync(rc, ZSHRC);
    await page.goto("/");
    await shellAnswered(page);
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    const tick = box.getByRole("checkbox", { name: "Turn off that alias for me" });
    // The shell's answer comes in the background, slower when many tests run at once.
    await expect(tick).toBeVisible({ timeout: SHELL_WAIT });
    await expect(tick).not.toBeChecked();
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    await expect(page.getByText("Installed the gg command. Wrote ~/.local/bin/gg.")).toBeVisible();
    expect(readFileSync(rc)).toEqual(ZSHRC);
    await page.keyboard.press("ControlOrMeta+,");
    const settings = page.getByTestId("settings");
    await expect(settings.getByTestId("settings-cli-line")).toHaveCount(0);
    await expect(settings.getByRole("button", { name: "Turn off that alias for me" })).toBeVisible();
  });

  test("ticked: turns the alias off with one line, which Settings shows and takes out again", async ({ gg }) => {
    const { page, home } = gg;
    const rc = path.join(home, ".zshrc");
    writeFileSync(rc, ZSHRC);
    const withLine = Buffer.concat([ZSHRC, Buffer.from(LINE)]);
    await page.goto("/");
    await shellAnswered(page);
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    await expect(box.getByTestId("cli-shadowed")).toContainText(
      "Your shell already defines gg as an alias for git gui citool; that wins over this command until you remove it.",
      { timeout: SHELL_WAIT },
    );
    await expect(box.getByTestId("cli-turn-off-note")).toHaveText(
      "Adds one line, marked as gg’s, to the end of ~/.zshrc: unalias gg 2>/dev/null || :. Removing the command in gg takes it out again. It takes effect in new terminal windows.",
    );
    await box.getByRole("checkbox", { name: "Turn off that alias for me" }).check();
    await expect(box.getByTestId("cli-written")).toHaveText(
      "Writes one small script, ~/.local/bin/gg, and one line at the end of ~/.zshrc, and changes nothing else. No administrator rights are needed.",
    );
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    const notice = page.locator(".toast").filter({ hasText: "Installed the gg command. Wrote ~/.local/bin/gg." });
    await expect(notice).toContainText(
      "Turned off your shell’s gg alias: added one line to the end of ~/.zshrc. It takes effect in new terminal windows.",
    );
    expect(readFileSync(rc)).toEqual(withLine);

    // Settings: the line, no warning, and a way to take it out.
    await page.keyboard.press("ControlOrMeta+,");
    const settings = page.getByTestId("settings");
    await expect(settings.getByTestId("settings-cli-line")).toHaveText(
      "gg added one line to the end of ~/.zshrc that turns off your shell’s own gg.",
    );
    await expect(settings.getByTestId("settings-cli-shadowed")).toHaveCount(0);
    await expect(settings.getByTestId("settings-cli-turn-off")).toHaveCount(0);
    await settings.getByRole("button", { name: "Take the line out" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Took gg’s line out of ~/.zshrc; new terminal windows no longer run it" })).toBeVisible();
    expect(readFileSync(rc)).toEqual(ZSHRC);
    await expect(settings.getByTestId("settings-cli-line")).toHaveCount(0);
    await expect(settings.getByTestId("settings-cli-shadowed")).toContainText(
      "Your shell already defines gg as an alias for git gui citool",
    );

    // Turned off again from Settings, then gone with the command.
    await settings.getByRole("button", { name: "Turn off that alias for me" }).click();
    await expect(settings.getByTestId("settings-cli-line")).toBeVisible();
    expect(readFileSync(rc)).toEqual(withLine);
    await settings.getByTestId("settings-cli-remove").click();
    const removed = page.locator(".toast").filter({ hasText: "Removed the gg command. ~/.local/bin/gg." });
    await expect(removed).toContainText("Took gg’s line out of ~/.zshrc; new terminal windows no longer run it.");
    expect(readFileSync(rc)).toEqual(ZSHRC);
    await expect(settings.getByTestId("settings-cli-state")).toHaveText("Not installed");
  });

  test("with Homebrew's command, Settings turns the alias off and takes the line out", async ({ gg }) => {
    const { page, home } = gg;
    // Homebrew's link, as in the test above that has no alias.
    const resources = path.join(home, "Applications", "gg.app", "Contents", "Resources");
    mkdirSync(resources, { recursive: true });
    mkdirSync(path.join(home, "Caskroom", "gg"), { recursive: true });
    const bundled = path.join(resources, "gg");
    writeFileSync(bundled, "#!/bin/sh\n# gg launcher, installed by gg 0.3.1\n", { mode: 0o755 });
    symlinkSync(bundled, path.join(home, "bin", "gg"));
    const rc = path.join(home, ".zshrc");
    writeFileSync(rc, ZSHRC);

    await page.goto("/");
    await shellAnswered(page);
    await page.keyboard.press("ControlOrMeta+,");
    const settings = page.getByTestId("settings");
    await expect(settings.getByTestId("settings-cli-state")).toHaveText("Installed by Homebrew at ~/bin/gg");
    await expect(settings.getByTestId("settings-cli-remove")).toHaveCount(0);
    await expect(settings.getByTestId("settings-cli-turn-off-note")).toHaveText(
      "gg can add one line, marked as gg’s, to the end of ~/.zshrc: unalias gg 2>/dev/null || :. It takes effect in new terminal windows.",
      { timeout: SHELL_WAIT },
    );
    await settings.getByRole("button", { name: "Turn off that alias for me" }).click();
    await expect(
      page.getByText("Turned off your shell’s gg alias. Added one line to the end of ~/.zshrc. It takes effect in new terminal windows."),
    ).toBeVisible();
    expect(readFileSync(rc)).toEqual(Buffer.concat([ZSHRC, Buffer.from(LINE)]));
    await settings.getByRole("button", { name: "Take the line out" }).click();
    await expect(settings.getByTestId("settings-cli-line")).toHaveCount(0);
    expect(readFileSync(rc)).toEqual(ZSHRC);
    expect(lstatSync(path.join(home, "bin", "gg")).isSymbolicLink()).toBe(true);
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

test.describe("with zsh and an alias made again after ~/.zshrc (a .zlogin, say)", () => {
  test.use({
    ggShellName: "zsh",
    ggShellSetup: `alias gg='git gui citool'\nif [ -f "$HOME/.zshrc" ]; then . "$HOME/.zshrc"; fi\nalias gg='git gui citool'`,
  });

  test("says the line did not turn it off, rather than that it worked", async ({ gg }) => {
    const { page, home } = gg;
    const rc = path.join(home, ".zshrc");
    writeFileSync(rc, ZSHRC);
    await page.goto("/");
    await shellAnswered(page);
    await page.getByTestId("cli-hint").getByRole("button", { name: "Install the command…" }).click();
    const box = dialog(page);
    await expect(box.getByRole("checkbox", { name: "Turn off that alias for me" })).toBeVisible({ timeout: SHELL_WAIT });
    await box.getByRole("checkbox", { name: "Turn off that alias for me" }).check();
    await box.getByRole("button", { name: "Install", exact: true }).click();
    await expect(box).toHaveCount(0);
    const notice = page.locator(".toast").filter({ hasText: "Installed the gg command. Wrote ~/.local/bin/gg." });
    await expect(notice).toContainText(
      "Added one line to the end of ~/.zshrc, but your shell still defines gg as an alias for git gui citool: something that runs after that line defines it again.",
    );
    expect(readFileSync(rc)).toEqual(Buffer.concat([ZSHRC, Buffer.from(LINE)]));
    await page.keyboard.press("ControlOrMeta+,");
    const settings = page.getByTestId("settings");
    await expect(settings.getByTestId("settings-cli-line")).toHaveText(
      "gg’s line in ~/.zshrc does not turn it off: something that runs after that line defines gg again.",
    );
    await settings.getByRole("button", { name: "Take the line out" }).click();
    await expect(settings.getByTestId("settings-cli-line")).toHaveCount(0);
    expect(readFileSync(rc)).toEqual(ZSHRC);
  });
});
