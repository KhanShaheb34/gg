// How a confirmation the core asks for looks: most dangers lose work and are red; a few only
// ask the user to agree to something that loses nothing of theirs (ignored files git deletes
// with a worktree, stale entries prune forgets), and are asked in the ordinary style.

import type { Danger, DangerKind } from "./types";

/** Dangers asked about in the ordinary style: nothing of the user's own work goes. */
const ORDINARY: ReadonlySet<DangerKind> = new Set<DangerKind>(["remove_worktree_ignored_files", "prune_worktrees"]);

/** Whether danger `kind` loses work (red), rather than only needing the user's go-ahead. */
export function isRed(kind: DangerKind): boolean {
  return !ORDINARY.has(kind);
}

/** Whether a confirmation of `dangers` is red: any one of them is. */
export function confirmationIsRed(dangers: Pick<Danger, "kind">[]): boolean {
  return dangers.some((d) => isRed(d.kind));
}
