// What the menu builders share: how git's names are shown in item labels, and the reasons
// items cannot apply.

import { visibleText } from "../format";
import type { RepoSnapshot } from "../types";
import type { MenuItem } from "./types";

/** Longest name shown whole in an item's label; longer ones lose their middle. */
const MAX_NAME = 44;

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/**
 * `name` (from git) for an item's label: invisible characters shown, a long one shortened
 * in the middle to MAX_NAME characters (code points). Cut only between graphemes, each shown
 * whole: never inside a character, a flag or an invisible character's marker (‹U+202E›).
 */
export function shown(name: string): string {
  const text = visibleText(name);
  if ([...text].length <= MAX_NAME) return text;
  const parts = [...graphemes.segment(name)].map((g) => visibleText(g.segment));
  const width = (part: string) => [...part].length;
  const budget = MAX_NAME - 1; // the ellipsis
  let head = "";
  let tail = "";
  let used = 0;
  for (let i = 0, j = parts.length - 1; i <= j; ) {
    // Alternate ends, the head first; stop at the first part that does not fit.
    const takeHead = width(head) <= width(tail);
    const part = takeHead ? parts[i] : parts[j];
    if (used + width(part) > budget) break;
    used += width(part);
    if (takeHead) {
      head += part;
      i++;
    } else {
      tail = part + tail;
      j--;
    }
  }
  return `${head}…${tail}`;
}

export const SEPARATOR: MenuItem = { separator: true };

/**
 * Why HEAD- or tree-changing items cannot run now, as the core refuses them: git is stopped
 * in a merge, rebase, cherry-pick or revert, until it is continued or aborted (R17); or files
 * still have conflicts nothing in progress explains (a stash pop's), until they are resolved
 * and staged (R20'). Undefined if neither.
 */
export function inProgressReason(snap: RepoSnapshot): string | undefined {
  if (snap.inProgress) return `A ${snap.inProgress.kind} is in progress`;
  if (snap.status.conflicted > 0) return "Files still have conflicts from an earlier step: resolve them first";
  return undefined;
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

/**
 * The remote a dialog chooses to begin with: `preferred` (a branch's upstream's) if it is
 * one, else origin, else the only one. Among several remotes none of which is origin,
 * nothing ("") is chosen: the user picks one before the dialog can go ahead (R29).
 */
export function defaultRemote(remotes: string[], preferred: string | null = null): string {
  if (preferred !== null && remotes.includes(preferred)) return preferred;
  if (remotes.includes("origin")) return "origin";
  return remotes.length === 1 ? remotes[0] : "";
}

/** Said under a remote list while nothing is chosen in it (R29). */
export const CHOOSE_REMOTE = "Choose the remote.";
