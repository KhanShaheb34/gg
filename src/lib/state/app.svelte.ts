// The app shell's state: open tabs, recents, settings and toasts, and the actions on them.
// Every change to the tabs (or the active one) is saved as the session.

import { api, CoreError, onEvent } from "../ipc";
import { applyTheme, nextTheme, onSystemThemeChange, type ThemeSetting } from "../theme";
import type { CoreErrorDto, Recent, RepoSnapshot, Settings } from "../types";
import { disposeRepoView, repoView } from "./repo.svelte";

export type Tab = { repoId: number; path: string; name: string };
export type ToastKind = "info" | "error";
/** A notice. A persistent one stays until dismissed; `action` is a button on it. */
export type Toast = {
  id: number;
  kind: ToastKind;
  message: string;
  detail?: string;
  persistent?: boolean;
  action?: { label: string; run: () => Promise<void> };
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
});

let nextToastId = 1;

/** Shows a toast; the Toast component dismisses it after a while. */
export function toast(kind: ToastKind, message: string, detail?: string): void {
  app.toasts.push(detail ? { id: nextToastId++, kind, message, detail } : { id: nextToastId++, kind, message });
}

/** Shows a notice that stays until it is dismissed (or its action succeeds). */
function notice(message: string, detail: string, action: NonNullable<Toast["action"]>): number {
  const id = nextToastId++;
  app.toasts.push({ id, kind: "error", message, detail, persistent: true, action });
  return id;
}

/**
 * The settings file could not be read: settings and the session apply for this run only.
 * Said once, with a way to move the file aside and save again.
 */
function reportSettingsProblem(problem: string): void {
  const id = notice("Settings are not being saved", problem, {
    label: "Reset settings",
    run: async () => {
      try {
        await api.resetSettings();
      } catch (e) {
        reportError(e, "Could not reset settings");
        return;
      }
      dismissToast(id);
      toast("info", "Settings reset: saving again");
    },
  });
}

export function dismissToast(id: number): void {
  app.toasts = app.toasts.filter((t) => t.id !== id);
}

/** Reports a failed core call: git missing switches the app to its no-git panel. */
function reportError(e: unknown, prefix?: string): void {
  if (e instanceof CoreError && e.code === "git_not_found") {
    app.gitMissing = true;
    return;
  }
  const message = e instanceof Error ? e.message : String(e);
  const detail = e instanceof CoreError ? (e.detail ?? undefined) : undefined;
  toast("error", prefix ? `${prefix}: ${message}` : message, detail);
}

function saveSession(): void {
  const active = app.tabs.find((t) => t.repoId === app.activeRepoId)?.path ?? null;
  api.saveSession(
    app.tabs.map((t) => t.path),
    active,
  ).catch((e: unknown) => reportError(e, "Could not save the open tabs"));
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

/**
 * Adds a tab (and its graph's view) for `snapshot`'s repository, unless it has one. A tab
 * reopened from the session (`rank`: its place there) goes before the tabs ranked after it,
 * whatever order the repositories finish opening in; others go last.
 */
function addTab(snapshot: RepoSnapshot, rank?: number): void {
  if (app.tabs.some((t) => t.repoId === snapshot.repoId)) return;
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

/** Closes a tab; closing the active one activates its right-hand neighbour, else its left. */
export async function closeTab(repoId: number): Promise<void> {
  const at = app.tabs.findIndex((t) => t.repoId === repoId);
  if (at < 0) return;
  app.tabs.splice(at, 1);
  if (app.activeRepoId === repoId) {
    app.activeRepoId = (app.tabs[at] ?? app.tabs[at - 1])?.repoId ?? null;
  }
  saveSession();
  disposeRepoView(repoId);
  try {
    await api.closeRepo(repoId);
  } catch (e) {
    // Already gone from the core is what closing wanted.
    if (!(e instanceof CoreError && e.code === "unknown_repo")) reportError(e);
  }
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
  const reopen = tabs.map(async (path, rank) => {
    try {
      opened(path, await api.openRepo(path), rank);
    } catch (e) {
      reportError(e, `Could not reopen ${path}`);
    }
  });
  const openLaunch = async (path: string) => {
    try {
      opened(path, await api.openRepo(path));
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
}
