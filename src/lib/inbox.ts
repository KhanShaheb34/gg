// The Pull requests page's list, as plain functions: its rows (pinned ones first, then the
// groups the user chose: by role, by repository or by status; pull requests not updated for a
// month apart, folded), at fixed heights so hundreds scroll as a window of rows; which rows a
// scroll position shows; moving through them with the keyboard; how a row's checks and review
// read; what the page says when it has no list, and when its list is out of date.

import { clockTime, formatClock, relativeTime, type DateFormat } from "./format";
import type { InboxChecks, InboxGroup, InboxGroupBy, InboxHost, InboxPr, InboxProblem, PrInbox, ReviewDecision } from "./types";

/** A section's heading row, a pull request's row, and a "Show N more" row, in pixels. */
export const HEAD_H = 34;
export const ROW_H = 56;
export const MORE_H = 36;

export const GROUP_TITLES: Record<InboxGroup, string> = {
  mine: "Your pull requests",
  review: "Waiting for your review",
  involved: "Mentioned or assigned",
};

/** A pull request not updated for longer than this is old: apart, folded (by its `updatedAt`, never when it was opened). */
export const OLD_AFTER_MS = 30 * 24 * 3_600_000;

/** Whether `pr` was last updated more than a month before `now` (one never said to be updated is not old). */
export function isOld(pr: InboxPr, now: number): boolean {
  return pr.updatedAtMs !== null && now - pr.updatedAtMs > OLD_AFTER_MS;
}

/**
 * A pull request's overall state, as its leading icon shows it: its glyph (open, draft, merged,
 * closed), its tone (danger: something needs attention, checks failing, a conflict or changes
 * requested; warning: checks running; success: checks passing; neutral: open and nothing known;
 * muted: a draft; merged; closed) and what the tone says, in words (its tooltip and name: the
 * colour is never all there is to it).
 */
export type PrLook = {
  glyph: "open" | "draft" | "merged" | "closed";
  tone: "danger" | "warning" | "success" | "neutral" | "muted" | "merged" | "closed";
  label: string;
};

/** What `pr`'s leading icon shows (see PrLook). */
export function lookOf(pr: InboxPr): PrLook {
  if (pr.state === "merged") return { glyph: "merged", tone: "merged", label: "Merged" };
  if (pr.state === "closed") return { glyph: "closed", tone: "closed", label: "Closed" };
  const red = [
    pr.checks === "failing" ? "checks failing" : null,
    pr.conflict ? "merge conflict" : null,
    pr.review === "changes_requested" ? "changes requested" : null,
  ].filter((r): r is string => r !== null);
  const say = (words: string[]) => words.map((w, i) => (i === 0 ? w.charAt(0).toUpperCase() + w.slice(1) : w)).join(" · ");
  // A draft stays calm, whatever it says.
  if (pr.draft) return { glyph: "draft", tone: "muted", label: say(["draft", ...red]) };
  if (!pr.details) return { glyph: "open", tone: "neutral", label: "Open" };
  if (red.length > 0) return { glyph: "open", tone: "danger", label: say(red) };
  if (pr.checks === "running") return { glyph: "open", tone: "warning", label: "Checks running" };
  if (pr.checks === "passing") return { glyph: "open", tone: "success", label: "Checks passing" };
  return { glyph: "open", tone: "neutral", label: "Open · no checks" };
}

/** Where a pull request goes when grouped by status: as its icon's tone says, what needs attention first. */
export type Status = "attention" | "running" | "passing" | "quiet" | "draft" | "unknown";
export const STATUS_ORDER: Status[] = ["attention", "running", "passing", "quiet", "draft", "unknown"];
export const STATUS_TITLES: Record<Status, string> = {
  attention: "Needs attention",
  running: "Checks running",
  passing: "Checks passing",
  quiet: "No checks",
  draft: "Drafts",
  unknown: "Not read yet",
};

/** Where `pr` goes when grouped by status (the same rules as its icon: lookOf). */
export function statusOf(pr: InboxPr): Status {
  if (pr.draft) return "draft";
  if (!pr.details) return "unknown";
  switch (lookOf(pr).tone) {
    case "danger":
      return "attention";
    case "warning":
      return "running";
    case "success":
      return "passing";
    default:
      return "quiet";
  }
}

/** What is unfolded: the old ones (by role or status), each repository's older ones, the repositories with only old ones. */
export type Unfolded = { older: boolean; repos: ReadonlySet<string>; moreRepos: boolean };
export const FOLDED: Unfolded = { older: false, repos: new Set(), moreRepos: false };

/** A toggle row's action. */
export type Toggle = { kind: "older" } | { kind: "repo"; repo: string } | { kind: "repos" };

export type InboxRow =
  | { kind: "head"; id: string; title: string; count: number; top: number; toggle: null }
  | { kind: "head"; id: string; title: string; count: number; top: number; toggle: Toggle; open: boolean; index: number }
  | { kind: "pr"; pr: InboxPr; index: number; top: number }
  | { kind: "more"; id: string; label: string; toggle: Toggle; top: number; index: number };

/** The rows that take the keyboard's selection: pull requests and the rows that fold and unfold. */
export type Item = Extract<InboxRow, { index: number }>;

/** A repository's key: its host, owner and name (the same owner/name on two hosts are two). */
export const repoKeyOf = (pr: InboxPr) => `${pr.host}/${pr.owner}/${pr.name}`;
/** How a repository's heading names it: owner/name, and its host when not github.com. */
const repoTitleOf = (pr: InboxPr) => (pr.host === "github.com" ? `${pr.owner}/${pr.name}` : `${pr.owner}/${pr.name} (${pr.host})`);
const newestFirst = (a: InboxPr, b: InboxPr) => (b.updatedAtMs ?? 0) - (a.updatedAtMs ?? 0);
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * The list's rows for `prs` (as the core gives them: each in its role's group, newest first),
 * grouped `by`, at `now`, with `unfolded` open: the pinned ones first (in `pins`' order, whatever
 * their age, not again below); then by role or status, each group of the ones updated in the
 * last month, and one "Older than a month" section at the end, folded by default; by
 * repository, each repository with one updated in the last month, its older ones behind "Show N
 * older", and the repositories with only older ones behind "Show N more repositories" at the
 * end. Each row with where it starts, and the list's height; `items`: the rows the keyboard
 * moves through, in order.
 */
export function layoutOf(
  prs: InboxPr[],
  by: InboxGroupBy,
  now: number,
  unfolded: Unfolded = FOLDED,
  pins: string[] = [],
): { rows: InboxRow[]; items: Item[]; height: number } {
  const rows: InboxRow[] = [];
  const items: Item[] = [];
  let top = 0;
  const head = (id: string, title: string, count: number) => {
    rows.push({ kind: "head", id, title, count, top, toggle: null });
    top += HEAD_H;
  };
  const toggleHead = (id: string, title: string, count: number, toggle: Toggle, open: boolean) => {
    const row: InboxRow = { kind: "head", id, title, count, top, toggle, open, index: items.length };
    rows.push(row);
    items.push(row);
    top += HEAD_H;
  };
  const pr = (p: InboxPr) => {
    const row: InboxRow = { kind: "pr", pr: p, index: items.length, top };
    rows.push(row);
    items.push(row);
    top += ROW_H;
  };
  const more = (id: string, label: string, toggle: Toggle) => {
    const row: InboxRow = { kind: "more", id, label, toggle, top, index: items.length };
    rows.push(row);
    items.push(row);
    top += MORE_H;
  };

  const pinnedKeys = new Set(pins);
  const pinned = pins.map((k) => prs.find((p) => p.key === k)).filter((p): p is InboxPr => p !== undefined);
  if (pinned.length > 0) {
    head("pinned", "Pinned", pinned.length);
    pinned.forEach(pr);
  }
  const rest = prs.filter((p) => !pinnedKeys.has(p.key) && p.listed);
  const recent = rest.filter((p) => !isOld(p, now));
  const old = rest.filter((p) => isOld(p, now)).sort(newestFirst);

  if (by === "repository") {
    const repos = new Map<string, InboxPr[]>();
    for (const p of [...rest].sort(newestFirst)) {
      const list = repos.get(repoKeyOf(p)) ?? [];
      list.push(p);
      repos.set(repoKeyOf(p), list);
    }
    const quiet: [string, InboxPr[]][] = [];
    for (const [repo, list] of repos) {
      const fresh = list.filter((p) => !isOld(p, now));
      const stale = list.filter((p) => isOld(p, now));
      if (fresh.length === 0) {
        quiet.push([repo, list]);
        continue;
      }
      head(`repo:${repo}`, repoTitleOf(list[0]), list.length);
      fresh.forEach(pr);
      if (stale.length > 0) {
        // One id, folded or not: the keyboard's selection stays on it.
        if (unfolded.repos.has(repo)) {
          stale.forEach(pr);
          more(`older:${repo}`, "Hide the older ones", { kind: "repo", repo });
        } else {
          more(`older:${repo}`, `Show ${plural(stale.length, "older one", "older ones")}`, { kind: "repo", repo });
        }
      }
    }
    if (quiet.length > 0) {
      if (unfolded.moreRepos) {
        for (const [repo, list] of quiet) {
          head(`repo:${repo}`, repoTitleOf(list[0]), list.length);
          list.forEach(pr);
        }
        more("repos", "Hide the repositories with only older ones", { kind: "repos" });
      } else {
        more("repos", `Show ${plural(quiet.length, "more repository", "more repositories")} (only older ones)`, { kind: "repos" });
      }
    }
  } else {
    const groups: [string, string, InboxPr[]][] =
      by === "role"
        ? (["mine", "review", "involved"] as InboxGroup[]).map((g) => [g, GROUP_TITLES[g], recent.filter((p) => p.group === g)])
        : STATUS_ORDER.map((st) => [st, STATUS_TITLES[st], recent.filter((p) => statusOf(p) === st).sort(newestFirst)]);
    for (const [id, title, list] of groups) {
      if (list.length === 0) continue;
      head(id, title, list.length);
      list.forEach(pr);
    }
    if (old.length > 0) {
      toggleHead("older", "Older than a month", old.length, { kind: "older" }, unfolded.older);
      if (unfolded.older) old.forEach(pr);
    }
  }
  return { rows, items, height: top };
}

/** The height of row `r`. */
export const heightOf = (r: InboxRow) => (r.kind === "head" ? HEAD_H : r.kind === "pr" ? ROW_H : MORE_H);

/** The rows `[start, end)` that show between `scrollTop` and `scrollTop + height`, `overscan` px more on each side. */
export function visibleRange(rows: InboxRow[], scrollTop: number, height: number, overscan = 300): [number, number] {
  const from = scrollTop - overscan;
  const to = scrollTop + height + overscan;
  let lo = 0;
  let hi = rows.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (rows[mid].top + heightOf(rows[mid]) <= from) lo = mid + 1;
    else hi = mid;
  }
  let end = lo;
  while (end < rows.length && rows[end].top < to) end++;
  return [lo, end];
}

/** The pull requests the rows show (what their details are read for). */
export function shownPrs(rows: InboxRow[]): InboxPr[] {
  return rows.flatMap((r) => (r.kind === "pr" ? [r.pr] : []));
}

/** The key the keyboard's selection is kept by: a pull request's key, or a toggle row's id. */
export function itemKey(item: Item): string {
  return item.kind === "pr" ? item.pr.key : `toggle:${item.id}`;
}

/**
 * Where the selection goes from `current` (null: none yet) for `key` among `count` pull
 * requests, a page being `page` rows: ↑ ↓ one, Page Up/Down a page, Home/End the ends. Null:
 * not a moving key, or nothing to move to.
 */
export function moveTo(current: number | null, key: string, count: number, page: number): number | null {
  if (count === 0) return null;
  const at = current ?? -1;
  const clamp = (n: number) => Math.max(0, Math.min(count - 1, n));
  switch (key) {
    case "ArrowDown":
      return current === null ? 0 : clamp(at + 1);
    case "ArrowUp":
      return current === null ? count - 1 : clamp(at - 1);
    case "PageDown":
      return clamp(at + Math.max(1, page));
    case "PageUp":
      return clamp((current ?? 0) - Math.max(1, page));
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

export type Tone = "success" | "warning" | "danger" | "neutral";

/** How a row's checks read: a word and a tone; null when it has none. */
export function checksOf(checks: InboxChecks): { text: string; tone: Tone } | null {
  switch (checks) {
    case "passing":
      return { text: "Checks passing", tone: "success" };
    case "failing":
      return { text: "Checks failing", tone: "danger" };
    case "running":
      return { text: "Checks running", tone: "warning" };
    default:
      return null;
  }
}

/** How a row's review decision reads; null when there is none. */
export function reviewOf(review: ReviewDecision): { text: string; short: string; tone: Tone } | null {
  switch (review) {
    case "approved":
      return { text: "Approved", short: "Approved", tone: "success" };
    case "changes_requested":
      return { text: "Changes requested", short: "Changes", tone: "danger" };
    case "review_required":
      return { text: "Review required", short: "Review", tone: "neutral" };
    default:
      return null;
  }
}

/** Why a pull request of the third group is there: "Mentioned", "Assigned", or both. */
export function involvedWhy(pr: InboxPr): string | null {
  if (pr.group !== "involved") return null;
  if (pr.mentioned && pr.assigned) return "Mentioned, assigned";
  return pr.assigned ? "Assigned" : "Mentioned";
}

/**
 * What the page shows: the list (`list`, maybe with notes about hosts that failed), the first
 * load (`loading`), nothing at all (`empty`), or why there is no list (`problem`: the first
 * host's that failed). `data` null: nothing kept and nothing loaded yet.
 */
export type PageState =
  | { kind: "loading" }
  | { kind: "list" }
  | { kind: "empty" }
  | { kind: "problem"; problem: InboxProblem; host: string; message: string; detail: string | null };

export function pageState(data: PrInbox | null, loading: boolean): PageState {
  if (data === null) return { kind: "loading" };
  if (data.prs.length > 0) return { kind: "list" };
  const failed = data.hosts.filter((h) => h.problem !== null);
  // Each host failed: why; one answered with nothing: empty.
  if (failed.length > 0 && failed.length === data.hosts.length) {
    const h = failed[0];
    return { kind: "problem", problem: h.problem!, host: h.host, message: h.message ?? "Could not load them.", detail: h.detail };
  }
  // Nothing loaded from any host yet: the first load.
  if (data.hosts.every((h) => h.loadedAtMs === null)) return loading || data.hosts.length === 0 ? { kind: "loading" } : { kind: "empty" };
  return { kind: "empty" };
}

/** Hosts that failed while others answered, or while their list from before shows: a quiet line each. */
export function hostNotes(data: PrInbox | null): InboxHost[] {
  if (data === null || pageState(data, false).kind === "problem") return [];
  return data.hosts.filter((h) => h.problem !== null && !(h.problem === "signed_out" && h.loadedAtMs === null && h.host !== "github.com"));
}

/** A list older than this is shown as of when it was loaded. */
export const STALE_AFTER_MS = 10 * 60_000;

/**
 * "as of 14:05" when the list may be out of date: a host's last load failed, or the oldest
 * list is older than 10 minutes. Null when it is fresh, or there is none.
 */
export function inboxStale(data: PrInbox | null, now: number, format: DateFormat = "standard"): string | null {
  if (data === null) return null;
  const loaded = data.hosts.map((h) => h.loadedAtMs).filter((t): t is number => t !== null);
  if (loaded.length === 0) return null;
  const oldest = Math.min(...loaded);
  const failed = data.hosts.some((h) => h.problem !== null && h.loadedAtMs !== null);
  if (!failed && now - oldest <= STALE_AFTER_MS) return null;
  if (format === "relative") return `as of ${relativeTime(oldest, now)}`;
  const at = format === "locale" ? formatClock(oldest, "locale") : clockTime(oldest).slice(0, 5);
  return now - oldest >= 24 * 3_600_000 ? `as of ${at}, ${relativeTime(oldest, now)}` : `as of ${at}`;
}

/** The pull requests with new activity (a dot on the page's button). */
export function activityCount(data: PrInbox | null): number {
  return data ? data.prs.filter((p) => p.activity !== "none").length : 0;
}

/** What details and drafts of `pr`'s repository are kept under (prDetails.svelte.ts). */
export function inboxKey(pr: Pick<InboxPr, "host" | "owner" | "name">): string {
  return `inbox:${pr.host}/${pr.owner}/${pr.name}`.toLowerCase();
}
