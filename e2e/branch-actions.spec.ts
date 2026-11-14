// Every local- and remote-branch menu item, end to end against the real core and a local
// bare remote (the `routine` fixture: `main` checked out; `staging` 2 behind
// `origin/staging`; `feature/login` level with its upstream; a second clone that moves the
// remote): what each dialog offers, what git did, and the labels afterwards.
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
const rowOf = (page: Page, subject: string) =>
  grid(page).locator('[role="row"]', { has: page.locator(".subject", { hasText: new RegExp(`^${esc(subject)}$`) }) });
const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const label = (page: Page, name: string) => grid(page).locator(`[data-ref="${name}"]`).first();
const menus = (page: Page) => page.getByRole("menu");
/** The generation of the rows the active tab shows. */
function generation(page: Page): Promise<number> {
  return page.evaluate(async () => {
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return findRepoView(app.activeRepoId!)!.snapshot.generation;
  });
}

const item = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });
const dialog = (page: Page) => page.getByRole("dialog");
const alertDialog = (page: Page) => page.getByRole("alertdialog");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
/** A red button takes no click just after its dialog opens (see Dialog): wait that out. */
const armed = (page: Page) => page.waitForTimeout(450);

/** Opens the menu of label `name` (its remote part with `remote`) and picks `itemName`. */
async function pickOnLabel(page: Page, name: string, itemName: string, part: ".name" | ".remotes" = ".name") {
  await label(page, name).locator(part).click({ button: "right" });
  await item(page, itemName).click();
}

/** A commit on the branch checked out, made outside the app. */
function commitOutside(gg: GG, subject: string) {
  writeFileSync(path.join(gg.repoPath("routine"), `${subject.replaceAll(" ", "-")}.txt`), `${subject}\n`);
  gg.git("routine", "add", "-A");
  gg.git("routine", "commit", "-q", "-m", subject);
}

/** The second clone pushes a new branch `name` to the remote, which this repository fetches. */
function pushFromOther(gg: GG, name: string) {
  const other = otherClone(gg.repoPath("routine"));
  gg.gitAt(other, "switch", "-q", "-c", name, "origin/main");
  writeFileSync(path.join(other, `${name}.txt`), `${name}\n`);
  gg.gitAt(other, "add", "-A");
  gg.gitAt(other, "commit", "-q", "-m", `Start ${name}`);
  gg.gitAt(other, "push", "-q", "origin", name);
  gg.git("routine", "fetch", "-q", "origin");
}

const current = (gg: GG) => gg.git("routine", "branch", "--show-current").trim();

test("check out: a level branch directly; double-clicking a label does the same, and a click still selects", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await pickOnLabel(page, "feature/login", "Check out");
  await expect(notice(page, "Checked out feature/login.")).toBeVisible();
  await expect(dialog(page)).toHaveCount(0);
  await expect(label(page, "feature/login")).toHaveClass(/current/);
  expect(current(gg)).toBe("feature/login");

  // A single click on a label selects its row, nothing more.
  await label(page, "main").locator(".name").click();
  await expect(rowOf(page, "Initial commit")).toHaveAttribute("aria-selected", "true");
  await page.waitForTimeout(400);
  expect(current(gg)).toBe("feature/login");
  // A double click checks it out.
  await label(page, "main").locator(".name").dblclick();
  await expect(notice(page, "Checked out main.")).toBeVisible();
  await expect(label(page, "main")).toHaveClass(/current/);
  await expect(rowOf(page, "Initial commit")).toHaveAttribute("aria-selected", "true");
  expect(current(gg)).toBe("main");
  // On a remote branch's label it does nothing.
  await label(page, "origin/staging").locator(".name").dblclick();
  await page.waitForTimeout(400);
  await expect(dialog(page)).toHaveCount(0);
  expect(current(gg)).toBe("main");
});

test("the fast-forward question: Cancel does nothing, No checks out as is; a fast-forward no longer possible is said", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await label(page, "staging").locator(".name").dblclick();
  const ask = dialog(page);
  await expect(ask.getByRole("heading")).toHaveText("Fast-forward 2 commits first?");
  await expect(ask).toContainText("staging is 2 commits behind origin/staging.");
  await ask.getByRole("button", { name: "Cancel" }).click();
  await expect(ask).toHaveCount(0);
  expect(current(gg)).toBe("main");

  await pickOnLabel(page, "staging", "Check out");
  await ask.getByRole("button", { name: "No", exact: true }).click();
  await expect(notice(page, "Checked out staging.")).toBeVisible();
  await expect(label(page, "staging").locator(".ab")).toHaveText("↓2");
  expect(current(gg)).toBe("staging");

  // Back on main; staging gets a commit of its own while the question is open: Yes checks it
  // out, and the notice says why it could not be fast-forwarded.
  await label(page, "main").locator(".name").dblclick();
  await expect(label(page, "main")).toHaveClass(/current/);
  await label(page, "staging").locator(".name").dblclick();
  await expect(ask.getByRole("heading")).toHaveText("Fast-forward 2 commits first?");
  const tree = gg.git("routine", "rev-parse", "staging^{tree}").trim();
  const own = gg.git("routine", "commit-tree", tree, "-p", "staging", "-m", "Own work").trim();
  gg.git("routine", "update-ref", "refs/heads/staging", own);
  await ask.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(
    notice(page, "It could not be fast-forwarded to origin/staging: staging has commits of its own"),
  ).toBeVisible();
  expect(current(gg)).toBe("staging");
});

test("rename a branch: the name is checked as it is typed", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await pickOnLabel(page, "feature/login", "Rename…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Rename branch feature/login");
  const field = d.getByRole("textbox", { name: "New name" });
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("feature/login");
  await expect(d.getByRole("button", { name: "Rename" })).toBeDisabled();
  await page.keyboard.type("staging"); // the whole name was selected: typed over
  await expect(field).toHaveValue("staging");
  await expect(d.getByText("A branch named “staging” already exists.")).toBeVisible();
  await expect(d.getByRole("button", { name: "Rename" })).toBeDisabled();
  await field.fill("feature/sign-in");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Renamed branch feature/login to feature/sign-in.")).toBeVisible();
  await expect(label(page, "feature/sign-in")).toBeVisible();
  expect(gg.git("routine", "branch", "--list", "feature/login").trim()).toBe("");
});

test("merge: squash stages the changes; no commit stops before committing; from a remote branch's menu", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await pickOnLabel(page, "origin/staging", "Merge origin/staging into main…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Merge origin/staging into main");
  await d.getByRole("checkbox", { name: /Squash/ }).check();
  await d.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(notice(page, "Squashed origin/staging into staged changes on main: commit them to finish.")).toBeVisible();
  expect(gg.git("routine", "diff", "--cached", "--name-only").trim().split("\n")).toContain("staging.txt");
  gg.git("routine", "reset", "-q", "--hard");

  await pickOnLabel(page, "feature/login", "Merge feature/login into main…");
  await d.getByRole("checkbox", { name: /No commit/ }).check();
  await d.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(notice(page, /Merged feature\/login into main without committing/)).toBeVisible();
  expect(gg.git("routine", "rev-parse", "-q", "--verify", "MERGE_HEAD").trim()).not.toBe("");
});

test("rebase the current branch onto another: a one-line confirmation", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  commitOutside(gg, "Main moves on");
  await expect(rowOf(page, "Main moves on")).toBeVisible();
  await label(page, "feature/login").locator(".name").dblclick();
  await expect(label(page, "feature/login")).toHaveClass(/current/);

  await pickOnLabel(page, "main", "Rebase feature/login onto main…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Rebase feature/login onto main");
  await expect(d).toContainText("new hashes");
  await d.getByRole("button", { name: "Rebase", exact: true }).click();
  await expect(notice(page, "Rebased feature/login onto main.")).toBeVisible();
  expect(gg.git("routine", "merge-base", "--is-ancestor", "main", "feature/login").trim()).toBe("");
  await expect(label(page, "feature/login").locator(".ab")).toHaveText(/↑\d+ ↓\d+/);
});

test("push a branch without an upstream: it gets one; the target is said plainly", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  gg.git("routine", "switch", "-q", "-c", "topic");
  commitOutside(gg, "Topic work");
  await expect(label(page, "topic")).toBeVisible();

  await pickOnLabel(page, "topic", "Push…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Push topic");
  await expect(d.getByRole("combobox", { name: "Remote" })).toHaveValue("origin");
  await expect(d.getByText("origin/topic", { exact: true })).toBeVisible();
  const upstream = d.getByRole("checkbox", { name: /Set as its upstream/ });
  await expect(upstream).toBeChecked();
  await expect(upstream).toBeDisabled();
  await expect(d.getByRole("checkbox", { name: /Force push/ })).not.toBeChecked();
  await d.getByRole("button", { name: "Push", exact: true }).click();
  await expect(notice(page, "Pushed topic to origin, and set origin/topic as its upstream.")).toBeVisible();
  await expect(label(page, "topic")).toContainText("origin");
  expect(gg.git("routine", "rev-parse", "--abbrev-ref", "topic@{upstream}").trim()).toBe("origin/topic");
});

test("pull: another branch is fast-forwarded without being checked out; the current one is pulled", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await pickOnLabel(page, "staging", "Pull 2 commits");
  await expect(notice(page, /Fast-forwarded staging to origin\/staging \([0-9a-f]+\), without checking it out\./)).toBeVisible();
  await expect(label(page, "staging").locator(".ab")).toHaveText("");
  expect(current(gg)).toBe("main");

  // The other clone moves main: fetched, main is behind; pulling it brings it level.
  const other = otherClone(gg.repoPath("routine"));
  gg.gitAt(other, "switch", "-q", "main");
  gg.gitAt(other, "commit", "-q", "--allow-empty", "-m", "Main elsewhere");
  gg.gitAt(other, "push", "-q", "origin", "main");
  gg.git("routine", "fetch", "-q", "origin");
  await expect(label(page, "main").locator(".ab")).toHaveText("↓1");
  await pickOnLabel(page, "main", "Pull 1 commit");
  await expect(notice(page, "Pulled origin/main into main.")).toBeVisible();
  await expect(label(page, "main").locator(".ab")).toHaveText("");
  expect(gg.git("routine", "log", "-1", "--format=%s").trim()).toBe("Main elsewhere");
});

test("check out a remote branch as a local one; an existing local name offers to check that out", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  // feature/login exists here: the dialog offers to check it out instead.
  await pickOnLabel(page, "feature/login", "Check out as a local branch…", ".remotes");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Check out origin/feature/login as a local branch");
  const field = d.getByRole("textbox", { name: "Local branch name" });
  await expect(field).toHaveValue("feature/login");
  await expect(d.getByText("A branch named feature/login exists here: check it out instead, or choose another name.")).toBeVisible();
  await d.getByRole("button", { name: "Check out feature/login" }).click();
  await expect(notice(page, "Checked out feature/login.")).toBeVisible();
  expect(current(gg)).toBe("feature/login");

  // A new one tracks the remote branch.
  pushFromOther(gg, "release");
  await expect(label(page, "origin/release")).toBeVisible();
  await pickOnLabel(page, "origin/release", "Check out as a local branch…");
  await expect(field).toHaveValue("release");
  await d.getByRole("button", { name: "Check out", exact: true }).click();
  await expect(notice(page, "Checked out origin/release as release, which tracks it.")).toBeVisible();
  await expect(label(page, "release")).toHaveClass(/current/);
  expect(gg.git("routine", "rev-parse", "--abbrev-ref", "release@{upstream}").trim()).toBe("origin/release");
});

test("fetch a remote branch into a local one, prefilled with its name", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  pushFromOther(gg, "release");
  await expect(label(page, "origin/release")).toBeVisible();

  await pickOnLabel(page, "origin/release", "Fetch into a local branch…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Fetch origin/release into a local branch");
  await expect(d.getByRole("textbox", { name: "Local branch name" })).toHaveValue("release");
  await d.getByRole("button", { name: "Fetch", exact: true }).click();
  await expect(notice(page, /Created branch release at origin\/release/)).toBeVisible();
  await expect(label(page, "release")).toBeVisible();
  expect(current(gg)).toBe("main");
  expect(gg.git("routine", "rev-parse", "release").trim()).toBe(gg.git("routine", "rev-parse", "origin/release").trim());
});

test("delete a remote branch: the core's confirmation only", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  pushFromOther(gg, "release");
  await expect(label(page, "origin/release")).toBeVisible();

  await pickOnLabel(page, "origin/release", "Delete from origin…");
  await expect(dialog(page)).toHaveCount(0);
  const confirm = alertDialog(page);
  await expect(confirm.getByTestId("dangers")).toContainText("release");
  await armed(page);
  await confirm.getByRole("button", { name: "Delete remote branch" }).click();
  await expect(notice(page, "Deleted branch release on origin.")).toBeVisible();
  await expect(grid(page).locator('[data-ref="origin/release"]')).toHaveCount(0);
  expect(gg.git("routine", "ls-remote", "--heads", "origin", "release").trim()).toBe("");
});

test("a menu stays open while the repository refreshes behind it, and closes when its target goes", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await label(page, "staging").locator(".name").click({ button: "right" });
  await expect(menus(page)).toHaveCount(1);
  commitOutside(gg, "Made elsewhere"); // new rows: the graph reloads
  await expect(rowOf(page, "Made elsewhere")).toBeVisible();
  // Once the menu has looked at the new rows, it is still open.
  await expect
    .poll(async () => (await menus(page).first().getAttribute("data-checked-generation")) === String(await generation(page)))
    .toBe(true);
  await expect(menus(page)).toHaveCount(1);

  // The branch goes: so does its menu.
  gg.git("routine", "branch", "-q", "-D", "staging");
  await expect(menus(page)).toHaveCount(0);

  // A row's menu closes when its commit goes.
  await rowOf(page, "Made elsewhere").locator(".subject").click({ button: "right" });
  await expect(menus(page)).toHaveCount(1);
  gg.git("routine", "reset", "-q", "--hard", "HEAD~1");
  await expect(rowOf(page, "Made elsewhere")).toHaveCount(0);
  await expect(menus(page)).toHaveCount(0);
});
