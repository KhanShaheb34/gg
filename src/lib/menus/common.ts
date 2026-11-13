// What the menu builders share: how git's names are shown in item labels, and the reasons
// items cannot apply.

import { visibleText } from "../format";
import type { RepoSnapshot } from "../types";
import type { MenuItem } from "./types";

/** Longest name shown whole in an item's label; longer ones lose their middle. */
const MAX_NAME = 44;

/** `name` (from git) for an item's label: invisible characters shown, a long one shortened in the middle. */
export function shown(name: string): string {
  const text = visibleText(name);
  if (text.length <= MAX_NAME) return text;
  const head = Math.ceil((MAX_NAME - 1) / 2);
  const tail = MAX_NAME - 1 - head;
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

export const SEPARATOR: MenuItem = { separator: true };

/**
 * Why HEAD-changing items cannot run now: git is stopped in a merge, rebase, cherry-pick or
 * revert, and the core refuses them until it is continued or aborted (R17). Undefined if not.
 */
export function inProgressReason(snap: RepoSnapshot): string | undefined {
  return snap.inProgress ? `A ${snap.inProgress.kind} is in progress` : undefined;
}

/** Why there is no current branch to merge or rebase into, or undefined when there is one. */
export function noCurrentBranchReason(snap: RepoSnapshot): string | undefined {
  if (snap.head.branch === null) return "HEAD is detached: there is no current branch";
  if (snap.head.hash === null) return "The current branch has no commits yet";
  return undefined;
}

/** Why nothing can be pushed: no remote. */
export function noRemoteReason(snap: RepoSnapshot): string | undefined {
  return snap.remotes.length === 0 ? "This repository has no remote to push to" : undefined;
}

/** The first reason given, if any: items list theirs most specific first. */
export function firstReason(...reasons: (string | undefined)[]): string | undefined {
  return reasons.find((r) => r !== undefined);
}

/** An item; `disabledReason` and `danger` are left out when not set, so items compare plainly. */
export function item(
  id: string,
  label: string,
  options: { disabledReason?: string; danger?: boolean; submenu?: MenuItem[] } = {},
): MenuItem {
  const out: MenuItem = { id, label };
  if (options.disabledReason !== undefined) out.disabledReason = options.disabledReason;
  if (options.danger) out.danger = true;
  if (options.submenu) out.submenu = options.submenu;
  return out;
}

/** "1 commit", "3 commits". */
export function commits(n: number): string {
  return `${n} commit${n === 1 ? "" : "s"}`;
}

/** The remote of remote branch `name` (`origin/x`): the longest remote name that prefixes it. */
export function remoteOf(name: string, remotes: string[]): string | null {
  let best: string | null = null;
  for (const r of remotes) {
    if (name.startsWith(`${r}/`) && name.length > r.length + 1 && (best === null || r.length > best.length)) best = r;
  }
  return best;
}
