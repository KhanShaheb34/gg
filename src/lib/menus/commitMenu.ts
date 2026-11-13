// The menu of a commit row (spec §5.2, "Commit"; "create worktree here" comes with Plan 3).

import type { CommitRow, RepoSnapshot } from "../types";
import { firstReason, inProgressReason, item, SEPARATOR, shown } from "./common";
import type { MenuItem } from "./types";

export function commitMenu(row: CommitRow, snap: RepoSnapshot): MenuItem[] {
  const busy = inProgressReason(snap);
  const detached = snap.head.branch === null;
  const isHead = row.hash === snap.head.hash;
  const current = detached ? "HEAD" : shown(snap.head.branch!);
  return [
    item("checkout", "Check out this commit", {
      disabledReason: firstReason(detached && isHead ? "HEAD is already detached at this commit" : undefined, busy),
    }),
    SEPARATOR,
    item("branch.create", "Create branch here…"),
    item("tag.create", "Create tag here…"),
    SEPARATOR,
    item("cherry_pick", "Cherry-pick…", {
      disabledReason: firstReason(isHead ? "This commit is already HEAD" : undefined, busy),
    }),
    item("revert", "Revert…", { disabledReason: busy }),
    item("reset", `Reset ${current} to here`, {
      submenu: [
        item("reset.soft", "Soft: keep changes, staged"),
        item("reset.mixed", "Mixed: keep changes, unstaged"),
        item("reset.hard", "Hard: discard all changes", { danger: true }),
      ],
    }),
    SEPARATOR,
    item("copy.hash", "Copy hash"),
    item("copy.message", "Copy message"),
  ];
}
