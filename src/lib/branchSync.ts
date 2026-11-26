// The sync control beside the branch picker: how the current branch stands with its upstream
// (↓ behind, ↑ ahead), and what clicking each count does. The counts are the snapshot's, the
// same the branch's label in the graph shows, so a fetch that lands updates them. Each runs
// the request its branch menu's item sends: Pull, the current branch's pull (the core's
// notices, the merge-or-rebase choice for a diverged branch, conflicts and the command log
// as there); Push, to the branch's upstream when it has the branch's name (git's own default),
// else the Push dialog, which names where a push goes.

import { visibleText } from "./format";
import { commits, inProgressReason, remoteOf, shown } from "./menus/common";
import type { OpRequest, RepoSnapshot } from "./types";

export type SyncPull = {
  count: number;
  /** Its tooltip and accessible name: "Pull 9 commits from origin/main". */
  title: string;
  request: OpRequest;
  /** Why it cannot run now, if it cannot. */
  disabledReason: string | undefined;
};

export type SyncPush = {
  count: number;
  /** Its accessible name: "Push 2 commits to origin/main", "Push 2 commits…" (the dialog). */
  title: string;
  /** More for its tooltip, after the title: why a push would be refused, or what the dialog is for. */
  note: string | null;
  /** The push to its upstream; null: the Push dialog instead (an upstream of another name). */
  request: OpRequest | null;
  disabledReason: string | undefined;
};

export type BranchSync = {
  branch: string;
  /** "origin/main". */
  upstream: string;
  /** Present when the branch is behind its upstream. */
  pull: SyncPull | null;
  /** Present when it is ahead of an upstream on a remote. */
  push: SyncPush | null;
};

/**
 * How the active worktree's branch stands with its upstream, or null when there is nothing to
 * offer: HEAD detached or unborn, no upstream (or one that is gone), or level with it.
 */
export function branchSync(snap: RepoSnapshot): BranchSync | null {
  const branch = snap.head.branch;
  if (branch === null || snap.head.hash === null) return null;
  const wt = snap.worktrees.find((w) => w.path === snap.activeWorktree);
  if (!wt || wt.branch !== branch || wt.upstream === null) return null;
  const { name: upstream, ahead, behind } = wt.upstream;
  if (ahead === 0 && behind === 0) return null;

  // Every action waits its turn in the core; while one runs, the counts are about to change.
  const running = snap.op === null ? undefined : `Wait for ${visibleText(snap.op.title)} to end`;
  const pull: SyncPull | null =
    behind === 0
      ? null
      : {
          count: behind,
          title: `Pull ${commits(behind)} from ${shown(upstream)}`,
          request: { op: "pull", branch, mode: "default", expectHead: snap.head, expectWorktree: snap.activeWorktree },
          disabledReason: running ?? inProgressReason(snap),
        };

  // Pushing goes to a remote: an upstream that is a local branch has nothing to push to.
  const remote = remoteOf(upstream, snap.remotes);
  let push: SyncPush | null = null;
  if (ahead > 0 && remote !== null) {
    const sameNamed = upstream.slice(remote.length + 1) === branch;
    const refused =
      behind > 0
        ? `${shown(upstream)} has ${commits(behind)} that ${shown(branch)} does not: git refuses the push until you pull them, or force push.`
        : null;
    push = sameNamed
      ? {
          count: ahead,
          title: `Push ${commits(ahead)} to ${shown(upstream)}`,
          note: refused,
          request: { op: "push", branch, remote, to: null, force: false },
          disabledReason: running,
        }
      : {
          count: ahead,
          title: `Push ${commits(ahead)}…`,
          note: `Its upstream, ${shown(upstream)}, has another name: choose where the push goes.`,
          request: null,
          disabledReason: running,
        };
  }
  if (pull === null && push === null) return null;
  return { branch, upstream, pull, push };
}
