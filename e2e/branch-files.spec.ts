// The Commit tab's "Committed on branch" against the real core (branch_files): the files the
// branch's commits change from where it left its base, under the commit message; when there is
// none (the default branch, a detached HEAD); the 500 shown and the rest
// counted; open or closed by whether the worktree has changes, the user's click holding until
// that changes; asked for again only when HEAD or the base moves. The `branchfiles` fixture is
// on feature/report, clean, origin's HEAD naming main.
import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, openFolder, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const section = (page: Page) => page.getByTestId("branch-files");
const toggle = (page: Page) => page.getByTestId("branch-files-toggle");
const rows = (page: Page) => page.getByTestId("branch-file");
const files = (page: Page) => page.getByRole("listbox", { name: "Changed files" });
const message = (page: Page) => page.getByTestId("commit-message");

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await expect(page.getByTestId("commit-panel")).toBeVisible();
}

/** Counts the branch_files requests the page sends from now on. */
function countAsks(page: Page): () => number {
  let n = 0;
  page.on("request", (r) => {
    if (r.url().endsWith("/invoke") && (r.postData() ?? "").includes('"cmd":"branch_files"')) n++;
  });
  return () => n;
}

/** The window loses and gets the focus back: the core reads the worktree again at once. */
async function nudge(page: Page) {
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
}

test("the branch's committed files, under the message: name, folder, counts, status; read-only", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchfiles"));
  await expect(section(page)).toBeVisible();
  // Under the commit message, below the uncommitted changes' list.
  const compose = (await message(page).boundingBox())!;
  expect((await toggle(page).boundingBox())!.y).toBeGreaterThan(compose.y + compose.height);
  // Open by default: nothing to commit.
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(toggle(page)).toContainText("Committed on branch");
  await expect(page.getByTestId("branch-files-count")).toHaveText("5");
  await expect(page.getByTestId("branch-files-base")).toHaveText("vs origin/main");
  await expect(toggle(page)).toHaveAttribute("title", /^The files feature\/report's commits change since it left origin\/main \(at [0-9a-f]{7,}\)/);
  await expect(section(page)).toHaveAttribute("data-base", "origin/main");

  // Every file the branch changes, one row for the rename; not main's later work.
  await expect(rows(page)).toHaveCount(5);
  const listed = await rows(page).evaluateAll((els) => els.map((e) => `${e.getAttribute("data-status")} ${e.getAttribute("data-path")}`));
  expect(listed).toEqual(["A assets/logo.bin", "R docs/guide.md", "D legacy.txt", "A src/new.ts", "M src/report.ts"]);
  const report = page.locator('[data-testid="branch-file"][data-path="src/report.ts"]');
  await expect(report.locator(".name")).toHaveText("report.ts");
  await expect(report.locator(".folder")).toHaveText("src");
  await expect(report.locator(".add")).toHaveText("+2");
  await expect(report.locator(".del")).toHaveText("−1");
  await expect(report.locator(".status")).toHaveText("M");
  await expect(report).toHaveAttribute("aria-label", "modified, src/report.ts, 2 added, 1 deleted");
  const renamed = page.locator('[data-testid="branch-file"][data-path="docs/guide.md"]');
  await expect(renamed.locator(".from")).toHaveText("← docs/old-guide.md");
  await expect(renamed).toHaveAttribute("title", "Renamed: docs/old-guide.md → docs/guide.md (+1 −0)");
  // A binary file: no counts.
  const logo = page.locator('[data-testid="branch-file"][data-path="assets/logo.bin"]');
  await expect(logo.locator(".counts")).toHaveCount(0);
  await expect(logo).toHaveAttribute("aria-label", "added, assets/logo.bin, binary");
  // A file at the top: no folder.
  await expect(page.locator('[data-testid="branch-file"][data-path="legacy.txt"] .folder')).toHaveCount(0);

  // Read-only: a row is not a control and opens nothing.
  expect(await rows(page).first().evaluate((el) => el.tabIndex)).toBe(-1);
  await expect(section(page).getByRole("button")).toHaveCount(1);
  const launched = gg.launches().length;
  await rows(page).first().click();
  await rows(page).first().dblclick();
  await page.waitForTimeout(200);
  expect(gg.launches().length).toBe(launched);
  expect(gg.git("branchfiles", "status", "--porcelain")).toBe("");

  // From the keyboard: the header opens and closes it.
  await toggle(page).focus();
  await page.keyboard.press("Enter");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(rows(page)).toHaveCount(0);
  // Closed, the header still counts the files.
  await expect(page.getByTestId("branch-files-count")).toHaveText("5");
  await page.keyboard.press("Space");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
});

test("closed by default with uncommitted changes, open without; the user's click holds until that changes", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("branchfiles");
  await open(page, repo);
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");

  // Clean, closed by the user.
  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");

  // A file written: changes now, closed by default (the clean choice no longer counts).
  writeFileSync(path.join(repo, "scratch.txt"), "x\n");
  await nudge(page);
  await expect(files(page).locator('[data-path="scratch.txt"]')).toBeVisible();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("branch-files-count")).toHaveText("5");
  // Opened by the user; another file written: still changes, it stays open.
  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  writeFileSync(path.join(repo, "scratch2.txt"), "y\n");
  await nudge(page);
  await expect(files(page).locator('[data-path="scratch2.txt"]')).toBeVisible();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  // Closed by the user while there are changes.
  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");

  // Committed from the panel: no changes left, open (the default again), the list grown by the commit.
  await message(page).fill("Scratch notes");
  await page.getByTestId("commit-button").click();
  await expect(page.locator("[data-notices] .toast", { hasText: /Committed 2 files/ })).toBeVisible();
  // Nothing left to commit: the panel goes back to the worktree list, as it does; back to the tab.
  await expect(page.getByTestId("commit-panel")).toHaveCount(0);
  await page.getByTestId("side-tab-commit").click();
  await expect(page.getByTestId("branch-files-count")).toHaveText("7");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(rows(page).filter({ hasText: "scratch.txt" })).toHaveCount(1);

  // A change again: closed by default; gone again: open by default.
  writeFileSync(path.join(repo, "again.txt"), "z\n");
  await nudge(page);
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  rmSync(path.join(repo, "again.txt"));
  await nudge(page);
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  // Not remembered: a new view of the repository starts from the default.
  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await page.reload();
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
});

test("none for the default branch or a detached HEAD; 500 rows and how many more; the branch it was made from without the remote's HEAD", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("branchfiles");
  await open(page, repo);
  await expect(rows(page)).toHaveCount(5);
  /** The branch named at the top of the window. */
  const on = (name: RegExp) => expect(page.getByRole("button", { name })).toBeVisible();

  gg.git("branchfiles", "switch", "-q", "main");
  await on(/^Switch branch: main,/);
  await expect(section(page)).toHaveCount(0);

  gg.git("branchfiles", "switch", "-q", "--detach", "feature/report");
  await on(/^Switch branch: [0-9a-f]+ \(detached\),/);
  await expect(section(page)).toHaveCount(0);

  // bulk: no upstream, the first remote's default branch; 500 rows, then the rest counted.
  gg.git("branchfiles", "switch", "-q", "bulk");
  await expect(page.getByTestId("branch-files-count")).toHaveText("520");
  await expect(page.getByTestId("branch-files-base")).toHaveText("vs origin/main");
  await expect(rows(page)).toHaveCount(500);
  await expect(page.getByTestId("branch-files-more")).toHaveText("and 20 more");
  // The panel keeps the message in reach: the list scrolls inside it.
  const panel = (await page.getByTestId("commit-panel").boundingBox())!;
  const list = (await page.getByTestId("branch-files-list").boundingBox())!;
  expect(list.y + list.height).toBeLessThanOrEqual(panel.y + panel.height + 1);
  await expect(message(page)).toBeInViewport();

  // origin's HEAD forgotten: the branch it was created from (main, by git's records) still
  // says what to compare with.
  gg.git("branchfiles", "remote", "set-head", "origin", "-d");
  gg.git("branchfiles", "switch", "-q", "feature/report");
  await on(/^Switch branch: feature\/report,/);
  await page.keyboard.press("ControlOrMeta+r");
  await expect(page.getByTestId("branch-files-base")).toHaveText("vs origin/main");
  await expect(page.getByTestId("branch-files-count")).toHaveText("5");
});

test("none on main when the remote has no HEAD (pushed with -u, never cloned): main is still the default branch", async ({ gg }) => {
  const { page } = gg;
  // branchy's origin was pushed to, never cloned or fetched from: git keeps no origin/HEAD.
  expect(gg.git("branchy", "for-each-ref", "refs/remotes/origin/HEAD").trim()).toBe("");
  const answers: unknown[] = [];
  page.on("response", async (r) => {
    if (!r.url().endsWith("/invoke") || !(r.request().postData() ?? "").includes('"cmd":"branch_files"')) return;
    answers.push(((await r.json()) as { ok?: unknown }).ok);
  });
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  await expect(page.getByRole("button", { name: /^Switch branch: main,/ })).toBeVisible();
  // The core answered for main: no list (before, "vs origin/develop", the nearest by history).
  await expect.poll(() => answers.length).toBeGreaterThan(0);
  expect(answers).toEqual(answers.map(() => null));
  await expect(section(page)).toHaveCount(0);
  await expect(page.getByTestId("branch-files-base")).toHaveCount(0);

  // A branch made from main there still has it: compared with origin/main.
  gg.git("branchy", "switch", "-q", "develop");
  await expect(page.getByTestId("branch-files-base")).toHaveText("vs origin/main");
});

test("asked for again when HEAD or the base moves, not for a file written or a refresh", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("branchfiles");
  const asks = countAsks(page);
  await open(page, repo);
  await expect(rows(page)).toHaveCount(5);
  await page.waitForTimeout(300);
  const settled = asks();
  expect(settled).toBeGreaterThan(0);

  // A file written (a new status, new snapshots) and a refresh: no new request.
  writeFileSync(path.join(repo, "touched.txt"), "x\n");
  await nudge(page);
  await expect(files(page).locator('[data-path="touched.txt"]')).toBeVisible();
  await page.keyboard.press("ControlOrMeta+r");
  await page.waitForTimeout(500);
  expect(asks()).toBe(settled);
  rmSync(path.join(repo, "touched.txt"));

  // The base moves: main gets the branch's first commit, fetched. The rows follow.
  gg.git("branchfiles", "push", "-q", "origin", "feature/report~1:main", "--force");
  gg.git("branchfiles", "fetch", "-q", "origin");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(page.getByTestId("branch-files-count")).toHaveText("2");
  const afterBase = asks();
  expect(afterBase).toBeGreaterThan(settled);

  // HEAD moves: a commit made outside the app.
  writeFileSync(path.join(repo, "later.txt"), "later\n");
  gg.git("branchfiles", "add", "later.txt");
  gg.git("branchfiles", "commit", "-q", "-m", "Later");
  await nudge(page);
  await expect(page.getByTestId("branch-files-count")).toHaveText("3");
  expect(asks()).toBeGreaterThan(afterBase);
});

test("none while the branch is where its base is, then the list once the base moves back (review)", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("branchfiles");
  // A branch at main's commit, with no upstream: the base is origin/main, at the same commit.
  gg.git("branchfiles", "switch", "-q", "-c", "same", "main");
  await open(page, repo);
  await expect(page.getByRole("button", { name: /^Switch branch: same,/ })).toBeVisible();
  await page.waitForTimeout(300);
  await expect(section(page)).toHaveCount(0);
  // origin's main goes back one commit: the branch now has one of its own.
  gg.git("branchfiles", "push", "-q", "--force", "origin", "main~1:main");
  gg.git("branchfiles", "fetch", "-q", "origin");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(page.getByTestId("branch-files-count")).toHaveText("1");
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toHaveAttribute("data-path", "main-only.txt");
});

test("a read that fails says so with Retry, keeping what was shown; with nothing shown, the header and that line (review)", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("branchfiles");
  // Every branch_files request fails while `failing` is set.
  let failing = true;
  await page.route(
    (url) => url.pathname.endsWith("/invoke"),
    async (route) => {
      if (failing && (route.request().postData() ?? "").includes('"cmd":"branch_files"')) {
        await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ err: { code: "git_failed", message: "git merge-base failed: bad object" } }) });
      } else {
        await route.fallback();
      }
    },
  );
  await open(page, repo);
  // Nothing read yet: the header and the reason, with Retry; no count, no rows.
  const error = page.getByTestId("branch-files-error");
  await expect(error).toContainText("Could not read the branch's files: git merge-base failed: bad object");
  await expect(toggle(page)).toHaveText("Committed on branch");
  await expect(page.getByTestId("branch-files-count")).toHaveCount(0);
  await expect(rows(page)).toHaveCount(0);
  // Retry, still failing: still said. Then working: the list, the line gone.
  await page.getByTestId("branch-files-retry").click();
  await expect(error).toBeVisible();
  failing = false;
  await page.getByTestId("branch-files-retry").click();
  await expect(rows(page)).toHaveCount(5);
  await expect(error).toHaveCount(0);

  // A later read failing (HEAD moved): the list as it was stays, with the line and Retry.
  failing = true;
  writeFileSync(path.join(repo, "later.txt"), "later\n");
  gg.git("branchfiles", "add", "later.txt");
  gg.git("branchfiles", "commit", "-q", "-m", "Later");
  await nudge(page);
  await expect(error).toBeVisible();
  await expect(rows(page)).toHaveCount(5);
  await expect(page.getByTestId("branch-files-count")).toHaveText("5");
  failing = false;
  await page.getByTestId("branch-files-retry").click();
  await expect(page.getByTestId("branch-files-count")).toHaveText("6");
  await expect(error).toHaveCount(0);
});
