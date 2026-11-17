// The menu of a branch label, local or remote, with its pull request.

import type { CommitRow, RefLabel, RepoSnapshot } from "../types";
import {
  commits,
  firstReason,
  inProgressReason,
  item,
  noCurrentBranchReason,
  noRemoteReason,
  pullRequestItem,
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
  // Ahead counts against the upstream; a push goes to the branch of the same name, so
  // the count is only that push's when the upstream has that name.
  const upstreamRemote = label.upstream === null ? null : remoteOf(label.upstream, snap.remotes);
  const sameNamed = upstreamRemote !== null && label.upstream!.slice(upstreamRemote.length + 1) === label.name;
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
    // On the current branch both are disabled: said in general, not "Merge main into main…".
    item("merge", label.current ? "Merge into the current branch…" : `Merge ${name} into ${current}…`, {
      disabledReason: firstReason(label.current ? "A branch cannot be merged into itself" : undefined, noCurrent, busy),
    }),
    item("rebase", label.current ? "Rebase the current branch onto this…" : `Rebase ${current} onto ${name}…`, {
      disabledReason: firstReason(label.current ? "A branch cannot be rebased onto itself" : undefined, noCurrent, busy),
    }),
    SEPARATOR,
    item("push", sameNamed && label.ahead > 0 ? `Push ${commits(label.ahead)}…` : "Push…", {
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
    pullRequestItem(label.name, label.pr, snap),
    SEPARATOR,
    // A branch checked out somewhere already gets a new branch at its commit (the dialog says).
    item("worktree.create", `Create worktree from ${name}…`),
    SEPARATOR,
    item("copy.name", "Copy name"),
  ];
}

function remoteMenu(label: RefLabel, snap: RepoSnapshot): MenuItem[] {
  const busy = inProgressReason(snap);
  const remote = remoteOf(label.name, snap.remotes);
  // Only its open pull request: one is made from a local branch.
  const pr = label.pr ? [item("pr.open", `Open pull request #${label.pr.number}`), SEPARATOR] : [];
  return [
    item("checkout", "Check out as a local branch…", { disabledReason: busy }),
    item("fetch_into_local", "Fetch into a local branch…"),
    SEPARATOR,
    item("merge", `Merge ${shown(label.name)} into ${currentName(snap)}…`, {
      disabledReason: firstReason(noCurrentBranchReason(snap), busy),
    }),
    SEPARATOR,
    ...pr,
    item("delete", remote === null ? "Delete from the remote…" : `Delete from ${shown(remote)}…`, { danger: true }),
    SEPARATOR,
    item("copy.name", "Copy name"),
  ];
}
