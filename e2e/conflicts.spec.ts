// What git stopped in: the banner above the graph, its files, Abort, Open in
// editor and Continue, end to end against the real core. The devserver records editor
// launches instead of starting an editor (fixtures: `launches`).
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { type Asked, expect, type GG, runOp, test } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const banner = (page: Page) => page.getByTestId("conflict-banner");
const item = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });
const dialog = (page: Page) => page.getByRole("dialog");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
const fileButton = (page: Page, file: string) => banner(page).getByRole("button", { name: file, exact: true });
const continueButton = (page: Page) => banner(page).getByRole("button", { name: "Continue…" });
/** A red button takes no click just after its dialog opens (see Dialog): wait that out. */
const armed = (page: Page) => page.waitForTimeout(450);


/** Runs git in the fixture, where it may fail (a merge that stops on conflicts exits 1). */
function gitMayFail(gg: GG, ...args: string[]): void {
  try {
    gg.git("conflicts", ...args);
  } catch {
    // Stopped on conflicts.
  }
}

/** Writes `content` to `file` of the fixture, as an editor would. */
function resolve(gg: GG, file: string, content = "resolved\n"): void {
  writeFileSync(path.join(gg.repoPath("conflicts"), file), content);
}

const MERGE_FEATURE: Asked = {
  op: "merge",
  target: { kind: "branch", name: "feature/x" },
  noFf: true,
  squash: false,
  noCommit: false,
};

test("merge_conflict_shows_banner_with_files", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  await expect(banner(page)).toHaveCount(0);

  await grid(page).locator('[data-ref="feature/x"] .name').click({ button: "right" });
  await item(page, "Merge feature/x into main…").click();
  await dialog(page).getByRole("button", { name: "Merge", exact: true }).click();

  await expect(banner(page)).toBeVisible();
  // What comes in is named; the merge message under it would only repeat it.
  await expect(banner(page).getByRole("heading")).toHaveText("Merging feature/x into main");
  await expect(banner(page)).not.toContainText("Merge branch 'feature/x'");
  await expect(banner(page)).toContainText("2 files have conflicts");
  await expect(page.getByTestId("banner-announce")).toHaveText("Merging feature/x into main");
  await expect(fileButton(page, "app.txt")).toBeVisible();
  await expect(fileButton(page, "style.css")).toBeVisible();
  await expect(banner(page).getByRole("button", { name: "Abort…" })).toBeEnabled();
  await expect(banner(page).getByRole("button", { name: "Open in editor" })).toBeEnabled();
  await expect(continueButton(page)).toBeDisabled();
  await expect(continueButton(page)).toHaveAccessibleDescription("2 files not resolved yet.");
  await expect(banner(page).getByRole("listitem").filter({ hasText: "app.txt" })).toContainText("Conflict markers");
  // A stop is no failure: no error notice.
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("continue_disabled_until_resolved_then_finishes", async ({ gg }) => {
  const { page } = gg;
  await page.clock.install();
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  expect((await runOp(page, MERGE_FEATURE)).status).toBe("stopped");
  await expect(continueButton(page)).toBeDisabled();

  // One resolved, outside the app: the banner learns it when the pointer comes to it.
  resolve(gg, "app.txt");
  await page.clock.fastForward(1100);
  await page.mouse.move(0, 0); // out of the banner, so coming back to it is an entry
  await banner(page).hover({ position: { x: 20, y: 10 } });
  await expect(banner(page).getByRole("listitem").filter({ hasText: "app.txt" })).toContainText("Resolved");
  await expect(banner(page)).toContainText("1 of 2 files resolved");
  await expect(continueButton(page)).toBeDisabled();
  await expect(continueButton(page)).toHaveAccessibleDescription("1 file not resolved yet.");

  resolve(gg, "style.css");
  await page.mouse.move(0, 0);
  await page.clock.fastForward(1100); // the banner looks again at most once a second
  await continueButton(page).hover();
  await expect(continueButton(page)).toBeEnabled();
  await expect(banner(page)).toContainText("All conflicts are resolved");

  await continueButton(page).click();
  const confirm = dialog(page);
  await expect(confirm.getByRole("heading")).toHaveText("Continue the merge");
  await expect(confirm).toHaveAccessibleDescription(/These files are staged as they are now/);
  await expect(confirm.getByRole("listitem")).toHaveText(["app.txt", "style.css"]);
  await confirm.getByRole("button", { name: "Stage and continue" }).click();

  await expect(notice(page, "Finished the merge: it is committed.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "log", "-1", "--format=%s").trim()).toBe("Merge branch 'feature/x'");
  expect(gg.git("conflicts", "show", "HEAD:app.txt")).toBe("resolved\n");
});

test("abort_returns_to_before", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  const before = gg.git("conflicts", "rev-parse", "HEAD").trim();
  expect((await runOp(page, MERGE_FEATURE)).status).toBe("stopped");
  resolve(gg, "app.txt");

  // The core asks to confirm, saying what goes back.
  await banner(page).getByRole("button", { name: "Abort…" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm.getByRole("heading")).toHaveText("Abort the merge");
  await expect(confirm.getByTestId("dangers")).toContainText(
    "Aborting the merge puts “main” back as it was before it: the changes made to resolve its conflicts are lost.",
  );
  await armed(page);
  await confirm.getByRole("button", { name: "Abort", exact: true }).click();

  await expect(notice(page, "Aborted the merge: main is back as it was before it.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "rev-parse", "HEAD").trim()).toBe(before);
  expect(gg.git("conflicts", "status", "--porcelain").trim()).toBe("");
});

test("rebase_banner_shows_steps_and_continues_through_two_stops", async ({ gg }) => {
  const { page } = gg;
  gg.git("conflicts", "switch", "-q", "topic");
  await page.clock.install();
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));

  await grid(page).locator('[data-ref="main"] .name').click({ button: "right" });
  await item(page, "Rebase topic onto main…").click();
  await dialog(page).getByRole("button", { name: "Rebase", exact: true }).click();

  await expect(banner(page).getByRole("heading")).toHaveText("Rebasing topic onto main, step 2 of 5");
  await expect(banner(page)).toContainText("Topic two");
  await expect(banner(page)).toContainText("1 file has conflicts. Resolve it in your editor, then continue.");
  await expect(fileButton(page, "app.txt")).toBeVisible();
  resolve(gg, "app.txt");
  // A look the banner took as it appeared (the pointer was there) must not hold this one back.
  await page.clock.fastForward(1100);
  await page.mouse.move(0, 0); // out of the banner, so coming back to it is an entry
  await banner(page).hover({ position: { x: 20, y: 10 } });
  await continueButton(page).click();
  await expect(dialog(page)).toContainText("then the rebase goes on with the next commit");
  await dialog(page).getByRole("button", { name: "Stage and continue" }).click();

  await expect(banner(page).getByRole("heading")).toHaveText("Rebasing topic onto main, step 4 of 5");
  await expect(page.getByTestId("banner-announce")).toHaveText("Rebasing topic onto main, step 4 of 5");
  await expect(banner(page)).toContainText("Topic four");
  await expect(fileButton(page, "style.css")).toBeVisible();
  await expect(continueButton(page)).toBeDisabled();
  resolve(gg, "style.css");
  await page.clock.fastForward(1100);
  await page.mouse.move(0, 0); // out of the banner, so coming back to it is an entry
  await banner(page).hover({ position: { x: 20, y: 10 } });
  await continueButton(page).click();
  await dialog(page).getByRole("button", { name: "Stage and continue" }).click();

  await expect(notice(page, "Finished rebasing topic.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "log", "--format=%s", "-6").trim().split("\n")).toEqual([
    "Topic five",
    "Topic four",
    "Topic three",
    "Topic two",
    "Topic one",
    "Rework on main",
  ]);
});

test("state_started_outside_appears", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  gitMayFail(gg, "merge", "wide");
  await expect(banner(page).getByRole("heading")).toHaveText("Merging wide into main");
  await expect(banner(page)).toContainText("12 files have conflicts");
  await expect(fileButton(page, "src/module-01.ts")).toBeVisible();
  // Past six files, the list scrolls inside the banner.
  const list = banner(page).getByRole("list");
  expect(await list.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);

  gg.git("conflicts", "merge", "--abort");
  await expect(banner(page)).toHaveCount(0);
});

test("banner_does_not_move_the_graph", async ({ gg }) => {
  const { page } = gg;
  // The uncommitted row is there before the banner: nothing is put above the rows in view.
  resolve(gg, "notes.txt", "untracked\n");
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText(/^Uncommitted changes/);
  // A row near the bottom selected, so keeping the selection in view would scroll.
  await grid(page).locator('[role="row"]', { hasText: "Earlier work 32" }).click();
  await expect(page.getByTestId("commit-details")).toBeVisible();
  await grid(page).evaluate((el) => (el.scrollTop = 10 * 26 + 7));
  const frames = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await frames();
  const scrollTop = () => grid(page).evaluate((el) => el.scrollTop);
  const before = await scrollTop();
  expect(before).toBe(10 * 26 + 7);

  gitMayFail(gg, "merge", "wide");
  await expect(banner(page)).toContainText("12 files have conflicts");
  await frames();
  expect(await scrollTop()).toBe(before);

  // The banner gets shorter (fewer files), then goes.
  gg.git("conflicts", "checkout", "--theirs", "--", ...modules(9));
  gg.git("conflicts", "add", "--", ...modules(9));
  await expect(banner(page)).toContainText("3 files have conflicts");
  await frames();
  expect(await scrollTop()).toBe(before);

  gg.git("conflicts", "merge", "--abort");
  await expect(banner(page)).toHaveCount(0);
  await frames();
  expect(await scrollTop()).toBe(before);
});

/** The first `n` of the fixture's twelve modules. */
const modules = (n: number) => Array.from({ length: n }, (_, i) => `src/module-${String(i + 1).padStart(2, "0")}.ts`);

test("open_in_editor_calls_the_core", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("conflicts");
  await page.goto("/");
  await open(page, dir);
  // An editor command of its own, so what is launched is the same on every platform.
  await page.evaluate(async () => {
    const { api } = await import("/src/lib/ipc.ts");
    await api.updateSettings({ editorCommand: "my-editor --wait" });
  });
  expect((await runOp(page, MERGE_FEATURE)).status).toBe("stopped");

  await fileButton(page, "style.css").click();
  await expect.poll(() => gg.launches().length).toBe(1);
  expect(gg.launches()[0]).toEqual(["my-editor", "--wait", path.join(dir, "style.css")]);

  await banner(page).getByRole("button", { name: "Open in editor" }).click();
  await expect.poll(() => gg.launches().length).toBe(2);
  expect(gg.launches()[1]).toEqual(["my-editor", "--wait", dir]);
  // The app never touches the files itself.
  expect(gg.git("conflicts", "diff", "--name-only", "--diff-filter=U").trim().split("\n")).toEqual([
    "app.txt",
    "style.css",
  ]);
});

test("conflicts left by a stash show a quieter banner until they are staged", async ({ gg }) => {
  const { page } = gg;
  resolve(gg, "app.txt", "stashed change\n");
  gg.git("conflicts", "stash", "push", "-q");
  resolve(gg, "app.txt", "committed change\n");
  gg.git("conflicts", "commit", "-qam", "Change the app");
  gitMayFail(gg, "stash", "pop");
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));

  await expect(banner(page).getByRole("heading")).toHaveText("1 file has conflicts");
  await expect(banner(page)).toContainText(
    "Left by applying a stash or an earlier step. Resolve it in your editor. The banner goes once git sees no conflict: stage the file (git add), or reset to the current commit (mixed).",
  );
  await expect(fileButton(page, "app.txt")).toBeVisible();
  await expect(banner(page).getByRole("button", { name: "Open in editor" })).toBeVisible();
  await expect(banner(page).getByRole("button", { name: "Continue…" })).toHaveCount(0);
  await expect(banner(page).getByRole("button", { name: "Abort…" })).toHaveCount(0);

  resolve(gg, "app.txt", "both\n");
  gg.git("conflicts", "add", "app.txt");
  await expect(banner(page)).toHaveCount(0);
});

test("continue_anyway_names_what_is_not_resolved_and_goes_ahead", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  expect((await runOp(page, MERGE_FEATURE)).status).toBe("stopped");
  // app.txt resolved with a Markdown-style underline (no marker); style.css left as git wrote it.
  resolve(gg, "app.txt", "Title\n=======\n");
  await banner(page).hover({ position: { x: 20, y: 10 } });
  await expect(banner(page).getByRole("listitem").filter({ hasText: "app.txt" })).toContainText("Resolved");
  await expect(continueButton(page)).toBeDisabled();

  await banner(page).getByRole("button", { name: "Continue anyway…" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm.getByRole("heading")).toHaveText("Continue the merge anyway?");
  await expect(confirm).toContainText("This file still contains conflict markers");
  await expect(confirm.getByRole("listitem")).toHaveText(["style.css"]);
  await armed(page);
  await confirm.getByRole("button", { name: "Continue anyway" }).click();

  await expect(notice(page, "Finished the merge: it is committed.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "show", "HEAD:style.css")).toContain("<<<<<<< HEAD");
  expect(gg.git("conflicts", "show", "HEAD:app.txt")).toBe("Title\n=======\n");
});

test("a merge subject of someone's own words is shown, though it holds the branch's name", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  try {
    gg.git("conflicts", "merge", "-m", "Bring the wide renames in", "wide");
  } catch {
    // Stopped on conflicts.
  }
  await expect(banner(page).getByRole("heading")).toHaveText("Merging wide into main");
  await expect(banner(page)).toContainText("Bring the wide renames in");
});

test("a cherry-pick that comes out empty is skipped from its notice", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  const hash = gg.git("conflicts", "rev-parse", "feature/x").trim();
  const head = gg.git("conflicts", "rev-parse", "HEAD").trim();
  expect((await runOp(page, { op: "cherry_pick", hash, mainline: null, noCommit: false })).status).toBe("stopped");
  // Resolved by keeping main's side of both files: nothing of the commit is left.
  resolve(gg, "app.txt", "app from main\n");
  resolve(gg, "style.css", "body { color: navy; }\n");
  const failed = await runOp(page, { op: "continue", evenWithMarkers: false });
  expect(failed.status).toBe("failed");
  const alert = page.getByRole("alert").filter({ hasText: "skip this commit, or abort the cherry-pick" });
  await alert.getByRole("button", { name: "Skip this commit" }).click();
  await expect(notice(page, "Skipped the commit; the cherry-pick is finished.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "rev-parse", "HEAD").trim()).toBe(head);
});

test("a commit with no changes left: the banner says so and offers Skip; the notice's Skip goes once git moves on", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  const hash = gg.git("conflicts", "rev-parse", "feature/x").trim();
  const head = gg.git("conflicts", "rev-parse", "HEAD").trim();
  expect((await runOp(page, { op: "cherry_pick", hash, mainline: null, noCommit: false })).status).toBe("stopped");
  resolve(gg, "app.txt", "app from main\n");
  resolve(gg, "style.css", "body { color: navy; }\n");
  expect((await runOp(page, { op: "continue", evenWithMarkers: false })).status).toBe("failed");
  const alert = page.getByRole("alert").filter({ hasText: "skip this commit, or abort the cherry-pick" });
  await expect(alert.getByRole("button", { name: "Skip this commit" })).toBeVisible();

  // The banner no longer says "ready to continue": it offers Skip in Continue's place.
  await expect(banner(page)).toContainText(
    "This commit has no changes left (they are already here): skip it, or abort the cherry-pick.",
  );
  await expect(continueButton(page)).toHaveCount(0);
  await expect(banner(page).getByRole("button", { name: "Skip this commit" })).toBeVisible();

  // Git moves on outside the app: the notice's Skip no longer applies, and goes.
  gg.git("conflicts", "cherry-pick", "--abort");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(banner(page)).toHaveCount(0);
  await expect(alert).toHaveCount(0);

  // Again, and Skip from the banner this time.
  expect((await runOp(page, { op: "cherry_pick", hash, mainline: null, noCommit: false })).status).toBe("stopped");
  resolve(gg, "app.txt", "app from main\n");
  resolve(gg, "style.css", "body { color: navy; }\n");
  gg.git("conflicts", "add", "app.txt", "style.css");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await banner(page).getByRole("button", { name: "Skip this commit" }).click();
  await expect(notice(page, "Skipped the commit; the cherry-pick is finished.")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
  expect(gg.git("conflicts", "rev-parse", "HEAD").trim()).toBe(head);
});
