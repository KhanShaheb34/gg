// The Commit tab's "Committed on branch": the files the branch checked out in the worktree it
// shows changes in its own commits (branch_files), by repository and worktree, kept while the
// app runs. Asked for only when what the list depends on moves (branchFilesKey: the worktree's
// HEAD and branch, what its base is chosen from, the base's commit), never with every
// snapshot; the core reads git again only for a new HEAD, base or base commit, and shares a
// read that runs. An answer is taken only while it is the latest asked for that worktree.
//
// Whether the section is open (BranchFold) follows the worktree: closed while it has
// uncommitted changes, open while it has none; the user's click holds until that changes.

import { SvelteMap } from "svelte/reactivity";
import { api } from "../ipc";
import type { BranchFiles, RepoSnapshot } from "../types";

export type BranchFilesEntry = {
  /** The list as last answered (null: none, or not answered yet). */
  data: BranchFiles | null;
  /** A request is out. */
  loading: boolean;
  /** Why the last request failed, in one line (what was shown stays). */
  error: string | null;
  /** The number of the latest request. */
  asked: number;
  /** What the list was last asked for (branchFilesKey), or null: never. */
  key: string | null;
};

const id = (root: string, path: string) => `${root}\n${path}`;
const entries = new SvelteMap<string, BranchFilesEntry>();

/** What is known of the list of worktree `path` of the repository at `root`, if anything. */
export function branchFilesOf(root: string, path: string): BranchFilesEntry | null {
  return entries.get(id(root, path)) ?? null;
}

function entry(root: string, path: string): BranchFilesEntry {
  const found = entries.get(id(root, path));
  if (found) return found;
  const made = $state<BranchFilesEntry>({ data: null, loading: false, error: null, asked: 0, key: null });
  entries.set(id(root, path), made);
  return made;
}

/** The commit branch `name` (a remote-tracking or a local one) points at in `snap`, or "". */
function tipOf(snap: RepoSnapshot, name: string): string {
  const r = snap.remoteBranches.indexOf(name);
  if (r >= 0) return snap.remoteBranchTips[r]?.hash ?? "";
  const l = snap.branches.indexOf(name);
  return l >= 0 ? (snap.branchTips[l]?.hash ?? "") : "";
}

/**
 * What the list of worktree `path` depends on, as `snap` shows it: its HEAD and branch, what
 * its base is chosen from (the branch's pull request's base, its upstream, the remotes and
 * their default branches) and the commit of `base`, the base last answered. "" for a worktree
 * `snap` does not list. Two equal keys: nothing to ask again.
 */
export function branchFilesKey(snap: RepoSnapshot, path: string, base: string | null): string {
  const wt = snap.worktrees.find((w) => w.path === path);
  if (!wt) return "";
  const branch = wt.branch;
  const prBase = branch !== null && Object.hasOwn(snap.branchPrs, branch) ? snap.branchPrs[branch].base : null;
  return JSON.stringify([
    path,
    wt.head,
    branch,
    wt.upstream?.name ?? null,
    prBase,
    snap.remotes,
    snap.remoteHeads,
    base,
    base === null ? null : tipOf(snap, base),
  ]);
}

/** The message of error `e`, in one line. */
function messageOf(e: unknown): string {
  const text = e instanceof Error ? e.message : typeof e === "object" && e !== null && "message" in e ? String((e as { message: unknown }).message) : String(e);
  return text.split("\n")[0] || "Could not read the branch's files.";
}

/**
 * Asks the core for the list of worktree `path` of repository `repoId` (at `root`), for `key`.
 * `keyOf` gives the key for a base: once answered, the entry is taken as asked for the key of
 * the base the answer names (as the snapshot of the request showed it), so the answer itself
 * asks for nothing more. What was shown stays while it loads, and when it fails.
 */
export async function loadBranchFiles(repoId: number, root: string, path: string, key: string, keyOf: (base: string | null) => string): Promise<void> {
  const e = entry(root, path);
  const n = ++e.asked;
  e.key = key;
  e.loading = true;
  try {
    const d = await api.branchFiles(repoId, path);
    if (n !== e.asked) return;
    e.data = d;
    e.error = null;
    e.key = keyOf(d?.base ?? null);
  } catch (err) {
    if (n !== e.asked) return;
    e.error = messageOf(err);
  } finally {
    if (n === e.asked) e.loading = false;
  }
}

/** Forgets the lists of the repository at `root` (tests; a repository closed). */
export function forgetBranchFiles(root: string): void {
  for (const k of [...entries.keys()]) if (k.startsWith(`${root}\n`)) entries.delete(k);
}

/**
 * Whether "Committed on branch" is open. By default closed while the worktree shown has
 * uncommitted changes and open while it has none. The user's click holds only while that
 * stays so for the same worktree: when the worktree goes from no changes to some, or back,
 * or another worktree shows, the default applies again (`see`). Kept while the app runs,
 * never saved.
 */
export class BranchFold {
  #choice = $state<{ path: string; dirty: boolean; open: boolean } | null>(null);

  /** Whether it is open for worktree `path`, with (`dirty`) or without uncommitted changes. */
  isOpen(path: string, dirty: boolean): boolean {
    const c = this.#choice;
    return c !== null && c.path === path && c.dirty === dirty ? c.open : !dirty;
  }

  /** The user's click on the header. */
  toggle(path: string, dirty: boolean): void {
    this.#choice = { path, dirty, open: !this.isOpen(path, dirty) };
  }

  /** The worktree shown and whether it has changes, now: a change of either drops the choice. */
  see(path: string, dirty: boolean): void {
    const c = this.#choice;
    if (c !== null && (c.path !== path || c.dirty !== dirty)) this.#choice = null;
  }
}
