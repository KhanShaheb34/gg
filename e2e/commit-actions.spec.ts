// Commit and tag actions from their menus, end to end against the real core: each dialog,
// what the graph shows afterwards (labels on the right row, HEAD moved, a new commit on
// top) and the notice.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import type { InProgress, OpRequest } from "../src/lib/types";

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
const topSubject = (page: Page) => grid(page).locator('[data-index="0"] .subject');
const item = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });
const dialog = (page: Page) => page.getByRole("dialog");
const notices = (page: Page) => page.getByRole("region", { name: "Notifications" }).locator(".toast");
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
const short = (hash: string) => hash.slice(0, 8);
/** A red button takes no click just after its dialog opens (see Dialog): wait that out. */
const armed = (page: Page) => page.waitForTimeout(450);

/** Opens the menu of the row with `subject` and picks `name` (a submenu item after `parent`). */
async function pickOnRow(page: Page, subject: string, name: string, parent?: string) {
  await rowOf(page, subject).locator(".subject").click({ button: "right" });
  if (parent) await item(page, parent).hover();
  await item(page, name).click();
}

/** Opens the menu of label `ref` and picks `name`. */
async function pickOnLabel(page: Page, ref: string, name: string) {
  await grid(page).locator(`[data-ref="${ref}"]`).first().click({ button: "right" });
  await item(page, name).click();
}

/** The active repository's snapshot, as the page has it. */
function inProgress(page: Page): Promise<InProgress | null> {
  return page.evaluate(async () => {
    const { findRepoView } = await import("/src/lib/state/repo.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return findRepoView(app.activeRepoId!)!.snapshot.inProgress;
  });
}

/** The repository commits as the fixtures' author (the core's git uses the repository's config). */
function identify(git: (...args: string[]) => string) {
  git("config", "user.name", "Ada Lovelace");
  git("config", "user.email", "ada@example.com");
}

test("check out a commit: HEAD moves there, detached, and reset then names HEAD", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hash = gg.git("small", "rev-parse", "HEAD~3").trim(); // "Add part 17"
  await pickOnRow(page, "Add part 17", "Check out this commit");

  await expect(notice(page, `Checked out ${short(hash)}: HEAD is detached`)).toBeVisible();
  expect(gg.git("small", "rev-parse", "HEAD").trim()).toBe(hash);
  const row = rowOf(page, "Add part 17");
  await expect(row.locator('[data-ref="HEAD"]')).toBeVisible();
  await expect(row).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-ref="main"]')).not.toHaveClass(/current/);

  // With HEAD detached, a reset moves HEAD, and says so.
  await pickOnRow(page, "Add part 15", "Soft: keep changes, staged", "Reset HEAD to here");
  await expect(dialog(page).getByRole("heading")).toHaveText(/^Reset HEAD to [0-9a-f]{8}$/);
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
});

test("create a branch here: checked out by default, or not", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await pickOnRow(page, "Add part 3", "Create branch here…");

  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Create branch");
  await expect(d.getByText("Add part 3")).toBeVisible();
  await expect(d.getByRole("textbox", { name: "Branch name" })).toBeFocused();
  await expect(d.getByRole("checkbox", { name: "Check out after creating" })).toBeChecked();
  await page.keyboard.type("topic/new");
  await page.keyboard.press("Enter");

  await expect(d).toHaveCount(0);
  await expect(notice(page, "Created branch topic/new and checked it out.")).toBeVisible();
  const row = rowOf(page, "Add part 3");
  await expect(row.locator('[data-ref="topic/new"]')).toHaveClass(/current/);
  await expect(row).toHaveAttribute("aria-selected", "true");
  expect(gg.git("small", "branch", "--show-current").trim()).toBe("topic/new");

  await pickOnRow(page, "Add part 5", "Create branch here…");
  await d.getByRole("textbox", { name: "Branch name" }).fill("later");
  await d.getByRole("checkbox", { name: "Check out after creating" }).uncheck();
  await d.getByRole("button", { name: "Create branch" }).click();
  await expect(notice(page, "Created branch later.")).toBeVisible();
  await expect(rowOf(page, "Add part 5").locator('[data-ref="later"]')).toBeVisible();
  await expect(rowOf(page, "Add part 5")).toHaveAttribute("aria-selected", "true");
  expect(gg.git("small", "branch", "--show-current").trim()).toBe("topic/new");
});

test("create a tag: lightweight without a message, annotated with one", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await pickOnRow(page, "Add part 18", "Create tag here…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Create tag");
  await expect(d.getByRole("textbox", { name: "Tag name" })).toBeFocused();
  await page.keyboard.type("v2");
  await page.keyboard.press("Enter");
  const hash = gg.git("small", "rev-parse", "HEAD~1").trim();
  await expect(notice(page, `Created tag v2 at ${short(hash)}.`)).toBeVisible();
  await expect(rowOf(page, "Add part 18").locator('[data-ref="v2"]')).toBeVisible();
  await expect(rowOf(page, "Add part 18")).toHaveAttribute("aria-selected", "true");
  expect(gg.git("small", "cat-file", "-t", "refs/tags/v2").trim()).toBe("commit");

  identify((...a) => gg.git("small", ...a));
  await pickOnRow(page, "Add part 19", "Create tag here…");
  await d.getByRole("textbox", { name: "Tag name" }).fill("v3");
  await d.getByRole("textbox", { name: "Message" }).fill("Release three\n\nWith notes.");
  await expect(d.getByText("Annotated: it records the message, who made it and when")).toBeVisible();
  // Enter in the message is a new line; Cmd/Ctrl+Enter creates.
  await d.getByRole("textbox", { name: "Message" }).press("ControlOrMeta+Enter");
  await expect(notice(page, /Created tag v3 at [0-9a-f]{8}\./)).toBeVisible();
  await expect(rowOf(page, "Add part 19").locator('[data-ref="v3"]')).toBeVisible();
  expect(gg.git("small", "cat-file", "-t", "refs/tags/v3").trim()).toBe("tag");
  expect(gg.git("small", "tag", "-l", "--format=%(contents)", "v3").trim()).toBe("Release three\n\nWith notes.");
});

test("invalid_name_disables_the_button_and_says_why", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await pickOnRow(page, "Add part 3", "Create branch here…");
  const d = dialog(page);
  const create = d.getByRole("button", { name: "Create branch" });
  const field = d.getByRole("textbox", { name: "Branch name" });

  // Nothing typed yet: disabled, and the hint says what is missing.
  await expect(create).toBeDisabled();
  await expect(d.getByText("A name is needed.")).toBeVisible();

  await field.fill("has space");
  await expect(create).toBeDisabled();
  await expect(d.getByText("It can’t contain spaces.")).toBeVisible();
  await expect(field).toHaveAttribute("aria-invalid", "true");
  // Enter does not submit while the name is refused.
  await field.press("Enter");
  await expect(d).toBeVisible();

  await field.fill("main");
  await expect(d.getByText("A branch named “main” already exists.")).toBeVisible();
  await expect(create).toBeDisabled();

  await field.fill("-x");
  await expect(d.getByText("It can’t start with a hyphen.")).toBeVisible();

  await field.fill("good-name");
  await expect(create).toBeEnabled();
  await expect(field).not.toHaveAttribute("aria-invalid", "true");
  await page.keyboard.press("Escape");
  await expect(d).toHaveCount(0);
  expect(gg.git("small", "branch", "--list", "good-name").trim()).toBe("");
});

test("a name the core refuses shows beside the field, and the dialog stays open with it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await pickOnRow(page, "Add part 19", "Create tag here…");
  const d = dialog(page);
  const field = d.getByRole("textbox", { name: "Tag name" });
  await field.fill("v1"); // the fixture's tag, on another commit
  await page.keyboard.press("Enter");

  await expect(d.getByText("A tag named “v1” already exists.")).toBeVisible();
  await expect(field).toHaveValue("v1");
  await expect(field).toHaveAttribute("aria-invalid", "true");
  await expect(d.getByRole("button", { name: "Create tag" })).toBeDisabled();
  await expect(notices(page)).toHaveCount(0);

  // Another name clears it.
  await field.fill("v1.1");
  await expect(d.getByText("A tag named “v1” already exists.")).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(d).toHaveCount(0);
  await expect(rowOf(page, "Add part 19").locator('[data-ref="v1.1"]')).toBeVisible();
});

test("cherry-pick a commit: a new commit on top, or its changes staged only", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  const hash = gg.git("picks", "rev-parse", "topic~1").trim(); // "Add topic notes"
  await pickOnRow(page, "Add topic notes", "Cherry-pick…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText(`Cherry-pick ${hash.slice(0, 8)} onto main`);
  await expect(d.getByRole("checkbox", { name: "Apply the changes without committing" })).not.toBeChecked();
  await expect(d.getByRole("radio")).toHaveCount(0); // not a merge: no parent to choose
  await page.keyboard.press("Enter");

  await expect(notice(page, `Cherry-picked ${short(hash)} onto main.`)).toBeVisible();
  await expect(topSubject(page)).toHaveText("Add topic notes");
  await expect(grid(page).locator('[data-index="0"] [data-ref="main"]')).toBeVisible();
  expect(gg.git("picks", "log", "-1", "--format=%s").trim()).toBe("Add topic notes");

  // Without committing: the changes are staged, nothing new on top.
  await pickOnRow(page, "Build the feature", "Cherry-pick…");
  await d.getByRole("checkbox", { name: "Apply the changes without committing" }).check();
  await d.getByRole("button", { name: "Cherry-pick" }).click();
  await expect(notice(page, /without committing: they are staged\./)).toBeVisible();
  expect(gg.git("picks", "status", "--porcelain").trim()).toBe("A  feature.txt");
  expect(gg.git("picks", "log", "-1", "--format=%s").trim()).toBe("Add topic notes");
});

test("cherry-pick a merge commit: the parent is chosen, each shown by short hash and subject", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  const first = gg.git("picks", "rev-parse", "integration^1").trim();
  const second = gg.git("picks", "rev-parse", "integration^2").trim();
  // The parents' subjects come from the rows already loaded, without asking the core.
  const asked: string[] = [];
  await page.route("**/invoke", (route) => {
    const body = route.request().postDataJSON() as { cmd: string; args: { hash?: string } } | null;
    if (body?.cmd === "commit_details" && body.args.hash) asked.push(body.args.hash);
    return route.continue();
  });
  await pickOnRow(page, "Merge branch 'feature' into integration", "Cherry-pick…");
  const d = dialog(page);
  const parents = d.getByRole("radio");
  await expect(parents).toHaveCount(2);
  const one = d.getByRole("radio", { name: new RegExp(first.slice(0, 8)) });
  const two = d.getByRole("radio", { name: new RegExp(second.slice(0, 8)) });
  await expect(one).toBeChecked();
  await expect(d.locator("label.choice", { hasText: first.slice(0, 8) })).toContainText("Prepare integration");
  await expect(d.locator("label.choice", { hasText: second.slice(0, 8) })).toContainText("Build the feature");
  await expect(two).not.toBeChecked();
  expect(asked).not.toContain(first);
  expect(asked).not.toContain(second);
  await d.getByRole("button", { name: "Cherry-pick" }).click();

  await expect(notice(page, /Cherry-picked [0-9a-f]{8} onto main\./)).toBeVisible();
  await expect(topSubject(page)).toHaveText("Merge branch 'feature' into integration");
  expect(gg.git("picks", "show", "--name-only", "--format=", "HEAD").trim()).toBe("feature.txt");
});

test("cherry-pick a merge commit relative to its second parent", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  const second = gg.git("picks", "rev-parse", "integration^2").trim();
  await pickOnRow(page, "Merge branch 'feature' into integration", "Cherry-pick…");
  const d = dialog(page);
  await d.getByRole("radio", { name: new RegExp(second.slice(0, 8)) }).check();
  await d.getByRole("button", { name: "Cherry-pick" }).click();

  await expect(notice(page, /Cherry-picked [0-9a-f]{8} onto main\./)).toBeVisible();
  // Relative to the feature side, the merge brought in integration's own change.
  expect(gg.git("picks", "show", "--name-only", "--format=", "HEAD").trim()).toBe("integration.txt");
});

test("cherry_pick_conflict_shows_no_error_toast", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  await pickOnRow(page, "Greet from topic", "Cherry-pick…");
  await dialog(page).getByRole("button", { name: "Cherry-pick" }).click();
  await expect(dialog(page)).toHaveCount(0);

  await expect.poll(() => inProgress(page)).toMatchObject({ kind: "cherry-pick", files: [{ path: "greeting.txt", state: "markers" }] });
  await expect(notices(page)).toHaveCount(0);
});

test("revert a commit: a one-line confirmation, then a new commit on top", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  identify((...a) => gg.git("small", ...a));
  const hash = gg.git("small", "rev-parse", "HEAD~1").trim(); // "Add part 18"
  await pickOnRow(page, "Add part 18", "Revert…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText(`Revert ${hash.slice(0, 8)}`);
  await expect(d).toContainText("A new commit on main undoes the changes of");
  await expect(d.getByRole("radio")).toHaveCount(0);
  await page.keyboard.press("Enter");

  await expect(notice(page, `Reverted ${short(hash)}: a new commit undoes its changes.`)).toBeVisible();
  await expect(topSubject(page)).toHaveText('Revert "Add part 18"');
  expect(gg.git("small", "ls-files", "src/part18.txt").trim()).toBe("");
});

test("revert a merge commit asks whose side to keep", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  identify((...a) => gg.git("small", ...a));
  const merge = gg.git("small", "rev-parse", "HEAD~2").trim();
  const mainSide = gg.git("small", "rev-parse", `${merge}^1`).trim();
  await pickOnRow(page, "Merge branch 'feature/login'", "Revert…");
  const d = dialog(page);
  await expect(d.getByRole("radio")).toHaveCount(2);
  const keep = d.getByRole("radio", { name: new RegExp(mainSide.slice(0, 8)) });
  await expect(keep).toBeChecked();
  await expect(d.locator("label.choice", { hasText: mainSide.slice(0, 8) })).toContainText("Add part 17");
  await d.getByRole("button", { name: "Revert" }).click();

  await expect(notice(page, `Reverted ${short(merge)}: a new commit undoes its changes.`)).toBeVisible();
  await expect(topSubject(page)).toHaveText(`Revert "Merge branch 'feature/login'"`);
  expect(gg.git("small", "ls-files", "login").trim()).toBe("");
});

test("revert a merge commit keeping the second parent's side", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  identify((...a) => gg.git("small", ...a));
  const merge = gg.git("small", "rev-parse", "HEAD~2").trim();
  const loginSide = gg.git("small", "rev-parse", `${merge}^2`).trim();
  await pickOnRow(page, "Merge branch 'feature/login'", "Revert…");
  const d = dialog(page);
  await d.getByRole("radio", { name: new RegExp(loginSide.slice(0, 8)) }).check();
  await d.getByRole("button", { name: "Revert" }).click();

  await expect(notice(page, `Reverted ${short(merge)}: a new commit undoes its changes.`)).toBeVisible();
  // Relative to feature/login, the merge brought in main's parts 13 to 17: those go; login stays.
  expect(gg.git("small", "ls-files", "login").trim()).not.toBe("");
  expect(gg.git("small", "ls-files", "src/part13.txt", "src/part17.txt").trim()).toBe("");
  expect(gg.git("small", "ls-files", "src/part18.txt").trim()).toBe("src/part18.txt");
});

test("reset soft and mixed move the branch and keep the changes", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const target = gg.git("small", "rev-parse", "HEAD~1").trim(); // "Add part 18"
  await pickOnRow(page, "Add part 18", "Soft: keep changes, staged", "Reset main to here");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText(`Reset main to ${target.slice(0, 8)}`);
  await expect(d.getByRole("radio", { name: /^Soft/ })).toBeChecked();
  await expect(d.getByRole("radio", { name: /^Soft/ })).toBeFocused();
  // True whether or not the commit is behind the branch.
  await expect(d.getByText("Moves main only. The difference to where it was stays staged; so do your changes.")).toBeVisible();
  await expect(d.getByRole("button", { name: "Reset" })).not.toHaveClass(/danger/);
  await page.keyboard.press("Enter");

  await expect(notice(page, `Reset main to ${short(target)}: the changes since are staged.`)).toBeVisible();
  expect(gg.git("small", "rev-parse", "HEAD").trim()).toBe(target);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("A  src/part19.txt");
  await expect(rowOf(page, "Add part 18").locator('[data-ref="main"]')).toBeVisible();
  await expect(rowOf(page, "Add part 18")).toHaveAttribute("aria-selected", "true");

  // Mixed, chosen in the dialog.
  const before = gg.git("small", "rev-parse", "HEAD~1").trim(); // the merge
  await pickOnRow(page, "Merge branch 'feature/login'", "Soft: keep changes, staged", "Reset main to here");
  await d.getByRole("radio", { name: /^Mixed/ }).check();
  await d.getByRole("button", { name: "Reset" }).click();
  await expect(notice(page, `Reset main to ${short(before)}: the changes since are in the working tree, unstaged.`)).toBeVisible();
  expect(gg.git("small", "status", "--porcelain").trim().split("\n").sort()).toEqual([
    "?? src/part18.txt",
    "?? src/part19.txt",
  ]);
});

test("hard_reset_asks_twice_and_cancel_changes_nothing", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  writeFileSync(path.join(dir, "src/part1.txt"), "changed\n");
  // The graph shows the change first (the window coming back reads the working tree): a
  // menu closes when its rows change under it.
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(grid(page).locator('[role="row"]', { hasText: "Uncommitted changes (1)" })).toBeVisible();
  const head = gg.git("small", "rev-parse", "HEAD").trim();
  const target = gg.git("small", "rev-parse", "HEAD~1").trim();

  // First: the reset dialog, hard chosen, its button red and not focused.
  await pickOnRow(page, "Add part 18", "Hard: discard all changes", "Reset main to here");
  const d = page.getByRole("alertdialog");
  await expect(d.getByRole("radio", { name: /^Hard/ })).toBeChecked();
  await expect(d.getByRole("radio", { name: /^Hard/ })).toBeFocused();
  const reset = d.getByRole("button", { name: "Reset", exact: true });
  await expect(reset).toHaveClass(/danger/);
  await expect(reset).not.toBeFocused();
  // Its focus ring is the danger colour, not the accent.
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowDown");
  const ring = await d.getByRole("radio", { name: /^Hard/ }).evaluate((el) => getComputedStyle(el).outlineColor);
  const dangerRing = await page.evaluate(() => {
    const probe = document.createElement("div");
    probe.style.color = "var(--danger-focus-ring)";
    document.body.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  });
  expect(dangerRing).toMatch(/^rgba?\(/);
  expect(ring).toBe(dangerRing);
  await armed(page);
  await reset.click();

  // Then: the core's confirmation, which says what would be lost. Cancel.
  await expect(d.getByTestId("dangers")).toContainText("discards the uncommitted changes to 1 file");
  await d.getByRole("button", { name: "Cancel" }).click();
  await expect(d).toHaveCount(0);
  expect(gg.git("small", "rev-parse", "HEAD").trim()).toBe(head);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");
  await expect(notices(page)).toHaveCount(0);

  // Cancelling the first one sends nothing either.
  await pickOnRow(page, "Add part 18", "Hard: discard all changes", "Reset main to here");
  await page.keyboard.press("Escape");
  await expect(d).toHaveCount(0);
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("M src/part1.txt");

  // Going through both resets.
  await pickOnRow(page, "Add part 18", "Hard: discard all changes", "Reset main to here");
  await armed(page);
  await d.getByRole("button", { name: "Reset", exact: true }).click();
  await expect(d.getByTestId("dangers")).toBeVisible();
  await armed(page);
  await d.getByRole("button", { name: "Reset and discard" }).click();
  await expect(notice(page, `Reset main to ${short(target)}: uncommitted changes were discarded.`)).toBeVisible();
  expect(gg.git("small", "status", "--porcelain").trim()).toBe("");
  await expect(topSubject(page)).toHaveText("Add part 18");
});

test("delete a tag here, or here and on the remote", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  gg.git("small", "tag", "v2", "HEAD~2");
  gg.git("small", "push", "-q", "origin", "v1", "v2");
  const remoteTags = () => gg.git("small", "ls-remote", "--tags", "origin").trim();
  await expect(grid(page).locator('[data-ref="v2"]')).toBeVisible();

  // Here only.
  await pickOnLabel(page, "v2", "Delete…");
  const d = page.getByRole("alertdialog");
  await expect(d.getByRole("heading")).toHaveText("Delete tag v2");
  const also = d.getByRole("checkbox", { name: "Also delete it on origin" });
  await expect(also).not.toBeChecked();
  await expect(d.getByRole("button", { name: "Delete tag" })).toHaveClass(/danger/);
  await armed(page);
  await d.getByRole("button", { name: "Delete tag" }).click();
  await expect(notice(page, "Deleted tag v2.")).toBeVisible();
  await expect(grid(page).locator('[data-ref="v2"]')).toHaveCount(0);
  expect(remoteTags()).toContain("refs/tags/v2");

  // Here and on origin.
  await grid(page).evaluate((el) => (el.scrollTop = el.scrollHeight));
  await pickOnLabel(page, "v1", "Delete…");
  await also.check();
  await armed(page);
  await d.getByRole("button", { name: "Delete tag" }).click();
  await expect(notice(page, "Deleted tag v1, here and on origin.")).toBeVisible();
  await expect(grid(page).locator('[data-ref="v1"]')).toHaveCount(0);
  expect(remoteTags()).not.toContain("refs/tags/v1");
});

test("push a tag: the only remote is chosen already", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await grid(page).evaluate((el) => (el.scrollTop = el.scrollHeight));
  await pickOnLabel(page, "v1", "Push…");
  const d = dialog(page);
  await expect(d.getByRole("heading")).toHaveText("Push tag v1");
  await expect(d.getByRole("combobox", { name: "Remote" })).toHaveValue("origin");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Pushed tag v1 to origin.")).toBeVisible();
  expect(gg.git("small", "ls-remote", "--tags", "origin").trim()).toContain("refs/tags/v1");
});

test("push a tag with several remotes: origin is chosen, another can be", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("small");
  gg.git("small", "init", "-q", "--bare", "../backup.git");
  gg.git("small", "remote", "add", "backup", path.join(path.dirname(dir), "backup.git"));
  await page.goto("/");
  await open(page, dir);
  await grid(page).evaluate((el) => (el.scrollTop = el.scrollHeight));
  await pickOnLabel(page, "v1", "Push…");
  const remote = dialog(page).getByRole("combobox", { name: "Remote" });
  await expect(remote).toHaveValue("origin");
  await remote.selectOption("backup");
  await dialog(page).getByRole("button", { name: "Push tag" }).click();
  await expect(notice(page, "Pushed tag v1 to backup.")).toBeVisible();
  expect(gg.git("small", "ls-remote", "--tags", "backup").trim()).toContain("refs/tags/v1");
});

test("several remotes, none named origin: nothing is chosen until the user picks one (R29)", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("small");
  gg.git("small", "remote", "rename", "origin", "upstream");
  gg.git("small", "init", "-q", "--bare", "../backup.git");
  gg.git("small", "remote", "add", "backup", path.join(path.dirname(dir), "backup.git"));
  await page.goto("/");
  await open(page, dir);
  await grid(page).evaluate((el) => (el.scrollTop = el.scrollHeight));

  await pickOnLabel(page, "v1", "Push…");
  const remote = dialog(page).getByRole("combobox", { name: "Remote" });
  await expect(remote).toHaveValue("");
  await expect(dialog(page).getByText("Choose the remote.")).toBeVisible();
  await expect(dialog(page).getByRole("button", { name: "Push tag" })).toBeDisabled();
  await remote.selectOption("backup");
  await expect(dialog(page).getByText("Choose the remote.")).toHaveCount(0);
  await expect(dialog(page).getByRole("button", { name: "Push tag" })).toBeEnabled();
  await page.keyboard.press("Escape");

  await pickOnLabel(page, "v1", "Delete…");
  const d = page.getByRole("alertdialog");
  await expect(d.getByRole("button", { name: "Delete tag" })).toBeEnabled();
  await d.getByRole("checkbox", { name: "Also delete it on a remote" }).check();
  await expect(d.getByRole("combobox", { name: "Remote" })).toHaveValue("");
  await expect(d.getByRole("button", { name: "Delete tag" })).toBeDisabled();
  await d.getByRole("combobox", { name: "Remote" }).selectOption("upstream");
  await expect(d.getByRole("button", { name: "Delete tag" })).toBeEnabled();
  await page.keyboard.press("Escape");

  // A branch without an upstream: nothing chosen either.
  await grid(page).evaluate((el) => (el.scrollTop = 0));
  await pickOnLabel(page, "feature/login", "Push…");
  await expect(remote).toHaveValue("");
  await expect(dialog(page).getByRole("button", { name: "Push", exact: true })).toBeDisabled();
  await remote.selectOption("backup");
  await expect(dialog(page).getByText("backup/feature/login")).toBeVisible();
  await expect(dialog(page).getByRole("button", { name: "Push", exact: true })).toBeEnabled();
});

test("delete a tag in a repository without remotes: no remote to choose", async ({ gg }) => {
  const { page } = gg;
  gg.git("picks", "tag", "solo");
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  await pickOnLabel(page, "solo", "Delete…");
  const d = page.getByRole("alertdialog");
  await expect(d.getByRole("checkbox")).toHaveCount(0);
  await expect(d.getByRole("combobox")).toHaveCount(0);
  await armed(page);
  await d.getByRole("button", { name: "Delete tag" }).click();
  await expect(notice(page, "Deleted tag solo.")).toBeVisible();
  expect(gg.git("picks", "tag", "--list", "solo").trim()).toBe("");
});

/** Runs `request` through the app's own runOp in the active tab, not waiting for it. */
function startOp(page: Page, request: OpRequest): Promise<unknown> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    void ops.runOp(app.activeRepoId!, req).catch(() => {});
  }, request);
}

test("a dialog whose request waits behind another: a name dialog says so and cannot be cancelled; others close", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("small");
  await open(page, dir);
  identify((...a) => gg.git("small", ...a));
  // Each checkout runs this hook, which takes a while.
  writeFileSync(path.join(dir, ".git/hooks/post-checkout"), "#!/bin/sh\nsleep 2\n", { mode: 0o755 });
  const busy = page.getByTestId("busy");

  await startOp(page, { op: "checkout_branch", branch: "fix/typo", fastForward: false });
  await expect(busy).toContainText("fix/typo");
  await pickOnRow(page, "Add part 3", "Create branch here…");
  const d = dialog(page);
  await page.keyboard.type("queued");
  await page.keyboard.press("Enter");
  // The core may yet refuse the name: the dialog waits, and says for what.
  await expect(d.getByText("Waiting for Check out fix/typo to finish…")).toBeVisible();
  await expect(d.getByRole("button", { name: "Cancel" })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(d).toBeVisible();
  await expect(d).toHaveCount(0, { timeout: 10_000 });
  await expect(notice(page, "Created branch queued and checked it out.")).toBeVisible();
  expect(gg.git("small", "branch", "--show-current").trim()).toBe("queued");

  // A dialog whose request cannot come back to it closes at once; the toolbar shows the queue.
  await startOp(page, { op: "checkout_branch", branch: "main", fastForward: false });
  await expect(busy).toContainText("main");
  await pickOnRow(page, "Add part 3", "Revert…");
  await page.keyboard.press("Enter");
  await expect(d).toHaveCount(0);
  await expect(busy).toContainText("+1");
  await expect(notice(page, /Reverted [0-9a-f]+: a new commit undoes its changes\./)).toBeVisible({ timeout: 10_000 });
});

test("a dialog closing under another leaves the focus in the one above; a covered dialog takes no input", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("picks"));
  const dir = gg.repoPath("small");
  await open(page, dir);
  writeFileSync(path.join(dir, ".git/hooks/post-checkout"), "#!/bin/sh\nsleep 2\n", { mode: 0o755 });

  await startOp(page, { op: "checkout_branch", branch: "fix/typo", fastForward: false });
  await expect(page.getByTestId("busy")).toContainText("fix/typo");
  await pickOnRow(page, "Add part 3", "Create branch here…");
  await page.keyboard.type("under");
  await page.keyboard.press("Enter");
  const below = dialog(page);
  await expect(below.getByText(/Waiting for/)).toBeVisible();

  // Another repository asks for a confirmation meanwhile: it opens on top, with the focus.
  await page.evaluate(async () => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const picks = app.tabs.find((t: { path: string }) => t.path.endsWith("/picks"))!.repoId;
    void ops.runOp(picks, { op: "delete_branch", branch: "topic" }).catch(() => {});
  });
  const above = page.getByRole("alertdialog");
  await expect(above.getByRole("button", { name: "Cancel" })).toBeFocused();
  expect(await below.evaluate((d) => d.closest("[inert]") !== null)).toBe(true);

  // The dialog below closes when its request is done; the focus stays above.
  await expect(below).toHaveCount(0, { timeout: 10_000 });
  await expect(above.getByRole("button", { name: "Cancel" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(above).toHaveCount(0);
  expect(gg.git("picks", "branch", "--list", "topic").trim()).toBe("topic");
});
