// The command log panel (spec §5.7), end to end against the real core: operations with their
// commands and output, "Show log" from a failed operation's notice, background reads behind a
// toggle, no requests while closed, and the keyboard.
import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, test } from "./fixtures";
import type { OpOutcome, OpRequest } from "../src/lib/types";

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const panel = (page: Page) => page.getByRole("region", { name: "Command log" });
const toggle = (page: Page) => page.getByRole("button", { name: "Command log", exact: true });
const entries = (page: Page) => panel(page).getByRole("log");

/** Runs `request` through the app's own runOp, in the active tab. */
function runOp(page: Page, request: OpRequest): Promise<OpOutcome> {
  return page.evaluate(async (req) => {
    const ops = await import("/src/lib/state/ops.svelte.ts");
    const { app } = await import("/src/lib/state/app.svelte.ts");
    return ops.runOp(app.activeRepoId!, req);
  }, request);
}

/** The merge commit is made by the core's git: give it the fixtures' author. */
function identity(gg: GG, repo: string) {
  gg.git(repo, "config", "user.name", "Ada Lovelace");
  gg.git(repo, "config", "user.email", "ada@example.com");
}

/** Counts the `command_log` requests the page sends from now on. */
function countLogRequests(page: Page): () => number {
  let n = 0;
  page.on("request", (request) => {
    if (!request.url().endsWith("/invoke")) return;
    try {
      if (JSON.parse(request.postData() ?? "{}").cmd === "command_log") n++;
    } catch {
      // Not JSON: not ours.
    }
  });
  return () => n;
}

test("operation_appears_with_its_commands_and_output", async ({ gg }) => {
  const { page } = gg;
  identity(gg, "branchy");
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  await expect(panel(page)).toHaveCount(0);
  const out = await runOp(page, {
    op: "merge",
    target: { kind: "branch", name: "feature/ui" },
    noFf: true,
    squash: false,
    noCommit: false,
  });
  expect(out.status).toBe("done");

  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");
  const op = entries(page).locator(`[data-op="${out.opId}"]`);
  const head = op.locator(".op-head");
  await expect(head).toContainText("Merge feature/ui into main");
  await expect(head).toHaveAttribute("aria-expanded", "true");
  // Its command, quoted as a shell would need it, with how long it took and its exit code.
  const merge = op.locator(".cmd", { hasText: "git merge --no-edit --no-ff feature/ui" });
  await expect(merge).toBeVisible();
  await expect(merge.locator(".exit")).toHaveText("ok");
  await expect(merge.locator(".dur")).toHaveText(/^\d+(\.\d)? (ms|s)$/);

  // Opened, git's own output.
  const row = merge.getByRole("button", { name: /git merge/ });
  await expect(row).toHaveAttribute("aria-expanded", "false");
  await row.click();
  await expect(row).toHaveAttribute("aria-expanded", "true");
  await expect(merge.getByRole("region")).toContainText("Merge made by the 'ort' strategy.");
  await expect(merge.getByRole("region")).toContainText("stdout");

  // Copy gives the command line.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await merge.hover();
  await merge.getByRole("button", { name: "Copy command line" }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "git merge --no-edit --no-ff feature/ui",
  );

  // A new operation comes in while the panel is open, newest last.
  const created = await runOp(page, { op: "create_branch", name: "after-merge", at: out.snapshot.head.hash!, checkout: false });
  await expect(entries(page).locator(".op").last()).toHaveAttribute("data-op", String(created.opId));
  await expect(entries(page).locator(".op").last()).toContainText("Create branch after-merge");

  // Open across a reload (remembered in the app's storage, which its windows share), closed
  // by its button.
  await page.reload();
  await expect(panel(page)).toBeVisible();
  await panel(page).getByRole("button", { name: "Close command log" }).click();
  await expect(panel(page)).toHaveCount(0);
});

test("failed_operation_notice_links_to_its_entry", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("branchy");
  await page.goto("/");
  await open(page, dir);
  // A change in the way of the checkout.
  const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
  writeFileSync(path.join(dir, differing), "in the way\n");
  const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
  expect(failed.status).toBe("failed");
  expect(failed.logFrom).toBeGreaterThan(0);

  const notice = page.getByRole("alert").filter({ hasText: failed.message });
  await notice.getByRole("button", { name: "Show log" }).click();
  await expect(panel(page)).toBeVisible();
  const op = entries(page).locator(`[data-op="${failed.opId}"]`);
  await expect(op).toHaveClass(/target/);
  await expect(op.locator(".op-head")).toContainText("Check out develop");
  await expect(op.locator(".op-head .exit")).toHaveText("exit 1");
  // Its failed command is open, with git's own message.
  const checkout = op.locator(".cmd", { hasText: "git checkout --no-guess develop --" });
  await expect(checkout.locator(".exit")).toHaveText("exit 1");
  await expect(checkout.getByRole("region")).toContainText("would be overwritten by checkout");
  await expect(checkout.getByRole("region")).toContainText("stderr");
  await expect(op.locator(".op-head")).toBeInViewport();
  // The notice stays: the log is read against it.
  await expect(notice).toBeVisible();
});

test("reads_hidden_by_default_and_shown_by_toggle", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  await toggle(page).click();
  await expect(panel(page)).toContainText("No operations yet");
  await expect(entries(page)).toHaveCount(0);

  const reads = panel(page).getByRole("checkbox", { name: "Background reads" });
  await expect(reads).not.toBeChecked();
  await reads.check();
  // The reads the app made to show the repository: the log, refs, status.
  await expect(entries(page).locator(".cmd.read").first()).toBeVisible();
  await expect(entries(page)).toContainText("git status --porcelain=v2");
  await expect(entries(page)).toContainText("git log");

  await reads.uncheck();
  await expect(panel(page)).toContainText("No operations yet");
});

test("panel_closed_makes_no_requests", async ({ gg }) => {
  const { page } = gg;
  const dir = gg.repoPath("small");
  await page.goto("/");
  await open(page, dir);
  const count = countLogRequests(page);

  // Closed: changes come in (the watcher sees each commit), nothing is asked for 3 s.
  for (let i = 0; i < 3; i++) {
    writeFileSync(path.join(dir, `closed-${i}.txt`), `${i}\n`);
    gg.git("small", "add", "-A");
    gg.git("small", "commit", "-q", "-m", `Closed ${i}`);
    await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText(`Closed ${i}`);
  }
  await page.waitForTimeout(3000);
  expect(count()).toBe(0);

  // Open: it asks, and again as the repository changes (at most once a second).
  await toggle(page).click();
  await expect.poll(count).toBeGreaterThan(0);
  const opened = count();
  gg.git("small", "commit", "-q", "--allow-empty", "-m", "While open");
  await expect.poll(count, { timeout: 5000 }).toBeGreaterThan(opened);

  // Closed again: quiet again.
  await toggle(page).click();
  await expect(panel(page)).toHaveCount(0);
  const closed = count();
  gg.git("small", "commit", "-q", "--allow-empty", "-m", "Closed again");
  await expect(grid(page).locator('[data-index="0"] .subject')).toHaveText("Closed again");
  await page.waitForTimeout(3000);
  expect(count()).toBe(closed);
});

test("escape_closes_and_focus_returns", async ({ gg }) => {
  const { page } = gg;
  identity(gg, "branchy");
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  const out = await runOp(page, {
    op: "merge",
    target: { kind: "branch", name: "feature/ui" },
    noFf: true,
    squash: false,
    noCommit: false,
  });
  await grid(page).focus();
  await expect(grid(page)).toBeFocused();

  // The shortcut opens it with the focus on its newest entry.
  await page.keyboard.press("ControlOrMeta+Shift+L");
  await expect(panel(page)).toBeVisible();
  const last = entries(page).locator(".row").last();
  await expect(last).toBeFocused();
  await expect(last).toContainText("git merge");

  // Arrow keys move between rows; Enter opens one; it says it is open.
  await page.keyboard.press("ArrowUp");
  const head = entries(page).locator(`[data-op="${out.opId}"] .op-head`);
  await expect(head).toBeFocused();
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("Enter");
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Space");
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("End");
  await expect(last).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(last).toHaveAttribute("aria-expanded", "true");

  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
  // The toolbar's log button says it is closed.
  await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");
});

test("the graph keeps its minimum under the banner, the details and the log", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 560 });
  await page.goto("/");
  await open(page, gg.repoPath("conflicts"));
  try {
    gg.git("conflicts", "merge", "wide");
  } catch {
    // Stopped on conflicts.
  }
  await expect(page.getByTestId("conflict-banner")).toContainText("12 files have conflicts");
  await grid(page).locator('[role="row"]', { hasText: "Earlier work 3" }).first().click();
  const details = page.getByTestId("commit-details");
  await expect(details).toBeVisible();
  // The details panel dragged as tall as it goes.
  const splitter = details.getByRole("separator", { name: "Resize details panel" });
  const box = (await splitter.boundingBox())!;
  await page.mouse.move(box.x + 100, box.y + 4);
  await page.mouse.down();
  await page.mouse.move(box.x + 100, 0, { steps: 4 });
  await page.mouse.up();
  await toggle(page).click();
  await expect(panel(page)).toBeVisible();

  const heights = await page.evaluate(() => {
    const h = (sel: string) => document.querySelector(sel)!.getBoundingClientRect();
    return {
      graph: h(".graph-area").height,
      body: h(".repo-body").bottom,
      details: h('[data-testid="commit-details"]').bottom,
      log: h('[data-testid="command-log"]').bottom,
      detailsTop: h('[data-testid="commit-details"]').top,
      graphBottom: h(".graph-area").bottom,
    };
  });
  expect(heights.graph).toBeGreaterThanOrEqual(160);
  expect(heights.log).toBeLessThanOrEqual(heights.body + 0.5);
  expect(heights.details).toBeLessThanOrEqual(heights.body + 0.5);
  expect(heights.detailsTop).toBeGreaterThanOrEqual(heights.graphBottom - 0.5);
});

test("show_log_in_a_small_window_shows_the_operation_above_the_details", async ({ gg }) => {
  const { page } = gg;
  await page.setViewportSize({ width: 900, height: 560 });
  const dir = gg.repoPath("conflicts");
  await page.goto("/");
  await open(page, dir);
  try {
    gg.git("conflicts", "merge", "wide");
  } catch {
    // Stopped on conflicts.
  }
  await expect(page.getByTestId("conflict-banner")).toContainText("12 files have conflicts");
  await grid(page).locator('[role="row"]', { hasText: "Earlier work 3" }).first().click();
  const details = page.getByTestId("commit-details");
  await expect(details).toBeVisible();
  // The details panel dragged as tall as it goes.
  const splitter = details.getByRole("separator", { name: "Resize details panel" });
  const box = (await splitter.boundingBox())!;
  await page.mouse.move(box.x + 100, box.y + 4);
  await page.mouse.down();
  await page.mouse.move(box.x + 100, 0, { steps: 4 });
  await page.mouse.up();
  // Another program holds the index: Continue fails before it stages anything, and the 12
  // files stay in the banner.
  writeFileSync(path.join(dir, ".git/index.lock"), "");
  const failed = await runOp(page, { op: "continue", evenWithMarkers: true });
  expect(failed.status).toBe("failed");
  await expect(page.getByTestId("conflict-banner")).toContainText("12 files have conflicts");

  await page.getByRole("alert").filter({ hasText: failed.message }).getByRole("button", { name: "Show log" }).click();
  const op = entries(page).locator(`[data-op="${failed.opId}"]`);
  await expect(op).toHaveClass(/target/);
  // At least two of its rows show in the log's list.
  const visibleRows = await panel(page).evaluate((log) => {
    const list = log.querySelector(".scroller")!.getBoundingClientRect();
    return [...log.querySelectorAll(".target .row")].filter((r) => {
      const b = r.getBoundingClientRect();
      return b.top >= list.top - 0.5 && b.bottom <= list.bottom + 0.5;
    }).length;
  });
  expect(visibleRows).toBeGreaterThanOrEqual(2);
  if (process.env.GG_SCREENS === "1") {
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.screenshot({ path: `docs/perf/screens/small-log-show-${scheme}.png` });
    }
  }
  rmSync(path.join(dir, ".git/index.lock"));
});

test("after_show_log_the_list_follows_the_next_operation", async ({ gg }) => {
  const { page } = gg;
  // A short log: the failed checkout's output fills it.
  await page.setViewportSize({ width: 1100, height: 600 });
  const dir = gg.repoPath("branchy");
  await page.goto("/");
  await open(page, dir);
  const differing = gg.git("branchy", "diff", "--name-only", "main", "develop").trim().split("\n")[0];
  writeFileSync(path.join(dir, differing), "in the way\n");
  const failed = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
  expect(failed.status).toBe("failed");
  await page.getByRole("alert").filter({ hasText: failed.message }).getByRole("button", { name: "Show log" }).click();
  await expect(entries(page).locator(`[data-op="${failed.opId}"]`)).toHaveClass(/target/);

  // The user moves the change out of the way and tries again: the retry shows.
  rmSync(path.join(dir, differing));
  const retried = await runOp(page, { op: "checkout_branch", branch: "develop", fastForward: false });
  expect(retried.status).toBe("done");
  const retry = entries(page).locator(`[data-op="${retried.opId}"] .op-head`);
  await expect(retry).toContainText("Check out develop");
  await expect(retry).toBeInViewport();
});

test("the shortcut closes the log with the focus put back, and another tab does not take it", async ({ gg }) => {
  const { page } = gg;
  identity(gg, "branchy");
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  await runOp(page, { op: "create_branch", name: "for-the-log", at: gg.git("branchy", "rev-parse", "HEAD").trim(), checkout: false });
  await grid(page).focus();
  await page.keyboard.press("ControlOrMeta+Shift+L");
  await expect(entries(page).locator(".row").last()).toBeFocused();
  // Closed by the same shortcut, with the focus inside: it goes back to the graph.
  await page.keyboard.press("ControlOrMeta+Shift+L");
  await expect(panel(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();

  // Opened by the shortcut, the focus moved back to the graph, then another tab: the log
  // shows there too, and the focus stays in the graph.
  await page.keyboard.press("ControlOrMeta+Shift+L");
  await expect(entries(page).locator(".row").last()).toBeFocused();
  await grid(page).focus();
  await open(page, gg.repoPath("small"));
  await expect(panel(page)).toBeVisible();
  await page.waitForTimeout(300);
  expect(await panel(page).evaluate((el) => el.contains(document.activeElement))).toBe(false);
});

test("a log that cannot be read says so and can be read again", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  let refuse = true;
  await page.route("**/invoke", (route) => {
    if (refuse && (route.request().postDataJSON() as { cmd: string } | null)?.cmd === "command_log") {
      return route.fulfill({ json: { err: { code: "io", message: "The core is not answering.", detail: null } } });
    }
    return route.continue();
  });
  await toggle(page).click();
  await expect(panel(page).getByRole("alert")).toContainText("Could not read the command log: The core is not answering.");
  refuse = false;
  await panel(page).getByRole("button", { name: "Try again" }).click();
  await expect(panel(page).getByRole("alert")).toHaveCount(0);
  await expect(panel(page)).toContainText("No operations yet.");
  await page.unrouteAll({ behavior: "wait" });
});

test("show log for an operation no longer kept says so", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("branchy"));
  await page.evaluate(async () => {
    const { showLog, app } = await import("/src/lib/state/app.svelte.ts");
    // A command id the log does not hold (its ids start at 1; older ones are dropped past 1000).
    showLog(app.activeRepoId!, 0);
  });
  await expect(panel(page)).toBeVisible();
  await expect(page.getByRole("status").or(page.getByRole("alert")).filter({ hasText: "That operation's commands are no longer in the log" })).toBeVisible();
});
