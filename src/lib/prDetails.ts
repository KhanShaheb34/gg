// How the Pull request tab words and orders a pull request's details (pr_details): its
// merge status and review decision, its checks (the order, the summary, how long each took),
// which comments a filter shows, which are drawn shortened, the tab's dot, and when the tab
// asks again while checks run. Pure: the panel's components and their tests use these.

import type { CheckState, ChecksSummary, MergeStatus, PrCheck, PrComment, PrDetails, ReviewDecision } from "./types";

/** A tone the status line, the dot and the icons take their colour from. */
export type Tone = "success" | "warning" | "danger" | "neutral";

// --- Merge status -----------------------------------------------------------------------------

/** The status line's words for where `d` stands for merging into `base`, and its tone. */
export function mergeWording(d: Pick<PrDetails, "state" | "merge" | "checksSummary">, base: string): { text: string; tone: Tone } {
  if (d.state === "merged") return { text: "Merged", tone: "success" };
  if (d.state === "closed") return { text: "Closed", tone: "neutral" };
  const checks = checksTone(d.checksSummary);
  const byChecks = (): { text: string; tone: Tone } | null =>
    checks === "danger" ? { text: "Checks failing", tone: "danger" } : checks === "warning" ? { text: "Checks running", tone: "warning" } : null;
  const merge: MergeStatus = d.merge;
  switch (merge) {
    case "draft":
      return { text: "Draft", tone: "neutral" };
    case "conflicts":
      return { text: `Conflicts with ${base}`, tone: "danger" };
    case "behind":
      return { text: `Behind ${base}`, tone: "warning" };
    case "blocked":
      return { text: "Blocked", tone: "warning" };
    case "unstable":
      return byChecks() ?? { text: "Checks not passing", tone: "warning" };
    case "ready":
      return { text: "Ready to merge", tone: "success" };
    default:
      return byChecks() ?? { text: "Merge status unknown", tone: "neutral" };
  }
}

/** The review decision's words and tone; null when there is none. */
export function reviewWording(review: ReviewDecision): { text: string; tone: Tone } | null {
  switch (review) {
    case "approved":
      return { text: "Approved", tone: "success" };
    case "changes_requested":
      return { text: "Changes requested", tone: "danger" };
    case "review_required":
      return { text: "Review required", tone: "neutral" };
    default:
      return null;
  }
}

// --- Checks -----------------------------------------------------------------------------------

/** Where each state goes in the list: failing first, then running, then the rest. */
const RANK: Record<CheckState, number> = { failing: 0, pending: 1, cancelled: 2, unknown: 3, passing: 4, neutral: 5, skipped: 6 };

/** `checks` in the order shown: failing, then running, then the rest (each group as gh gave them). */
export function orderChecks(checks: readonly PrCheck[]): PrCheck[] {
  return checks
    .map((c, i) => ({ c, i }))
    .sort((a, b) => RANK[a.c.state] - RANK[b.c.state] || a.i - b.i)
    .map(({ c }) => c);
}

/**
 * The words for each state in the summary, in its order: what needs looking at first, so a
 * summary cut short at the panel's least width still says it.
 */
const SUMMARY: [CheckState, string][] = [
  ["failing", "failing"],
  ["pending", "running"],
  ["passing", "passing"],
  ["cancelled", "cancelled"],
  ["skipped", "skipped"],
  ["neutral", "neutral"],
  ["unknown", "unknown"],
];

/** "1 failing · 1 running · 2 passing"; "No checks" when there are none. */
export function checksSummaryText(s: ChecksSummary): string {
  if (s.total === 0) return "No checks";
  return SUMMARY.filter(([state]) => s[state] > 0)
    .map(([state, word]) => `${s[state]} ${word}`)
    .join(" · ");
}

/**
 * The checks' tone: danger when any fails, warning while any runs, success when every one
 * passed (or was skipped or neutral); null with none, or with only cancelled or unknown ones.
 */
export function checksTone(s: ChecksSummary): Tone | null {
  if (s.total === 0) return null;
  if (s.failing > 0) return "danger";
  if (s.pending > 0) return "warning";
  if (s.passing + s.skipped + s.neutral === s.total && s.passing > 0) return "success";
  return null;
}

/** What the tab's tooltip says of the checks ("checks failing"), or null. */
export function checksPhrase(s: ChecksSummary): string | null {
  switch (checksTone(s)) {
    case "danger":
      return "checks failing";
    case "warning":
      return "checks running";
    case "success":
      return "checks passing";
    default:
      return null;
  }
}

/** Whether the checks section starts open: something fails or runs (until the user chooses). */
export function checksOpenByDefault(s: ChecksSummary): boolean {
  return s.failing > 0 || s.pending > 0;
}

/** A duration, short: "45s", "11m 52s", "1h 5m". */
export function shortDuration(ms: number): string {
  const secs = Math.max(0, Math.round(ms / 1000));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return secs % 60 === 0 ? `${mins}m` : `${mins}m ${secs % 60}s`;
  const hours = Math.floor(mins / 60);
  return mins % 60 === 0 ? `${hours}h` : `${hours}h ${mins % 60}m`;
}

/** A running duration, coarse (it is redrawn a minute at a time): "under 1m", "2m", "1h 5m". */
function coarseDuration(ms: number): string {
  const mins = Math.floor(Math.max(0, ms) / 60_000);
  if (mins < 1) return "under 1m";
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  return mins % 60 === 0 ? `${hours}h` : `${hours}h ${mins % 60}m`;
}

/** How long check `c` took, or has run ("running 2m", "queued"); null when not known. */
export function checkTiming(c: PrCheck, now: number): string | null {
  if (c.state === "pending") return c.startedAtMs === null ? "queued" : `running ${coarseDuration(now - c.startedAtMs)}`;
  // A skipped check never ran.
  if (c.state === "skipped") return null;
  if (c.startedAtMs !== null && c.completedAtMs !== null && c.completedAtMs >= c.startedAtMs) return shortDuration(c.completedAtMs - c.startedAtMs);
  return null;
}

/** The workflow's name beside the check's, when it says more than the check's own name. */
export function workflowShown(c: PrCheck): string | null {
  const w = c.workflow?.trim();
  if (!w) return null;
  return c.name.toLowerCase().includes(w.toLowerCase()) ? null : w;
}

/** How a check's state is said (its icon's name, for assistive technology and the tooltip); `started`: a pending one runs, else it is queued. */
export function checkStateText(state: CheckState, started = true): string {
  if (state === "pending" && !started) return "Queued";
  return { pending: "Running", passing: "Passed", failing: "Failed", skipped: "Skipped", cancelled: "Cancelled", neutral: "Neutral", unknown: "Unknown" }[state];
}

// --- Comments ---------------------------------------------------------------------------------

export type CommentFilter = "all" | "humans" | "bots";

/**
 * The comments `filter` shows: all; those by people; those by bots. A review thread shows
 * when any of its comments (the first or a reply) is by whom the filter asks for.
 */
export function filterComments(comments: readonly PrComment[], filter: CommentFilter): PrComment[] {
  if (filter === "all") return [...comments];
  const want = filter === "bots";
  return comments.filter((c) => c.isBot === want || (c.thread?.replies ?? []).some((r) => r.isBot === want));
}

/** What the comments list says with none to show under `filter`. */
export function noComments(filter: CommentFilter): string {
  return { all: "No comments yet.", humans: "No comments from people.", bots: "No comments from bots." }[filter];
}

/** How many comments there are of each kind of author. */
export function commentCounts(comments: readonly PrComment[]): Record<CommentFilter, number> {
  return {
    all: comments.length,
    humans: filterComments(comments, "humans").length,
    bots: filterComments(comments, "bots").length,
  };
}

/**
 * How a comment is drawn at first: `resolved` (a resolved thread, or one hidden on GitHub:
 * its first lines only, replies hidden), else `long` (its body shortened when it is longer
 * than the room for ~12 lines). Either way the user's "Show more" wins.
 */
export function startsShortened(c: Pick<PrComment, "minimized" | "thread">): "resolved" | "long" {
  return c.thread?.resolved || c.minimized ? "resolved" : "long";
}

/** Lines of a body shown before "Show more": a resolved thread's few, any other's dozen. */
export function shownLines(shortened: "resolved" | "long"): number {
  return shortened === "resolved" ? 3 : 12;
}

// --- Asking again -----------------------------------------------------------------------------

/** How often the tab asks again while some check runs. */
export const POLL_MS = 15_000;

/** What decides whether the tab asks again while it shows a pull request. */
export type PollConditions = {
  /** The details shown have a check still running. */
  pending: boolean;
  /** The Pull request tab shows the pull request (the panel shown, on that tab). */
  tabShown: boolean;
  /** The window is focused. */
  focused: boolean;
  /** The page is visible (not minimised, not behind another space). */
  visible: boolean;
};

/** Whether the tab asks for the details again every POLL_MS: only all of these at once. */
export function shouldPoll(c: PollConditions): boolean {
  return c.pending && c.tabShown && c.focused && c.visible;
}
