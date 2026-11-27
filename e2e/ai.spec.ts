// Generate with AI against the real core and a stand-in for the AI tools (gg-fake-ai in the
// devserver's tool folder; a real one never runs): the commit message filled from the ticked
// files, Undo, Cancel, the shortcut, each failure said at the field with its next step, no tool
// installed (the popover, the install pages, Check again), the first-use notice once, the pull
// request's title and description, nothing to describe, and the settings (tool, model, prompt)
// used and kept.
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

const MOD = process.platform === "darwin" ? "Meta" : "Control";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
}

const message = (page: Page) => page.getByTestId("commit-message");
const aiButton = (page: Page) => page.getByTestId("commit-ai");
const popover = (page: Page) => page.getByTestId("commit-ai-popover");
const problem = (page: Page) => page.getByTestId("commit-ai-problem");

/** The settings as they are saved, through the app's own module. */
function setSettings(page: Page, patch: object) {
  return page.evaluate(async (p) => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    app.settings = await api.updateSettings(p);
  }, patch);
}

/** The commit panel of the `commit` fixture, with `claude` installed and its notice seen. */
async function commitPanel(gg: GG, options: { tool?: boolean; seen?: boolean } = {}) {
  const { page } = gg;
  if (options.tool ?? true) gg.ai.install("claude");
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  if (options.seen ?? true) await setSettings(page, { aiNoticeSeen: ["claude", "codex"] });
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByTestId("commit-panel")).toBeVisible();
  await expect(page.getByTestId("commit-count")).toHaveText("6 of 6 files ticked");
}

test("a commit message is written from the ticked files; Undo puts back what it replaced", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "```\nTidy the notes and the app.\n\nKeeps the roadmap in one place.\n```\n");
  await commitPanel(gg);
  await expect(aiButton(page)).toHaveAttribute("title", /^Generate with Claude Code \(haiku\) \((⌘G|Ctrl\+G)\)$/);
  await message(page).fill("my own words");
  await aiButton(page).click();
  await expect(message(page)).toHaveValue("Tidy the notes and the app\n\nKeeps the roadmap in one place.");
  await expect(message(page)).toBeFocused();
  // Never committed by itself.
  await expect(page.getByTestId("commit-button")).toBeEnabled();
  expect(gg.git("commit", "log", "-1", "--format=%s").trim()).not.toContain("Tidy");
  // What the tool was given: the prompt on stdin, the changes in it, never on its command line.
  const [call] = gg.ai.calls("claude");
  expect(call.argv.slice(0, 5)).toEqual(["-p", "--output-format", "text", "--model", "haiku"]);
  expect(call.stdin).toContain("You are generating a single git commit message.");
  expect(call.stdin).toContain("+++ b/src/app.ts");
  expect(call.argv.join(" ")).not.toContain("src/app.ts");

  // Undo, the link.
  await page.getByTestId("commit-ai-undo").click();
  await expect(message(page)).toHaveValue("my own words");
  await expect(page.getByTestId("commit-ai-undo")).toHaveCount(0);

  // Again, then Undo with the keys in the field.
  await aiButton(page).click();
  await expect(message(page)).toHaveValue(/^Tidy the notes/);
  await message(page).press(`${MOD}+z`);
  await expect(message(page)).toHaveValue("my own words");

  // Once edited, the generated text is the user's: no Undo.
  await message(page).press(`${MOD}+g`);
  await expect(message(page)).toHaveValue(/^Tidy the notes/);
  await message(page).press("End");
  await message(page).pressSequentially("!");
  await expect(page.getByTestId("commit-ai-undo")).toHaveCount(0);
});

test("only the ticked files are sent; with none ticked the button says why and does nothing", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "Update the todo\n");
  await commitPanel(gg);
  await page.getByTestId("tick-all").click();
  await expect(page.getByTestId("commit-count")).toHaveText("0 of 6 files ticked");
  await expect(aiButton(page)).toHaveAttribute("aria-disabled", "true");
  await expect(aiButton(page)).toHaveAttribute("title", "Tick the files to describe.");
  // Playwright waits for an aria-disabled button; a click still reaches it, and does nothing.
  await aiButton(page).click({ force: true });
  await expect(aiButton(page)).toHaveAttribute("data-state", "blocked");
  // One ticked: only it is sent, and this is the tool's only run (the click before ran none).
  await page.getByRole("listbox", { name: "Changed files" }).locator('[data-path="todo.md"]').click();
  await aiButton(page).click();
  await expect(message(page)).toHaveValue("Update the todo");
  expect(gg.ai.calls("claude")).toHaveLength(1);
  const { stdin } = gg.ai.calls("claude")[0];
  expect(stdin).toContain("todo.md");
  expect(stdin).not.toContain("src/app.ts");
});

test("Cancel ends the tool: the field is as it was, unlocked", async ({ gg }) => {
  const { page } = gg;
  // Soon enough that a tool Cancel left running would answer before the test ends.
  gg.ai.reply("claude", 0, "Too late\n", "", 3_000);
  await commitPanel(gg);
  await message(page).fill("draft");
  const before = (await aiButton(page).boundingBox())!;
  await aiButton(page).click();
  await expect(aiButton(page)).toHaveAttribute("data-state", "running");
  // The same size: nothing moves.
  const running = (await aiButton(page).boundingBox())!;
  expect([running.width, running.height, running.y]).toEqual([before.width, before.height, before.y]);
  await expect(message(page)).toHaveAttribute("readonly", "");
  await expect(page.getByTestId("commit-button")).toBeDisabled();
  // No line appears under the buttons meanwhile (it would move the field).
  await expect(page.getByTestId("commit-reason")).toHaveCount(0);
  await expect.poll(() => gg.ai.calls("claude").length).toBe(1);
  await page.getByTestId("commit-ai-cancel").click();
  await expect(aiButton(page)).toHaveAttribute("data-state", "ready");
  await expect(message(page)).toHaveValue("draft");
  await expect(message(page)).not.toHaveAttribute("readonly", "");
  await expect(problem(page)).toHaveCount(0);
  // The tool's process is gone, and it never answered: it was ended, not left to finish.
  expect(await gg.ai.ended("claude", 0)).toEqual({ answered: false });
  await expect(message(page)).toHaveValue("draft");
});

test("Escape in the message cancels too, and leaves the panel open", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "Too late\n", "", 30_000);
  await commitPanel(gg);
  await message(page).focus();
  await message(page).press(`${MOD}+g`);
  await expect(aiButton(page)).toHaveAttribute("data-state", "running");
  await message(page).press("Escape");
  await expect(aiButton(page)).toHaveAttribute("data-state", "ready");
  await expect(page.getByTestId("commit-panel")).toBeVisible();
});

test("failures are said at the message, with the next step", async ({ gg }) => {
  const { page } = gg;
  await commitPanel(gg);

  gg.ai.reply("claude", 1, "Not logged in · Please run /login\n");
  await aiButton(page).click();
  await expect(problem(page)).toHaveAttribute("data-kind", "signed_out");
  await expect(problem(page)).toContainText("Claude Code is not signed in. Run claude in a terminal to sign in, then try again.");
  await expect(problem(page).locator("code")).toHaveText("claude");
  await problem(page).getByRole("button", { name: "Open terminal" }).click();
  await expect.poll(() => gg.launches().length).toBe(1);

  gg.ai.reply("claude", 1, "You've hit your limit · resets 3pm\n");
  await aiButton(page).click();
  await expect(problem(page)).toHaveAttribute("data-kind", "limited");
  await expect(problem(page)).toContainText("Claude Code has reached a limit: You've hit your limit · resets 3pm.");

  gg.ai.reply("claude", 1, "There's an issue with the selected model (haiku).\n");
  await aiButton(page).click();
  await expect(problem(page)).toHaveAttribute("data-kind", "bad_model");
  await expect(problem(page).getByRole("button", { name: "Settings" })).toBeVisible();

  gg.ai.reply("claude", 3, "", "panic: odd\n");
  await aiButton(page).click();
  await expect(problem(page)).toContainText("Claude Code failed: panic: odd.");
  // Show log: the generation's commands, the tool's among them.
  await problem(page).getByRole("button", { name: "Show log" }).click();
  await expect(page.getByTestId("command-log")).toBeVisible();
  await expect(page.getByTestId("command-log")).toContainText("claude");
  // Try again, now answered.
  gg.ai.reply("claude", 0, "Fix it\n");
  await problem(page).getByRole("button", { name: "Try again" }).click();
  await expect(message(page)).toHaveValue("Fix it");
  await expect(problem(page)).toHaveCount(0);

  // Typing clears a failure.
  gg.ai.reply("claude", 0, "   \n");
  await aiButton(page).click();
  await expect(problem(page)).toContainText("Claude Code returned an empty message.");
  await message(page).pressSequentially("x");
  await expect(problem(page)).toHaveCount(0);
});

test("no tool installed: a quiet button whose popover says so, links the install pages, and Check again finds one", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "Add the notes\n");
  await commitPanel(gg, { tool: false, seen: false });
  await expect(aiButton(page)).toHaveAttribute("data-state", "none");
  await expect(aiButton(page)).toHaveAttribute("title", "Generate with AI: no AI tool was found on this computer");
  await aiButton(page).click();
  await expect(popover(page)).toContainText("Generate with AI uses Claude Code or Codex installed on this computer");
  await expect(popover(page)).toContainText("Neither was found.");
  await popover(page).getByTestId("ai-get-claude").click();
  await expect.poll(() => gg.launches().flat().join(" ")).toContain("https://code.claude.com/docs/en/setup");
  await popover(page).getByTestId("ai-get-codex").click();
  await expect.poll(() => gg.launches().flat().join(" ")).toContain("https://developers.openai.com/codex/cli");
  // Escape closes it.
  await page.keyboard.press("Escape");
  await expect(popover(page)).toHaveCount(0);
  await expect(aiButton(page)).toBeFocused();

  // Installed meanwhile: Check again finds it, and the first-use notice follows.
  await aiButton(page).click();
  gg.ai.install("claude");
  await popover(page).getByTestId("ai-check-again").click();
  await expect(popover(page)).toContainText("Generate with Claude Code");
  await expect(page.getByTestId("ai-notice-text")).toHaveText(
    "The ticked changes (their diff and file names) and the branch name go to Anthropic through the Claude Code you installed, under your own account. gg itself sends nothing anywhere.",
  );
  await page.getByTestId("ai-notice-continue").click();
  await expect(message(page)).toHaveValue("Add the notes");
});

test("the first-use notice comes once per tool, and Cancel there sends nothing", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "Add the notes\n");
  await commitPanel(gg, { seen: false });
  await aiButton(page).click();
  await expect(popover(page)).toBeVisible();
  await popover(page).getByRole("button", { name: "Cancel" }).click();
  await expect(popover(page)).toHaveCount(0);
  expect(gg.ai.calls("claude")).toHaveLength(0);
  await aiButton(page).click();
  await page.getByTestId("ai-notice-continue").click();
  await expect(message(page)).toHaveValue("Add the notes");
  // Kept: not asked again, also after a restart.
  await message(page).fill("");
  await aiButton(page).click();
  await expect(message(page)).toHaveValue("Add the notes");
  await expect(popover(page)).toHaveCount(0);
  await gg.relaunch();
  await expect(page.getByRole("tab", { name: "commit", exact: true })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await message(page).fill("");
  await aiButton(page).click();
  await expect(message(page)).toHaveValue("Add the notes");
  await expect(popover(page)).toHaveCount(0);
});

test("the settings choose the tool, its model and the prompt; they are used and kept", async ({ gg }) => {
  const { page } = gg;
  gg.ai.install("codex");
  gg.ai.reply("codex", 0, "Use codex\n");
  await commitPanel(gg);
  await page.keyboard.press("ControlOrMeta+,");
  const ai = page.getByTestId("settings-ai");
  await ai.scrollIntoViewIfNeeded();
  await expect(ai.locator('[data-tool="claude"]')).toContainText("0.0.1");
  await expect(ai.locator('[data-tool="codex"]')).toContainText("0.0.1");
  await page.getByTestId("settings-ai-tool").getByRole("radio", { name: "Codex" }).click();
  await page.getByTestId("settings-ai-model-codex").fill("gpt-5.4-mini");
  await page.getByTestId("settings-ai-prompt-commit").locator("summary").click();
  const prompt = page.getByTestId("settings-ai-prompt-commit-text");
  await expect(prompt).toHaveValue(/^You are generating a single git commit message\./);
  await prompt.fill("Write it as a haiku.\n\n{stagedPatch}");
  await expect(page.getByTestId("settings-ai-prompt-commit")).toContainText("Your own");
  await page.keyboard.press("Escape");
  await expect(aiButton(page)).toHaveAttribute("title", /^Generate with Codex \(gpt-5\.4-mini\)/);
  await aiButton(page).click();
  await expect(message(page)).toHaveValue("Use codex");
  const call = gg.ai.calls("codex")[0];
  expect(call.argv).toContain("gpt-5.4-mini");
  expect(call.argv).toEqual(expect.arrayContaining(["--disable", "shell_tool"]));
  expect(call.stdin.startsWith("Write it as a haiku.\n\n```")).toBe(true);
  expect(gg.ai.calls("claude")).toHaveLength(0);

  // Kept across a restart; Reset to default puts gg's prompt back.
  await gg.relaunch();
  await expect(page.getByRole("tab", { name: "commit", exact: true })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+,");
  await expect(page.getByTestId("settings-ai-tool").getByRole("radio", { name: "Codex" })).toBeChecked();
  await expect(page.getByTestId("settings-ai-model-codex")).toHaveValue("gpt-5.4-mini");
  await page.getByTestId("settings-ai-prompt-commit").locator("summary").click();
  await expect(page.getByTestId("settings-ai-prompt-commit-text")).toHaveValue("Write it as a haiku.\n\n{stagedPatch}");
  await page.getByTestId("settings-ai-prompt-commit-reset").click();
  await expect(page.getByTestId("settings-ai-prompt-commit-text")).toHaveValue(/^You are generating a single git commit message\./);
  await expect(page.getByTestId("settings-ai-prompt-commit")).toContainText("gg’s default");
  // A model that could be read as an option is refused, said beside it.
  await page.getByTestId("settings-ai-model-codex").fill("--yolo");
  await expect(page.getByTestId("settings-ai").getByRole("alert")).toContainText("A model name starts with a letter or a digit.");
});

test("a pull request's title and description are written from the branch; the base stays; Undo puts back the form", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-list", 0, "[]");
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  gg.ai.install("claude");
  gg.ai.reply(
    "claude",
    0,
    JSON.stringify({ base: "develop", title: "Fix the README typo.", body: "## Problem\nA typo.\n\n## Solution\nFix it.", draft: true }),
  );
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await setSettings(page, { aiNoticeSeen: ["claude"] });
  await page.getByRole("grid", { name: "Commits" }).locator('[data-ref="fix/typo"]').first().locator(".name").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Create pull request…" }).click();
  const panel = page.getByTestId("pr-panel");
  await expect(panel.getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(panel.getByTestId("pr-base")).toHaveValue("main");
  await panel.getByTestId("pr-ai").click();
  await expect(panel.getByTestId("pr-title")).toHaveValue("Fix the README typo");
  await expect(panel.getByTestId("pr-body")).toHaveValue("## Problem\nA typo.\n\n## Solution\nFix it.");
  await expect(panel.getByTestId("pr-draft")).toBeChecked();
  await expect(panel.getByTestId("pr-base")).toHaveValue("main");
  const { stdin } = gg.ai.calls("claude")[0];
  expect(stdin).toContain("Head branch: fix/typo");
  expect(stdin).toContain("Current base: main");
  expect(stdin).toContain("Current title: Fix a typo in the README");
  await panel.getByTestId("pr-ai-undo").click();
  await expect(panel.getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await expect(panel.getByTestId("pr-draft")).not.toBeChecked();
});

test("a branch with nothing ahead of its base: the pull request button says there is nothing to describe", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-list", 0, "[]");
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  gg.ai.install("claude");
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  gg.git("prs", "branch", "level", "main");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await page.getByRole("grid", { name: "Commits" }).locator('[data-ref="level"]').first().locator(".name").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Create pull request…" }).click();
  const button = page.getByTestId("pr-panel").getByTestId("pr-ai");
  await expect(button).toHaveAttribute("aria-disabled", "true");
  await expect(button).toHaveAttribute("title", "Nothing to describe: level has no commits that are not on main.");
});

test("a generation belongs to the form, not to the panel showing it: shown again, the form still waits for it, can cancel it, and gets its answer once", async ({ gg }) => {
  const { page } = gg;
  gg.gh.reply("pr-list", 0, "[]");
  gg.gh.reply("pr-list-head", 0, "[]");
  gg.gh.reply("repo-view", 0, JSON.stringify({ defaultBranchRef: { name: "main" } }));
  gg.ai.install("claude");
  gg.ai.reply("claude", 0, JSON.stringify({ title: "Fix the README typo", body: "## Problem\nA typo.", draft: false }), "", 2_500);
  await page.goto("/");
  await open(page, gg.repoPath("prs"));
  await setSettings(page, { aiNoticeSeen: ["claude"] });
  await page.getByRole("grid", { name: "Commits" }).locator('[data-ref="fix/typo"]').first().locator(".name").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Create pull request…" }).click();
  const panel = page.getByTestId("pr-panel");
  await expect(panel.getByTestId("pr-title")).toHaveValue("Fix a typo in the README");
  await panel.getByTestId("pr-ai").click();
  await expect(panel.getByTestId("pr-ai")).toHaveAttribute("data-state", "running");
  // Another tab, then back: the form is made again, and still waits for it.
  await page.getByTestId("side-tab-worktrees").click();
  await expect(panel).toHaveCount(0);
  await page.getByTestId("side-tab-pr").click();
  await expect(panel.getByTestId("pr-ai")).toHaveAttribute("data-state", "running");
  await expect(panel.getByTestId("pr-title")).toHaveAttribute("readonly", "");
  await expect(panel.getByTestId("pr-ai-cancel")).toBeVisible();
  // Its answer lands once, in the form shown now, with Undo.
  await expect(panel.getByTestId("pr-title")).toHaveValue("Fix the README typo");
  await expect(panel.getByTestId("pr-ai-undo")).toBeVisible();
  expect(gg.ai.calls("claude")).toHaveLength(1);

  // Cancelled from a form shown again: nothing of it lands, the tool is ended.
  await panel.getByTestId("pr-title").fill("My own title");
  // Soon enough that a tool Cancel left running would answer before the test ends.
  gg.ai.reply("claude", 0, JSON.stringify({ title: "Too late", body: "x" }), "", 3_000);
  await panel.getByTestId("pr-ai").click();
  await expect.poll(() => gg.ai.calls("claude").length).toBe(2);
  await page.getByTestId("side-tab-worktrees").click();
  await page.getByTestId("side-tab-pr").click();
  await panel.getByTestId("pr-ai-cancel").click();
  await expect(panel.getByTestId("pr-ai")).toHaveAttribute("data-state", "ready");
  // The tool's process is gone, and it never answered; nothing of it lands.
  expect(await gg.ai.ended("claude", 1)).toEqual({ answered: false });
  await expect(panel.getByTestId("pr-title")).toHaveValue("My own title");
});

test("the commit message's generation outlives the panel too: its answer is in the field when the Commit tab shows again", async ({ gg }) => {
  const { page } = gg;
  gg.ai.reply("claude", 0, "Tidy the notes\n", "", 2_000);
  await commitPanel(gg);
  await aiButton(page).click();
  await expect(aiButton(page)).toHaveAttribute("data-state", "running");
  await page.getByTestId("side-tab-worktrees").click();
  await expect(page.getByTestId("commit-panel")).toHaveCount(0);
  await page.getByTestId("side-tab-commit").click();
  await expect(aiButton(page)).toHaveAttribute("data-state", "running");
  await expect(message(page)).toHaveAttribute("readonly", "");
  await expect(message(page)).toHaveValue("Tidy the notes");
  await expect(aiButton(page)).toHaveAttribute("data-state", "ready");
  expect(gg.ai.calls("claude")).toHaveLength(1);
});
