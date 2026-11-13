// View options remembered per repository (by its path) in localStorage, and the pure rules
// about them that the repository view uses.

import type { ViewOptions } from "../types";

/** localStorage key prefix of the view options remembered per repository path. */
const OPTIONS_KEY = "gg.view.v1:";

export const DEFAULT_OPTIONS: ViewOptions = { branches: null, showRemotes: true, showTags: true };

export function sameOptions(a: ViewOptions | undefined, b: ViewOptions | undefined): boolean {
  if (!a || !b) return a === b;
  if (a.showRemotes !== b.showRemotes || a.showTags !== b.showTags) return false;
  if (a.branches === null || b.branches === null) return a.branches === b.branches;
  return a.branches.length === b.branches.length && a.branches.every((n, i) => n === b.branches![i]);
}

/** The view options remembered for the repository at `path`, if any (and well formed). */
export function readOptions(path: string): ViewOptions | null {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(OPTIONS_KEY + path) ?? "null");
    if (typeof raw !== "object" || raw === null) return null;
    const o = raw as Record<string, unknown>;
    const branches = o.branches;
    const validBranches =
      branches === null || (Array.isArray(branches) && branches.every((b) => typeof b === "string"));
    if (!validBranches || typeof o.showRemotes !== "boolean" || typeof o.showTags !== "boolean") return null;
    return { branches: branches as string[] | null, showRemotes: o.showRemotes, showTags: o.showTags };
  } catch {
    return null;
  }
}

/** Remembers `options` for the repository at `path` (the defaults by forgetting). */
export function writeOptions(path: string, options: ViewOptions): void {
  try {
    if (sameOptions(options, DEFAULT_OPTIONS)) localStorage.removeItem(OPTIONS_KEY + path);
    else localStorage.setItem(OPTIONS_KEY + path, JSON.stringify(options));
  } catch {
    // Not remembered; the choice still applies until the repository is closed.
  }
}

/**
 * The part of `options` the graph can show: with remote branches hidden, the selection
 * without them (and all branches when nothing else is selected).
 */
export function visible(options: ViewOptions, remoteBranches: string[] | undefined): ViewOptions {
  if (options.showRemotes || options.branches === null) return options;
  const remote = new Set(remoteBranches ?? []);
  const branches = options.branches.filter((name) => !remote.has(name));
  if (branches.length === options.branches.length) return options;
  return { ...options, branches: branches.length > 0 ? branches : null };
}
