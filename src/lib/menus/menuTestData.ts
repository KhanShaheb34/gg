// Rows, labels and snapshots for the menu builders' tests, and ways to look into menus.

import type { CommitRow, InProgress, RefLabel, RepoSnapshot } from "../types";
import type { MenuItem } from "./types";

export const HASH = "a".repeat(40);
export const HEAD_HASH = "b".repeat(40);

export function row(overrides: Partial<CommitRow> = {}): CommitRow {
  return {
    index: 3,
    kind: "commit",
    hash: HASH,
    parents: ["c".repeat(40)],
    author: "Ada",
    email: "ada@example.com",
    time: 0,
    subject: "Add parser",
    lane: 0,
    color: 0,
    edges: [],
    labels: [],
    isHead: false,
    isMerge: false,
    detachedWorktrees: [],
    ...overrides,
  };
}

export function label(overrides: Partial<RefLabel> = {}): RefLabel {
  return {
    kind: "branch",
    name: "feature/x",
    remotes: [],
    current: false,
    worktreePath: null,
    upstream: null,
    ahead: 0,
    behind: 0,
    ...overrides,
  };
}

export function snap(overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return {
    repoId: 1,
    name: "r",
    rootPath: "/r",
    activeWorktree: "/r",
    generation: 1,
    totalRows: 10,
    loading: false,
    maxLane: 0,
    head: { branch: "main", hash: HEAD_HASH },
    branches: ["main", "feature/x"],
    remoteBranches: ["origin/main"],
    remotes: ["origin"],
    worktrees: [],
    status: { changed: 0, untracked: 0, conflicted: 0, conflictedPaths: [] },
    uncommittedSubject: "Uncommitted changes (0)",
    viewOptions: { branches: null, showRemotes: true, showTags: true, showStashes: true },
    fetch: { running: false, lastError: null, lastErrorDetail: null, lastAtMs: null },
    op: null,
    inProgress: null,
    seq: 1,
    ...overrides,
  };
}

export const DETACHED = { head: { branch: null, hash: HEAD_HASH } } satisfies Partial<RepoSnapshot>;
export const UNBORN = { head: { branch: "main", hash: null }, totalRows: 0 } satisfies Partial<RepoSnapshot>;

export function inProgress(kind: InProgress["kind"]): Partial<RepoSnapshot> {
  return { inProgress: { kind, files: [{ path: "a.txt", state: "markers" }], fileCount: 1, step: null, total: null, subject: null, incoming: null, rebasing: null, commit: null, empty: false } };
}

export const RUNNING = {
  op: { id: 4, title: "Push main to origin", startedAtMs: 0, queued: 1 },
} satisfies Partial<RepoSnapshot>;

type Item = Extract<MenuItem, { id: string }>;

/** The item `id` (looking into submenus too); throws if there is none. */
export function find(items: MenuItem[], id: string): Item {
  for (const it of items) {
    if ("separator" in it) continue;
    if (it.id === id) return it;
    const inner = it.submenu ? findOrNull(it.submenu, id) : null;
    if (inner) return inner;
  }
  throw new Error(`no item ${id} in ${JSON.stringify(ids(items))}`);
}

function findOrNull(items: MenuItem[], id: string): Item | null {
  try {
    return find(items, id);
  } catch {
    return null;
  }
}

/** The items' ids in order, "-" for a separator. */
export function ids(items: MenuItem[]): string[] {
  return items.map((it) => ("separator" in it ? "-" : it.id));
}

/** Ids of the items (submenus included) that are disabled. */
export function disabled(items: MenuItem[]): string[] {
  const out: string[] = [];
  for (const it of items) {
    if ("separator" in it) continue;
    if (it.disabledReason) out.push(it.id);
    if (it.submenu) out.push(...disabled(it.submenu));
  }
  return out;
}
