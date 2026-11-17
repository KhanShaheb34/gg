// The staging routine of spec §5.3, end to end against the real core and a local bare remote
// (the `routine` fixture: `staging` 2 behind `origin/staging`, a second clone that moves the
// remote): check out staging (fast-forwarding it), merge a feature branch, push; a rejected
// push and what follows it; a force push and its confirmation; the merge dialog's memory;
// deleting branches; operations queued behind one another.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, otherClone, test, type GG } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const label = (page: Page, name: string) => grid(page).locator(`[data-ref="${name}"]`).first();
const item = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });
const dialog = (page: Page) => page.getByRole("dialog");
const alertDialog = (page: Page) => page.getByRole("alertdialog");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
const failure = (page: Page, text: string | RegExp) => page.getByRole("alert").filter({ hasText: text });
/** A red button takes no click just after its dialog opens (see Dialog): wait that out. */
const armed = (page: Page) => page.waitForTimeout(450);

/** Opens the menu of label `name` and picks `item`. */
async function pickOnLabel(page: Page, name: string, itemName: string) {
  await label(page, name).locator(".name").click({ button: "right" });
  await item(page, itemName).click();
}

/** The commit `rev` names in the remote (`routine-origin.git`). */
function remoteRef(gg: GG, rev: string): string {
  return gg.git("routine", "--git-dir", "../routine-origin.git", "rev-parse", rev).trim();
}

/** Checks out `staging` from its menu without fast-forwarding it: it stays 2 behind. */
async function checkOutStagingAsIs(page: Page) {
  await pickOnLabel(page, "staging", "Check out");
  await dialog(page).getByRole("button", { name: "No", exact: true }).click();
  await expect(label(page, "staging")).toHaveClass(/current/);
  await expect(label(page, "staging").locator(".ab")).toHaveText("↓2");
}

/** A commit on `staging` made outside the app. */
function commitOutside(gg: GG, subject: string) {
  writeFileSync(path.join(gg.repoPath("routine"), `${subject.replaceAll(" ", "-")}.txt`), `${subject}\n`);
  gg.git("routine", "add", "-A");
  gg.git("routine", "commit", "-q", "-m", subject);
}

test("checkout_staging_offers_fast_forward_then_merge_then_push", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await expect(label(page, "staging").locator(".ab")).toHaveText("↓2");
  // The commits that came from the other clone are someone else's.
  expect(gg.git("routine", "log", "-1", "--format=%an <%ae>", "origin/staging").trim()).toBe(
    "Grace Hopper <grace@example.com>",
  );

  // Check out: staging is behind its upstream and not ahead, so the app asks first.
  await pickOnLabel(page, "staging", "Check out");
  const ask = dialog(page);
  await expect(ask.getByRole("heading")).toHaveText("Fast-forward 2 commits first?");
  // A dialog of words only is described by them.
  await expect(ask).toHaveAccessibleDescription(/staging is 2 commits behind origin\/staging/);
  await expect(ask.getByRole("button", { name: "Yes", exact: true })).toBeFocused();
  await expect(ask.getByRole("button", { name: "No", exact: true })).toBeVisible();
  await expect(ask.getByRole("button", { name: "Cancel" })).toBeVisible();
  await ask.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(notice(page, "Checked out staging. It was fast-forwarded to origin/staging.")).toBeVisible();
  await expect(label(page, "staging")).toHaveClass(/current/);
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
  expect(gg.git("routine", "rev-parse", "staging").trim()).toBe(remoteRef(gg, "staging"));

  // Merge the feature branch: a merge commit, then the notice offers to push.
  await pickOnLabel(page, "feature/login", "Merge feature/login into staging…");
  const merge = dialog(page);
  await expect(merge.getByRole("heading")).toHaveText("Merge feature/login into staging");
  await expect(merge.getByRole("checkbox", { name: /Create a merge commit even if fast-forward is possible/ })).toBeChecked();
  await merge.getByRole("button", { name: "Merge", exact: true }).click();
  const merged = notice(page, "Merged feature/login into staging.");
  await expect(merged).toBeVisible();
  await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText("Merge branch 'feature/login' into staging");
  await expect(label(page, "staging").locator(".ab")).toHaveText("↑3");

  await merged.getByRole("button", { name: "Push staging to origin/staging", exact: true }).click();
  await expect(notice(page, "Pushed staging to origin/staging.")).toBeVisible();
  // The counts follow without a refresh.
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
  expect(remoteRef(gg, "staging")).toBe(gg.git("routine", "rev-parse", "staging").trim());
});

test("the merge notice's push waits for the user, is reached from the keyboard, and goes when another operation runs", async ({
  gg,
}) => {
  const { page } = gg;
  await page.clock.install();
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await pickOnLabel(page, "staging", "Check out");
  await dialog(page).getByRole("button", { name: "Yes", exact: true }).click();
  await expect(label(page, "staging")).toHaveClass(/current/);
  await pickOnLabel(page, "feature/login", "Merge feature/login into staging…");
  await dialog(page).getByRole("button", { name: "Merge", exact: true }).click();
  const merged = notice(page, "Merged feature/login into staging.");
  const push = merged.getByRole("button", { name: "Push staging to origin/staging", exact: true });
  await expect(push).toBeVisible();

  // Long after other notices have gone, it is still there.
  await page.clock.fastForward(12_000);
  await expect(push).toBeVisible();

  // Tab reaches it from the page…
  await grid(page).focus();
  const inNotices = () => page.evaluate(() => !!document.activeElement?.closest("[data-notices]"));
  for (let i = 0; i < 40 && !(await inNotices()); i++) await page.keyboard.press("Tab");
  expect(await inNotices()).toBe(true);
  // …and F6 goes straight to its first button.
  await grid(page).focus();
  await page.keyboard.press("F6");
  await expect(push).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(notice(page, "Pushed staging to origin/staging.")).toBeVisible();
  await expect(merged).toHaveCount(0);
  expect(remoteRef(gg, "staging")).toBe(gg.git("routine", "rev-parse", "staging").trim());

  // A rejected push's notice stays too, until the next operation runs here.
  commitOutside(gg, "Local tweak");
  const other = otherClone(gg.repoPath("routine"));
  gg.gitAt(other, "fetch", "-q", "origin");
  gg.gitAt(other, "reset", "-q", "--hard", "origin/staging");
  gg.gitAt(other, "commit", "-q", "--allow-empty", "-m", "Theirs");
  gg.gitAt(other, "push", "-q", "origin", "HEAD:staging");
  await page.getByRole("button", { name: "Fetch" }).click();
  await expect(label(page, "staging").locator(".ab")).toHaveText("↑1 ↓1");
  await pickOnLabel(page, "staging", "Push 1 commit…");
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  const rejected = failure(page, "The remote has commits you don't have");
  await expect(rejected).toBeVisible();
  await page.clock.fastForward(12_000);
  await expect(rejected).toBeVisible();
  await pickOnLabel(page, "main", "Check out");
  await expect(label(page, "main")).toHaveClass(/current/);
  await expect(rejected).toHaveCount(0);
});

test("push_rejected_then_pull_then_push", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await checkOutStagingAsIs(page);
  commitOutside(gg, "Local tweak");
  await expect(label(page, "staging").locator(".ab")).toHaveText("↑1 ↓2");

  await pickOnLabel(page, "staging", "Push 1 commit…");
  await expect(dialog(page).getByText("origin/staging", { exact: true })).toBeVisible();
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  const rejected = failure(page, "The remote has commits you don't have");
  await expect(rejected).toBeVisible();
  await expect(rejected.getByRole("button")).toContainText(["Fetch", "Pull staging", "Force push staging to origin/staging…"]);

  // Pull: the branches have diverged and git's config does not say how to reconcile them.
  await rejected.getByRole("button", { name: "Pull staging", exact: true }).click();
  const choose = dialog(page);
  await expect(choose.getByRole("heading")).toHaveText("Pull origin/staging into staging");
  await expect(choose.getByText("This branch and its upstream have diverged")).toBeVisible();
  await expect(choose.getByRole("radio", { name: /Merge/ })).toBeChecked();
  await choose.getByRole("button", { name: "Pull", exact: true }).click();
  await expect(notice(page, "Pulled origin/staging into staging.")).toBeVisible();
  await expect(label(page, "staging").locator(".ab")).toHaveText("↑2");

  await pickOnLabel(page, "staging", "Push 2 commits…");
  await dialog(page).getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, "Pushed staging to origin/staging.")).toBeVisible();
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
  expect(remoteRef(gg, "staging")).toBe(gg.git("routine", "rev-parse", "staging").trim());
  expect(gg.git("routine", "log", "-1", "--format=%s", "staging").trim()).toMatch(/^Merge branch 'staging'/);
});

test("force_push_needs_the_second_confirmation", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await checkOutStagingAsIs(page);
  commitOutside(gg, "Local rewrite");
  await expect(label(page, "staging").locator(".ab")).toHaveText("↑1 ↓2");
  const before = remoteRef(gg, "staging");

  // The push dialog's force choice turns its button red; the core's confirmation follows.
  await pickOnLabel(page, "staging", "Push 1 commit…");
  const push = dialog(page);
  await expect(push.getByRole("checkbox", { name: /Force push \(with lease\)/ })).not.toBeChecked();
  await push.getByRole("checkbox", { name: /Force push \(with lease\)/ }).check();
  // Now a dangerous dialog (red button), then the core's confirmation.
  const confirm = alertDialog(page);
  await armed(page);
  await confirm.getByRole("button", { name: "Force push", exact: true }).click();
  await expect(confirm.getByTestId("dangers")).toContainText(
    "A force push replaces origin/staging with staging, discarding 2 commits there",
  );
  await expect(confirm.getByTestId("dangers")).toContainText("Hotfix two");
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(alertDialog(page)).toHaveCount(0);
  expect(remoteRef(gg, "staging")).toBe(before);

  // Again; meanwhile the remote moves and is fetched: the core asks again, with the new count.
  await pickOnLabel(page, "staging", "Push 1 commit…");
  await push.getByRole("checkbox", { name: /Force push \(with lease\)/ }).check();
  await armed(page);
  await confirm.getByRole("button", { name: "Force push", exact: true }).click();
  await expect(confirm.getByTestId("dangers")).toContainText("discarding 2 commits there");
  const other = otherClone(gg.repoPath("routine"));
  writeFileSync(path.join(other, "hotfix-three.txt"), "three\n");
  gg.gitAt(other, "add", "-A");
  gg.gitAt(other, "commit", "-q", "-m", "Hotfix three");
  gg.gitAt(other, "push", "-q", "origin", "staging");
  gg.git("routine", "fetch", "-q", "origin");
  await armed(page);
  await confirm.getByRole("button", { name: "Force push" }).click();
  await expect(confirm.getByTestId("dangers")).toContainText("discarding 3 commits there");
  // Asked again, it says so (R39).
  await expect(confirm.getByTestId("confirm-again")).toHaveText("This changed since you confirmed it. Here it is as it is now.");
  await armed(page);
  await confirm.getByRole("button", { name: "Force push" }).click();
  await expect(notice(page, "Force-pushed staging to origin/staging.")).toBeVisible();
  expect(remoteRef(gg, "staging")).toBe(gg.git("routine", "rev-parse", "staging").trim());
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
});

test("merge_dialog_remembers_no_ff_per_repository", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const box = () => dialog(page).getByRole("checkbox", { name: /Create a merge commit even if fast-forward is possible/ });

  // Squash rules the merge commit out; Cancel saves nothing.
  await pickOnLabel(page, "staging", "Merge staging into main…");
  await expect(box()).toBeChecked();
  await dialog(page).getByRole("checkbox", { name: /Squash/ }).check();
  await expect(box()).toBeDisabled();
  await dialog(page).getByRole("checkbox", { name: /Squash/ }).uncheck();
  await box().uncheck();
  await dialog(page).getByRole("button", { name: "Cancel" }).click();
  await pickOnLabel(page, "staging", "Merge staging into main…");
  await expect(box()).toBeChecked();

  // Unticked and merged: a fast-forward, and remembered for this repository.
  await box().uncheck();
  await dialog(page).getByRole("button", { name: "Merge", exact: true }).click();
  await expect(notice(page, /Fast-forwarded|Merged staging into main/)).toBeVisible();
  expect(gg.git("routine", "rev-parse", "main").trim()).toBe(gg.git("routine", "rev-parse", "staging").trim());
  await pickOnLabel(page, "feature/login", "Merge feature/login into main…");
  await expect(box()).not.toBeChecked();
  await dialog(page).getByRole("button", { name: "Cancel" }).click();

  // Another repository has its own.
  await open(page, gg.repoPath("small"));
  await pickOnLabel(page, "fix/typo", "Merge fix/typo into main…");
  await expect(box()).toBeChecked();
  await dialog(page).getByRole("button", { name: "Cancel" }).click();
});

test("the merge dialog never writes the default over a stored choice it has not read, nor saves a refused merge's", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const prefs = () =>
    page.evaluate(async () => {
      const { api } = await import("/src/lib/ipc.ts");
      const { app } = await import("/src/lib/state/app.svelte.ts");
      return (await api.getRepoPrefs(app.activeRepoId!)).mergeNoFf;
    });
  await page.evaluate(async () => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    await api.setRepoPrefs(app.activeRepoId!, { mergeNoFf: false });
  });

  // The stored choice has not arrived when Enter is pressed: nothing is saved.
  let release: () => void = () => {};
  const held = new Promise<void>((r) => (release = r));
  await page.route("**/invoke", async (route) => {
    if ((route.request().postDataJSON() as { cmd: string } | null)?.cmd === "get_repo_prefs") await held;
    return route.continue();
  });
  await pickOnLabel(page, "feature/login", "Merge feature/login into main…");
  // The dialog is up and has the focus before Enter goes to it.
  await expect(dialog(page)).toBeVisible();
  await expect.poll(() => dialog(page).evaluate((d) => d.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Enter");
  await expect(notice(page, /Merged feature\/login into main/)).toBeVisible();
  release();
  await page.unrouteAll({ behavior: "wait" });
  expect(await prefs()).toBe(false);

  // Changed by hand, but the core refuses the request before it runs: nothing is saved.
  // Diagnosis of a rare flake (Task 9 review minor 10): the right-click below once waited
  // 30 s for the label to be "stable". Tell a page that stopped drawing frames from a label
  // that keeps moving.
  const frame = await page.evaluate(
    () =>
      new Promise<string>((resolve) => {
        requestAnimationFrame(() => resolve("drawn"));
        setTimeout(() => resolve("no frame for 2 s"), 2000);
      }),
  );
  expect(frame, "the page stopped rendering").toBe("drawn");
  await pickOnLabel(page, "staging", "Merge staging into main…");
  const box = dialog(page).getByRole("checkbox", { name: /Create a merge commit even if fast-forward is possible/ });
  await expect(box).not.toBeChecked();
  await box.check();
  // Routed only now, just before the request it answers.
  await page.route("**/invoke", (route) => {
    if ((route.request().postDataJSON() as { cmd: string } | null)?.cmd === "run_op") {
      return route.fulfill({
        json: { err: { code: "bad_request", message: "That is not a branch.", detail: null } },
      });
    }
    return route.continue();
  });
  await dialog(page).getByRole("button", { name: "Merge", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "That is not a branch." })).toBeVisible();
  await page.unrouteAll({ behavior: "wait" });
  expect(await prefs()).toBe(false);
});

test("delete_unmerged_branch_double_confirm", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  // Merged into its upstream: the dialog alone.
  await pickOnLabel(page, "feature/login", "Delete…");
  await expect(dialog(page).getByRole("heading")).toHaveText("Delete branch feature/login");
  await dialog(page).getByRole("button", { name: "Delete branch" }).click();
  await expect(notice(page, "Deleted branch feature/login.")).toBeVisible();
  await expect(alertDialog(page)).toHaveCount(0);
  expect(gg.git("routine", "branch", "--list", "feature/login").trim()).toBe("");

  // A commit no other branch has: the core's confirmation follows, and says what is lost.
  gg.git("routine", "switch", "-q", "-c", "spike");
  commitOutside(gg, "Try an idea");
  gg.git("routine", "switch", "-q", "main");
  await expect(label(page, "spike")).toBeVisible();
  await pickOnLabel(page, "spike", "Delete…");
  await dialog(page).getByRole("button", { name: "Delete branch" }).click();
  const confirm = alertDialog(page);
  await expect(confirm.getByTestId("dangers")).toContainText(
    "Branch “spike” has 1 commit not merged into main: deleting it loses it.",
  );
  await armed(page);
  await confirm.getByRole("button", { name: "Delete branch" }).click();
  await expect(notice(page, "Deleted branch spike.")).toBeVisible();
  expect(gg.git("routine", "branch", "--list", "spike").trim()).toBe("");
  await expect(grid(page).locator('[data-ref="spike"]')).toHaveCount(0);
});

test("actions_queue_while_one_runs", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("routine");
  await open(page, dir);
  // Each checkout runs this hook, which takes a while.
  const hook = path.join(dir, ".git/hooks/post-checkout");
  writeFileSync(hook, "#!/bin/sh\nsleep 2\n", { mode: 0o755 });

  // Check out feature/login, and in the same instant staging, fast-forwarding it: they run
  // in the order they were sent (R28), the second waiting behind the first.
  const outcomes = page.evaluate(async () => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const repoId = app.activeRepoId!;
    const expectWorktree = findRepoView(repoId)!.snapshot.activeWorktree;
    return Promise.all([
      ops.runOp(repoId, { op: "checkout_branch", branch: "feature/login", fastForward: false, expectWorktree }),
      ops.runOp(repoId, { op: "checkout_branch", branch: "staging", fastForward: true, expectWorktree }),
    ]);
  });
  const busy = page.getByTestId("busy");
  await expect(busy).toContainText("feature/login");
  await expect(busy).toContainText("+1");

  const [first, second] = await outcomes;
  expect(first.status).toBe("done");
  expect(second.status).toBe("done");
  expect(second.opId).toBeGreaterThan(first.opId);
  expect(second.message).toBe("Checked out staging. It was fast-forwarded to origin/staging.");
  // The repository's own record of what happened, newest first.
  expect(gg.git("routine", "reflog", "-n", "3", "--format=%gs", "HEAD").trim().split("\n")).toEqual([
    "merge @{upstream}: Fast-forward",
    "checkout: moving from feature/login to staging",
    "checkout: moving from main to feature/login",
  ]);
  await expect(busy).toHaveCount(0);
  await expect(label(page, "staging")).toHaveClass(/current/);
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
});
