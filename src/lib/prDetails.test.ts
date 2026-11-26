import { describe, expect, it } from "vitest";
import {
  checksOpenByDefault,
  checkStateText,
  checksPhrase,
  checksSummaryText,
  checksTone,
  checkTiming,
  commentCounts,
  filterComments,
  mergeWording,
  noComments,
  orderChecks,
  reviewWording,
  shortDuration,
  shouldPoll,
  shownLines,
  startsShortened,
  workflowShown,
} from "./prDetails";
import type { CheckState, ChecksSummary, PrCheck, PrComment } from "./types";

const check = (name: string, state: CheckState, o: Partial<PrCheck> = {}): PrCheck => ({
  name,
  workflow: "CI",
  state,
  startedAtMs: null,
  completedAtMs: null,
  url: null,
  ...o,
});

function summary(counts: Partial<Record<CheckState, number>>): ChecksSummary {
  const s = { passing: 0, failing: 0, pending: 0, skipped: 0, cancelled: 0, neutral: 0, unknown: 0, ...counts };
  return { ...s, total: Object.values(s).reduce((a, b) => a + b, 0) };
}

const comment = (id: string, isBot: boolean, o: Partial<PrComment> = {}): PrComment => ({
  id,
  kind: "comment",
  author: isBot ? "coderabbitai" : "ada",
  isBot,
  createdAtMs: 0,
  body: "",
  url: "https://github.com/o/r/pull/1",
  reviewState: null,
  thread: null,
  minimized: false,
  ...o,
});

describe("checks", () => {
  it("are ordered failing, then running, then the rest, each group as gh gave it", () => {
    const checks = [
      check("a", "passing"),
      check("b", "pending"),
      check("c", "skipped"),
      check("d", "failing"),
      check("e", "cancelled"),
      check("f", "failing"),
      check("g", "unknown"),
      check("h", "neutral"),
      check("i", "pending"),
    ];
    expect(orderChecks(checks).map((c) => c.name)).toEqual(["d", "f", "b", "i", "e", "g", "a", "h", "c"]);
    expect(orderChecks([])).toEqual([]);
  });

  it("are summed up in words, what needs looking at first; none said as such", () => {
    expect(checksSummaryText(summary({ passing: 2, pending: 1, failing: 1 }))).toBe("1 failing · 1 running · 2 passing");
    expect(checksSummaryText(summary({ passing: 3, neutral: 1 }))).toBe("3 passing · 1 neutral");
    expect(checksSummaryText(summary({ cancelled: 1, skipped: 2, unknown: 1 }))).toBe("1 cancelled · 2 skipped · 1 unknown");
    expect(checksSummaryText(summary({}))).toBe("No checks");
  });

  it("give the dot its tone: red when one fails, amber while one runs, green when all passed", () => {
    expect(checksTone(summary({ failing: 1, pending: 2, passing: 3 }))).toBe("danger");
    expect(checksTone(summary({ pending: 1, passing: 3 }))).toBe("warning");
    expect(checksTone(summary({ passing: 3, skipped: 1, neutral: 1 }))).toBe("success");
    expect(checksTone(summary({}))).toBe(null);
    expect(checksTone(summary({ cancelled: 1, passing: 1 }))).toBe(null);
    expect(checksTone(summary({ skipped: 2 }))).toBe(null);
    expect(checksPhrase(summary({ failing: 1 }))).toBe("checks failing");
    expect(checksPhrase(summary({ pending: 1 }))).toBe("checks running");
    expect(checksPhrase(summary({ passing: 1 }))).toBe("checks passing");
    expect(checksPhrase(summary({}))).toBe(null);
  });

  it("start open when something fails or runs, closed when all is well", () => {
    expect(checksOpenByDefault(summary({ failing: 1, passing: 2 }))).toBe(true);
    expect(checksOpenByDefault(summary({ pending: 1 }))).toBe(true);
    expect(checksOpenByDefault(summary({ passing: 4, skipped: 1 }))).toBe(false);
    expect(checksOpenByDefault(summary({}))).toBe(false);
  });

  it("say how long they took, or have run", () => {
    const now = 10 * 3_600_000;
    expect(checkTiming(check("a", "passing", { startedAtMs: now - 712_000, completedAtMs: now - 1000 }), now)).toBe("11m 51s");
    expect(checkTiming(check("a", "failing", { startedAtMs: 0, completedAtMs: 45_000 }), now)).toBe("45s");
    expect(checkTiming(check("a", "pending", { startedAtMs: now - 125_000 }), now)).toBe("running 2m");
    expect(checkTiming(check("a", "pending", { startedAtMs: now - 20_000 }), now)).toBe("running under 1m");
    expect(checkTiming(check("a", "pending", { startedAtMs: now - 3_900_000 }), now)).toBe("running 1h 5m");
    expect(checkTiming(check("a", "pending"), now)).toBe("queued");
    expect(checkTiming(check("a", "skipped", { startedAtMs: 0, completedAtMs: 0 }), now)).toBe(null);
    expect(checkTiming(check("a", "passing"), now)).toBe(null);
    expect(checkTiming(check("a", "passing", { startedAtMs: 10, completedAtMs: 5 }), now)).toBe(null);
    expect(shortDuration(3_600_000)).toBe("1h");
    expect(shortDuration(120_000)).toBe("2m");
    expect(shortDuration(3_660_000)).toBe("1h 1m");
  });

  it("name their workflow only when it says more than their own name", () => {
    expect(workflowShown(check("ubuntu-latest", "passing"))).toBe("CI");
    expect(workflowShown(check("CI / lint", "passing"))).toBe(null);
    expect(workflowShown(check("CodeRabbit", "neutral", { workflow: null }))).toBe(null);
    expect(workflowShown(check("x", "neutral", { workflow: "  " }))).toBe(null);
    expect(checkStateText("pending")).toBe("Running");
    expect(checkStateText("pending", false)).toBe("Queued");
    expect(checkStateText("failing")).toBe("Failed");
  });
});

describe("the status line", () => {
  const s = (merge: string, counts: Partial<Record<CheckState, number>> = {}, state = "open") =>
    mergeWording({ state: state as "open", merge: merge as "ready", checksSummary: summary(counts) }, "main").text;

  it("words every merge status, against the base", () => {
    expect(s("ready")).toBe("Ready to merge");
    expect(s("draft")).toBe("Draft");
    expect(s("conflicts")).toBe("Conflicts with main");
    expect(s("behind")).toBe("Behind main");
    expect(s("blocked")).toBe("Blocked");
    expect(s("unstable", { failing: 1, pending: 1 })).toBe("Checks failing");
    expect(s("unstable", { pending: 1 })).toBe("Checks running");
    expect(s("unstable", { cancelled: 1 })).toBe("Checks not passing");
    expect(s("unknown", { pending: 2 })).toBe("Checks running");
    expect(s("unknown")).toBe("Merge status unknown");
    expect(s("ready", {}, "merged")).toBe("Merged");
    expect(s("unknown", {}, "closed")).toBe("Closed");
    expect(mergeWording({ state: "open", merge: "conflicts", checksSummary: summary({}) }, "main").tone).toBe("danger");
    expect(mergeWording({ state: "open", merge: "ready", checksSummary: summary({}) }, "main").tone).toBe("success");
  });

  it("adds the review decision when there is one", () => {
    expect(reviewWording("approved")).toEqual({ text: "Approved", tone: "success" });
    expect(reviewWording("changes_requested")?.text).toBe("Changes requested");
    expect(reviewWording("review_required")?.text).toBe("Review required");
    expect(reviewWording("none")).toBe(null);
  });
});

describe("comments", () => {
  const thread = (replies: boolean[], firstBot: boolean) =>
    comment(`t${replies.join("")}${firstBot}`, firstBot, {
      kind: "thread",
      thread: {
        id: "T1",
        path: "a.ts",
        canReply: true,
        canResolve: true,
        canUnresolve: false,
        line: 1,
        resolved: false,
        outdated: false,
        replies: replies.map((isBot, i) => ({ id: `r${i}`, author: isBot ? "bot" : "ada", isBot, createdAtMs: 0, body: "", url: "" })),
      },
    });
  const list = [comment("h", false), comment("b", true), thread([false], true), thread([true], false), thread([], true)];

  it("are filtered by who wrote them: a thread shows for whoever wrote any of it", () => {
    expect(filterComments(list, "all").map((c) => c.id)).toEqual(list.map((c) => c.id));
    expect(filterComments(list, "humans").map((c) => c.id)).toEqual(["h", "tfalsetrue", "ttruefalse"]);
    expect(filterComments(list, "bots").map((c) => c.id)).toEqual(["b", "tfalsetrue", "ttruefalse", "ttrue"]);
    expect(commentCounts(list)).toEqual({ all: 5, humans: 3, bots: 4 });
    expect(filterComments([], "bots")).toEqual([]);
  });

  it("say so when there are none to show", () => {
    expect(noComments("all")).toBe("No comments yet.");
    expect(noComments("humans")).toBe("No comments from people.");
    expect(noComments("bots")).toBe("No comments from bots.");
  });

  it("start shortened: a resolved thread or a hidden comment to its first lines, any other when long", () => {
    expect(startsShortened(thread([], false))).toBe("long");
    const resolved = thread([false], false);
    resolved.thread!.resolved = true;
    expect(startsShortened(resolved)).toBe("resolved");
    expect(startsShortened(comment("m", false, { minimized: true }))).toBe("resolved");
    expect(startsShortened(comment("x", true))).toBe("long");
    expect(shownLines("resolved")).toBe(3);
    expect(shownLines("long")).toBe(12);
  });
});

describe("asking again while checks run", () => {
  const all = { pending: true, tabShown: true, focused: true, visible: true };

  it("only while a check runs, the tab shows it, and the window is focused and visible", () => {
    expect(shouldPoll(all)).toBe(true);
    for (const key of Object.keys(all) as (keyof typeof all)[]) {
      expect(shouldPoll({ ...all, [key]: false }), key).toBe(false);
    }
    expect(shouldPoll({ pending: false, tabShown: false, focused: false, visible: false })).toBe(false);
  });
});
