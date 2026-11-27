import { describe, expect, it } from "vitest";
import {
  FOLDED,
  HEAD_H,
  MORE_H,
  OLD_AFTER_MS,
  ROW_H,
  activityCount,
  isOld,
  layoutOf,
  statusOf,
  checksOf,
  hostNotes,
  inboxKey,
  inboxStale,
  involvedWhy,
  moveTo,
  pageState,
  reviewOf,
  visibleRange,
  type InboxRow,
} from "./inbox";
import type { InboxHost, InboxPr, PrInbox } from "./types";

function pr(number: number, group: InboxPr["group"], o: Partial<InboxPr> = {}): InboxPr {
  return {
    key: `github.com/octo/cat#${number}`,
    group,
    host: "github.com",
    owner: "octo",
    name: "cat",
    number,
    title: `Change ${number}`,
    url: `https://github.com/octo/cat/pull/${number}`,
    draft: false,
    head: "fix/typo",
    base: "main",
    headRepo: "octo/cat",
    cross: false,
    author: "ada",
    updatedAtMs: 0,
    checks: "none",
    review: "none",
    mentioned: false,
    assigned: false,
    activity: "none",
    state: "open",
    details: true,
    pinned: false,
    listed: true,
    ...o,
  };
}

const NOW = Date.UTC(2026, 5, 1);
const DAY = 24 * 3_600_000;
/** The rows as words: a heading "title:count" (+ ▸/▾ when it folds), a pull request its number, a "more" row its label. */
const words = (rows: InboxRow[]) =>
  rows.map((r) => (r.kind === "head" ? `${r.title}:${r.count}${r.toggle ? (r.open ? "▾" : "▸") : ""}` : r.kind === "pr" ? r.pr.number : r.label));

const host = (o: Partial<InboxHost> = {}): InboxHost => ({
  host: "github.com",
  login: "octo",
  problem: null,
  message: null,
  detail: null,
  partial: false,
  loadedAtMs: 1_000,
  ...o,
});
const page = (prs: InboxPr[], hosts: InboxHost[] = [host()]): PrInbox => ({ prs, hosts, fresh: true, pins: [], groupBy: "role" });

describe("the Pull requests page's list", () => {
  it("by role: a heading per group that has pull requests, then its rows, at fixed heights", () => {
    const { rows, items, height } = layoutOf([pr(1, "mine", { updatedAtMs: NOW }), pr(2, "mine", { updatedAtMs: NOW }), pr(3, "involved", { updatedAtMs: NOW })], "role", NOW);
    expect(words(rows)).toEqual(["Your pull requests:2", 1, 2, "Mentioned or assigned:1", 3]);
    expect(rows.map((r) => r.top)).toEqual([0, HEAD_H, HEAD_H + ROW_H, HEAD_H + 2 * ROW_H, 2 * HEAD_H + 2 * ROW_H]);
    expect(height).toBe(2 * HEAD_H + 3 * ROW_H);
    expect(items.map((i) => i.index)).toEqual([0, 1, 2]);
    expect(layoutOf([], "role", NOW)).toEqual({ rows: [], items: [], height: 0 });
  });

  it("old is by the last update, more than a month ago: exactly a month is not, a moment past is", () => {
    expect(isOld(pr(1, "mine", { updatedAtMs: NOW - OLD_AFTER_MS }), NOW)).toBe(false);
    expect(isOld(pr(1, "mine", { updatedAtMs: NOW - OLD_AFTER_MS - 1 }), NOW)).toBe(true);
    expect(isOld(pr(1, "mine", { updatedAtMs: null }), NOW)).toBe(false);
    expect(isOld(pr(1, "mine", { updatedAtMs: NOW - 29 * DAY }), NOW)).toBe(false);
  });

  it("by role or status, the old ones leave their groups for one section at the end, folded by default", () => {
    const prs = [pr(1, "mine", { updatedAtMs: NOW - DAY }), pr(2, "mine", { updatedAtMs: NOW - 40 * DAY }), pr(3, "review", { updatedAtMs: NOW - 90 * DAY })];
    const folded = layoutOf(prs, "role", NOW);
    expect(words(folded.rows)).toEqual(["Your pull requests:1", 1, "Older than a month:2▸"]);
    // The folding heading is an item: the keyboard reaches it.
    expect(folded.items.map((i) => i.kind)).toEqual(["pr", "head"]);
    const open = layoutOf(prs, "role", NOW, { ...FOLDED, older: true });
    expect(words(open.rows)).toEqual(["Your pull requests:1", 1, "Older than a month:2▾", 2, 3]);
    expect(words(layoutOf(prs, "status", NOW).rows)).toEqual(["No checks or review:1", 1, "Older than a month:2▸"]);
  });

  it("by status: what needs attention first, drafts and the unread last", () => {
    const at = { updatedAtMs: NOW };
    const prs = [
      pr(1, "mine", { ...at, review: "approved" }),
      pr(2, "mine", { ...at, draft: true, checks: "failing" }),
      pr(3, "review", { ...at, checks: "failing", review: "approved" }),
      pr(4, "review", { ...at, review: "review_required" }),
      pr(5, "review", { ...at, review: "changes_requested", checks: "running" }),
      pr(6, "involved", { ...at, checks: "running" }),
      pr(7, "involved", { ...at, details: false }),
      pr(8, "involved", at),
    ];
    expect(prs.map(statusOf)).toEqual(["approved", "draft", "failing", "review", "changes", "running", "unknown", "quiet"]);
    expect(words(layoutOf(prs, "status", NOW).rows)).toEqual([
      "Checks failing:1", 3,
      "Changes requested:1", 5,
      "Review required:1", 4,
      "Checks running:1", 6,
      "Approved:1", 1,
      "Drafts:1", 2,
      "No checks or review:1", 8,
      "Not read yet:1", 7,
    ]);
  });

  it("by repository: newest first, older ones behind Show N older, repositories with only old ones behind one more row", () => {
    const prs = [
      pr(1, "mine", { owner: "octo", name: "cat", updatedAtMs: NOW - DAY }),
      pr(2, "review", { owner: "octo", name: "cat", updatedAtMs: NOW - 50 * DAY }),
      pr(3, "review", { owner: "hub", name: "dog", updatedAtMs: NOW - 2 * DAY }),
      pr(4, "involved", { owner: "hub", name: "owl", updatedAtMs: NOW - 60 * DAY }),
      pr(5, "mine", { owner: "hub", name: "elk", updatedAtMs: NOW - 70 * DAY }),
      pr(6, "mine", { owner: "hub", name: "elk", updatedAtMs: NOW - 80 * DAY }),
    ];
    const folded = layoutOf(prs, "repository", NOW);
    expect(words(folded.rows)).toEqual(["octo/cat:2", 1, "Show 1 older one", "hub/dog:1", 3, "Show 2 more repositories (only older ones)"]);
    expect(folded.rows.at(-1)!.top).toBe(2 * HEAD_H + 2 * ROW_H + MORE_H);
    const open = layoutOf(prs, "repository", NOW, { older: false, repos: new Set(["octo/cat"]), moreRepos: true });
    expect(words(open.rows)).toEqual([
      "octo/cat:2", 1, 2, "Hide the older ones",
      "hub/dog:1", 3,
      "hub/owl:1", 4,
      "hub/elk:2", 5, 6,
      "Hide the repositories with only older ones",
    ]);
    expect(open.height).toBe(4 * HEAD_H + 6 * ROW_H + 2 * MORE_H);
  });

  it("pinned ones come first in every grouping, whatever their age or list, and not again below", () => {
    const prs = [
      pr(1, "mine", { updatedAtMs: NOW }),
      pr(2, "review", { updatedAtMs: NOW - 400 * DAY, pinned: true }),
      pr(3, "review", { updatedAtMs: NOW, pinned: true, listed: false, state: "merged" }),
    ];
    const pins = ["github.com/octo/cat#3", "github.com/octo/cat#2"];
    for (const by of ["role", "repository", "status"] as const) {
      const w = words(layoutOf(prs, by, NOW, FOLDED, pins).rows);
      expect(w.slice(0, 3)).toEqual(["Pinned:2", 3, 2]);
      expect(w.filter((x) => x === 2 || x === 3)).toHaveLength(2);
      expect(w.some((x) => typeof x === "string" && x.startsWith("Older"))).toBe(false);
    }
  });

  it("draws only the rows in view, however many there are", () => {
    const many = Array.from({ length: 600 }, (_, i) => pr(i + 1, i < 300 ? "mine" : "review", { updatedAtMs: NOW - i }));
    const { rows } = layoutOf(many, "role", NOW);
    const [start, end] = visibleRange(rows, 10_000, 600, 200);
    expect(end - start).toBeLessThan(25);
    expect(rows[start].top + ROW_H).toBeGreaterThan(10_000 - 200);
    expect(rows[end - 1].top).toBeLessThan(10_000 + 600 + 200);
    expect(visibleRange(rows, 0, 600, 0)[0]).toBe(0);
    expect(visibleRange([], 0, 600)).toEqual([0, 0]);
  });

  it("moves with the arrows, a page, and to the ends, never past them", () => {
    expect(moveTo(null, "ArrowDown", 5, 8)).toBe(0);
    expect(moveTo(null, "ArrowUp", 5, 8)).toBe(4);
    expect(moveTo(2, "ArrowDown", 5, 8)).toBe(3);
    expect(moveTo(4, "ArrowDown", 5, 8)).toBe(4);
    expect(moveTo(0, "ArrowUp", 5, 8)).toBe(0);
    expect(moveTo(1, "PageDown", 50, 8)).toBe(9);
    expect(moveTo(3, "PageUp", 50, 8)).toBe(0);
    expect(moveTo(3, "Home", 5, 8)).toBe(0);
    expect(moveTo(0, "End", 5, 8)).toBe(4);
    expect(moveTo(0, "Enter", 5, 8)).toBeNull();
    expect(moveTo(null, "ArrowDown", 0, 8)).toBeNull();
  });

  it("says checks and reviews in words, and why a pull request is mentioned", () => {
    expect(checksOf("failing")).toEqual({ text: "Checks failing", tone: "danger" });
    expect(checksOf("none")).toBeNull();
    expect(reviewOf("changes_requested")?.text).toBe("Changes requested");
    expect(reviewOf("none")).toBeNull();
    expect(involvedWhy(pr(1, "involved", { mentioned: true, assigned: true }))).toBe("Mentioned, assigned");
    expect(involvedWhy(pr(1, "involved", { assigned: true }))).toBe("Assigned");
    expect(involvedWhy(pr(1, "involved", { mentioned: true }))).toBe("Mentioned");
    expect(involvedWhy(pr(1, "mine", { mentioned: true }))).toBeNull();
  });

  it("tells the first load, an empty page and each problem apart", () => {
    expect(pageState(null, true).kind).toBe("loading");
    expect(pageState(page([], []), true).kind).toBe("loading");
    expect(pageState(page([], [host({ loadedAtMs: null })]), true).kind).toBe("loading");
    expect(pageState(page([pr(1, "mine")]), false).kind).toBe("list");
    expect(pageState(page([]), false).kind).toBe("empty");
    const signedOut = page([], [host({ problem: "signed_out", message: "gh is not signed in to github.com.", loadedAtMs: null })]);
    expect(pageState(signedOut, false)).toMatchObject({ kind: "problem", problem: "signed_out", host: "github.com" });
    // One host answered (with nothing), another failed: empty, the failure a note.
    const mixed = page([], [host(), host({ host: "ghe.example", problem: "offline", message: "x" })]);
    expect(pageState(mixed, false).kind).toBe("empty");
    expect(hostNotes(mixed).map((h) => h.host)).toEqual(["ghe.example"]);
    // A list from before, its host failing now: the list, and a note.
    const kept = page([pr(1, "mine")], [host({ problem: "rate_limited", message: "x" })]);
    expect(pageState(kept, false).kind).toBe("list");
    expect(hostNotes(kept)).toHaveLength(1);
    expect(hostNotes(signedOut)).toEqual([]);
  });

  it("is stale past ten minutes, or once a reload failed", () => {
    const at = new Date(2026, 2, 1, 14, 5).getTime();
    expect(inboxStale(page([], [host({ loadedAtMs: at })]), at + 60_000)).toBeNull();
    expect(inboxStale(page([], [host({ loadedAtMs: at })]), at + 11 * 60_000)).toBe("as of 14:05");
    expect(inboxStale(page([], [host({ loadedAtMs: at, problem: "offline" })]), at + 60_000)).toBe("as of 14:05");
    expect(inboxStale(page([], [host({ loadedAtMs: at })]), at + 11 * 60_000, "relative")).toBe("as of 11 min ago");
    expect(inboxStale(page([], [host({ loadedAtMs: null })]), at)).toBeNull();
    expect(inboxStale(null, at)).toBeNull();
  });

  it("counts the pull requests with activity, and keys a repository's details", () => {
    expect(activityCount(page([pr(1, "mine", { activity: "updated" }), pr(2, "review", { activity: "new" }), pr(3, "review")]))).toBe(2);
    expect(activityCount(null)).toBe(0);
    expect(inboxKey({ host: "GitHub.com", owner: "Octo", name: "Cat" })).toBe("inbox:github.com/octo/cat");
  });
});
