// The app shell's state: open tabs, recents, settings and toasts, and the actions on them.
// Every change to the tabs (or the active one) is saved as the session.

import { api, CoreError, onEvent } from "../ipc";
import { applyTheme, nextTheme, onSystemThemeChange, type ThemeSetting } from "../theme";
import type { CoreErrorDto, Recent, Settings } from "../types";

export type Tab = { repoId: number; path: string; name: string };
export type ToastKind = "info" | "error";
export type Toast = { id: number; kind: ToastKind; message: string; detail?: string };

export const app = $state({
  tabs: [] as Tab[],
  activeRepoId: null as number | null,
  recents: [] as Recent[],
  settings: { theme: "system", fetchIntervalSecs: 300 } as Settings,
  toasts: [] as Toast[],
  /** The stored state has been loaded (or failed to load); until then nothing is drawn. */
  ready: false,
  /** The core could not find git: nothing can be opened. */
  gitMissing: false,
});

let nextToastId = 1;

/** Shows a toast; the Toast component dismisses it after a while. */
export function toast(kind: ToastKind, message: string, detail?: string): void {
  app.toasts.push(detail ? { id: nextToastId++, kind, message, detail } : { id: nextToastId++, kind, message });
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
  if (!app.tabs.some((t) => t.repoId === snapshot.repoId)) {
    app.tabs.push({ repoId: snapshot.repoId, path: snapshot.rootPath, name: snapshot.name });
  }
  app.activeRepoId = snapshot.repoId;
  return true;
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

export function activate(repoId: number): void {
  if (app.activeRepoId === repoId || !app.tabs.some((t) => t.repoId === repoId)) return;
  app.activeRepoId = repoId;
  saveSession();
}

/** Activates the tab `delta` places from the active one, wrapping around. */
export function cycleTab(delta: 1 | -1): void {
  const count = app.tabs.length;
  if (count === 0) return;
  const at = app.tabs.findIndex((t) => t.repoId === app.activeRepoId);
  activate(app.tabs[(at + delta + count) % count].repoId);
}

/** Activates the tab at `index` (0-based), if there is one. */
export function activateIndex(index: number): void {
  const tab = app.tabs[index];
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

export async function setTheme(theme: ThemeSetting): Promise<void> {
  app.settings.theme = theme;
  applyTheme(theme);
  try {
    app.settings = await api.updateSettings({ theme });
  } catch (e) {
    reportError(e, "Could not save the theme");
  }
}

/** Moves the theme setting on: system, light, dark. */
export function cycleTheme(): Promise<void> {
  return setTheme(nextTheme(app.settings.theme));
}

let started = false;

/**
 * Loads the stored state, applies the theme, reopens the session's tabs (each failure a
 * toast) and then the launch path. Call once.
 */
export async function init(): Promise<void> {
  if (!started) {
    started = true;
    onSystemThemeChange(() => applyTheme(app.settings.theme));
    onEvent<{ repoId: number; error: CoreErrorDto }>("repo-error", ({ repoId, error }) => {
      const name = app.tabs.find((t) => t.repoId === repoId)?.name;
      reportError(new CoreError(error), name);
    });
  }

  let state;
  try {
    state = await api.getAppState();
  } catch (e) {
    reportError(e);
    app.ready = true;
    return;
  }
  app.recents = state.recents;
  app.settings = state.settings;
  applyTheme(state.settings.theme);

  // Reopened together, added in the session's order.
  const { tabs, active } = state.session;
  const reopened = await Promise.allSettled(tabs.map((path) => api.openRepo(path)));
  reopened.forEach((result, i) => {
    if (result.status === "rejected") {
      reportError(result.reason, `Could not reopen ${tabs[i]}`);
      return;
    }
    const snapshot = result.value;
    if (!app.tabs.some((t) => t.repoId === snapshot.repoId)) {
      app.tabs.push({ repoId: snapshot.repoId, path: snapshot.rootPath, name: snapshot.name });
    }
  });
  app.activeRepoId = app.tabs.find((t) => t.path === active)?.repoId ?? app.tabs[0]?.repoId ?? null;
  app.ready = true;

  if (state.launchPath) await openTab(state.launchPath);
  if (tabs.length > 0 || state.launchPath) {
    saveSession();
    await refreshRecents();
  }
}
