// The sync control in the toolbar (just before Fetch), end to end against the real core and a local
// bare remote (the `routine` fixture: `main` checked out, level with `origin/main`; a second
// clone that moves the remote): it shows how far the current branch is behind or ahead of its
// upstream, its counts pull and push in one click, and it is absent when there is nothing to do.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, otherClone, test, type GG } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const label = (page: Page, name: string) => grid(page).locator(`[data-ref="${name}"]`).first();
const sync = (page: Page) => page.getByTestId("branch-sync");
const pull = (page: Page) => page.getByTestId("sync-pull");
const push = (page: Page) => page.getByTestId("sync-push");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
const rev = (gg: GG, name: string) => gg.git("routine", "rev-parse", name).trim();

/** The second clone commits `subjects` on `branch` and pushes them; nothing is fetched here. */
function pushFromOther(gg: GG, branch: string, ...subjects: string[]) {
  const other = otherClone(gg.repoPath("routine"));
  gg.gitAt(other, "switch", "-q", branch);
  for (const subject of subjects) {
    writeFileSync(path.join(other, `${subject.replaceAll(" ", "-")}.txt`), `${subject}\n`);
    gg.gitAt(other, "add", "-A");
    gg.gitAt(other, "commit", "-q", "-m", subject);
  }
  gg.gitAt(other, "push", "-q", "origin", branch);
}

/** A commit on the branch checked out here, made outside the app. */
function commitHere(gg: GG, subject: string) {
  writeFileSync(path.join(gg.repoPath("routine"), `${subject.replaceAll(" ", "-")}.txt`), `${subject}\n`);
  gg.git("routine", "add", "-A");
  gg.git("routine", "commit", "-q", "-m", subject);
}

test("behind its upstream: a fetch shows ↓N in the toolbar, and one click pulls it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  // Level with origin/main: nothing shows (staging, behind, is not the current branch).
  await expect(sync(page)).toHaveCount(0);

  pushFromOther(gg, "main", "Remote one", "Remote two");
  await page.getByRole("button", { name: "Fetch", exact: true }).click();
  await expect(pull(page)).toHaveText("2");
  await expect(pull(page)).toHaveAccessibleName("Pull 2 commits from origin/main");
  await expect(pull(page)).toHaveAttribute("title", "Pull 2 commits from origin/main");
  await expect(push(page)).toHaveCount(0);
  // The same count as main's label in the graph.
  await expect(label(page, "main").locator(".ab")).toHaveText("↓2");

  await pull(page).click();
  await expect(notice(page, "Pulled origin/main into main.")).toBeVisible();
  await expect(sync(page)).toHaveCount(0);
  expect(rev(gg, "main")).toBe(rev(gg, "origin/main"));
  expect(gg.git("routine", "log", "-1", "--format=%s").trim()).toBe("Remote two");
});

test("ahead of its upstream: ↑N, and one click pushes it there", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await expect(sync(page)).toHaveCount(0);

  commitHere(gg, "Local work");
  await expect(push(page)).toHaveText("1");
  await expect(push(page)).toHaveAccessibleName("Push 1 commit to origin/main");
  await expect(pull(page)).toHaveCount(0);

  await push(page).click();
  await expect(notice(page, "Pushed main to origin/main.")).toBeVisible();
  await expect(sync(page)).toHaveCount(0);
  expect(gg.git("routine", "ls-remote", "origin", "refs/heads/main").split("\t")[0]).toBe(rev(gg, "main"));
});

test("diverged: both counts, each its own button; the pull asks to merge or rebase", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  commitHere(gg, "Local work");
  pushFromOther(gg, "main", "Remote work");
  gg.git("routine", "fetch", "-q", "origin");
  await expect(push(page)).toHaveText("1");
  await expect(pull(page)).toHaveText("1");
  await expect(push(page)).toHaveAttribute("title", /git refuses the push until you pull them, or force push/);

  // Pushing first: refused by the remote, with what to do next on its notice.
  await push(page).click();
  const refused = page.getByRole("alert").filter({ hasText: "The remote has commits you don't have" });
  await expect(refused).toBeVisible();
  await expect(refused.getByRole("button", { name: "Pull main", exact: true })).toBeVisible();

  // The pull: git's config here says nothing, so the app asks.
  await pull(page).click();
  const asked = page.getByRole("dialog", { name: "Pull origin/main into main" });
  await expect(asked).toBeVisible();
  await asked.getByRole("button", { name: "Pull", exact: true }).click();
  await expect(notice(page, "Pulled origin/main into main.")).toBeVisible();
  // Merged: only the merge and the local commit are left to push.
  await expect(pull(page)).toHaveCount(0);
  await expect(push(page)).toHaveText("2");
});

test("nothing shows for a detached HEAD or a branch without an upstream", async ({ gg }) => {
  const { page } = gg;
  commitHere(gg, "Local work"); // main is 1 ahead
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await expect(push(page)).toHaveText("1");

  gg.git("routine", "switch", "-q", "--detach", "HEAD");
  await expect(page.getByTestId("active-worktree")).toContainText("(detached)");
  await expect(sync(page)).toHaveCount(0);

  gg.git("routine", "switch", "-q", "-c", "topic");
  await expect(page.getByTestId("active-worktree")).toContainText("topic");
  await expect(sync(page)).toHaveCount(0);

  gg.git("routine", "switch", "-q", "main");
  await expect(push(page)).toHaveText("1");
});

test("an upstream of another name: ↑ opens the Push dialog, which names where it goes", async ({ gg }) => {
  const { page } = gg;
  gg.git("routine", "switch", "-q", "-c", "topic", "--track", "origin/main");
  commitHere(gg, "Topic work");
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await expect(push(page)).toHaveAccessibleName("Push 1 commit…");
  await push(page).click();
  const d = page.getByRole("dialog", { name: "Push topic" });
  await expect(d).toBeVisible();
  await expect(d.getByRole("radio", { name: /origin\/main/ })).toBeVisible();
  await d.getByRole("button", { name: "Cancel" }).click();
  // Nothing was pushed.
  expect(rev(gg, "origin/main")).not.toBe(rev(gg, "topic"));
});

test("remotes up and up/stream: stream/main on up is another name, not main on up/stream", async ({ gg }) => {
  const { page } = gg;
  const origin = path.join(path.dirname(gg.repoPath("routine")), "routine-origin.git");
  gg.git("routine", "remote", "add", "up", origin);
  // git remote add refuses a name under another remote's; its config does not.
  gg.git("routine", "config", "remote.up/stream.url", origin);
  gg.git("routine", "config", "remote.up/stream.fetch", "+refs/heads/*:refs/remotes/up/stream/*");
  gg.git("routine", "push", "-q", "up", "main:refs/heads/stream/main");
  gg.git("routine", "fetch", "-q", "up");
  // Both read up/stream/main; this one is stream/main on up.
  gg.git("routine", "config", "branch.main.remote", "up");
  gg.git("routine", "config", "branch.main.merge", "refs/heads/stream/main");
  commitHere(gg, "Local work");
  const before = gg.git("routine", "ls-remote", origin, "refs/heads/main");
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await expect(push(page)).toHaveAccessibleName("Push 1 commit…");
  await push(page).click();
  const d = page.getByRole("dialog", { name: "Push main" });
  await expect(d).toBeVisible();
  await d.getByRole("button", { name: "Cancel" }).click();
  // Nothing was pushed to main.
  expect(gg.git("routine", "ls-remote", origin, "refs/heads/main")).toBe(before);
});
