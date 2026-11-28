// The Clone dialog's rules, as plain functions: git's progress line read into a phase and a
// percent; the user's repositories filtered as typed; a repository gg already has, found by
// its host, owner and name; where a clone goes; the + menu's rows.

import type { InboxProblem, KnownRepo, ListedRepo, Recent } from "./types";

/** git's progress, read from its latest line: the phase ("Receiving objects"), its percent when it says one, and the rest ("(45/100), 1.2 MiB | 2 MiB/s"). */
export type Progress = { phase: string; percent: number | null; rest: string };

/**
 * `line` (git's latest progress line, as `clone-progress` carries it) read: "remote: " taken
 * off, the phase up to the colon, the percent before "%". A line with no colon is all phase
 * ("Cloning into 'cat'..."). Null for an empty line.
 */
export function readProgress(line: string): Progress | null {
  const text = line
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^remote: /, "");
  if (text === "") return null;
  const colon = text.indexOf(": ");
  if (colon < 0) return { phase: text.replace(/\.\.\.$/, "…"), percent: null, rest: "" };
  const phase = text.slice(0, colon);
  const after = text.slice(colon + 2);
  const m = /^(\d{1,3})% ?(.*)$/.exec(after);
  if (!m) return { phase, percent: null, rest: after.replace(/, done\.$/, "").replace(/^done\.$/, "") };
  const percent = Math.min(100, Number(m[1]));
  return { phase, percent, rest: m[2].replace(/, done\.$/, "") };
}

/**
 * The repositories of `repos` that `query` matches: each of its words in the owner/name or the
 * description, any case. Those whose name starts with the query's first word come first; the
 * order is otherwise kept (most recently pushed first).
 */
export function filterRepos(repos: ListedRepo[], query: string): ListedRepo[] {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w !== "");
  if (words.length === 0) return repos;
  const matching = repos.filter((r) => {
    const text = `${r.owner}/${r.name} ${r.description ?? ""}`.toLowerCase();
    return words.every((w) => text.includes(w));
  });
  const first = words[0];
  const leads = (r: ListedRepo) => r.name.toLowerCase().startsWith(first) || `${r.owner}/${r.name}`.toLowerCase().startsWith(first);
  return [...matching.filter(leads), ...matching.filter((r) => !leads(r))];
}

/** The repository gg has for `host`/`owner`/`name` (any case), an open one before a recent one; null when none. */
export function knownFor(known: KnownRepo[], host: string, owner: string, name: string): KnownRepo | null {
  const same = (k: KnownRepo) =>
    k.host.toLowerCase() === host.toLowerCase() && k.owner.toLowerCase() === owner.toLowerCase() && k.name.toLowerCase() === name.toLowerCase();
  return known.find((k) => same(k) && k.repoId !== null) ?? known.find(same) ?? null;
}

/** `folder` inside `parent`, with the parent's own separator. */
export function joinPath(parent: string, folder: string): string {
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  const base = parent.replace(/[\\/]+$/, "");
  return `${base === "" ? "" : base}${sep}${folder}`;
}

/** `typed` with a leading `~` read as the home folder `home` (as the folder fields show it). */
export function expandHome(typed: string, home: string | null): string {
  const t = typed.trim();
  if (!home || !(t === "~" || t.startsWith("~/") || t.startsWith("~\\"))) return t;
  return home.replace(/[\\/]+$/, "") + t.slice(1);
}

/** Where clones go: the folder the settings name, else the platform's default; "" when neither is known. */
export function cloneLocation(setting: string | null, platformDefault: string | null): string {
  return setting ?? platformDefault ?? "";
}

/** What the list says it holds: all of them, or the most recently pushed of how many. */
export function listCaption(shown: number, total: number): string {
  if (total <= shown) return total === 1 ? "Your one repository" : `Your ${total} repositories, most recently pushed first`;
  return `Your ${shown} most recently pushed repositories, of ${total}. For another, paste its link.`;
}

/** What the list says when gh cannot list the repositories (the Pull requests page's words for gh's states). */
export const LIST_PROBLEM: Record<InboxProblem, string> = {
  no_gh: "The GitHub CLI (gh) was not found",
  signed_out: "gh is not signed in",
  rate_limited: "GitHub’s rate limit was reached",
  offline: "GitHub could not be reached",
  failed: "Your repositories could not be listed",
};

/** A row of the + menu: open a folder, a recent repository, or clone. */
export type NewTabRow =
  | { kind: "open" }
  | { kind: "clone"; link: string | null }
  | { kind: "mine" }
  | { kind: "recent"; recent: Recent; open: boolean };

/** Whether `text` looks like a link to clone (an address, or GitHub's owner/name) rather than a name to filter by. */
export function looksLikeLink(text: string): boolean {
  const t = text.trim();
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(t) || /^[\w.-]+@[\w.-]+:/.test(t) || /^[A-Za-z0-9-]+\/[\w.-]+$/.test(t);
}

/**
 * The + menu's rows for `query`: Open folder…, the recent repositories that match it (by name
 * or folder; those open in a tab say so), then Clone: from a link (the query itself, when it
 * looks like one) and from the user's repositories. With a query that matches nothing, the
 * clone rows stay.
 */
export function newTabRows(recents: Recent[], openPaths: ReadonlySet<string>, query: string): NewTabRow[] {
  const q = query.trim().toLowerCase();
  const link = looksLikeLink(query) ? query.trim() : null;
  const matching = recents.filter((r) => q === "" || r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q));
  const rows: NewTabRow[] = [];
  if (q === "" || "open folder".includes(q)) rows.push({ kind: "open" });
  for (const recent of matching) rows.push({ kind: "recent", recent, open: openPaths.has(recent.path) });
  rows.push({ kind: "clone", link });
  if (link === null) rows.push({ kind: "mine" });
  return rows;
}
