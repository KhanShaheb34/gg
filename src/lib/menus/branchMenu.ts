// The menu of a branch label, local or remote (spec §5.2, "Branch (local)" and "Branch
// (remote)"; "create worktree" and pull-request items come with Plan 3).

import type { CommitRow, RefLabel, RepoSnapshot } from "../types";
import {
  commits,
  firstReason,
  inProgressReason,
  item,
  noCurrentBranchReason,
  noRemoteReason,
  remoteOf,
  SEPARATOR,
  shown,
} from "./common";
import type { MenuItem } from "./types";

/** The menu of `label` (kind "branch" or "remote") on `row`. */
export function branchMenu(label: RefLabel, _row: CommitRow, snap: RepoSnapshot): MenuItem[] {
  return label.kind === "remote" ? remoteMenu(label, snap) : localMenu(label, snap);
}

/** "main", or a phrase for the current branch when there is none to name. */
function currentName(snap: RepoSnapshot): string {
  return snap.head.branch === null ? "the current branch" : shown(snap.head.branch);
}

function localMenu(label: RefLabel, snap: RepoSnapshot): MenuItem[] {
  const busy = inProgressReason(snap);
  const name = shown(label.name);
  const current = currentName(snap);
  const elsewhere =
    label.worktreePath !== null && !label.current
      ? `Checked out in the worktree at ${shown(label.worktreePath)}`
      : undefined;
  const noCurrent = noCurrentBranchReason(snap);
  const tracked = label.upstream !== null;
  return [
    item("checkout", "Check out", {
      disabledReason: firstReason(label.current ? "Already checked out" : undefined, elsewhere, busy),
    }),
    SEPARATOR,
    item("rename", "Rename…"),
    item("delete", "Delete…", {
      danger: true,
      disabledReason: firstReason(label.current ? "The current branch cannot be deleted" : undefined, elsewhere),
    }),
    SEPARATOR,
    item("merge", `Merge ${name} into ${current}…`, {
      disabledReason: firstReason(label.current ? "A branch cannot be merged into itself" : undefined, noCurrent, busy),
    }),
    item("rebase", `Rebase ${current} onto ${name}…`, {
      disabledReason: firstReason(label.current ? "A branch cannot be rebased onto itself" : undefined, noCurrent, busy),
    }),
    SEPARATOR,
    item("push", tracked && label.ahead > 0 ? `Push ${commits(label.ahead)}…` : "Push…", {
      disabledReason: noRemoteReason(snap),
    }),
    item("pull", tracked && label.behind > 0 ? `Pull ${commits(label.behind)}` : "Pull", {
      disabledReason: firstReason(
        tracked ? undefined : `${name} has no upstream branch to pull from`,
        elsewhere && `${elsewhere}: pull it there`,
        // Pulling another branch only fetches into it; pulling the current one is refused.
        label.current ? busy : undefined,
      ),
    }),
    SEPARATOR,
    item("copy.name", "Copy name"),
  ];
}

function remoteMenu(label: RefLabel, snap: RepoSnapshot): MenuItem[] {
  const busy = inProgressReason(snap);
  const remote = remoteOf(label.name, snap.remotes);
  return [
    item("checkout", "Check out as a local branch…", { disabledReason: busy }),
    item("fetch_into_local", "Fetch into a local branch…"),
    SEPARATOR,
    item("merge", `Merge ${shown(label.name)} into ${currentName(snap)}…`, {
      disabledReason: firstReason(noCurrentBranchReason(snap), busy),
    }),
    SEPARATOR,
    item("delete", remote === null ? "Delete from the remote…" : `Delete from ${shown(remote)}…`, { danger: true }),
    SEPARATOR,
    item("copy.name", "Copy name"),
  ];
}
