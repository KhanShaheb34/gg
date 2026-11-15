// The side panel and its worktree list: whether the panel shows (per repository, kept by the
// core in RepoPrefs.sidePanel), its width (one for the app, in localStorage), which worktree is
// selected in it, the switch underway, and what its rows and menus do. In a window narrower
// than DOCK_MIN_W the panel lies over the graph instead of taking room from it, and shows only
// when asked for in this session.

import CreateWorktreeDialog, { type CreateFrom } from "../components/dialogs/CreateWorktreeDialog.svelte";
import DeleteWorktreeDialog from "../components/dialogs/DeleteWorktreeDialog.svelte";
import PushDialog from "../components/dialogs/PushDialog.svelte";
import { api } from "../ipc";
import { shown } from "../menus/common";
import { worktreeMenu, worktreePanelMenu } from "../menus/worktreeMenu";
import type { RepoSnapshot, WorktreeInfo } from "../types";
import { app, reportError, toast } from "./app.svelte";
import { runOp } from "./ops.svelte";
import { openMenu, showDialog } from "./overlays.svelte";
import { PanelState } from "./panelState.svelte";
import { findRepoView, type RepoView } from "./repo.svelte";

/** Least, greatest and first width of the panel, px. */
export const SIDE_MIN_W = 240;
export const SIDE_MAX_W = 480;
export const SIDE_DEFAULT_W = 300;
/** Narrower windows lay the panel over the graph rather than squeezing it. */
export const DOCK_MIN_W = 1000;

const STORAGE_KEY = "gg.side.v1";

function readWidth(): number {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    const width = raw && typeof raw === "object" ? (raw as Record<string, unknown>).width : undefined;
    if (typeof width === "number" && Number.isFinite(width)) return clampWidth(width);
  } catch {
    // The default, then.
  }
  return SIDE_DEFAULT_W;
}

/** `width` kept between the panel's least and greatest widths. */
export function clampWidth(width: number): number {
  return Math.round(Math.min(SIDE_MAX_W, Math.max(SIDE_MIN_W, width)));
}

/** The panel as the whole app has it. */
export const sidePanel = $state({
  /** Width the user chose, px. */
  width: SIDE_DEFAULT_W,
  /** The window's width, px (set by the app shell). */
  windowW: typeof window === "undefined" ? 1280 : window.innerWidth,
  /** Shown over the graph in a narrow window: asked for in this session. */
  overlayOpen: false,
});
sidePanel.width = typeof localStorage === "undefined" ? SIDE_DEFAULT_W : readWidth();

/** Saves the width the user chose. */
export function saveWidth(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ width: sidePanel.width }));
  } catch {
    // Kept until the window closes.
  }
}

/** The panel sits beside the graph (a wide window), rather than over it. */
export function docked(): boolean {
  return sidePanel.windowW >= DOCK_MIN_W;
}

/** A panel state for a repository with no view (never shown). */
const NO_PANEL = new PanelState(null);

/** Repository `repoId`'s panel state (made with its view). */
export function panelState(repoId: number): PanelState {
  return findRepoView(repoId)?.panel ?? NO_PANEL;
}

/** Whether repository `repoId`'s side panel shows now. */
export function panelShown(repoId: number): boolean {
  return docked() ? panelState(repoId).shownPref : sidePanel.overlayOpen;
}

/**
 * Shows or hides repository `repoId`'s side panel (toggles without `show`). Docked, the choice
 * is remembered for the repository; over the graph, for this session only. `focus`: once
 * shown, the focus goes into its list.
 */
export function setPanelShown(repoId: number, show?: boolean, focus = false): void {
  const state = panelState(repoId);
  const next = show ?? !panelShown(repoId);
  if (docked()) {
    state.chosen = true;
    if (state.shownPref !== next) {
      state.shownPref = next;
      api.setRepoPrefs(repoId, { sidePanel: next }).catch((e: unknown) => reportError(e, "Could not remember the side panel"));
    }
  } else {
    sidePanel.overlayOpen = next;
  }
  if (next && focus) state.focusRequest++;
}

/**
 * Makes worktree `path` of repository `repoId` the active one. Its row shows the switch at
 * once; the core's refusal (an operation runs) is said.
 */
export async function makeActive(repoId: number, path: string, view: RepoView | undefined): Promise<void> {
  const state = panelState(repoId);
  if (state.switching !== null) return;
  state.switching = path;
  try {
    const snapshot = await api.setActiveWorktree(repoId, path);
    await view?.applySnapshot(snapshot);
  } catch (e) {
    reportError(e, "Could not make that worktree active");
  } finally {
    state.switching = null;
  }
}

/**
 * Selects worktree `wt` in the list and shows its HEAD in the graph: scrolled to the middle
 * and selected. A HEAD the graph does not show (filtered out) is said.
 */
export async function revealWorktree(view: RepoView, wt: WorktreeInfo): Promise<void> {
  panelState(view.repoId).selected = wt.path;
  if (wt.head === null) return;
  let index: number | null;
  try {
    index = await api.findRow(view.repoId, wt.head);
  } catch {
    return;
  }
  if (panelState(view.repoId).selected !== wt.path) return; // another one chosen meanwhile
  if (index === null) {
    toast("info", `The commit ${wt.branch === null ? "it is on" : `of ${shown(wt.branch)}`} is not in the graph as filtered.`);
    return;
  }
  await view.selectRow(index, true, { hash: wt.head });
}

/** Opens the Create worktree dialog for repository `snap`, starting `from` a branch, a commit or HEAD. */
export function openCreateWorktree(snap: RepoSnapshot, from: CreateFrom): void {
  showDialog(CreateWorktreeDialog, {
    repoId: snap.repoId,
    from,
    branches: snap.branches,
    worktrees: snap.worktrees,
    homeDir: app.homeDir,
  });
}

/** What the panel's add button starts from: a new branch at HEAD. */
function fromHead(snap: RepoSnapshot): CreateFrom | null {
  if (snap.head.hash === null) return null;
  return { kind: "head", hash: snap.head.hash, branch: snap.head.branch };
}

/** The panel's own menu (add, prune) at `at`. */
export function openPanelMenu(view: RepoView, at: { x: number; y: number }, keyboard: boolean): void {
  const snap = view.snapshot;
  openMenu(at, worktreePanelMenu(snap), (id) => {
    if (id === "add") addWorktree(view.snapshot);
    else if (id === "prune") runOp(view.repoId, { op: "worktree_prune" }).catch(() => {}); // reported by runOp
  }, { keyboard, repoId: view.repoId });
}

/** The add button: the Create worktree dialog at HEAD. */
export function addWorktree(snap: RepoSnapshot): void {
  const from = fromHead(snap);
  if (from === null) toast("info", "The repository has no commits yet: there is nothing to check out in a worktree.");
  else openCreateWorktree(snap, from);
}

/**
 * Opens the menu of worktree `wt` at `at`. Its items act on the snapshot it was built from;
 * it closes when the worktree is no longer listed.
 */
export function openWorktreeMenu(view: RepoView, wt: WorktreeInfo, at: { x: number; y: number }, keyboard: boolean): void {
  const snap = view.snapshot;
  openMenu(at, worktreeMenu(wt, snap), (id) => pickWorktree(id, view, wt, snap), {
    keyboard,
    repoId: view.repoId,
    alive: async () => view.snapshot.worktrees.some((w) => w.path === wt.path),
  });
}

function pickWorktree(id: string, view: RepoView, wt: WorktreeInfo, snap: RepoSnapshot): void {
  switch (id) {
    case "make_active":
      void makeActive(view.repoId, wt.path, view);
      return;
    case "push":
      if (wt.branch !== null) {
        showDialog(PushDialog, { repoId: view.repoId, branch: wt.branch, upstream: wt.upstream?.name ?? null, remotes: snap.remotes });
      }
      return;
    case "copy.path":
      navigator.clipboard.writeText(wt.path).then(
        () => toast("info", "Copied the path."),
        () => toast("error", "Could not copy the path."),
      );
      return;
    case "delete":
      showDialog(DeleteWorktreeDialog, { repoId: view.repoId, worktree: wt, homeDir: app.homeDir });
      return;
  }
}
