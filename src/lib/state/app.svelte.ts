// The app shell's state: open tabs, recents, settings and toasts, and the actions on them.
// Every change to the tabs (or the active one) is saved as the session.

import { api, CoreError, onEvent } from "../ipc";
import { applyTheme, nextTheme, onSystemThemeChange, type ThemeSetting } from "../theme";
import type { CoreErrorDto, Recent, RepoSnapshot, Settings } from "../types";
import { setLogOpen } from "./commandLog.svelte";
import { disposeRepoView, repoView } from "./repo.svelte";

export type Tab = { repoId: number; path: string; name: string };
export type ToastKind = "info" | "error";
/**
 * A button on a notice. Clicking it dismisses the notice, unless `keepOpen` (then `run`
 * dismisses it when done). `danger`: it destroys something.
 */
export type ToastAction = { label: string; run: () => void | Promise<void>; danger?: boolean; keepOpen?: boolean };
/**
 * A notice. A persistent one stays until dismissed. `detail` (git's own text, say) shows
 * below the message, or behind a disclosure with `foldDetail`. `actions` are buttons on it;
 * `link` a quieter one after them ("Show log").
 */
export type Toast = {
  id: number;
  kind: ToastKind;
  message: string;
  detail?: string;
  foldDetail?: boolean;
  persistent?: boolean;
  /**
   * With `persistent`: it stays until dismissed, one of its buttons is used, or an operation
   * after `opId` runs on repository `repoId` (what its buttons offer may no longer hold).
   */
  until?: { repoId: number; opId: number };
  actions?: ToastAction[];
  link?: ToastAction;
};

export const app = $state({
  tabs: [] as Tab[],
  activeRepoId: null as number | null,
  recents: [] as Recent[],
  settings: { theme: "system", fetchIntervalSecs: 300 } as Settings,
  toasts: [] as Toast[],
  /** The stored state has been loaded (or failed to load); until then nothing is drawn. */
  ready: false,
  /**
   * The saved session's tabs are being reopened and the one to show is not open yet: the
   * window shows the tabs that are, and no welcome screen.
   */
  restoring: false,
  /** The core could not find git: nothing can be opened. */
  gitMissing: false,
  /** The user's home folder, for showing paths as `~/…`. */
  homeDir: null as string | null,
  /** The command-log entries asked for last ("Show log" on a notice): from id `from` on. */
  logRequest: null as { repoId: number; from: number } | null,
});

let nextToastId = 1;

/** Shows a toast; the Toast component dismisses it after a while. */
export function toast(kind: ToastKind, message: string, detail?: string): void {
  app.toasts.push(detail ? { id: nextToastId++, kind, message, detail } : { id: nextToastId++, kind, message });
}

/**
 * Moves the focus to the newest notice: its first button (Push…, Show log), else its dismiss
 * button. False when there is none.
 */
export function focusNewestNotice(): boolean {
  const notices = document.querySelectorAll<HTMLElement>("[data-notices] .toast");
  const newest = notices[notices.length - 1];
  const button = newest?.querySelector<HTMLElement>("button");
  if (!button) return false;
  button.focus();
  return true;
}

/** Shows `notice`; returns its id. */
export function notify(notice: Omit<Toast, "id">): number {
  const id = nextToastId++;
  app.toasts.push({ ...notice, id });
  return id;
}

/**
 * Opens the command log at the entry holding command `from` of repository `repoId` (its tab
 * comes to the front): "Show log" on a notice. The panel goes there and highlights it.
 */
export function showLog(repoId: number, from: number): void {
  app.logRequest = { repoId, from };
  activate(repoId);
  setLogOpen(true);
}

/**
 * The settings file could not be read: settings and the session apply for this run only.
 * Said once, with a way to move the file aside and save again.
 */
function reportSettingsProblem(problem: string): void {
  const id = notify({
    kind: "error",
    message: "Settings are not being saved",
    detail: problem,
    persistent: true,
    actions: [
      {
        label: "Reset settings",
        // The notice stays if the reset fails.
        keepOpen: true,
        run: async () => {
          try {
            await api.resetSettings();
          } catch (e) {
            reportError(e, "Could not reset settings");
            return;
          }
          dismissToast(id);
          toast("info", "Settings reset: saving again.");
        },
      },
    ],
  });
}

export function dismissToast(id: number): void {
  app.toasts = app.toasts.filter((t) => t.id !== id);
}

/** Reports a failed core call: git missing switches the app to its no-git panel. */
export function reportError(e: unknown, prefix?: string): void {
  if (e instanceof CoreError && e.code === "git_not_found") {
    app.gitMissing = true;
    return;
  }
  const message = e instanceof Error ? e.message : String(e);
  const detail = e instanceof CoreError ? (e.detail ?? undefined) : undefined;
  toast("error", prefix ? `${prefix}: ${message}` : message, detail);
}

/**
 * Saves the tabs and the active one. While the session is being reopened, its tabs that are
 * not open yet are saved too, at their places (and the tab still to be shown as active, if
 * none is), so a save made meanwhile loses nothing.
 */
function saveSession(): void {
  const open = app.tabs.map((t, i) => ({ rank: sessionRank.get(t.repoId) ?? Infinity, i, path: t.path }));
  // A session tab already open another way (the launch path) is not listed twice.
  const placed = new Set(app.tabs.map((t) => sessionRank.get(t.repoId)));
  const waiting = [...stillReopening]
    .filter(([rank]) => !placed.has(rank))
    .map(([rank, path]) => ({ rank, i: -1, path }));
  // Session tabs by their place there; tabs the user opened after them, in tab order.
  const paths = [...open, ...waiting].sort((a, b) => a.rank - b.rank || a.i - b.i).map((t) => t.path);
  const active = app.tabs.find((t) => t.repoId === app.activeRepoId)?.path ?? restoreTarget;
  api.saveSession(paths, active).catch((e: unknown) => reportError(e, "Could not save the open tabs"));
}

async function refreshRecents(): Promise<void> {
  try {
    app.recents = (await api.getAppState()).recents;
  } catch {
    // The list is only stale until the next successful refresh.
  }
}

/** Opens the repository at `path` as a tab (or the tab it already has) without saving. */
async function openTab(path: string): Promise<boolean> {
  let snapshot;
  try {
    snapshot = await api.openRepo(path);
  } catch (e) {
    reportError(e);
    return false;
  }
  addTab(snapshot);
  show(snapshot.repoId);
  return true;
}

/** Where each tab reopened from the session goes: its place in the saved order. */
const sessionRank = new Map<number, number>();
/** The session's tabs still being reopened: place in the saved order → path. */
const stillReopening = new Map<number, string>();

/**
 * Adds a tab (and its graph's view) for `snapshot`'s repository, unless it has one. A tab
 * reopened from the session (`rank`: its place there) goes before the tabs ranked after it,
 * whatever order the repositories finish opening in; others go last.
 */
function addTab(snapshot: RepoSnapshot, rank?: number): void {
  const existing = app.tabs.findIndex((t) => t.repoId === snapshot.repoId);
  if (existing >= 0) {
    // Opened first another way (the launch path) but a session tab: to its session place.
    if (rank !== undefined && !sessionRank.has(snapshot.repoId)) {
      const [tab] = app.tabs.splice(existing, 1);
      sessionRank.set(snapshot.repoId, rank);
      const at = app.tabs.findIndex((t) => (sessionRank.get(t.repoId) ?? Infinity) > rank);
      app.tabs.splice(at < 0 ? app.tabs.length : at, 0, tab);
    }
    return;
  }
  const tab = { repoId: snapshot.repoId, path: snapshot.rootPath, name: snapshot.name };
  if (rank === undefined) {
    app.tabs.push(tab);
  } else {
    sessionRank.set(snapshot.repoId, rank);
    const at = app.tabs.findIndex((t) => (sessionRank.get(t.repoId) ?? Infinity) > rank);
    app.tabs.splice(at < 0 ? app.tabs.length : at, 0, tab);
  }
  repoView(snapshot.repoId, snapshot).onNotice = (message, detail) => toast("error", message, detail);
  const held = heldErrors.get(snapshot.repoId);
  heldErrors.delete(snapshot.repoId);
  for (const error of held ?? []) reportError(new CoreError(error), snapshot.name);
}

/**
 * `repo-error`s of repositories that have no tab yet: the core sends the errors of a
 * repository it opened at start while the UI's `open_repo` for it is being answered, before
 * its tab exists. Shown, named, when the tab is added. A few repositories' worth at most.
 */
const heldErrors = new Map<number, CoreErrorDto[]>();
const MAX_HELD_REPOS = 16;

function onRepoError(repoId: number, error: CoreErrorDto): void {
  const tab = app.tabs.find((t) => t.repoId === repoId);
  if (tab) {
    reportError(new CoreError(error), tab.name);
    return;
  }
  const held = heldErrors.get(repoId) ?? [];
  held.push(error);
  heldErrors.set(repoId, held);
  if (heldErrors.size > MAX_HELD_REPOS) heldErrors.delete(heldErrors.keys().next().value!);
}

/** Opens the repository at `path` in a tab and activates it; failures become toasts. */
export async function openPath(path: string): Promise<void> {
  if (!(await openTab(path))) return;
  saveSession();
  await refreshRecents();
}

/** Asks for a folder and opens it. */
export async function openViaPicker(): Promise<void> {
  let path;
  try {
    path = await api.pickFolder();
  } catch (e) {
    reportError(e);
    return;
  }
  if (path) await openPath(path);
}

/** Makes `repoId` the active tab, refreshing it first if the window was away meanwhile. */
function show(repoId: number): void {
  app.activeRepoId = repoId;
  // A choice made while the session is reopened stands.
  restoreTarget = null;
  const view = repoView(repoId);
  if (view.stale) void view.sync();
}

export function activate(repoId: number): void {
  if (app.activeRepoId === repoId || !app.tabs.some((t) => t.repoId === repoId)) return;
  show(repoId);
  saveSession();
}

/**
 * The window gained or lost focus. Coming back, only the active repository is re-read (its
 * working tree may have changed; only git dirs are watched); the others are re-read when
 * they are next shown. The core is told too: it runs scheduled fetches only around focus.
 */
export function windowFocusChanged(focused: boolean): void {
  api.setWindowFocused(focused).catch(() => {
    // Focus only tunes background work; a missed report is harmless.
  });
  if (!focused) return;
  for (const tab of app.tabs) {
    const view = repoView(tab.repoId);
    if (tab.repoId === app.activeRepoId) void view.sync();
    else view.stale = true;
  }
}

/** Activates the tab `delta` places from the active one, wrapping around. */
export function cycleTab(delta: 1 | -1): void {
  const count = app.tabs.length;
  if (count === 0) return;
  const at = app.tabs.findIndex((t) => t.repoId === app.activeRepoId);
  activate(app.tabs[(at + delta + count) % count].repoId);
}

/** Activates the tab at `index` (0-based; -1 is the last), if there is one. */
export function activateIndex(index: number): void {
  const tab = app.tabs.at(index);
  if (tab) activate(tab.repoId);
}

/** Tabs being closed: the core has been asked and has not answered yet. */
const closing = new Set<number>();

/**
 * Closes a tab; closing the active one activates its right-hand neighbour, else its left.
 * The core is asked first: while an operation (or a fetch the user asked for) runs in the
 * repository it refuses, and the tab stays, with a notice saying why.
 */
export async function closeTab(repoId: number): Promise<void> {
  if (closing.has(repoId) || !app.tabs.some((t) => t.repoId === repoId)) return;
  closing.add(repoId);
  try {
    await api.closeRepo(repoId);
  } catch (e) {
    if (e instanceof CoreError && e.code === "busy") {
      toast("info", e.message);
      return;
    }
    // Already gone from the core is what closing wanted; anything else is said, and the
    // tab goes all the same.
    if (!(e instanceof CoreError && e.code === "unknown_repo")) reportError(e);
  } finally {
    closing.delete(repoId);
  }
  const at = app.tabs.findIndex((t) => t.repoId === repoId);
  if (at < 0) return;
  app.tabs.splice(at, 1);
  if (app.activeRepoId === repoId) {
    app.activeRepoId = (app.tabs[at] ?? app.tabs[at - 1])?.repoId ?? null;
  }
  saveSession();
  disposeRepoView(repoId);
}

/**
 * Closes the active tab (Cmd+W). Pressed again while it is still closing, it closes the tab
 * that will be active once that one has gone, as if the first close had already finished.
 */
export function closeActiveTab(): Promise<void> {
  let at = app.tabs.findIndex((t) => t.repoId === app.activeRepoId);
  if (at < 0) return Promise.resolve();
  // Step to the neighbour each closing tab will hand the activity to (right, else left).
  const gone = new Set<number>();
  while (closing.has(app.tabs[at].repoId)) {
    gone.add(app.tabs[at].repoId);
    const right = app.tabs.findIndex((t, i) => i > at && !gone.has(t.repoId));
    const left = app.tabs.findLastIndex((t, i) => i < at && !gone.has(t.repoId));
    const next = right >= 0 ? right : left;
    if (next < 0) return Promise.resolve();
    at = next;
  }
  return closeTab(app.tabs[at].repoId);
}

export async function removeRecent(path: string): Promise<void> {
  try {
    app.recents = await api.removeRecent(path);
  } catch (e) {
    reportError(e);
  }
}

let themeRequest = 0;

export async function setTheme(theme: ThemeSetting): Promise<void> {
  app.settings.theme = theme;
  applyTheme(theme);
  const request = ++themeRequest;
  try {
    const settings = await api.updateSettings({ theme });
    // An answer to an earlier choice must not undo a later one.
    if (request === themeRequest) app.settings = settings;
  } catch (e) {
    reportError(e, "Could not save the theme");
  }
}

/** Moves the theme setting on: system, light, dark. */
export function cycleTheme(): Promise<void> {
  return setTheme(nextTheme(app.settings.theme));
}

let started = false;
/** The session tab (or launch path) to show once it is open, while the session is reopened. */
let restoreTarget: string | null = null;

/**
 * Loads the stored state, applies the theme, reopens the session's tabs (each failure a
 * toast) and then the launch path. Call once.
 */
export async function init(): Promise<void> {
  if (!started) {
    started = true;
    onSystemThemeChange(() => applyTheme(app.settings.theme));
    onEvent<{ repoId: number; error: CoreErrorDto }>("repo-error", ({ repoId, error }) => onRepoError(repoId, error));
    onEvent("paths-opened", () => void openSystemPaths());
  }

  let state;
  try {
    state = await api.getAppState();
  } catch (e) {
    reportError(e);
    app.ready = true;
    return;
  }
  // Is git there? Any repository command says, and for no repository (id 0) it is cheap:
  // `git_not_found` without git, else `unknown_repo`. So the no-git panel shows at once
  // rather than after the first attempt to open something.
  api.getSnapshot(0).catch((e: unknown) => {
    if (e instanceof CoreError && e.code === "git_not_found") app.gitMissing = true;
  });
  if (state.settingsProblem) reportSettingsProblem(state.settingsProblem);
  app.recents = state.recents;
  app.settings = state.settings;
  app.homeDir = state.homeDir;
  applyTheme(state.settings.theme);

  // Reopened together; each tab shows as soon as its repository is open, in the session's
  // order. The tab to show (the launch path, else the saved active one) is shown when it is
  // open; until then nothing else is put in front, unless the user picks a tab.
  const { tabs, active } = state.session;
  const launch = state.launchPath;
  restoreTarget = launch ?? (active !== null && tabs.includes(active) ? active : null);
  app.restoring = restoreTarget !== null;
  app.ready = true;
  const opened = (path: string, snapshot: RepoSnapshot, rank?: number) => {
    addTab(snapshot, rank);
    if (restoreTarget === path) {
      show(snapshot.repoId);
      app.restoring = false;
    }
  };
  tabs.forEach((path, rank) => stillReopening.set(rank, path));
  const reopen = tabs.map(async (path, rank) => {
    try {
      const snapshot = await api.openRepo(path);
      stillReopening.delete(rank);
      opened(path, snapshot, rank);
    } catch (e) {
      stillReopening.delete(rank);
      reportError(e, `Could not reopen ${path}`);
    }
  });
  // A launch path that is also a session tab keeps its place there.
  const launchRank = launch === null ? -1 : tabs.indexOf(launch);
  const openLaunch = async (path: string) => {
    try {
      opened(path, await api.openRepo(path), launchRank >= 0 ? launchRank : undefined);
    } catch (e) {
      reportError(e);
    }
  };
  await Promise.all(launch ? [...reopen, openLaunch(launch)] : reopen);
  // The tab to show could not be opened (or there was none): the first one.
  if (app.activeRepoId === null && app.tabs.length > 0) show(app.tabs[0].repoId);
  restoreTarget = null;
  app.restoring = false;
  if (tabs.length > 0 || launch) {
    saveSession();
    await refreshRecents();
  }
  // Folders the system asked to open meanwhile (an Open With that launched the app).
  await openSystemPaths();
}

/**
 * Opens the folders the operating system asked the app to open (Finder's Open With, a
 * folder dropped on the Dock icon), in order; the last one is shown.
 */
async function openSystemPaths(): Promise<void> {
  let paths: string[];
  try {
    paths = await api.takeOpenedPaths();
  } catch {
    return; // asked again with the next announcement
  }
  for (const path of paths) await openPath(path);
}
