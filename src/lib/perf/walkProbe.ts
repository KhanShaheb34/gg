// Performance runs only (GG_PERF_SCROLL=walk, loaded with import()): uses the app in its real
// window the way a person would, through its menus, dialogs, notices, banner and log (DOM
// events on the real elements), on a throwaway repository made by the walk's driver script.
// Each step is reported as a perf mark `walk <name>` (1 = done as expected, 0 = not) with
// what was seen in `walk-note <name>: <text>` marks. Where the files must change on disk (a
// conflict resolved in an editor, a change to stash) it marks `walk-need <what>` and waits for
// the driver to do it; at `walk-shot <name>` it holds still for 1.5 s while the driver takes a
// screenshot of the window.
//
// The repository (see the driver): `main` checked out; `staging` 2 behind `origin/staging`;
// `feature/login` from staging; `clash` and `topic` change `app.txt` like main does (a merge
// and a rebase that conflict); a tag `v1`; `origin` a bare repository beside it.

import { api } from "../ipc";
import type { RepoView } from "../state/repo.svelte";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const frame = () => new Promise((r) => requestAnimationFrame(() => r(null)));

const mark = (name: string, value = 0) => api.perfMark(name, value).catch(() => {});

async function until<T>(what: string, find: () => T | null | undefined | false, ms = 8000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

const text = (el: Element | null | undefined) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();

/** The walk's means of using the window (shared by both walks). */
export function walkKit(view: RepoView, scroller: HTMLElement) {
  let passed = 0;
  let failed = 0;

  // A remote branch at the same commit as its local one is the remote half of that label.
  const label = (name: string) =>
    scroller.querySelector<HTMLElement>(`.ref[data-ref="${CSS.escape(name)}"]`) ??
    (name.startsWith("origin/")
      ? scroller.querySelector<HTMLElement>(`.ref[data-ref="${CSS.escape(name.slice(7))}"] .remotes`)
      : null);
  const rowWith = (subject: string) =>
    [...scroller.querySelectorAll<HTMLElement>('[role="row"]')].find((r) => {
      const shown = text(r.querySelector(".subject"));
      // "Uncommitted changes (2)": the count follows the subject.
      return shown === subject || shown.startsWith(`${subject} (`);
    });
  const menu = () => document.querySelector<HTMLElement>('[data-testid="context-menu"]');
  const dialog = () => document.querySelector<HTMLElement>('[role="dialog"], [role="alertdialog"]');
  const notices = () => [...document.querySelectorAll<HTMLElement>(".toast")];
  const banner = () => document.querySelector<HTMLElement>('[data-testid="conflict-banner"]');

  function rightClick(el: HTMLElement): void {
    const r = el.getBoundingClientRect();
    el.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2, clientX: r.left + 6, clientY: r.top + r.height / 2 }),
    );
  }
  /** What an element the walk right-clicks shows: a label's name, else its row's subject. */
  const shows = (el: HTMLElement) =>
    el.closest<HTMLElement>(".ref")?.dataset.ref ?? text(el.closest('[role="row"]')?.querySelector(".subject"));
  /**
   * Opens the menu of what `el` finds. Scrolling it into view can redraw the rows: the
   * element found before the frame may by then show another commit or label (its slot reused
   * for another row). So it is looked up again after the frame and right-clicked only once it
   * shows the same name it did; and the menu must open on that same name (R43).
   */
  async function menuOn(el: () => HTMLElement | null | undefined, what: string): Promise<void> {
    const first = await until(what, el);
    const name = shows(first);
    first.scrollIntoView({ block: "center" });
    for (let attempt = 0; ; attempt++) {
      await frame();
      await frame();
      const target = await until(what, el);
      if (shows(target) !== name) {
        if (attempt >= 20) throw new Error(`${what} shows ${shows(target)}, not ${name}`);
        continue;
      }
      rightClick(target);
      await until("its menu", menu);
      // The element still shows what was meant once the menu is open: it is that one's menu.
      if (shows(target) === name && target.isConnected) return;
      document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until("the menu closed", () => !menu());
      if (attempt >= 20) throw new Error(`${what} changed under its menu`);
    }
  }
  async function pick(name: RegExp): Promise<void> {
    const item = await until(`menu item ${name}`, () =>
      // Every open panel: a submenu is a panel of its own.
      [...document.querySelectorAll<HTMLElement>('[data-testid="context-menu"] [role="menuitem"]')].find((el) =>
        name.test(text(el)),
      ),
    );
    if (item.getAttribute("aria-disabled") === "true") throw new Error(`"${text(item)}" is disabled: ${item.title}`);
    item.click();
  }
  async function primary(named?: RegExp): Promise<void> {
    const d = await until("a dialog", dialog);
    const button = await until("its primary button", () => {
      const b = d.querySelector<HTMLButtonElement>("button.primary");
      return b && !b.disabled && (!named || named.test(text(b))) ? b : null;
    });
    // A red button takes no click for a moment after its dialog opens.
    if (button.classList.contains("danger")) await sleep(600);
    button.click();
  }
  async function field(value: string): Promise<void> {
    const input = await until("a text field", () => dialog()?.querySelector<HTMLInputElement>("input:not([type=checkbox]):not([type=radio])"));
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await frame();
  }
  async function notice(re: RegExp, ms = 15000): Promise<HTMLElement> {
    return until(`a notice ${re}`, () => notices().find((n) => re.test(text(n))), ms);
  }
  async function dismissAll(): Promise<void> {
    for (const b of document.querySelectorAll<HTMLButtonElement>('button[aria-label="Dismiss notification"]')) b.click();
    await frame();
  }
  async function idle(): Promise<void> {
    await until("the operation to end", () => view.snapshot.op === null && !document.querySelector('[data-testid="busy"]'), 20000);
    await sleep(150);
  }
  async function need(what: string): Promise<void> {
    await mark(`walk-need ${what}`);
    await sleep(2500); // the driver writes the files
    await view.refresh();
  }
  async function shot(name: string): Promise<void> {
    await frame();
    await mark(`walk-shot ${name}`);
    await sleep(1500);
  }
  /** Runs a step; its notices are dismissed after it unless `keepNotices` (the next step uses them). */
  async function step(name: string, body: () => Promise<string | void>, opts: { keepNotices?: boolean } = {}): Promise<void> {
    try {
      const note = await body();
      passed++;
      await mark(`walk ${name}`, 1);
      if (note) await mark(`walk-note ${name}: ${note}`);
    } catch (e) {
      failed++;
      await mark(`walk ${name}`, 0);
      const head = view.snapshot.head.branch ?? "detached";
      const shown = notices().map(text).join(" | ");
      const ops = await api.commandLog(0, { repoId: view.repoId, writesOnly: true }).catch(() => []);
      const last = ops.slice(-3).map((r) => `${r.opTitle}: git ${r.args.join(" ")}`).join(" | ");
      await mark(`walk-note ${name} last writes: ${last}`);
      await mark(`walk-note ${name}: ${e instanceof Error ? e.message : String(e)} (HEAD ${head}; notices: ${shown})`);
      // Leave nothing open for the next step.
      for (let i = 0; i < 3 && (dialog() || menu()); i++) {
        document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        await sleep(100);
      }
    }
    if (!opts.keepNotices) await dismissAll();
    await idle().catch(() => {});
  }
  const commitRow = (subject: string) => () => rowWith(subject)?.querySelector<HTMLElement>(".subject");

  /** Reports how many steps passed and failed. */
  async function finish(): Promise<void> {
    await mark("walk-passed", passed);
    await mark("walk-failed", failed);
  }

  return { label, rowWith, menu, dialog, notices, banner, menuOn, pick, primary, field, notice, dismissAll, idle, need, shot, step, commitRow, finish };
}

type Kit = ReturnType<typeof walkKit>;

/** The staging routine: check out staging (fast-forwarded), merge a feature in, push. */
export async function stagingRoutine(k: Kit): Promise<void> {
  const { label, dialog, menuOn, pick, primary, notice, shot, step } = k;
  await step("checkout staging with fast-forward", async () => {
    await menuOn(() => label("staging"), "the staging label");
    await shot("menu-branch");
    await pick(/^Check out$/);
    await until("the fast-forward question", () => /Fast-forward/.test(text(dialog())));
    await shot("dialog-fast-forward");
    await primary(/^Yes$/);
    const n = await notice(/staging/);
    return text(n);
  });
  await step("merge feature/login into staging", async () => {
    await menuOn(() => label("feature/login"), "the feature/login label");
    await pick(/^Merge feature\/login into staging/);
    await shot("dialog-merge");
    await primary(/^Merge$/);
    const n = await notice(/Merged|merge/i);
    await shot("notice-merge-push");
    const push = await until("its Push button", () =>
      [...n.querySelectorAll<HTMLButtonElement>("button")].find((b) => /^Push staging/.test(text(b))),
    );
    push.click();
    const pushed = await notice(/Pushed staging/);
    return text(pushed);
  });
}

export async function runWalk(view: RepoView, scroller: HTMLElement): Promise<void> {
  const k = walkKit(view, scroller);
  const { label, rowWith, dialog, banner, menuOn, pick, primary, field, notice, dismissAll, idle, need, shot, step, commitRow } = k;
  await stagingRoutine(k);

  // --- Each commit menu item once --------------------------------------------------------------
  await step("create branch here", async () => {
    await menuOn(commitRow("Prepare staging"), "a commit");
    await shot("menu-commit");
    await pick(/^Create branch here/);
    await field("walk/branch");
    await primary();
    await until("the new label", () => label("walk/branch"));
  });
  await step("create tag here", async () => {
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Create tag here/);
    await field("walk-tag");
    await primary();
    await until("the new tag", () => label("walk-tag"));
  });
  await step("copy hash and message", async () => {
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Copy hash$/);
    await notice(/Copied/);
    await dismissAll();
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Copy message$/);
    await notice(/Copied/);
  });
  await step("cherry-pick", async () => {
    await menuOn(commitRow("Side note"), "the side commit");
    await pick(/^Cherry-pick/);
    await primary();
    return text(await notice(/Cherry-pick|Applied|picked/i));
  });
  await step("revert", async () => {
    await menuOn(commitRow("Side note"), "the side commit");
    await pick(/^Revert/);
    await primary();
    return text(await notice(/Revert/i));
  });
  await step("reset soft", async () => {
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Reset \S+ to here$/);
    await pick(/^Soft/);
    await primary();
    return text(await notice(/Reset|reset/));
  });
  await step("reset hard needs the red confirmation", async () => {
    await need("dirty");
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Reset \S+ to here$/);
    await pick(/^Hard/);
    await primary();
    await until("the red confirmation", () => document.querySelector('[role="alertdialog"]'));
    await shot("dialog-reset-hard-confirm");
    await primary();
    return text(await notice(/Reset|reset/));
  });
  await step("check out this commit, then main again", async () => {
    await menuOn(commitRow("Prepare staging"), "a commit");
    await pick(/^Check out this commit$/);
    await notice(/detached/);
    await dismissAll();
    await menuOn(() => label("main"), "the main label");
    await pick(/^Check out$/);
    if (dialog()) await primary(/^Yes$|^No$/);
    return text(await notice(/main/));
  });

  // --- Branch, remote and tag items ----------------------------------------------------------
  await step("rename branch", async () => {
    await menuOn(() => label("walk/branch"), "walk/branch");
    await pick(/^Rename/);
    await field("walk/renamed");
    await primary();
    await until("the renamed label", () => label("walk/renamed"));
  });
  await step("push branch (sets upstream)", async () => {
    await menuOn(() => label("walk/renamed"), "walk/renamed");
    await pick(/^Push/);
    await shot("dialog-push");
    await primary();
    return text(await notice(/Pushed/));
  });
  await step("delete branch (merged: one question)", async () => {
    await menuOn(() => label("walk/renamed"), "walk/renamed");
    await pick(/^Delete/);
    await primary();
    await until("the label gone", () => !label("walk/renamed"));
  });
  await step("delete branch on the remote (red confirmation)", async () => {
    await menuOn(() => label("origin/walk/renamed"), "origin/walk/renamed");
    await pick(/^Delete from origin/);
    await until("the red confirmation", () => document.querySelector('[role="alertdialog"]'));
    await shot("dialog-delete-remote-confirm");
    await primary();
    await until("the remote label gone", () => !label("origin/walk/renamed"));
  });
  await step("check out a remote branch as local", async () => {
    await menuOn(() => label("origin/remote-only"), "origin/remote-only");
    await pick(/^Check out as a local branch/);
    await primary();
    return text(await notice(/remote-only/));
  });
  await step("fetch a remote branch into a local one", async () => {
    await menuOn(() => label("origin/remote-only"), "origin/remote-only");
    await pick(/^Fetch into a local branch/);
    await field("from-remote");
    await primary();
    await until("the new label", () => label("from-remote"));
  });
  await step("pull", async () => {
    await menuOn(() => label("remote-only"), "remote-only");
    await pick(/^Pull/);
    return text(await notice(/up to date|Pulled|Fast-forward|pull/i));
  });
  await step("tag: push and delete", async () => {
    await menuOn(() => label("walk-tag"), "walk-tag");
    await shot("menu-tag");
    await pick(/^Push/);
    await primary();
    await notice(/Pushed/);
    await dismissAll();
    await menuOn(() => label("walk-tag"), "walk-tag");
    await pick(/^Delete/);
    await primary();
    await until("the tag gone", () => !label("walk-tag"));
  });
  await step("back on main", async () => {
    await menuOn(() => label("main"), "the main label");
    await pick(/^Check out$/);
    if (dialog()) await primary(/^Yes$|^No$/);
    await until("HEAD on main", () => view.snapshot.head.branch === "main");
  });

  // --- A conflicted merge and rebase through the banner ----------------------------------------
  await step("conflicted merge through the banner", async () => {
    await menuOn(() => label("clash"), "the clash label");
    await pick(/^Merge clash into main/);
    await primary(/^Merge$/);
    const b = await until("the banner", banner);
    await shot("banner-merge");
    await need("resolve app.txt");
    b.dispatchEvent(new PointerEvent("pointerenter"));
    const go = await until("Continue enabled", () => {
      b.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
      const c = [...b.querySelectorAll<HTMLButtonElement>("button")].find((x) => text(x) === "Continue…");
      return c && c.getAttribute("aria-disabled") !== "true" ? c : null;
    }, 10000);
    go.click();
    await shot("dialog-continue");
    await primary();
    await until("the banner gone", () => !banner(), 15000);
    return text(await notice(/Finished the merge/));
  });
  await step("conflicted rebase through the banner", async () => {
    await menuOn(() => label("topic"), "the topic label");
    await pick(/^Check out$/);
    await until("HEAD on topic", () => view.snapshot.head.branch === "topic");
    await idle();
    await menuOn(() => label("main"), "the main label");
    await pick(/^Rebase topic onto main/);
    await primary();
    const b = await until("the banner", banner);
    await shot("banner-rebase");
    await need("resolve app.txt");
    const go = await until("Continue enabled", () => {
      b.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
      const c = [...b.querySelectorAll<HTMLButtonElement>("button")].find((x) => text(x) === "Continue…");
      return c && c.getAttribute("aria-disabled") !== "true" ? c : null;
    }, 10000);
    go.click();
    await primary();
    await until("the banner gone", () => !banner(), 15000);
    const done = text(await notice(/Finished rebasing|rebas/i));
    await dismissAll();
    await menuOn(() => label("main"), "the main label");
    // The menu's items name the branch meant: main's has "Check out" and merges main in.
    const items = [...document.querySelectorAll('[data-testid="context-menu"] [role="menuitem"]')].map(text);
    if (!items.some((i) => /^Merge main into /.test(i))) throw new Error(`not main's menu: ${items.join(", ")}`);
    await pick(/^Check out$/);
    await until("HEAD on main", () => view.snapshot.head.branch === "main");
    return done;
  });

  // --- Stash round trip ------------------------------------------------------------------------
  await step("stash and pop", async () => {
    await need("dirty");
    await menuOn(commitRow("Uncommitted changes"), "the uncommitted row");
    await pick(/^Stash changes/);
    await field("Walk stash");
    await primary(/^Stash$/);
    await until("the stash row", () => rowWith("Walk stash") ?? [...scroller.querySelectorAll('[role="row"]')].find((r) => /Walk stash/.test(text(r))));
    await menuOn(
      () => [...scroller.querySelectorAll<HTMLElement>('[role="row"]')].find((r) => /Walk stash/.test(text(r)))?.querySelector<HTMLElement>(".subject"),
      "the stash row",
    );
    await shot("menu-stash");
    await pick(/^Pop$/);
    await until("the uncommitted row back", () => rowWith("Uncommitted changes"), 15000);
    return text(await notice(/stash|Stash/));
  });

  // --- The command log -------------------------------------------------------------------------
  await step("command log by shortcut, Show log from a failure", async () => {
    const mac = navigator.platform.toLowerCase().includes("mac");
    const logPanel = () => document.querySelector<HTMLElement>('[data-testid="command-log"]');
    // The panel is remembered open or closed across launches: closed first, then the shortcut.
    if (logPanel()) document.querySelector<HTMLButtonElement>('button[aria-label="Close command log"]')?.click();
    await until("the log closed", () => !logPanel());
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "L", shiftKey: true, metaKey: mac, ctrlKey: !mac, bubbles: true }));
    await until("the log panel", logPanel);
    const ops = await until("its operations", () => {
      const all = logPanel()?.querySelectorAll(".op");
      return all && all.length >= 10 ? all : null;
    });
    await shot("log-open");
    // A failure: checking out staging with the uncommitted change in the way.
    await menuOn(() => label("staging"), "the staging label");
    await pick(/^Check out$/);
    if (dialog()) await primary(/^Yes$|^No$/);
    const n = await notice(/would be overwritten|in the way|changes/i);
    const show = await until("Show log", () => [...n.querySelectorAll<HTMLButtonElement>("button")].find((b) => text(b) === "Show log"));
    show.click();
    await until("the highlighted entry", () => logPanel()?.querySelector(".op.target"));
    await shot("log-show-failure");
    return `${ops.length} operations listed`;
  });

  await k.finish();
}
