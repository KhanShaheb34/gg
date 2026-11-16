// The commit panel against the real core: committing a subset of the changed files, Commit &
// Push to a local bare remote, a failing hook (its output shown, the message kept), the list
// cut at 5,000 files, the ways in (the shortcut, the worktree menu and its row, the graph's
// uncommitted row), another worktree made active in one click, the keyboard, what stops a
// commit said under the buttons, and the worktree menu's launchers (recorded, never started).
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const panel = (page: Page) => page.getByTestId("commit-panel");
const files = (page: Page) => page.getByRole("listbox", { name: "Changed files" });
const file = (page: Page, p: string) => files(page).locator(`[role="option"][data-path="${p}"]`);
const message = (page: Page) => page.getByTestId("commit-message");
const commitButton = (page: Page) => page.getByTestId("commit-button");
const pushButton = (page: Page) => page.getByTestId("commit-push-button");
const reason = (page: Page) => page.getByTestId("commit-reason");
const notice = (page: Page, text: string | RegExp) => page.locator("[data-notices] .toast", { hasText: text });
const worktreeRow = (page: Page, folder: string) =>
  page.getByRole("grid", { name: "Worktrees" }).locator(`[role="row"][data-path$="${path.sep}${folder}"]`);
const item = (page: Page, name: string | RegExp) => page.getByRole("menuitem", { name, exact: typeof name === "string" });

const DEEP = "packages/payments-service/src/components/checkout/summary/LineItemPriceBreakdown.tsx";
const ALL = [
  "a file with spaces.txt",
  "notes/roadmap.md",
  "old.txt",
  DEEP,
  "src/app.ts",
  "todo.md",
];

/** The paths commit `rev` changed (renames as both paths). */
function changed(gg: { git: (name: string, ...args: string[]) => string }, rev = "HEAD"): string[] {
  return gg
    .git("commit", "diff-tree", "--no-commit-id", "-r", "--no-renames", "--name-only", rev)
    .split("\n")
    .filter((l) => l !== "")
    .sort();
}

test("a subset is committed: exactly the ticked files, the rest stay; the notice offers the push", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(panel(page)).toBeVisible();
  await expect(message(page)).toBeFocused();
  // Every changed file, sorted by path, all ticked; a rename as old → new.
  await expect(files(page).getByRole("option")).toHaveCount(6);
  expect(await files(page).getByRole("option").evaluateAll((els) => els.map((e) => e.getAttribute("data-path")))).toEqual(ALL);
  await expect(page.getByTestId("commit-count")).toHaveText("6 of 6 files ticked");
  await expect(file(page, "notes/roadmap.md")).toContainText(/notes\/plan\.md→\s*notes\/roadmap\.md/);
  await expect(file(page, "old.txt").locator(".status")).toHaveText("D");
  await expect(file(page, "todo.md").locator(".status")).toHaveText("?");
  // A long path keeps its file name in sight.
  const deep = file(page, DEEP).locator(".mid").last();
  await expect(deep).toHaveAttribute("data-shown", "LineItemPriceBreakdown.tsx");

  // Untick two, by click and by the keyboard.
  await file(page, "todo.md").click();
  await files(page).focus();
  await page.keyboard.press("End");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Space");
  await expect(file(page, "src/app.ts")).toHaveAttribute("aria-selected", "false");
  await expect(page.getByTestId("commit-count")).toHaveText("4 of 6 files ticked");
  await expect(page.getByTestId("tick-all")).toHaveJSProperty("indeterminate", true);

  // No message: said under the buttons.
  await expect(commitButton(page)).toBeDisabled();
  await expect(reason(page)).toHaveText("Write a commit message.");
  await message(page).fill("Tidy the notes");
  await expect(reason(page)).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+Enter");

  await expect(notice(page, /Committed 4 files to main \(/)).toBeVisible();
  expect(changed(gg)).toEqual(["a file with spaces.txt", DEEP, "notes/plan.md", "notes/roadmap.md", "old.txt"].sort());
  expect(gg.git("commit", "log", "-1", "--format=%s").trim()).toBe("Tidy the notes");
  // The rest stay, unticked, and the message is cleared; the panel stays open.
  await expect(files(page).getByRole("option")).toHaveCount(2);
  await expect(page.getByTestId("commit-count")).toHaveText("0 of 2 files ticked");
  await expect(message(page)).toHaveValue("");
  expect(gg.git("commit", "status", "--porcelain").split("\n").filter((l) => l !== "").sort()).toEqual([" M src/app.ts", "?? todo.md"]);
  // Not pushed: the notice offers it, naming where it goes (R30, R38).
  const push = notice(page, /Committed 4 files/).getByRole("button", { name: "Push main to origin/main" });
  await push.click();
  await expect.poll(() => gg.gitAt(path.join(path.dirname(repo), "commit-origin.git"), "rev-parse", "main").trim()).toBe(
    gg.git("commit", "rev-parse", "HEAD").trim(),
  );
});

test("Commit & Push names where it goes and sends the branch there; with nothing left the panel goes back", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(pushButton(page)).toHaveText("Commit & Push to origin/main");
  await message(page).fill("Everything");
  await pushButton(page).click();
  await expect(notice(page, /Committed 6 files to main \(.*\)\. Pushed main to origin\/main/)).toBeVisible();
  const origin = path.join(path.dirname(repo), "commit-origin.git");
  expect(gg.gitAt(origin, "rev-parse", "main").trim()).toBe(gg.git("commit", "rev-parse", "HEAD").trim());
  // Nothing left: back to the worktree list.
  await expect(panel(page)).toHaveCount(0);
  await expect(page.getByRole("grid", { name: "Worktrees" })).toBeVisible();

  // No remote: no Commit & Push. Detached: none either.
  gg.git("commit", "remote", "remove", "origin");
  writeFileSync(path.join(repo, "more.txt"), "more\n");
  await page.keyboard.press("ControlOrMeta+k");
  await expect(files(page).getByRole("option")).toHaveCount(1);
  await expect(commitButton(page)).toBeVisible();
  await expect(pushButton(page)).toHaveCount(0);
});

test("Commit & Push goes where git pushes the branch, as Create pull request does: remote.pushDefault sends it to the fork (final review M7)", async ({ gg }) => {
  const { page } = gg;
  const repo = gg.repoPath("commit");
  const fork = path.join(path.dirname(repo), "commit-fork.git");
  gg.git("commit", "init", "-q", "--bare", "-b", "main", fork);
  gg.git("commit", "remote", "add", "fork", fork);
  gg.git("commit", "config", "remote.pushDefault", "fork");
  await page.goto("/");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(pushButton(page)).toHaveText("Commit & Push to fork/main");
  await message(page).fill("Everything");
  await pushButton(page).click();
  await expect(notice(page, /Pushed main to fork\/main/)).toBeVisible();
  expect(gg.gitAt(fork, "rev-parse", "main").trim()).toBe(gg.git("commit", "rev-parse", "HEAD").trim());
});

test("back to a tab whose commit panel is open, the focus stays where it was (final review M10)", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  await page.keyboard.press("ControlOrMeta+k");
  await expect(message(page)).toBeFocused();
  await open(page, gg.repoPath("small"));
  // Back with the keyboard (⌘1), from the graph of the other tab (⌘2 there and back).
  await page.keyboard.press("ControlOrMeta+1");
  await expect(panel(page)).toBeVisible();
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+2");
  await expect(panel(page)).toHaveCount(0);
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+1");
  await expect(panel(page)).toBeVisible();
  await page.waitForTimeout(300);
  await expect(message(page)).not.toBeFocused();
  // Asked for again (Cmd+K), it does take the focus.
  await page.keyboard.press("ControlOrMeta+k");
  await expect(message(page)).toBeFocused();
});

test("a failing hook: nothing committed, its output in the notice with Show log, the message kept", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  const hooks = path.join(repo, ".git", "hooks");
  mkdirSync(hooks, { recursive: true });
  writeFileSync(path.join(hooks, "pre-commit"), "#!/bin/sh\necho 'lint: src/app.ts: missing semicolon' >&2\nexit 1\n", { mode: 0o755 });
  const before = gg.git("commit", "rev-parse", "HEAD").trim();
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await message(page).fill("Will not pass");
  await commitButton(page).click();
  const failed = notice(page, /refused the commit/);
  await expect(failed).toBeVisible();
  await failed.getByRole("button", { name: "Git output" }).click();
  await expect(failed).toContainText("lint: src/app.ts: missing semicolon");
  await expect(failed.getByRole("button", { name: "Show log" })).toBeVisible();
  expect(gg.git("commit", "rev-parse", "HEAD").trim()).toBe(before);
  await expect(message(page)).toHaveValue("Will not pass");
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await expect(page.getByTestId("commit-count")).toHaveText("6 of 6 files ticked");
});

test("over 5,000 changed files the list says it shows the first 5,000", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  const many = path.join(repo, "generated");
  mkdirSync(many);
  for (let i = 0; i < 5001; i++) writeFileSync(path.join(many, `f${String(i).padStart(5, "0")}.txt`), "x");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByTestId("commit-count")).toHaveText("5000 of 5000+ files ticked", { timeout: 15_000 });
  await expect(page.getByTestId("commit-truncated")).toHaveText(
    "Only the first 5,000 changed files are listed. Commit these, and the rest come next.",
  );
  // Only the rows in view are drawn.
  expect(await files(page).getByRole("option").count()).toBeLessThan(80);
  await files(page).focus();
  await page.keyboard.press("End");
  await expect(files(page).getByRole("option").last()).toBeVisible();
  // The active descendant is only ever a row that is drawn.
  const target = () =>
    files(page).evaluate((el) => {
      const id = el.getAttribute("aria-activedescendant");
      if (id === null) return "none";
      return document.getElementById(id) ? "rendered" : "missing";
    });
  await expect.poll(target).toBe("rendered");
  await files(page).evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(target).not.toBe("missing");
  await expect(files(page).getByRole("option").first()).toHaveAttribute("data-path", "a file with spaces.txt");
  expect(await target()).toBe("none");
});

test("the ways in: the menu, the row's changes, the uncommitted row; another worktree is made active in one click", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");

  // The worktree menu's Commit… on a worktree that is not active: its files, and the offer.
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(page.getByTestId("side-detail")).toHaveText("topic");
  await expect(files(page).getByRole("option")).toHaveCount(1);
  await expect(page.getByTestId("commit-elsewhere")).toContainText("Commits run in the active worktree.");
  await expect(commitButton(page)).toHaveCount(0);
  await message(page).fill("Topic work");
  await page.getByRole("button", { name: "Make it active" }).click();
  await expect(page.getByTestId("commit-elsewhere")).toHaveCount(0);
  await expect(page.getByTestId("active-worktree")).toContainText("topic");
  // No upstream yet: Commit & Push sends it to origin's namesake (R38).
  await expect(pushButton(page)).toHaveText("Commit & Push to origin/topic");
  await commitButton(page).click();
  await expect(notice(page, /Committed 1 file to topic/)).toBeVisible();
  expect(gg.gitAt(topic, "log", "-1", "--format=%s").trim()).toBe("Topic work");

  // Back to the list; the main worktree's row: its changes are a button to its commit panel.
  await expect(page.getByRole("grid", { name: "Worktrees" })).toBeVisible();
  await worktreeRow(page, "commit").locator(".changes").click();
  await expect(page.getByTestId("side-detail")).toHaveText("main");
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  // The draft survives going back and coming again.
  await message(page).fill("Half written");
  await page.getByTestId("side-back").click();
  await expect(panel(page)).toHaveCount(0);
  await worktreeRow(page, "commit").dblclick();
  await expect(page.getByTestId("active-worktree")).toContainText("main");

  // The graph's uncommitted row, double-clicked.
  await grid(page).locator('[role="row"].uncommitted').dblclick();
  await expect(panel(page)).toBeVisible();
  await expect(message(page)).toHaveValue("Half written");
  // Escape goes back to the list.
  await message(page).press("Escape");
  await expect(panel(page)).toHaveCount(0);
});

test("ticks: Cmd+A ticks every file, the all box clears them, a conflicted file cannot be ticked", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await page.getByTestId("tick-all").click();
  await expect(page.getByTestId("commit-count")).toHaveText("0 of 6 files ticked");
  await expect(reason(page)).toHaveText("Tick the files to commit.");
  await files(page).focus();
  await page.keyboard.press("ControlOrMeta+a");
  await expect(page.getByTestId("commit-count")).toHaveText("6 of 6 files ticked");

  // A file that appears while everything is ticked comes ticked; ticks and the message stay.
  await message(page).fill("Keep me");
  writeFileSync(path.join(repo, "zz-new.txt"), "new\n");
  await expect(file(page, "zz-new.txt")).toHaveAttribute("aria-selected", "true");
  await file(page, "zz-new.txt").click();
  writeFileSync(path.join(repo, "zz-later.txt"), "later\n");
  await expect(file(page, "zz-later.txt")).toHaveAttribute("aria-selected", "false");
  await expect(file(page, "zz-new.txt")).toHaveAttribute("aria-selected", "false");
  await expect(message(page)).toHaveValue("Keep me");
});

test("the worktree menu opens a worktree in the editor, a terminal and the file manager (recorded)", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");
  for (const name of ["Open in editor", "Open in terminal", /^(Reveal in Finder|Show in File Explorer|Open containing folder)$/]) {
    await worktreeRow(page, "topic").click({ button: "right" });
    await item(page, name).click();
  }
  await expect.poll(() => gg.launches().length).toBe(3);
  // No editor is set: the notice says the folder was shown instead, and where to set one.
  await expect(notice(page, /No editor is set; showed the folder in .+\. Set an editor command in Settings\./)).toBeVisible();
  const [editor, terminal, reveal] = gg.launches();
  // No editor command: the folder with the platform's opener; Terminal; the folder shown.
  expect(editor.at(-1)).toBe(topic);
  if (process.platform === "linux") {
    // The first terminal found, started in the folder (the record keeps the program and its
    // arguments, not the folder it starts in).
    expect(terminal).toHaveLength(1);
    expect(["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"]).toContain(path.basename(terminal[0]));
  } else {
    expect(terminal.at(-1)).toBe(topic);
  }
  expect(reveal.at(-1)).toBe(process.platform === "linux" ? path.dirname(topic) : topic);
  if (process.platform === "darwin") {
    expect(terminal).toEqual(["/usr/bin/open", "-a", "Terminal", topic]);
    expect(reveal).toEqual(["/usr/bin/open", "-R", topic]);
  }
});

test("an open commit panel reads the files only when something changed", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  await page.keyboard.press("ControlOrMeta+k");
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await page.waitForTimeout(600);
  /** The status reads the core has run so far. */
  const reads = () =>
    page.evaluate(async () => {
      const { api } = await import("/src/lib/ipc.ts");
      return (await api.commandLog(0)).filter((c: { args: string[] }) => c.args.includes("status")).length;
    });
  const before = await reads();
  await page.waitForTimeout(1500);
  expect(await reads()).toBe(before);
});

test("only the worktree shown commits, and only while it is active: elsewhere, after a switch, after its folder is gone", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  // The same new file in both worktrees: a commit of the shown one's list would also take
  // in the main worktree.
  writeFileSync(path.join(repo, "topic.txt"), "main's own\n");
  await open(page, repo);
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");
  const heads = () => [gg.git("commit", "rev-parse", "HEAD").trim(), gg.gitAt(topic, "rev-parse", "HEAD").trim()];
  const before = heads();
  const writes = () =>
    page.evaluate(async () => {
      const { api } = await import("/src/lib/ipc.ts");
      return (await api.commandLog(0)).filter((c: { kind: string }) => c.kind === "write").length;
    });

  // Shown but not active: Cmd+Enter in the message commits nothing, anywhere.
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  await message(page).fill("Not here");
  const writesBefore = await writes();
  await message(page).press("ControlOrMeta+Enter");
  await page.waitForTimeout(500);
  expect(await writes()).toBe(writesBefore);
  expect(heads()).toEqual(before);

  // Made active, then another made active under the open panel: it goes back to the offer.
  await page.getByRole("button", { name: "Make it active" }).click();
  await expect(page.getByTestId("commit-elsewhere")).toHaveCount(0);
  await expect(commitButton(page)).toBeVisible();
  await page.evaluate(async (main) => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    await api.setActiveWorktree(app.activeRepoId!, main);
  }, gg.git("commit", "rev-parse", "--show-toplevel").trim());
  await expect(page.getByTestId("active-worktree")).toContainText("main");
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  await message(page).press("ControlOrMeta+Enter");
  await page.waitForTimeout(500);
  expect(heads()).toEqual(before);

  // Active again, then its folder deleted outside the app: the repository falls back to the
  // main worktree; the panel offers nothing to commit there and commits nothing.
  await page.getByRole("button", { name: "Make it active" }).click();
  await expect(commitButton(page)).toBeVisible();
  rmSync(topic, { recursive: true, force: true });
  await page.keyboard.press("ControlOrMeta+r");
  await expect(page.getByTestId("active-worktree")).toContainText("main");
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  await expect(page.getByRole("button", { name: "Make it active" })).toBeDisabled();
  // Said why, as the worktree row says it (final review M11).
  await expect(page.getByTestId("commit-elsewhere-why")).toHaveText("Its folder is missing.");
  await message(page).press("ControlOrMeta+Enter");
  await page.waitForTimeout(500);
  expect(gg.git("commit", "rev-parse", "HEAD").trim()).toBe(before[0]);
});

test("a commit panel whose worktree is no longer listed is closed for good, not reopened when the path comes back", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(panel(page)).toBeVisible();
  gg.git("commit", "worktree", "remove", "--force", topic);
  await page.keyboard.press("ControlOrMeta+r");
  await expect(panel(page)).toHaveCount(0);
  await expect(page.getByRole("grid", { name: "Worktrees" })).toBeVisible();
  // The same path made a worktree again: the list stays.
  gg.git("commit", "worktree", "add", "-q", topic, "topic");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(worktreeRow(page, "topic")).toBeVisible();
  await expect(panel(page)).toHaveCount(0);
});

test("concluding a merge: tracked files ticked and kept so, new files up to the user, git's message offered, a merge commit made", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");
  const at = (...args: string[]) => gg.gitAt(topic, ...args);
  writeFileSync(path.join(topic, "shared.txt"), "base\n");
  at("add", "shared.txt");
  at("commit", "-q", "-m", "Share a file");
  at("switch", "-q", "-c", "other");
  writeFileSync(path.join(topic, "shared.txt"), "other\n");
  at("commit", "-q", "-am", "Other side");
  at("switch", "-q", "topic");
  writeFileSync(path.join(topic, "shared.txt"), "topic\n");
  at("commit", "-q", "-am", "Topic side");
  expect(() => at("merge", "-q", "other")).toThrow();
  writeFileSync(path.join(topic, "src", "app.ts"), "export const app = 3;\n");

  await open(page, repo);
  await page.evaluate(async (wt) => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    await api.setActiveWorktree(app.activeRepoId!, wt);
  }, topic);
  await expect(page.getByTestId("active-worktree")).toContainText("topic");
  await page.keyboard.press("ControlOrMeta+k");
  // Still conflicted: the row says why it cannot be ticked, to the eye and in its name; the
  // panel says it up front, and why Commit cannot be pressed.
  await expect(file(page, "shared.txt")).toHaveAttribute("aria-selected", "false");
  // An option is selected, not checked (final review M13).
  await expect(file(page, "shared.txt")).not.toHaveAttribute("aria-checked");
  await expect(file(page, "shared.txt")).toHaveAccessibleName(/shared\.txt.*Resolve its conflicts first\./);
  await expect(page.getByTestId("commit-merge")).toContainText("shared.txt");
  await expect(reason(page)).toHaveText("Resolve the conflicts in your editor, then continue: commit here, or Continue in the banner.");
  // Said mid-sentence, lower-cased (final review M12).
  await expect(page.getByTestId("commit-merge")).toContainText("shared.txt cannot go in yet: resolve its conflicts first.");
  // Resolved and staged.
  writeFileSync(path.join(topic, "shared.txt"), "both\n");
  at("add", "shared.txt");
  await expect(page.getByTestId("commit-merge")).toContainText("Concluding the merge of other");
  await expect(page.getByTestId("commit-merge")).not.toContainText("shared.txt");
  for (const tracked of ["shared.txt", "src/app.ts"]) {
    await expect(file(page, tracked)).toHaveAttribute("aria-selected", "true");
    await expect(file(page, tracked)).toHaveAccessibleName(/goes in with the merge/);
    await expect(file(page, tracked)).toHaveAttribute("aria-disabled", "true");
    await file(page, tracked).click({ force: true });
    await expect(file(page, tracked)).toHaveAttribute("aria-selected", "true");
  }
  // The new file is the user's to leave out.
  await file(page, "topic.txt").click();
  await expect(file(page, "topic.txt")).toHaveAttribute("aria-selected", "false");
  await expect(message(page)).toHaveValue("Merge branch 'other' into topic");
  await commitButton(page).click();
  await expect(notice(page, /Concluded the merge: committed 2 files to topic/)).toBeVisible();
  expect(at("log", "-1", "--format=%P").trim().split(" ")).toHaveLength(2);
  expect(at("log", "-1", "--format=%s").trim()).toBe("Merge branch 'other' into topic");
  expect(at("status", "--porcelain").trim()).toBe("?? topic.txt");
});

test("Commit & Push is not offered on a detached HEAD; a stopped rebase is said under the buttons", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  gg.git("commit", "switch", "-q", "--detach");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await expect(commitButton(page)).toBeVisible();
  await expect(pushButton(page)).toHaveCount(0);

  // A rebase stopped in the topic worktree, made active.
  const topic = path.join(path.dirname(repo), "commit.worktrees", "topic");
  const at = (...args: string[]) => gg.gitAt(topic, ...args);
  at("switch", "-q", "-c", "base");
  writeFileSync(path.join(topic, "clash.txt"), "base\n");
  at("add", "clash.txt");
  at("commit", "-q", "-m", "Base clash");
  at("switch", "-q", "topic");
  writeFileSync(path.join(topic, "clash.txt"), "topic\n");
  at("add", "clash.txt");
  at("commit", "-q", "-m", "Topic clash");
  expect(() => at("rebase", "-q", "base")).toThrow();
  await page.keyboard.press("Escape");
  await page.evaluate(async (wt) => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    await api.setActiveWorktree(app.activeRepoId!, wt);
  }, topic);
  // Detached mid-rebase: the active worktree is named by its folder.
  await expect(page.getByRole("button", { name: /^Active worktree: .*commit\.worktrees\/topic/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await message(page).fill("Not now");
  await expect(reason(page)).toHaveText("A rebase is stopped here: Continue, in the banner, commits its changes (or abort it).");
  await expect(commitButton(page)).toBeDisabled();
});

test("looks: a disabled Commit & Push looks disabled; the old side of a rename keeps its file name", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  // A file deep in long folder names, committed, then renamed in its folder.
  const dir = path.posix.dirname(DEEP);
  writeFileSync(path.join(repo, dir, "Totals.tsx"), "export {};\n");
  gg.git("commit", "add", `${dir}/Totals.tsx`);
  gg.git("commit", "commit", "-q", "-m", "Add totals", "--", `${dir}/Totals.tsx`);
  const renamed = `${dir}/Summary.tsx`;
  gg.git("commit", "mv", `${dir}/Totals.tsx`, renamed);
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  // No message: both buttons disabled, the push's label painted as faint as Commit's.
  await expect(pushButton(page)).toBeDisabled();
  const colours = await page.evaluate(() => {
    const colour = (sel: string) => getComputedStyle(document.querySelector(sel)!).color;
    const label = document.querySelector('[data-testid="commit-push-button"] .mid');
    return {
      commit: colour('[data-testid="commit-button"]'),
      push: colour('[data-testid="commit-push-button"]'),
      label: label ? getComputedStyle(label).color : null,
      enabled: getComputedStyle(document.querySelector('[data-testid="commit-message"]')!).color,
    };
  });
  expect(colours.push).toBe(colours.commit);
  expect(colours.label).toBe(colours.commit);
  expect(colours.push).not.toBe(colours.enabled);

  // The rename's old side is cut in the middle like any path: its file name stays.
  const row = file(page, renamed);
  await expect(row.locator(".mid").last()).toHaveAttribute("data-shown", "Summary.tsx");
  await expect(row.locator(".old")).toHaveAttribute("data-shown", "Totals.tsx");
});
