// The Pull requests page's list, as plain functions: its rows (a heading per group, then its
// pull requests; fixed heights, so hundreds scroll as a window of rows), which rows a scroll
// position shows, moving through them with the keyboard, how a row's checks and review read,
// what the page says when it has no list, and when its list is out of date.

import { clockTime, formatClock, relativeTime, type DateFormat } from "./format";
import type { InboxChecks, InboxGroup, InboxHost, InboxPr, InboxProblem, PrInbox, ReviewDecision } from "./types";

/** A group's heading row, and a pull request's row, in pixels. */
export const HEAD_H = 34;
export const ROW_H = 56;

export const GROUP_TITLES: Record<InboxGroup, string> = {
  mine: "Your pull requests",
  review: "Waiting for your review",
  involved: "Mentioned or assigned",
};

export type InboxRow =
  | { kind: "head"; group: InboxGroup; count: number; top: number }
  | { kind: "pr"; pr: InboxPr; index: number; top: number };

/**
 * The list's rows for `prs` (grouped and in order, as the core gives them): each group that
 * has any, its heading then its pull requests, with where each starts; and the list's height.
 * `index`: the pull request's place among the pull requests alone (what the keyboard moves).
 */
export function rowsOf(prs: InboxPr[]): { rows: InboxRow[]; height: number } {
  const rows: InboxRow[] = [];
  let top = 0;
  let group: InboxGroup | null = null;
  prs.forEach((pr, index) => {
    if (pr.group !== group) {
      group = pr.group;
      const count = prs.filter((p) => p.group === pr.group).length;
      rows.push({ kind: "head", group: pr.group, count, top });
      top += HEAD_H;
    }
    rows.push({ kind: "pr", pr, index, top });
    top += ROW_H;
  });
  return { rows, height: top };
}

/** The rows `[start, end)` that show between `scrollTop` and `scrollTop + height`, `overscan` px more on each side. */
export function visibleRange(rows: InboxRow[], scrollTop: number, height: number, overscan = 300): [number, number] {
  const from = scrollTop - overscan;
  const to = scrollTop + height + overscan;
  // The first row that ends below `from` (rows are in order of `top`).
  let lo = 0;
  let hi = rows.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const r = rows[mid];
    const bottom = r.top + (r.kind === "head" ? HEAD_H : ROW_H);
    if (bottom <= from) lo = mid + 1;
    else hi = mid;
  }
  let end = lo;
  while (end < rows.length && rows[end].top < to) end++;
  return [lo, end];
}

/** The row of pull request `index` (its place among the pull requests), if any. */
export function rowOfIndex(rows: InboxRow[], index: number): (InboxRow & { kind: "pr" }) | null {
  for (const r of rows) if (r.kind === "pr" && r.index === index) return r;
  return null;
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
