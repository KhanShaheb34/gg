// Performance runs only (GG_PERF_SCROLL=walk3 or walk3-dark, loaded with import()): uses plan 3's
// features in the real window the way a person would, on the walk's throwaway repository
// (scripts/walk-fixture.sh; `origin` is https://github.com/octo/cat.git to the app, which git's
// insteadOf sends to the bare repository beside it, and `gh` is the driver's recording fake).
// The settings first (the theme the run asks for, the date format), the staging routine, then
// a worktree: created and made active, a file edited on disk shows in the panel by itself,
// a subset committed, pushed, a pull request created; a clean worktree deleted with one plain
// question, the one with work through the red confirmation; and `gg` from a terminal opening
// another repository in the running app. Marks as in walkProbe.ts.

import type { RepoView } from "../state/repo.svelte";
import { stagingRoutine, walkKit } from "./walkProbe";
import { api } from "../ipc";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const frame = () => new Promise((r) => requestAnimationFrame(() => r(null)));
const text = (el: Element | null | undefined) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();

async function until<T>(what: string, find: () => T | null | undefined | false, ms = 8000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

export async function runWorktreeWalk(view: RepoView, scroller: HTMLElement, theme: "light" | "dark"): Promise<void> {
  const k = walkKit(view, scroller);
  const { dialog, pick, primary, notice, dismissAll, shot, step } = k;
  const mac = navigator.platform.toLowerCase().includes("mac");

  const sheet = () => document.querySelector<HTMLElement>('[data-testid="settings"]');
  const list = () => document.querySelector<HTMLElement>('[data-testid="worktree-list"]');
  const rows = () => [...(list()?.querySelectorAll<HTMLElement>('[role="row"]') ?? [])];
  const row = (folder: string) => rows().find((r) => (r.dataset.path ?? "").endsWith(`/${folder}`));
  const activeRow = () => rows().find((r) => r.dataset.active === "true");
  const byTestId = (id: string, root: ParentNode = document) => root.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  const buttonIn = (root: Element | null | undefined, re: RegExp) =>
    [...(root?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((b) => re.test(text(b)));
  /** A dialog's input whose label (or aria-label) matches `re`. */
  const inputNamed = (re: RegExp) =>
    [...(dialog()?.querySelectorAll<HTMLInputElement>("input") ?? [])].find(
      (i) => re.test(i.getAttribute("aria-label") ?? "") || [...(i.labels ?? [])].some((l) => re.test(text(l))),
    );
  function type(input: HTMLInputElement | HTMLTextAreaElement, value: string): void {
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }
  function key(k: string, mods: { shift?: boolean; command?: boolean } = {}): void {
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: k, shiftKey: mods.shift, metaKey: mac && mods.command, ctrlKey: !mac && mods.command, bubbles: true }),
    );
  }
  function rightClick(el: HTMLElement): void {
    const r = el.getBoundingClientRect();
    el.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2, clientX: r.left + 8, clientY: r.top + r.height / 2 }),
    );
  }
  /** Asks the driver for a change on disk and waits for it, without refreshing anything. */
  async function needOnly(what: string): Promise<number> {
    const at = performance.now();
    await api.perfMark(`walk-need ${what}`, 0).catch(() => {});
    return at;
  }
  async function showPanel(): Promise<void> {
    if (!list()) key("b", { command: true });
    await until("the worktree list", list);
  }

  // --- Settings take effect ------------------------------------------------------------------
  await step(`settings: theme ${theme} and ISO dates take effect at once`, async () => {
    key(",", { command: true });
    const s = await until("the settings sheet", sheet);
    const radio = (group: string, name: string) =>
      [...(byTestId(group, s)?.querySelectorAll<HTMLElement>('[role="radio"]') ?? [])].find((r) => text(r) === name);
    (await until("the theme choice", () => radio("settings-theme", theme === "dark" ? "Dark" : "Light"))).click();
    await until(`the ${theme} theme`, () => document.documentElement.dataset.theme === theme);
    (await until("the date choice", () => radio("settings-dates", "ISO 8601"))).click();
    await until("ISO dates in the graph", () => /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/.test(text(scroller.querySelector('[role="row"] .date'))));
    await shot(`settings-${theme}`);
    s.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until("the sheet closed", () => !sheet());
    return `theme ${document.documentElement.dataset.theme}; first date ${text(scroller.querySelector('[role="row"] .date'))}`;
  });

  // --- The staging routine (plan 2) still works ----------------------------------------------
  await stagingRoutine(k);

  // --- Worktrees -----------------------------------------------------------------------------
  await step("the side panel lists the main worktree and says what worktrees are for", async () => {
    await showPanel();
    await until("one row", () => rows().length === 1);
    const intro = await until("the intro", () => byTestId("worktrees-intro"));
    await shot("panel-one");
    return `${window.innerWidth}x${window.innerHeight}: ${text(intro).slice(0, 60)}…`;
  });
  await step("create a worktree, made active", async () => {
    (await until("Add worktree", () => buttonIn(byTestId("worktrees-intro"), /^Add worktree$/))).click();
    await until("the Create worktree dialog", () => /Create worktree/.test(text(dialog())));
    type(await until("the branch name field", () => inputNamed(/New branch name/)), "walk/wt");
    const location = await until("the location", () => byTestId("worktree-location", dialog()!) as HTMLInputElement | null);
    await until("the location follows the name", () => location.value.endsWith("work.worktrees/walk-wt"));
    const activeBox = await until("Make it active", () => inputNamed(/Make it the active worktree/));
    if (!activeBox.checked) activeBox.click();
    await frame();
    await shot("dialog-create-worktree");
    await primary(/^Create worktree$/);
    await until("two rows, the new one active", () => rows().length === 2 && activeRow()?.dataset.path?.endsWith("/walk-wt"), 15000);
    await until("the header names it", () => /walk\/wt/.test(text(byTestId("active-worktree"))));
    return location.value;
  });
  await step("files changed on disk show in the panel by themselves", async () => {
    const at = await needOnly("edit worktree walk-wt");
    const count = await until("3 changes on the row", () => text(row("walk-wt")?.querySelector(".changes")) === "3" && performance.now(), 10000);
    await shot("panel-changes");
    return `seen ${Math.round(count - at)} ms after the driver was asked (it writes after reading the mark)`;
  });
  await step("commit a subset of the files", async () => {
    (await until("the changes button", () => row("walk-wt")?.querySelector<HTMLButtonElement>(".changes"))).click();
    const panel = await until("the commit panel", () => byTestId("commit-panel"));
    const options = () => [...panel.querySelectorAll<HTMLElement>('[role="option"]')];
    await until("3 files", () => options().length === 3);
    (await until("scratch.txt", () => options().find((o) => o.dataset.path === "scratch.txt"))).click();
    await until("2 of 3 ticked", () => text(byTestId("commit-count")) === "2 of 3 files ticked");
    type(await until("the message", () => byTestId("commit-message") as HTMLTextAreaElement | null), "Walk: a change in a worktree");
    await frame();
    await shot("commit-subset");
    (await until("Commit", () => {
      const b = byTestId("commit-button") as HTMLButtonElement | null;
      return b && !b.disabled ? b : null;
    })).click();
    const n = await notice(/Committed 2 files to walk\/wt/);
    await until("1 file left", () => options().length === 1 && options()[0].dataset.path === "scratch.txt");
    await shot("notice-committed");
    return text(n);
  }, { keepNotices: true });
  await step("push from the commit notice", async () => {
    const n = await notice(/Committed 2 files/);
    (await until("its Push button", () => buttonIn(n, /^Push walk\/wt/))).click();
    return text(await notice(/Pushed walk\/wt/));
  });
  await step("create a pull request (the fake gh)", async () => {
    await dismissAll();
    // Back to the list, then the worktree's menu.
    byTestId("side-back")?.click();
    await until("the worktree list", list);
    rightClick(await until("the worktree's row", () => row("walk-wt")));
    await pick(/^Create pull request…$/);
    const d = await until("the dialog", () => (/pull request/i.test(text(dialog())) ? dialog() : null));
    await until("the suggested title", () => (byTestId("pr-title", d) as HTMLInputElement | null)?.value);
    await until("the repository named", () => text(byTestId("pr-repo", d)) === "octo/cat");
    await shot("dialog-create-pr");
    // gh lists it once made: the app reloads the list right after making one.
    await needOnly("list pr 21");
    await sleep(1000);
    // "Create pull request", or "Push … and create pull request" when the branch is not there.
    await primary(/create pull request$/i);
    const n = await notice(/Opened pull request #21 of walk\/wt into main/, 20000);
    await shot("notice-pr");
    const said = text(n);
    await until("the #21 marker on the worktree", () => text(row("walk-wt")?.querySelector(".pr-chip")) === "#21", 15000);
    await dismissAll();
    await shot("panel-pr");
    return said;
  });
  await step("delete a clean worktree: one plain question", async () => {
    // Made from the panel's own menu, not made active.
    (await until("Worktree actions", () => document.querySelector<HTMLButtonElement>('button[aria-label="Worktree actions"]'))).click();
    await pick(/^Add worktree…$/);
    await until("the Create worktree dialog", () => /Create worktree/.test(text(dialog())));
    type(await until("the branch name field", () => inputNamed(/New branch name/)), "walk/clean");
    const activeBox = await until("Make it active", () => inputNamed(/Make it the active worktree/));
    if (activeBox.checked) activeBox.click();
    await primary(/^Create worktree$/);
    await until("its row", () => row("walk-clean"), 15000);
    await until("its status read", () => row("walk-clean")?.querySelector(".pending") === null);
    rightClick(row("walk-clean")!);
    await pick(/^Delete…$/);
    const d = await until("the delete dialog", () => (/Delete worktree walk-clean/.test(text(dialog())) ? dialog() : null));
    await shot("dialog-delete-clean");
    if (d.getAttribute("role") === "alertdialog") throw new Error("a clean worktree asked in red");
    await primary(/^Delete worktree$/);
    await until("its row gone", () => !row("walk-clean"), 15000);
    if (document.querySelector('[role="alertdialog"]')) throw new Error("a red confirmation came");
  });
  await step("delete the worktree with work: the red confirmation", async () => {
    rightClick(await until("the worktree's row", () => row("walk-wt")));
    await pick(/^Delete…$/);
    await until("the delete dialog", () => /Delete worktree walk-wt/.test(text(dialog())));
    const note = text(byTestId("delete-work-note"));
    await primary(/^Delete worktree$/);
    const red = await until("the red confirmation", () => document.querySelector<HTMLElement>('[role="alertdialog"]'));
    await until("scratch.txt listed", () => /scratch\.txt/.test(text(byTestId("dangers", red))));
    await shot("dialog-delete-red");
    await primary(/^Delete worktree$/);
    await until("its row gone, main active", () => !row("walk-wt") && rows().length === 1 && activeRow()?.dataset.path === rows()[0].dataset.path, 15000);
    return note;
  });

  // --- `gg` from a terminal into the running app -----------------------------------------------
  await step("gg in a terminal opens another repository in the running app", async () => {
    const before = document.querySelectorAll('[role="tab"]').length;
    await needOnly("gg other");
    const tab = await until("its tab", () => [...document.querySelectorAll<HTMLElement>('[role="tab"]')].find((t) => text(t) === "other"), 15000);
    await until("selected", () => tab.getAttribute("aria-selected") === "true");
    await shot("gg-opened");
    return `${before} tab(s) before, ${document.querySelectorAll('[role="tab"]').length} after`;
  });

  await k.finish();
}
