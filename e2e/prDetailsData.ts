// What the fake gh answers for a pull request's details (pr_details) in the end-to-end tests:
// `gh pr view <n> --json …` and the review threads' `gh api graphql`, built from the answers
// captured from a public repository (crates/gg-core/tests/fixtures/gh) and made-up ones.
import { readFileSync } from "node:fs";
import path from "node:path";
import type { GG } from "./fixtures";

const FIXTURES = path.resolve(import.meta.dirname, "..", "crates", "gg-core", "tests", "fixtures", "gh");
const read = (name: string) => JSON.parse(readFileSync(path.join(FIXTURES, name), "utf8"));

/** The captured `gh pr view` of a merged pull request: four finished checks, CodeRabbit's long comment and review. */
export const VIEW_22 = read("pr22-view.json");
/** Its captured review threads: one resolved, CodeRabbit's with the owner's reply. */
export const THREADS_22 = read("pr22-threads.json");
/** The captured `gh pr view` of a pull request whose three checks run. */
export const VIEW_25 = read("pr25-view-pending.json");

/** `ms` from now, as GitHub writes a time. */
export const ago = (ms: number) => new Date(Date.now() - ms).toISOString().replace(/\.\d+Z$/, "Z");
const MIN = 60_000;

/** A GitHub Actions check run. `state`: what it is now. */
export function run(name: string, state: "passing" | "failing" | "running" | "queued" | "skipped" | "cancelled", took = 4 * MIN, workflow = "CI") {
  const finished = state !== "running" && state !== "queued";
  const conclusion = { passing: "SUCCESS", failing: "FAILURE", skipped: "SKIPPED", cancelled: "CANCELLED", running: "", queued: "" }[state];
  return {
    __typename: "CheckRun",
    name,
    workflowName: workflow,
    status: finished ? "COMPLETED" : state === "queued" ? "QUEUED" : "IN_PROGRESS",
    conclusion,
    startedAt: state === "queued" ? "0001-01-01T00:00:00Z" : ago(took + (finished ? 3 * MIN : 0)),
    completedAt: finished ? ago(3 * MIN) : "0001-01-01T00:00:00Z",
    detailsUrl: `https://github.com/octo/cat/actions/runs/1/job/${name.length}`,
  };
}

/** A commit status (an older CI service). */
export function status(context: string, state: "SUCCESS" | "FAILURE" | "PENDING") {
  return { __typename: "StatusContext", context, state, targetUrl: "https://ci.example.com/build/7", createdAt: ago(10 * MIN) };
}

/** A conversation comment. */
export function comment(id: string, login: string, body: string, minutesAgo: number) {
  return { id, author: { login }, body, createdAt: ago(minutesAgo * MIN), url: `https://github.com/octo/cat/pull/31#issuecomment-${id}`, isMinimized: false };
}

/** A review summary. */
export function review(id: string, login: string, body: string, state: string, minutesAgo: number) {
  return { id, author: { login }, body, submittedAt: ago(minutesAgo * MIN), state };
}

/** A `gh pr view` answer for pull request `number`. */
export function view(
  number: number,
  o: { merge?: string; mergeable?: string; draft?: boolean; state?: string; review?: string; checks?: object[]; comments?: object[]; reviews?: object[] } = {},
) {
  return {
    number,
    state: o.state ?? "OPEN",
    isDraft: o.draft ?? false,
    mergeable: o.mergeable ?? "MERGEABLE",
    mergeStateStatus: o.merge ?? "CLEAN",
    reviewDecision: o.review ?? "",
    statusCheckRollup: o.checks ?? [],
    comments: o.comments ?? [],
    reviews: o.reviews ?? [],
    url: `https://github.com/octo/cat/pull/${number}`,
    updatedAt: ago(MIN),
  };
}

/**
 * A review thread: its place, state and comments; `id` (else `T<i>`); what the user may do to it
 * (GitHub's viewerCan…): by default reply, and resolve it or unresolve it as it stands.
 */
export type ThreadSpec = {
  id?: string;
  path: string;
  line: number;
  resolved?: boolean;
  outdated?: boolean;
  canReply?: boolean;
  canResolve?: boolean;
  canUnresolve?: boolean;
  comments: { id: string; login: string; bot?: boolean; body: string; minutesAgo: number }[];
};

/**
 * The review threads' GraphQL answer: `threads`, and which authors are bots (`bots`, besides the
 * threads'); `room`: whether the conversation is locked, the repository archived, and the user's
 * permission on it (default: open, not archived, WRITE).
 */
export function threads(threads: ThreadSpec[], bots: string[] = [], room: { locked?: boolean; archived?: boolean; permission?: string } = {}) {
  return {
    data: {
      repository: {
        isArchived: room.archived ?? false,
        viewerPermission: room.permission ?? "WRITE",
        pullRequest: {
          locked: room.locked ?? false,
          reviewThreads: {
            pageInfo: { hasNextPage: false, endCursor: null },
            nodes: threads.map((t, i) => ({
              id: t.id ?? `T${i}`,
              viewerCanReply: t.canReply ?? true,
              viewerCanResolve: t.canResolve ?? !(t.resolved ?? false),
              viewerCanUnresolve: t.canUnresolve ?? (t.resolved ?? false),
              isResolved: t.resolved ?? false,
              isOutdated: t.outdated ?? false,
              path: t.path,
              line: t.line,
              originalLine: t.line,
              comments: {
                nodes: t.comments.map((c) => ({
                  id: c.id,
                  databaseId: 1,
                  url: `https://github.com/octo/cat/pull/31#discussion_r${c.id}`,
                  body: c.body,
                  createdAt: ago(c.minutesAgo * MIN),
                  author: { login: c.login, __typename: c.bot ? "Bot" : "User" },
                })),
              },
            })),
          },
          reviews: { nodes: [] },
          comments: { nodes: bots.map((login) => ({ author: { login, __typename: "Bot" } })) },
        },
      },
    },
  };
}

/** CodeRabbit's long comment and review as captured, for pull request `number`'s answers. */
export function coderabbit(): { comments: object[]; reviews: object[] } {
  return { comments: VIEW_22.comments, reviews: VIEW_22.reviews };
}

/** Has the fake gh answer pull request `number`'s details with `v` and `t`. */
export function answer(gg: GG, number: number, v: object, t: object = threads([])) {
  gg.gh.reply(`pr-view-${number}`, 0, JSON.stringify(v));
  gg.gh.reply(`api-graphql-${number}`, 0, JSON.stringify(t));
}

/** The calls the fake gh had for pull request `number`'s details: `gh pr view` ones. */
export function viewCalls(gg: GG, number: number): number {
  return gg.gh.calls().filter((c) => c[0] === "pr" && c[1] === "view" && c[2] === String(number)).length;
}
