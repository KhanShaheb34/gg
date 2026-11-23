// The branch picker's list (⌘P): what it shows and in what order, without git or the core.
// Everything comes from the snapshot the window already has: the branches, each one's commit
// (`branchTips`), HEAD and the worktrees. What filtering needs is worked out once per set of
// branches (`pickerIndex`), so a keystroke only compares precomputed lowercase names.
//
// Matching is the branch filter's (branchFilter.ts `branchItems`): the trimmed text, ignoring
// case, anywhere in the branch's short name ("log" finds feature/login and origin/backlog).
// No fuzzy matching: what shows is exactly the names containing what was typed. Within each
// group the newest commit comes first; a branch named exactly as typed comes before the rest
// of its group.

import { remoteOf } from "./menus/common";
import type { BranchTip, RepoSnapshot } from "./types";

/** A branch as the picker lists it. */
export type PickerBranch = {
  name: string;
  remote: boolean;
  hash: string;
  /** Committer time of its commit, unix seconds (0: unknown). */
  time: number;
  author: string;
  subject: string;
  /** `name` lowercased: what the text is looked for in. */
  key: string;
  /** A local branch checked out in the active worktree. */
  current: boolean;
  /** A local branch checked out in another worktree: that worktree's path. */
  worktree: string | null;
};

/** The repository's branches, each group newest first. Made once per set of branches. */
export type PickerIndex = {
  local: PickerBranch[];
  remote: PickerBranch[];
  /** The local branches by name. */
  byName: Map<string, PickerBranch>;
  /** HEAD's commit and the branch whose commit it is, for the "from" step's HEAD row. */
  head: { hash: string; branch: PickerBranch | null } | null;
};

/** What the picker is at: listing branches, choosing where a new branch starts, or naming it. */
export type PickerStep =
  | { kind: "list" }
  /** `name`: the new branch's name, when it was typed before (and could be used). */
  | { kind: "from"; name: string | null }
  /** The new branch's name, for a branch at `at`. */
  | { kind: "name"; at: StartPoint };

/** Where a new branch starts: HEAD, or a branch's commit. */
export type StartPoint = { kind: "head"; hash: string } | { kind: "branch"; branch: PickerBranch };

/** A row of the list. */
export type PickerRow =
  | { kind: "branch"; branch: PickerBranch; group: "local" | "remote"; first: boolean }
  | { kind: "head"; hash: string; branch: PickerBranch | null }
  /** "Create new branch ‘name’" (at `at`); with no name yet, "Create new branch…". */
  | { kind: "create"; name: string; at: StartPoint | null }
  | { kind: "createFrom"; name: string };

/** The rows of a step, kept as the matching groups: a row is made only when it is shown. */
export type PickerList = {
  head: PickerRow | null;
  local: PickerBranch[];
  remote: PickerBranch[];
  actions: PickerRow[];
  length: number;
};

/** A row's height, px: every row has two lines. */
export const ROW_H = 42;

function byNewest(a: PickerBranch, b: PickerBranch): number {
  if (a.time !== b.time) return b.time - a.time;
  return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
}

function branchesOf(names: string[], tips: BranchTip[] | undefined, remote: boolean, snap: RepoSnapshot): PickerBranch[] {
  const elsewhere = new Map<string, string>();
  if (!remote) {
    for (const wt of snap.worktrees) if (wt.branch !== null && !wt.active) elsewhere.set(wt.branch, wt.path);
  }
  const out: PickerBranch[] = new Array(names.length);
  for (let i = 0; i < names.length; i++) {
    const name = names[i];
    const tip = tips?.[i];
    const current = !remote && name === snap.head.branch;
    out[i] = {
      name,
      remote,
      hash: tip?.hash ?? "",
      time: tip?.time ?? 0,
      author: tip?.author ?? "",
      subject: tip?.subject ?? "",
      key: name.toLowerCase(),
      current,
      worktree: remote || current ? null : (elsewhere.get(name) ?? null),
    };
  }
  return out.sort(byNewest);
}

/** The picker's index of `snap`'s branches (see `pickerIndex`, which reuses one). */
export function buildIndex(snap: RepoSnapshot): PickerIndex {
  const local = branchesOf(snap.branches, snap.branchTips, false, snap);
  const remote = branchesOf(snap.remoteBranches, snap.remoteBranchTips, true, snap);
  const byName = new Map(local.map((b) => [b.name, b]));
  const hash = snap.head.hash;
  let head: PickerIndex["head"] = null;
  if (hash !== null) {
    const branch = (snap.head.branch !== null ? byName.get(snap.head.branch) : undefined) ?? local.find((b) => b.hash === hash) ?? null;
    head = { hash, branch: branch && branch.hash === hash ? branch : null };
  }
  return { local, remote, byName, head };
}

/** Whether `a` and `b` give the same index: the same branches at the same commits, HEAD and worktrees. */
function sameBranches(a: RepoSnapshot, b: RepoSnapshot): boolean {
  if (a === b) return true;
  if (a.head.branch !== b.head.branch || a.head.hash !== b.head.hash) return false;
  const sameList = (x: string[], y: string[], tx: BranchTip[], ty: BranchTip[]) => {
    if (x === y && tx === ty) return true;
    if (x.length !== y.length || tx?.length !== ty?.length) return false;
    for (let i = 0; i < x.length; i++) {
      if (x[i] !== y[i]) return false;
      const p = tx?.[i];
      const q = ty?.[i];
      if (p?.hash !== q?.hash || p?.time !== q?.time || p?.subject !== q?.subject || p?.author !== q?.author) return false;
    }
    return true;
  };
  if (a.worktrees.length !== b.worktrees.length) return false;
  for (let i = 0; i < a.worktrees.length; i++) {
    const p = a.worktrees[i];
    const q = b.worktrees[i];
    if (p.path !== q.path || p.branch !== q.branch || p.active !== q.active) return false;
  }
  return sameList(a.branches, b.branches, a.branchTips, b.branchTips) && sameList(a.remoteBranches, b.remoteBranches, a.remoteBranchTips, b.remoteBranchTips);
}

let last: { snap: RepoSnapshot; index: PickerIndex } | null = null;

/**
 * The index for `snap`: the one made for the last snapshot asked about when its branches,
 * their commits, HEAD and the worktrees are the same (a snapshot for a changed file reuses
 * it), else a new one.
 */
export function pickerIndex(snap: RepoSnapshot): PickerIndex {
  if (last !== null && last.snap.repoId === snap.repoId && sameBranches(last.snap, snap)) {
    last.snap = snap;
    return last.index;
  }
  const index = buildIndex(snap);
  last = { snap, index };
  return index;
}

/** `text` as the branches are matched against it: trimmed, lowercased. */
export function needleOf(text: string): string {
  return text.trim().toLowerCase();
}

/**
 * The branches of `list` whose name contains `needle` (from `needleOf`), in `list`'s order,
 * except that one named exactly `exact` comes first.
 */
export function matching(list: PickerBranch[], needle: string, exact: string): PickerBranch[] {
  if (needle === "") return list;
  const out: PickerBranch[] = [];
  let exactAt = -1;
  for (const b of list) {
    if (!b.key.includes(needle)) continue;
    if (b.name === exact) exactAt = out.length;
    out.push(b);
  }
  if (exactAt > 0) out.unshift(out.splice(exactAt, 1)[0]);
  return out;
}

/** The rows of `step` for the text in the field. */
export function pickerList(index: PickerIndex, step: PickerStep, text: string): PickerList {
  const typed = text.trim();
  if (step.kind === "name") {
    const actions: PickerRow[] = [{ kind: "create", name: typed, at: step.at }];
    return { head: null, local: [], remote: [], actions, length: 1 };
  }
  const needle = needleOf(text);
  const local = matching(index.local, needle, typed);
  const remote = matching(index.remote, needle, typed);
  if (step.kind === "from") {
    const head: PickerRow | null =
      index.head !== null && (needle === "" || "head".includes(needle) || index.head.hash.startsWith(needle))
        ? { kind: "head", hash: index.head.hash, branch: index.head.branch }
        : null;
    return { head, local, remote, actions: [], length: (head ? 1 : 0) + local.length + remote.length };
  }
  const actions: PickerRow[] = [];
  // A name that is a local branch's is that branch to check out, not one to create.
  if (!index.byName.has(typed)) actions.push({ kind: "create", name: typed, at: null });
  actions.push({ kind: "createFrom", name: typed });
  return { head: null, local, remote, actions, length: local.length + remote.length + actions.length };
}

/** Row `i` of `list`. */
export function rowAt(list: PickerList, i: number): PickerRow {
  let at = i;
  if (list.head) {
    if (at === 0) return list.head;
    at--;
  }
  if (at < list.local.length) return { kind: "branch", branch: list.local[at], group: "local", first: at === 0 };
  at -= list.local.length;
  if (at < list.remote.length) return { kind: "branch", branch: list.remote[at], group: "remote", first: at === 0 };
  return list.actions[at - list.remote.length];
}

/** The local branch remote branch `remote` would be checked out as: its name on the remote. */
export function localNameOf(remote: PickerBranch, remotes: string[]): string {
  const of = remoteOf(remote.name, remotes);
  return of === null ? remote.name : remote.name.slice(of.length + 1);
}

/** A key that names row `row` across lists (for keyed rendering). */
export function rowKey(row: PickerRow): string {
  switch (row.kind) {
    case "branch":
      return `${row.group === "local" ? "l" : "r"}:${row.branch.name}`;
    case "head":
      return "head";
    case "create":
      return "create";
    case "createFrom":
      return "from";
  }
}

/** The rows `[first, last)` to render for a list of `length` rows scrolled to `top` in `height` px. */
export function visibleRange(length: number, top: number, height: number, overscan = 4): [number, number] {
  const first = Math.max(0, Math.floor(top / ROW_H) - overscan);
  const last = Math.min(length, Math.ceil((top + height) / ROW_H) + overscan);
  return [first, Math.max(first, last)];
}
