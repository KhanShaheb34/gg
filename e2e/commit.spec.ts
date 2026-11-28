// The commit panel against the real core: committing a subset of the changed files, Commit &
// Push to a local bare remote, an empty commit, a failing hook (its output shown, the message kept), the list
// cut at 5,000 files, the ways in (the shortcut, the worktree menu and its row, the graph's
// uncommitted row), another worktree made active in one click, the keyboard, what stops a
// commit said under the buttons, the worktree menu's launchers (recorded, never started), and
// the side panel around it: its one toolbar button (with the count of files to commit), its
// Commit, Pull request and Worktrees icon tabs (the last one used remembered per repository; a dot for files to commit), another worktree's
// commit panel on the Commit tab, and the tab strip at the panel's least width.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, openFolder, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
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
/** The side panel's tab `id`. */
const tab = (page: Page, id: "commit" | "pr" | "worktrees") => page.getByTestId(`side-tab-${id}`);
const list = (page: Page) => page.getByRole("grid", { name: "Worktrees" });

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

  await expect(notice(page, /Committed 4 files\. [0-9a-f]{8} on main\./)).toBeVisible();
  expect(changed(gg)).toEqual(["a file with spaces.txt", DEEP, "notes/plan.md", "notes/roadmap.md", "old.txt"].sort());
  expect(gg.git("commit", "log", "-1", "--format=%s").trim()).toBe("Tidy the notes");
  // The rest stay, unticked, and the message is cleared; the panel stays open.
  await expect(files(page).getByRole("option")).toHaveCount(2);
  await expect(page.getByTestId("commit-count")).toHaveText("0 of 2 files ticked");
  await expect(message(page)).toHaveValue("");
  expect(gg.git("commit", "status", "--porcelain").split("\n").filter((l) => l !== "").sort()).toEqual([" M src/app.ts", "?? todo.md"]);
  // Not pushed: the notice offers it, naming where it goes.
  const push = notice(page, /Committed 4 files/).getByRole("button", { name: "Push to origin" });
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
  await expect(notice(page, /Committed 6 files and pushed\. [0-9a-f]{8} on main, pushed to origin/)).toBeVisible();
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

test("with nothing ticked, Commit with no changes makes an empty commit, pushed or not", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  await page.keyboard.press("ControlOrMeta+k");
  const empty = page.getByTestId("commit-empty");
  // Files ticked: not offered.
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await expect(empty).toHaveCount(0);
  // Nothing ticked: offered, off; Commit says to tick files.
  await page.getByTestId("tick-all").click();
  await expect(page.getByTestId("commit-count")).toHaveText("0 of 6 files ticked");
  await expect(empty).not.toBeChecked();
  await message(page).fill("Run CI again");
  await expect(reason(page)).toHaveText("Tick the files to commit.");
  await expect(commitButton(page)).toBeDisabled();
  await empty.check();
  await expect(reason(page)).toHaveCount(0);
  // The file list read slowly (as on a busy machine): after a commit, Commit waits for the
  // files on the new HEAD instead of sending the old one.
  await page.route("**/invoke", async (route) => {
    if ((route.request().postData() ?? "").includes('"worktree_files"')) await new Promise((r) => setTimeout(r, 500));
    await route.continue();
  });
  const before = gg.git("commit", "rev-parse", "HEAD").trim();
  const status = gg.git("commit", "status", "--porcelain");
  await commitButton(page).click();
  await expect(notice(page, /Made an empty commit\. [0-9a-f]{8} on main\./)).toBeVisible();
  expect(gg.git("commit", "rev-parse", "HEAD^").trim()).toBe(before);
  expect(gg.git("commit", "rev-parse", "HEAD^{tree}").trim()).toBe(gg.git("commit", "rev-parse", "HEAD^^{tree}").trim());
  expect(gg.git("commit", "log", "-1", "--format=%s").trim()).toBe("Run CI again");
  // The changes stay as they were; the form starts afresh, the box off again.
  expect(gg.git("commit", "status", "--porcelain")).toBe(status);
  await expect(message(page)).toHaveValue("");
  await expect(empty).not.toBeChecked();

  // A clean worktree: Commit says how to commit anyway; with the push, the branch goes too.
  await message(page).fill("Everything");
  await page.getByTestId("tick-all").click();
  await commitButton(page).click();
  await expect(panel(page)).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByText("No changes: everything here is committed.")).toBeVisible();
  await message(page).fill("Deploy again");
  await expect(reason(page)).toHaveText("Nothing to commit: no file has changed. To commit anyway, tick Commit with no changes.");
  await expect(pushButton(page)).toBeDisabled();
  await empty.check();
  await pushButton(page).click();
  await expect(notice(page, /Made an empty commit and pushed\. [0-9a-f]{8} on main, pushed to origin/)).toBeVisible();
  const origin = path.join(path.dirname(repo), "commit-origin.git");
  expect(gg.gitAt(origin, "rev-parse", "main").trim()).toBe(gg.git("commit", "rev-parse", "HEAD").trim());
  expect(gg.git("commit", "log", "-1", "--format=%s").trim()).toBe("Deploy again");
});

test("Commit & Push goes where git pushes the branch, as Create pull request does: remote.pushDefault sends it to the fork", async ({ gg }) => {
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
  await expect(notice(page, /on main, pushed to fork/)).toBeVisible();
  expect(gg.gitAt(fork, "rev-parse", "main").trim()).toBe(gg.git("commit", "rev-parse", "HEAD").trim());
});

test("back to a tab whose commit panel is open, the focus stays where it was", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  await page.keyboard.press("ControlOrMeta+k");
  await expect(message(page)).toBeFocused();
  await open(page, gg.repoPath("small"));
  // The other repository's side panel on its worktree list.
  await tab(page, "worktrees").click();
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
  // The hook's own line says why; all its output is in the log.
  await expect(failed.locator(".note")).toContainText("lint: src/app.ts: missing semicolon");
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
  await tab(page, "worktrees").click();
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(page.getByTestId("commit-worktree")).toHaveText("topic");
  await expect(files(page).getByRole("option")).toHaveCount(1);
  await expect(page.getByTestId("commit-elsewhere")).toContainText("Commits run in the active worktree.");
  await expect(commitButton(page)).toHaveCount(0);
  await message(page).fill("Topic work");
  await page.getByRole("button", { name: "Make it active" }).click();
  await expect(page.getByTestId("commit-elsewhere")).toHaveCount(0);
  await expect(page.getByTestId("active-worktree")).toContainText("topic");
  // No upstream yet: Commit & Push sends it to origin's namesake.
  await expect(pushButton(page)).toHaveText("Commit & Push to origin/topic");
  await commitButton(page).click();
  await expect(notice(page, /Committed 1 file\. [0-9a-f]{8} on topic/)).toBeVisible();
  expect(gg.gitAt(topic, "log", "-1", "--format=%s").trim()).toBe("Topic work");

  // Back to the list; the main worktree's row: its changes are a button to its commit panel.
  await expect(page.getByRole("grid", { name: "Worktrees" })).toBeVisible();
  await worktreeRow(page, "commit").locator(".changes").click();
  await expect(page.getByTestId("commit-worktree")).toHaveText("main");
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  // The draft survives going back and coming again.
  await message(page).fill("Half written");
  await tab(page, "worktrees").click();
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
  await tab(page, "worktrees").click();
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
  await tab(page, "worktrees").click();
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
  // Said why, as the worktree row says it.
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
  await tab(page, "worktrees").click();
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
  // An option is selected, not checked.
  await expect(file(page, "shared.txt")).not.toHaveAttribute("aria-checked");
  await expect(file(page, "shared.txt")).toHaveAccessibleName(/shared\.txt.*Resolve its conflicts first\./);
  await expect(page.getByTestId("commit-merge")).toContainText("shared.txt");
  await expect(reason(page)).toHaveText("Resolve the conflicts in your editor, then continue: commit here, or Continue in the banner.");
  // Said mid-sentence, lower-cased.
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
  await expect(notice(page, /Concluded the merge\. [0-9a-f]{8} on topic, 2 files/)).toBeVisible();
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
  await expect(page.getByRole("button", { name: /^Switch branch: .*, in the active worktree .*commit\.worktrees\/topic/ })).toBeVisible();
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

test("one toolbar button shows and hides the side panel, on the tab last used in the repository", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  const toggle = page.getByTestId("side-toggle");
  const side = page.locator('[data-region="side-panel"]');
  await expect(toggle).toHaveAccessibleName("Side panel");
  // The old pair is gone.
  await expect(page.getByTestId("commit-toggle")).toHaveCount(0);

  // The first time in a repository: the Commit tab, the active worktree's commit panel.
  await expect(side).toBeVisible();
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  await expect(panel(page)).toBeVisible();
  await expect(page.getByTestId("commit-worktree")).toHaveCount(0);
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(toggle).toHaveAttribute("title", /^Hide the side panel \((⌘|Ctrl\+)B\)\nUncommitted: 5 changed, 1 new$/);

  // Hidden and shown again: the same tab.
  await toggle.click();
  await expect(side).toHaveCount(0);
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await expect(toggle).toHaveAttribute("title", /^Show the side panel \((⌘|Ctrl\+)B\)\n/);
  await toggle.click();
  await expect(panel(page)).toBeVisible();
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");

  // The Worktrees tab: the button and Cmd+B bring it back.
  await tab(page, "worktrees").click();
  await expect(list(page)).toBeVisible();
  await toggle.click();
  await expect(side).toHaveCount(0);
  await toggle.click();
  await expect(list(page)).toBeVisible();
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ControlOrMeta+b");
  await expect(side).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+b");
  await expect(list(page)).toBeVisible();
  await expect(panel(page)).toHaveCount(0);
  // Shown again, the list does not take the focus.
  await page.waitForTimeout(200);
  await expect(list(page)).not.toBeFocused();
  // The panel has no Hide button of its own; Cmd+B from inside it hides it and the focus goes
  // to the graph, not the page.
  await expect(side.getByRole("button", { name: /hide/i })).toHaveCount(0);
  await tab(page, "worktrees").focus();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(side).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(list(page)).toBeVisible();

  // Per repository: another one opens on the Commit tab, and this one keeps Worktrees.
  await open(page, gg.repoPath("worktrees"));
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  await page.getByRole("tab", { name: "commit", exact: true }).click();
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(list(page)).toBeVisible();

  // The header's worktree count, shown while the panel is hidden: the Worktrees tab, the focus in its list.
  await tab(page, "commit").click();
  await page.keyboard.press("ControlOrMeta+b");
  await page.getByTestId("worktree-count").click();
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(list(page)).toBeFocused();
});

test("the side panel's tabs: Commit, Pull request and Worktrees, icons named and with tooltips, switched by click and by the arrow keys", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  const strip = page.getByRole("tablist", { name: "Side panel" });
  await expect(strip.getByRole("tab")).toHaveCount(3);
  // Icons, no text: named for assistive technology, and by their tooltips (with the counts).
  await expect(strip.getByRole("tab", { name: "Commit", exact: true })).toHaveAttribute("data-tab", "commit");
  await expect(strip.getByRole("tab", { name: "Pull request", exact: true })).toHaveAttribute("data-tab", "pr");
  await expect(strip.getByRole("tab", { name: "Worktrees", exact: true })).toHaveAttribute("data-tab", "worktrees");
  await expect(strip).toHaveText("");
  await expect(tab(page, "commit")).toHaveAttribute("title", /^Commit · 6 changed files \((⌘|Ctrl\+)K\)$/);
  await expect(tab(page, "pr")).toHaveAttribute("title", /^Pull request \((⌘⇧|Ctrl\+Shift\+)P\)$/);
  await expect(tab(page, "worktrees")).toHaveAttribute("title", "Worktrees · 2");
  for (const t of ["commit", "pr", "worktrees"] as const) {
    const b = (await tab(page, t).boundingBox())!;
    expect(b.width).toBeGreaterThanOrEqual(28);
    expect(b.width).toBeLessThanOrEqual(32);
    await expect(tab(page, t).locator("svg")).toHaveAttribute("width", "14");
  }
  // The body is the selected tab's panel.
  const body = page.locator("#side-tabpanel");
  await expect(body).toHaveAttribute("aria-labelledby", "side-tab-commit");
  await expect(tab(page, "commit")).toHaveAttribute("aria-controls", (await body.getAttribute("id"))!);
  // Only the selected tab is in the Tab order; the add button belongs to the Worktrees tab.
  await expect(tab(page, "commit")).toHaveAttribute("tabindex", "0");
  await expect(tab(page, "pr")).toHaveAttribute("tabindex", "-1");
  await expect(tab(page, "worktrees")).toHaveAttribute("tabindex", "-1");
  await expect(page.getByRole("button", { name: "Add worktree" })).toHaveCount(0);

  // By click.
  await tab(page, "worktrees").click();
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "false");
  await expect(list(page)).toBeVisible();
  await expect(panel(page)).toHaveCount(0);
  await expect(body).toHaveAttribute("aria-labelledby", "side-tab-worktrees");
  await expect(page.getByRole("button", { name: "Add worktree" })).toBeVisible();
  await tab(page, "commit").click();
  await expect(panel(page)).toBeVisible();
  // Shown again by its tab, the commit panel leaves the focus where it is.
  await expect(message(page)).not.toBeFocused();

  // By the arrow keys (round the ends), Home and End: the focus moves with the selection,
  // drawn as a ring.
  await tab(page, "commit").focus();
  await page.keyboard.press("ArrowRight");
  await expect(tab(page, "pr")).toBeFocused();
  await expect(page.getByTestId("pr-panel")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(tab(page, "worktrees")).toBeFocused();
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(list(page)).toBeVisible();
  expect(await tab(page, "worktrees").evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("solid");
  await page.keyboard.press("ArrowRight");
  await expect(tab(page, "commit")).toBeFocused();
  await expect(panel(page)).toBeVisible();
  await page.keyboard.press("ArrowLeft");
  await expect(tab(page, "worktrees")).toBeFocused();
  await expect(list(page)).toBeVisible();
  await page.keyboard.press("ArrowLeft");
  await expect(tab(page, "pr")).toBeFocused();
  await page.keyboard.press("Home");
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("End");
  await expect(tab(page, "worktrees")).toHaveAttribute("aria-selected", "true");
  await expect(tab(page, "worktrees")).toBeFocused();
  // Tab goes on from the selected tab, past the others.
  await page.keyboard.press("Shift+Tab");
  await expect(tab(page, "commit")).not.toBeFocused();
});

test("Cmd+K: the active worktree's Commit tab, from anywhere, its message focused", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  const side = page.locator('[data-region="side-panel"]');
  // From the Worktrees tab.
  await tab(page, "worktrees").click();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  await expect(message(page)).toBeFocused();
  await expect(page.getByTestId("commit-worktree")).toHaveCount(0);
  // Pressed again with it shown, it stays (the focus in the message).
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(side).toBeVisible();
  await expect(message(page)).toBeFocused();
  // The panel hidden on the Worktrees tab: shown, on the Commit tab.
  await tab(page, "worktrees").click();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(side).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  await expect(message(page)).toBeFocused();
  // From another worktree's commit panel: the active one's.
  await tab(page, "worktrees").click();
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(page.getByTestId("commit-worktree")).toHaveText("topic");
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByTestId("commit-worktree")).toHaveCount(0);
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await expect(message(page)).toBeFocused();
  // And Cmd+B, from there, remembers the Commit tab.
  await page.keyboard.press("ControlOrMeta+b");
  await page.keyboard.press("ControlOrMeta+b");
  await expect(panel(page)).toBeVisible();
});

test("another worktree's commit panel shows on the Commit tab, named, with a way back to the active worktree's", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("commit"));
  await tab(page, "worktrees").click();
  // Its row's changes: the Commit tab, selected, on that worktree.
  await worktreeRow(page, "topic").locator(".changes").click();
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");
  const line = page.getByTestId("commit-worktree");
  await expect(line).toHaveText("topic");
  await expect(line.locator(".mid")).toHaveAttribute("title", /commit\.worktrees\/topic$/);
  await expect(files(page).getByRole("option")).toHaveCount(1);
  await expect(page.getByTestId("commit-elsewhere")).toBeVisible();
  const back = page.getByTestId("commit-worktree-return");
  await expect(back).toHaveAccessibleName("Back to the active worktree's commit panel");
  await expect(back).toHaveAttribute("title", "Back to the active worktree's commit panel (main)");
  // The line sits under the tabs, inside the panel, on one line.
  const side = (await page.locator('[data-region="side-panel"]').boundingBox())!;
  const at = (await line.boundingBox())!;
  const head = (await page.locator('[data-region="side-panel"] header').boundingBox())!;
  expect(at.y).toBeGreaterThanOrEqual(head.y + head.height - 1);
  expect(at.height).toBeLessThanOrEqual(31);
  expect(at.x + at.width).toBeLessThanOrEqual(side.x + side.width + 0.5);

  // Back: the active worktree's commit panel, the focus in its message.
  await back.click();
  await expect(line).toHaveCount(0);
  await expect(page.getByTestId("commit-elsewhere")).toHaveCount(0);
  await expect(files(page).getByRole("option")).toHaveCount(6);
  await expect(message(page)).toBeFocused();
  await expect(tab(page, "commit")).toHaveAttribute("aria-selected", "true");

  // The Worktrees tab is the other way back, as the old Back button: the list, that worktree
  // selected in it; the Commit tab then holds the active worktree's again.
  await tab(page, "worktrees").click();
  await worktreeRow(page, "topic").click({ button: "right" });
  await item(page, "Commit…").click();
  await expect(line).toHaveText("topic");
  await tab(page, "worktrees").click();
  await expect(panel(page)).toHaveCount(0);
  await expect(worktreeRow(page, "topic")).toHaveAttribute("aria-selected", "true");
  await tab(page, "commit").click();
  await expect(line).toHaveCount(0);
  await expect(files(page).getByRole("option")).toHaveCount(6);
});

test("at the panel's least width the tab strip and the buttons fit on one line, with room for one more tab", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 700 });
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  for (let i = 0; i < 120; i++) writeFileSync(path.join(repo, `new-${i}.txt`), "new\n");
  await open(page, repo);
  const side = page.locator('[data-region="side-panel"]');
  await expect.poll(async () => (await side.boundingBox())!.width).toBe(240);

  const fits = async () => {
    const s = (await side.boundingBox())!;
    const head = side.locator("header");
    const h = (await head.boundingBox())!;
    expect(h.height).toBeCloseTo(28, 0);
    // Nothing overflows the header.
    expect(await head.evaluate((el) => el.scrollWidth - el.clientWidth)).toBe(0);
    const boxes = [];
    for (const t of ["commit", "pr", "worktrees"] as const) {
      const b = (await tab(page, t).boundingBox())!;
      expect(b.x).toBeGreaterThanOrEqual(s.x);
      expect(b.y).toBeGreaterThanOrEqual(h.y);
      expect(b.y + b.height).toBeLessThanOrEqual(h.y + h.height + 1);
      boxes.push(b);
    }
    expect(boxes[1].x).toBeGreaterThanOrEqual(boxes[0].x + boxes[0].width);
    expect(boxes[2].x).toBeGreaterThanOrEqual(boxes[1].x + boxes[1].width);
    // The shown tab's own buttons on the right, inside the panel and on the same line; no
    // button of the panel's own hides it (the toolbar's does).
    await expect(side.getByRole("button", { name: /hide/i })).toHaveCount(0);
    const buttons = await side.locator("header .actions button").all();
    for (const button of buttons) {
      const b = (await button.boundingBox())!;
      expect(b.x + b.width).toBeLessThanOrEqual(s.x + s.width);
      expect(b.y).toBeGreaterThanOrEqual(h.y);
      expect(b.y + b.height).toBeLessThanOrEqual(h.y + h.height);
    }
    // Between the tabs and the buttons: room for one more tab (Files), and a gap that keeps the
    // two groups apart.
    const last = boxes[2];
    const right = buttons.length > 0 ? (await buttons[0].boundingBox())!.x : s.x + s.width;
    expect(right - (last.x + last.width)).toBeGreaterThanOrEqual(last.width + 2 + 12);
  };
  // The Commit tab (no buttons), the Pull request tab with Refresh, then the Worktrees tab with
  // its own two buttons.
  await fits();
  await expect(side.locator("header .actions button")).toHaveCount(0);
  await tab(page, "pr").click();
  await expect(page.getByTestId("pr-panel")).toBeVisible();
  await fits();
  await expect(side.locator("header .actions button")).toHaveCount(1);
  await tab(page, "worktrees").click();
  await expect(list(page)).toBeVisible();
  await fits();
  await expect(side.locator("header .actions button")).toHaveCount(2);

  // The same strip at the default width.
  await page.setViewportSize({ width: 1280, height: 700 });
  await expect.poll(async () => (await side.boundingBox())!.width).toBe(300);
  await fits();
});

test("the Commit tab's dot: the worktree it would show has files to commit", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("small");
  await open(page, repo);
  gg.git("small", "config", "user.name", "Ada Lovelace");
  gg.git("small", "config", "user.email", "ada@example.com");
  const dot = tab(page, "commit").getByTestId("side-tab-dot");
  await expect(dot).toHaveCount(0);
  await expect(tab(page, "commit")).toHaveAttribute("title", /^Commit \((⌘|Ctrl\+)K\)$/);
  await expect(tab(page, "worktrees")).toHaveAttribute("title", "Worktrees");

  writeFileSync(path.join(repo, "notes.txt"), "new\n");
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(dot).toHaveCount(1);
  await expect(dot).toHaveAttribute("aria-hidden", "true");
  await expect(tab(page, "commit")).toHaveAttribute("title", /^Commit · 1 changed file \((⌘|Ctrl\+)K\)$/);
  // Shown on either tab, at the icon's top right, inside the tab; none on Worktrees.
  await tab(page, "worktrees").click();
  await expect(dot).toHaveCount(1);
  await expect(tab(page, "worktrees").getByTestId("side-tab-dot")).toHaveCount(0);
  const d = (await dot.boundingBox())!;
  const t = (await tab(page, "commit").boundingBox())!;
  const icon = (await tab(page, "commit").locator("svg").boundingBox())!;
  expect(d.x).toBeGreaterThanOrEqual(icon.x + icon.width / 2);
  expect(d.y + d.height).toBeLessThanOrEqual(icon.y + icon.height / 2);
  expect(d.x + d.width).toBeLessThanOrEqual(t.x + t.width);
  expect(d.y).toBeGreaterThanOrEqual(t.y);

  // Committed: it goes.
  await page.keyboard.press("ControlOrMeta+k");
  await message(page).fill("Notes");
  await commitButton(page).click();
  await expect(notice(page, /Committed 1 file\. [0-9a-f]{8} on main/)).toBeVisible();
  await expect(dot).toHaveCount(0);
});

test("the Commit tab's dot follows the worktree the tab holds, another one too", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("commit");
  await open(page, repo);
  const dot = tab(page, "commit").getByTestId("side-tab-dot");
  await expect(dot).toHaveCount(1);
  // The main worktree's files all gone (stashed outside the app): no dot, though topic has one.
  gg.git("commit", "stash", "-q", "-u");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(dot).toHaveCount(0);
  // Topic's commit panel on the Commit tab: its file, its dot.
  await tab(page, "worktrees").click();
  await worktreeRow(page, "topic").locator(".changes").click();
  await expect(page.getByTestId("commit-worktree")).toHaveText("topic");
  await expect(dot).toHaveCount(1);
  await expect(tab(page, "commit")).toHaveAttribute("title", /^Commit · 1 changed file/);
  // Back to the active worktree's: none.
  await page.getByTestId("commit-worktree-return").click();
  await expect(dot).toHaveCount(0);
});

test("the side panel button counts the active worktree's uncommitted files, without moving the toolbar; the count goes with the commit", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("small");
  await open(page, repo);
  // The app commits here: the repository needs an identity (a machine without one refuses).
  gg.git("small", "config", "user.name", "Ada Lovelace");
  gg.git("small", "config", "user.email", "ada@example.com");
  const toggle = page.getByTestId("side-toggle");
  const badge = toggle.getByTestId("commit-badge");
  await expect(badge).toHaveCount(0);
  const before = (await toggle.boundingBox())!;
  const log = (await page.getByTestId("log-toggle").boundingBox())!;

  writeFileSync(path.join(repo, "src", "part1.txt"), "changed\n");
  writeFileSync(path.join(repo, "notes.txt"), "new\n");
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(badge).toHaveText("2");
  await expect(badge).toHaveAttribute("aria-hidden", "true");
  await expect(toggle).toHaveAttribute("title", /\nUncommitted: 1 changed, 1 new$/);
  await expect(tab(page, "commit").getByTestId("side-tab-dot")).toHaveCount(1);
  expect(await toggle.boundingBox()).toEqual(before);
  expect(await page.getByTestId("log-toggle").boundingBox()).toEqual(log);
  // It stays inside the header, over the button's corner.
  const head = (await page.locator(".repo-head").boundingBox())!;
  const b = (await badge.boundingBox())!;
  expect(b.y).toBeGreaterThanOrEqual(head.y);
  expect(b.x + b.width).toBeLessThanOrEqual(head.x + head.width);
  expect(b.x).toBeGreaterThan(before.x);

  await page.keyboard.press("ControlOrMeta+k");
  await expect(message(page)).toBeFocused();
  await message(page).fill("Both");
  await commitButton(page).click();
  await expect(notice(page, /Committed 2 files\. [0-9a-f]{8} on main/)).toBeVisible();
  await expect(badge).toHaveCount(0);
  await expect(tab(page, "commit").getByTestId("side-tab-dot")).toHaveCount(0);
  await expect(toggle).toHaveAttribute("title", /^(Show|Hide) the side panel \((⌘|Ctrl\+)B\)$/);
});
