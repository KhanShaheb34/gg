// The menus of a stash row (spec §5.2, "Stash") and of the uncommitted-changes row, where a
// stash is saved.

import type { CommitRow, RepoSnapshot } from "../types";
import { inProgressReason, item, SEPARATOR } from "./common";
import type { MenuItem } from "./types";

export function stashMenu(_row: CommitRow, snap: RepoSnapshot): MenuItem[] {
  const busy = inProgressReason(snap);
  return [
    item("stash.apply", "Apply", { disabledReason: busy }),
    item("stash.pop", "Pop", { disabledReason: busy }),
    SEPARATOR,
    item("stash.branch", "Create branch from stash…", { disabledReason: busy }),
    SEPARATOR,
    item("stash.drop", "Drop…", { danger: true }),
  ];
}

export function uncommittedMenu(snap: RepoSnapshot): MenuItem[] {
  let disabledReason: string | undefined;
  if (snap.head.hash === null) disabledReason = "Stashing needs a first commit";
  else if (snap.status.conflicted > 0) disabledReason = "Conflicted files cannot be stashed: resolve them first";
  return [item("stash.save", "Stash changes…", { disabledReason })];
}
