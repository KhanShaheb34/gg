// The menu of a worktree in the side panel (spec §5.4), and the panel's own menu. Like the
// other builders, a pure function of the worktree and the snapshot the panel shows (and, for
// the file manager's name, the platform). The pull-request items come with the pull requests.

import { revealLabel } from "../platform";
import type { RepoSnapshot, WorktreeInfo } from "../types";
import { firstReason, item, noRemoteReason, SEPARATOR, shown } from "./common";
import type { MenuItem } from "./types";

/** Why worktree `wt` cannot be made active now, or undefined. */
export function makeActiveReason(wt: WorktreeInfo, snap: RepoSnapshot): string | undefined {
  return firstReason(
    wt.active ? "Already the active worktree" : undefined,
    wt.prunable ? "Its folder is missing" : undefined,
    // The core refuses a switch while an operation runs or waits there.
    snap.op !== null ? "An operation is running in this repository" : undefined,
  );
}

/** Why worktree `wt` cannot be deleted, or undefined (the core then says what would be lost). */
export function deleteReason(wt: WorktreeInfo): string | undefined {
  return firstReason(
    wt.isMain ? "The main worktree cannot be deleted" : undefined,
    wt.locked ? `It is locked${wt.lockReason ? ` (${wt.lockReason})` : ""}: unlock it first` : undefined,
    wt.prunable ? "Its folder is missing: prune stale entries instead" : undefined,
  );
}

/** Why worktree `wt`'s changes cannot be committed from its menu, or undefined. */
export function commitReason(wt: WorktreeInfo): string | undefined {
  const s = wt.status;
  return firstReason(
    wt.prunable ? "Its folder is missing" : undefined,
    s !== null && s.changed + s.untracked + s.conflicted === 0 ? "Nothing to commit: no file has changed" : undefined,
  );
}

/** The menu of worktree `wt`. */
export function worktreeMenu(wt: WorktreeInfo, snap: RepoSnapshot): MenuItem[] {
  const gone = wt.prunable ? "Its folder is missing" : undefined;
  const items: MenuItem[] = [
    item("make_active", "Make active", { disabledReason: makeActiveReason(wt, snap) }),
    item("commit", "Commit…", { disabledReason: commitReason(wt) }),
    SEPARATOR,
  ];
  items.push(
    item("push", wt.branch === null ? "Push…" : `Push ${shown(wt.branch)}…`, {
      disabledReason: firstReason(
        wt.branch === null ? "HEAD is detached there: there is no branch to push" : undefined,
        noRemoteReason(snap),
      ),
    }),
    SEPARATOR,
  );
  if (wt.locked) {
    // Shown, not picked: what the lock means.
    items.push(
      item("locked", wt.lockReason ? `Locked: ${shown(wt.lockReason)}` : "Locked", {
        disabledReason: "git keeps a locked worktree: unlock it in a terminal (git worktree unlock)",
      }),
    );
  }
  items.push(
    item("open.editor", "Open in editor", { disabledReason: gone }),
    item("open.terminal", "Open in terminal", { disabledReason: gone }),
    item("open.reveal", revealLabel(), { disabledReason: gone }),
    item("copy.path", "Copy path"),
    SEPARATOR,
    item("delete", "Delete…", { danger: true, disabledReason: deleteReason(wt) }),
  );
  return items;
}

/** The side panel's own menu: add a worktree, prune stale entries. */
export function worktreePanelMenu(snap: RepoSnapshot): MenuItem[] {
  return [
    item("add", "Add worktree…", { disabledReason: snap.head.hash === null ? "The repository has no commits yet" : undefined }),
    SEPARATOR,
    item("prune", "Prune stale entries…", {
      disabledReason: snap.worktrees.some((w) => w.prunable) ? undefined : "No worktree's folder is missing",
    }),
  ];
}
