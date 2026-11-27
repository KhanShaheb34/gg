// What the fake gh answers for the Pull requests page in the end-to-end tests: the page's one
// GraphQL query (`gql-inbox`), made up, shaped as GitHub answers it.
import type { GG } from "./fixtures";

export type Node = {
  number: number;
  repo?: string;
  title?: string;
  author?: string;
  draft?: boolean;
  head?: string;
  base?: string;
  headRepo?: string | null;
  cross?: boolean;
  checks?: "SUCCESS" | "FAILURE" | "PENDING" | null;
  review?: "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED" | null;
  comments?: number;
  lastCommenter?: string;
  reviews?: number;
  oid?: string;
  minutesAgo?: number;
  host?: string;
  /** As the list's lean query gives it: no checks, review or activity (those come with the details). */
  lean?: boolean;
  /** Its state, in the details' answer. */
  state?: "OPEN" | "MERGED" | "CLOSED";
  /** It conflicts with its base. */
  conflict?: boolean;
};

/** A pull request's node id, as the fixtures make it. */
export const idOf = (n: Pick<Node, "number" | "repo">) => `PR_${(n.repo ?? "octo/cat").replace("/", "_")}_${n.number}`;

/** The details' part of a node: state, draft, checks, review, activity. */
function detailsOf(n: Node) {
  return {
    state: n.state ?? "OPEN",
    mergeable: n.conflict ? "CONFLICTING" : "MERGEABLE",
    reviewDecision: n.review === undefined ? "REVIEW_REQUIRED" : n.review,
    comments: { totalCount: n.comments ?? 1, nodes: [{ author: { login: n.lastCommenter ?? "ada" } }] },
    reviews: { totalCount: n.reviews ?? 0, nodes: [] },
    commits: { nodes: [{ commit: { oid: n.oid ?? "a1", statusCheckRollup: n.checks === null ? null : { state: n.checks ?? "SUCCESS" } } }] },
  };
}

/** The details' answer (`gql-inbox-details`) for `nodes`, by their ids. */
export function detailsAnswer(nodes: Node[]) {
  return { data: { nodes: nodes.map((n) => ({ id: idOf(n), isDraft: n.draft ?? false, ...detailsOf(n) })) } };
}

/** Makes the fake gh answer the details' query with `nodes`' details. */
export function replyDetails(gg: GG, nodes: Node[]): void {
  gg.gh.reply("gql-inbox-details", 0, JSON.stringify(detailsAnswer(nodes)));
}

/** The details' queries the fake gh had so far: the ids each asked about. */
export function detailsCalls(gg: GG): string[][] {
  return gg.gh
    .calls()
    .filter((c) => c[0] === "api" && c[1] === "graphql" && c.some((a) => a.includes("GgInboxDetails")))
    .map((c) => c.filter((a) => a.startsWith("ids[]=")).map((a) => a.slice(6)));
}

/** A pull request's files as `gh api --paginate …/files` gives them: one JSON array per page. */
export function replyFiles(gg: GG, number: number, pages: object[][], code = 0, stderr = ""): void {
  gg.gh.reply(`api-files-${number}`, code, pages.map((p) => JSON.stringify(p)).join("\n"), stderr);
}

/** One pull request node of the answer. */
export function node(n: Node) {
  const repo = n.repo ?? "octo/cat";
  const [owner, name] = repo.split("/");
  const headRepo = n.headRepo === undefined ? repo : n.headRepo;
  const host = n.host ?? "github.com";
  const lean = {
    id: idOf(n),
    number: n.number,
    title: n.title ?? `Change ${n.number}`,
    url: `https://${host}/${repo}/pull/${n.number}`,
    isDraft: n.draft ?? false,
    updatedAt: new Date(Date.now() - (n.minutesAgo ?? 30) * 60_000).toISOString().replace(/\.\d+Z$/, "Z"),
    headRefName: n.head ?? "fix/typo",
    baseRefName: n.base ?? "main",
    isCrossRepository: n.cross ?? false,
    repository: { owner: { login: owner }, name },
    headRepository: headRepo === null ? null : { owner: { login: headRepo.split("/")[0] }, name: headRepo.split("/")[1] },
    author: { login: n.author ?? "ada" },
  };
  return n.lean ? lean : { ...lean, ...detailsOf(n) };
}

/** The page's answer: the signed-in account and each group's pull requests. */
export function inboxAnswer(groups: { mine?: Node[]; review?: Node[]; mentioned?: Node[]; assigned?: Node[] }, login = "octo") {
  const search = (nodes: Node[] = []) => ({ issueCount: nodes.length, nodes: nodes.map(node) });
  return {
    data: {
      viewer: { login },
      mine: search(groups.mine),
      review: search(groups.review),
      mentioned: search(groups.mentioned),
      assigned: search(groups.assigned),
    },
  };
}

/** Makes the fake gh answer the page's query with `groups`. */
export function replyInbox(gg: GG, groups: Parameters<typeof inboxAnswer>[0]): void {
  gg.gh.reply("gql-inbox", 0, JSON.stringify(inboxAnswer(groups)));
}

/** The page's queries the fake gh had so far. */
export function inboxCalls(gg: GG): string[][] {
  return gg.gh.calls().filter((c) => c[0] === "api" && c[1] === "graphql" && c.some((a) => a.includes("GgInbox(")));
}

/** A day's mix: two of yours, three to review, two mentioning you. */
export const MIX = {
  mine: [
    { number: 31, title: "Fix a typo in the README", head: "fix/typo", author: "octo", checks: "SUCCESS", review: "APPROVED", minutesAgo: 12 },
    { number: 29, title: "Add the login form", head: "feature/login", author: "octo", draft: true, checks: "PENDING", review: null, minutesAgo: 180 },
  ],
  review: [
    { number: 42, repo: "hub/dog", title: "Speed up the search index", author: "ada", checks: "FAILURE", review: "CHANGES_REQUESTED", minutesAgo: 25 },
    { number: 7, repo: "hub/owl", title: "Explain the parser in the docs", author: "grace", head: "docs/readme", checks: null, minutesAgo: 300 },
    { number: 33, title: "Read the settings file once", author: "grace", head: "docs/readme", checks: "SUCCESS", minutesAgo: 600 },
  ],
  mentioned: [{ number: 12, repo: "hub/dog", title: "Rename the toolbar's buttons", author: "mia", checks: "SUCCESS", review: "APPROVED", minutesAgo: 1440 }],
  assigned: [{ number: 5, repo: "hub/owl", title: "Tidy the release notes", author: "grace", checks: null, review: null, minutesAgo: 4000 }],
} satisfies Parameters<typeof inboxAnswer>[0];
