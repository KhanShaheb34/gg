// The branch picker (⌘P, or the branch name in the header), end to end against the real core:
// the `routine` fixture (`main` checked out; `staging` 2 behind `origin/staging`;
// `feature/login` level with its upstream; a second clone that moves the remote) and the
// `worktrees` one (branches checked out in linked worktrees). What each choice does in git,
// what the window shows, and that opening and typing ask the core nothing.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, makeRepo, otherClone, test, type GG } from "./fixtures";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const picker = (page: Page) => page.getByTestId("branch-picker");
const field = (page: Page) => picker(page).getByRole("combobox");
/** What the step asks, above the field (none in the first step). */
const prompt = (page: Page) => picker(page).locator(".prompt");
const options = (page: Page) => picker(page).getByRole("option");
const option = (page: Page, name: string) => picker(page).locator(`[role="option"][data-name="${name}"]`);
const activeOption = (page: Page) => picker(page).locator('[role="option"][data-active="true"]');
const createRow = (page: Page) => picker(page).locator('[role="option"][data-kind="create"]');
const createFromRow = (page: Page) => picker(page).locator('[role="option"][data-kind="createFrom"]');
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
/** The names of the branch rows shown, in order. */
const shownNames = (page: Page) => picker(page).locator('[role="option"][data-kind="branch"]').evaluateAll((els) =>
  els
    .map((el) => ({ name: el.getAttribute("data-name")!, y: new DOMMatrix(getComputedStyle(el).transform).m42 }))
    .sort((a, b) => a.y - b.y)
    .map((r) => r.name),
);

async function openPicker(page: Page) {
  await page.keyboard.press("ControlOrMeta+p");
  await expect(picker(page)).toBeVisible();
  await expect(field(page)).toBeFocused();
}

/**
 * Records the commands the page sends the core from now on, but for the graph's own paging
 * of rows (`get_rows`), which a snapshot arriving late on a slow machine may ask for.
 */
function recordInvokes(page: Page): () => string[] {
  const cmds: string[] = [];
  page.on("request", (request) => {
    if (!request.url().endsWith("/invoke")) return;
    let cmd = "?";
    try {
      cmd = String(JSON.parse(request.postData() ?? "{}").cmd);
    } catch {
      // Recorded as "?".
    }
    if (cmd !== "get_rows") cmds.push(cmd);
  });
  return () => cmds;
}

const current = (gg: GG, repo = "routine") => gg.git(repo, "branch", "--show-current").trim();

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

test("⌘P lists local then remote branches, newest first, each with its commit; typing filters; nothing is asked of the core", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  await page.waitForTimeout(300); // what opening the repository asks for is done
  const asked = recordInvokes(page);
  // The window's handler takes ⌘P: the webview never prints.
  await page.evaluate(() => {
    window.addEventListener("keydown", (e) => {
      if (e.key.toLowerCase() === "p") (window as unknown as { printPrevented: boolean }).printPrevented = e.defaultPrevented;
    });
  });

  await openPicker(page);
  expect(await page.evaluate(() => (window as unknown as { printPrevented: boolean }).printPrevented)).toBe(true);
  await expect(field(page)).toHaveAttribute("placeholder", "Switch to a branch, or type a name for a new one");
  // Shown whatever the graph's "Remote branches" toggle says.
  expect(await shownNames(page)).toEqual([
    "feature/login",
    "staging",
    "main",
    "origin/staging",
    "origin/feature/login",
    "origin/main",
  ]);
  const login = option(page, "feature/login");
  await expect(login.locator(".line.one")).toContainText("branches");
  await expect(login.locator(".line.two")).toHaveText("Ada Lovelace•Validate login");
  await expect(login.locator(".when")).toHaveText(/ago$/);
  await expect(option(page, "origin/staging").locator(".line.one")).toContainText("remote branches");
  await expect(option(page, "origin/staging").locator(".line.two")).toHaveText("Grace Hopper•Hotfix two");
  await expect(option(page, "main").getByRole("img", { name: "checked out here" })).toBeVisible();
  // Below the branches, the two ways to make one; the first row is the active one.
  await expect(createRow(page).locator(".line.one")).toHaveText("Create new branch…");
  await expect(createFromRow(page).locator(".line.one")).toHaveText("Create new branch from…");
  await expect(activeOption(page)).toHaveAttribute("data-name", "feature/login");
  await expect(field(page)).toHaveAttribute("aria-activedescendant", (await login.getAttribute("id"))!);

  // Any case, anywhere in the name; the part that matches is marked.
  await field(page).fill("LOG");
  expect(await shownNames(page)).toEqual(["feature/login", "origin/feature/login"]);
  await expect(login.locator("mark")).toHaveText("log");
  await expect(createRow(page).locator(".line.one")).toHaveText("Create new branch ‘LOG’");

  // A branch named exactly as typed comes first and is offered, not made again.
  await field(page).fill("main");
  expect(await shownNames(page)).toEqual(["main", "origin/main"]);
  await expect(createRow(page)).toHaveCount(0);
  await expect(createFromRow(page)).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(picker(page)).toHaveCount(0);
  expect(asked()).toEqual([]);
});

test("choosing a local branch checks it out as the graph does; the current one only closes", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const asked = recordInvokes(page);

  // The current branch: closes, runs nothing.
  await openPicker(page);
  await option(page, "main").click();
  await expect(picker(page)).toHaveCount(0);
  await page.waitForTimeout(300);
  expect(asked().filter((c) => c === "run_op")).toEqual([]);

  await openPicker(page);
  await field(page).fill("feature/l");
  await page.keyboard.press("Enter");
  await expect(picker(page)).toHaveCount(0);
  await expect(notice(page, "Checked out feature/login")).toBeVisible();
  expect(current(gg)).toBe("feature/login");
  await expect(page.getByTestId("active-worktree")).toContainText("feature/login");

  // Behind its upstream: the same question as from the graph.
  await openPicker(page);
  await option(page, "staging").click();
  const ask = page.getByRole("dialog").filter({ hasText: "Fast-forward" });
  await expect(ask.getByRole("heading")).toHaveText("Fast-forward 2 commits first?");
  await ask.getByRole("button", { name: "No", exact: true }).click();
  await expect(notice(page, "Checked out staging")).toBeVisible();
  expect(current(gg)).toBe("staging");
});

test("a remote branch checks out its local branch when there is one, else makes it, tracking the remote one", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await openPicker(page);
  await field(page).fill("origin/feature");
  await expect(activeOption(page)).toHaveAttribute("data-name", "origin/feature/login");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Checked out feature/login")).toBeVisible();
  expect(current(gg)).toBe("feature/login");
  expect(gg.git("routine", "branch", "--list", "login*").trim()).toBe("");

  pushFromOther(gg, "topic");
  await openPicker(page);
  await expect(option(page, "origin/topic")).toBeVisible();
  // The newest commit: first among the remote branches.
  expect((await shownNames(page)).filter((n) => n.startsWith("origin/"))[0]).toBe("origin/topic");
  await option(page, "origin/topic").click();
  await expect(picker(page)).toHaveCount(0);
  await expect(notice(page, "Checked out topic. A new branch tracking origin/topic.")).toBeVisible();
  expect(current(gg)).toBe("topic");
  expect(gg.git("routine", "rev-parse", "--abbrev-ref", "topic@{upstream}").trim()).toBe("origin/topic");
});

test("a name no branch has: Enter makes it at HEAD and checks it out, in one git command", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const head = gg.git("routine", "rev-parse", "HEAD").trim();

  await openPicker(page);
  await field(page).fill("brand-new");
  await expect(picker(page).locator('[role="option"][data-kind="branch"]')).toHaveCount(0);
  await expect(activeOption(page)).toHaveAttribute("data-kind", "create");
  await expect(createRow(page).locator(".line.one")).toHaveText("Create new branch ‘brand-new’");
  await expect(createRow(page).locator(".line.two")).toHaveText(`At HEAD (${head.slice(0, 8)}), then checked out`);
  await page.keyboard.press("Enter");
  await expect(picker(page)).toHaveCount(0);
  await expect(notice(page, "Created and checked out brand-new")).toBeVisible();
  expect(current(gg)).toBe("brand-new");
  expect(gg.git("routine", "rev-parse", "brand-new").trim()).toBe(head);

  // The command log has the one command that wrote (reads of the new branch, such as its
  // base for Committed on branch, may follow).
  const log = await page.evaluate(async () => {
    const { api } = await import("/src/lib/ipc.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    const records: { args: string[] }[] = await api.commandLog(0, { repoId: app.activeRepoId!, writesOnly: true });
    return records.filter((r) => r.args.some((a) => a.includes("brand-new"))).map((r) => r.args);
  });
  expect(log).toEqual([["checkout", "-b", "brand-new", head]]);
});

test("Create new branch from…: HEAD and the branches to start from; the name typed before, or asked for after", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const head = gg.git("routine", "rev-parse", "HEAD").trim();

  // Typed first, then where.
  await openPicker(page);
  await field(page).fill("from-staging");
  await createFromRow(page).click();
  await expect(field(page)).toHaveValue("");
  await expect(prompt(page)).toHaveText("Select a ref to create ‘from-staging’ from");
  await expect(field(page)).toHaveAttribute("placeholder", "Filter HEAD and the branches");
  await expect(field(page)).toHaveAccessibleName("Select a ref to create ‘from-staging’ from");
  const first = picker(page).locator('[role="option"]').first();
  await expect(first).toHaveAttribute("data-kind", "head");
  await expect(first.locator(".line.one")).toHaveText(new RegExp(`^\\s*HEAD\\s+${head.slice(0, 8)}\\s+\\d+ \\w+ ago$`));
  await expect(createRow(page)).toHaveCount(0);
  await field(page).fill("origin/sta");
  await expect(activeOption(page)).toHaveAttribute("data-name", "origin/staging");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Created and checked out from-staging")).toBeVisible();
  expect(current(gg)).toBe("from-staging");
  expect(gg.git("routine", "rev-parse", "from-staging").trim()).toBe(gg.git("routine", "rev-parse", "origin/staging").trim());

  // Where first, then the name, asked for in the same field.
  await openPicker(page);
  await page.keyboard.press("End");
  await expect(activeOption(page)).toHaveAttribute("data-kind", "createFrom");
  await page.keyboard.press("Enter");
  await field(page).fill("feature/login");
  await expect(activeOption(page)).toHaveAttribute("data-name", "feature/login");
  await page.keyboard.press("Enter");
  await expect(prompt(page)).toHaveText("Name the new branch, at feature/login");
  await expect(field(page)).toHaveAttribute("placeholder", "Branch name");
  await expect(createRow(page).locator(".line.one")).toHaveText("Create new branch…");
  await field(page).fill("named-after");
  await expect(createRow(page).locator(".line.one")).toHaveText("Create new branch ‘named-after’");
  await expect(createRow(page).locator(".line.two")).toHaveText("At feature/login, then checked out");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Created and checked out named-after")).toBeVisible();
  expect(gg.git("routine", "rev-parse", "named-after").trim()).toBe(gg.git("routine", "rev-parse", "feature/login").trim());
});

test("a name git would not take is refused in place, the picker open and nothing run", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const asked = recordInvokes(page);

  await openPicker(page);
  await field(page).fill("has space");
  await expect(createRow(page).locator(".line.two")).toHaveText("It can’t contain spaces.");
  await expect(createRow(page).locator(".line.two")).toHaveClass(/problem/);
  await page.keyboard.press("Enter");
  await expect(picker(page).getByRole("alert")).toHaveText("It can’t contain spaces.");
  await expect(field(page)).toHaveAttribute("aria-invalid", "true");
  await expect(picker(page)).toBeVisible();
  // Typing again clears it.
  await field(page).press("Backspace");
  await expect(picker(page).getByRole("alert")).toHaveCount(0);

  // Asked for the name: a taken one is refused the same way.
  await field(page).fill("");
  await createRow(page).click();
  await field(page).fill("staging");
  await expect(createRow(page).locator(".line.two")).toHaveText("A branch named “staging” already exists.");
  await page.keyboard.press("Enter");
  await expect(picker(page).getByRole("alert")).toHaveText("A branch named “staging” already exists.");
  await field(page).fill("..");
  await page.keyboard.press("Enter");
  await expect(picker(page).getByRole("alert")).toHaveText("It can’t contain two dots in a row.");
  await expect(picker(page)).toBeVisible();
  expect(asked().filter((c) => c === "run_op")).toEqual([]);
  expect(current(gg)).toBe("main");
});

test("keyboard only: arrows, pages, Home and End move one active row with the pointer; Backspace and Esc go back a step", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));

  await openPicker(page);
  const at = () => activeOption(page);
  await expect(at()).toHaveAttribute("data-name", "feature/login");
  await page.keyboard.press("ArrowDown");
  await expect(at()).toHaveAttribute("data-name", "staging");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowUp");
  await expect(at()).toHaveAttribute("data-name", "staging");
  await page.keyboard.press("End");
  await expect(at()).toHaveAttribute("data-kind", "createFrom");
  await page.keyboard.press("Home");
  await expect(at()).toHaveAttribute("data-name", "feature/login");
  await page.keyboard.press("PageDown");
  await expect(at()).toHaveAttribute("data-kind", "createFrom");
  await page.keyboard.press("PageUp");
  await expect(at()).toHaveAttribute("data-name", "feature/login");
  // The pointer moves the same active row; typing puts it back on the first.
  await option(page, "origin/main").hover();
  await expect(at()).toHaveAttribute("data-name", "origin/main");
  await expect(field(page)).toHaveAttribute("aria-activedescendant", (await at().getAttribute("id"))!);
  await page.keyboard.type("in");
  await expect(at()).toHaveAttribute("data-name", "feature/login");

  // Into "from", back with Backspace in the empty field: the text comes back.
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await expect(prompt(page)).toHaveText("Select a ref to create ‘in’ from");
  await page.keyboard.press("Backspace");
  await expect(field(page)).toHaveValue("in");
  await expect(prompt(page)).toHaveCount(0);
  // In the first step Backspace edits the text; it never closes.
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await expect(picker(page)).toBeVisible();
  await expect(field(page)).toHaveValue("");

  // Esc goes back too, and closes from the first step.
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter"); // HEAD
  await expect(prompt(page)).toHaveText(/^Name the new branch, at HEAD \([0-9a-f]{8}\)$/);
  await page.keyboard.press("Escape");
  await expect(prompt(page)).toHaveText("Select a ref to create the branch from");
  await page.keyboard.press("Escape");
  await expect(prompt(page)).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(picker(page)).toHaveCount(0);

  // The whole way by keys: a new branch from staging.
  await openPicker(page);
  await page.keyboard.type("kb-only");
  await page.keyboard.press("ArrowDown");
  await expect(at()).toHaveAttribute("data-kind", "createFrom");
  await page.keyboard.press("Enter");
  await page.keyboard.type("staging");
  await page.keyboard.press("Enter");
  await expect(notice(page, "Created and checked out kb-only")).toBeVisible();
  expect(current(gg)).toBe("kb-only");
  expect(gg.git("routine", "rev-parse", "kb-only").trim()).toBe(gg.git("routine", "rev-parse", "staging").trim());
});

test("the branch name in the header opens it; a press outside closes it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("routine"));
  const header = page.getByTestId("active-worktree");
  await expect(header).toHaveAttribute("aria-haspopup", "dialog");
  await header.click();
  await expect(picker(page)).toBeVisible();
  await expect(field(page)).toBeFocused();
  await page.mouse.click(1100, 600);
  await expect(picker(page)).toHaveCount(0);
  expect(current(gg)).toBe("main");
});

test("a branch checked out in another worktree makes that worktree active, running no git; a merge stopped here refuses checkouts as the graph does", async ({
  gg,
}) => {
  const { page } = gg;
  await page.goto("/");
  const repo = gg.repoPath("worktrees");
  await open(page, repo);
  const asked = recordInvokes(page);

  await openPicker(page);
  await field(page).fill("feature/login");
  const login = option(page, "feature/login");
  await expect(login.locator(".chip")).toHaveText("in worktree feature-login");
  await page.keyboard.press("Enter");
  await expect(picker(page)).toHaveCount(0);
  await expect(page.getByTestId("active-worktree")).toContainText("feature/login");
  expect(asked().filter((c) => c === "run_op")).toEqual([]);
  expect(asked()).toContain("set_active_worktree");

  // hotfix is stopped in a merge: a checkout there is refused before the core is asked.
  await openPicker(page);
  await field(page).fill("hotfix");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("active-worktree")).toContainText("hotfix");
  await openPicker(page);
  await field(page).fill("patch");
  await page.keyboard.press("Enter");
  await expect(picker(page)).toHaveCount(0);
  await expect(notice(page, "A merge is in progress")).toBeVisible();
  expect(asked().filter((c) => c === "run_op")).toEqual([]);
});

test("10,000 branches: opening and each keystroke render only the rows in view and ask the core nothing", async ({ gg }) => {
  const { page } = gg;
  const repo = makeRepo(
    "many",
    (git, write, dir) => {
      git("init", "-q", "-b", "main");
      const hashes: string[] = [];
      for (let i = 0; i < 20; i++) {
        write(`f${i}.txt`, `${i}\n`);
        git("add", "-A");
        git("commit", "-q", "-m", `Commit ${i}`);
        hashes.push(git("rev-parse", "HEAD").trim());
      }
      // 10,000 local and 2,000 remote branches, written as git packs them.
      const refs: string[] = [];
      for (let i = 0; i < 10_000; i++) refs.push(`${hashes[i % 20]} refs/heads/team/b${String(i).padStart(5, "0")}`);
      for (let i = 0; i < 2_000; i++) refs.push(`${hashes[i % 20]} refs/remotes/origin/r${String(i).padStart(5, "0")}`);
      refs.push(`${hashes[19]} refs/heads/main`);
      refs.sort((a, b) => (a.slice(41) < b.slice(41) ? -1 : 1));
      writeFileSync(path.join(dir, ".git", "packed-refs"), `# pack-refs with: peeled fully-peeled sorted \n${refs.join("\n")}\n`);
    },
    gg.folder("many"),
  );
  await page.goto("/");
  await open(page, repo);
  await page.waitForTimeout(300);
  const asked = recordInvokes(page);

  const opened = await page.evaluate(async () => {
    const t = performance.now();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "p", metaKey: navigator.platform.startsWith("Mac"), ctrlKey: !navigator.platform.startsWith("Mac"), bubbles: true }));
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    return performance.now() - t;
  });
  await expect(picker(page)).toBeVisible();
  const rendered = await options(page).count();
  expect(rendered).toBeGreaterThan(5);
  expect(rendered).toBeLessThanOrEqual(20); // ten in view and a few either side, of 12,002
  await expect(picker(page).locator(".space")).toHaveCSS("height", `${(10_001 + 2_000 + 2) * 42}px`);

  // A keystroke, to the frame that shows its result.
  const keyed = await page.evaluate(async () => {
    const input = document.querySelector<HTMLInputElement>('[data-testid="branch-picker"] input')!;
    const t = performance.now();
    input.value = "b0777";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    return performance.now() - t;
  });
  await expect(option(page, "team/b07770")).toBeVisible();
  expect(await options(page).count()).toBeLessThanOrEqual(20);
  await page.keyboard.press("End");
  await expect(activeOption(page)).toHaveAttribute("data-kind", "createFrom");
  expect(asked()).toEqual([]);
  console.log(`branch picker, 12,002 branches in Chromium: open to a frame ${opened.toFixed(1)} ms, a keystroke to a frame ${keyed.toFixed(1)} ms`);
  // Loose: CI runners are slower; the unit test bounds the work itself.
  expect(opened).toBeLessThan(1000);
  expect(keyed).toBeLessThan(500);
});
