// The Pull requests page across repositories: whether it shows (app.inboxOpen), the list as
// last known (what gg kept shows at once; gh is asked again behind it), the pull request whose
// page is open, and what that page offers: going to the repository it is open in, checking out
// its branch there (gg's own checkout, only when asked), cloning it, its details and writes
// (PullRequestView, through inboxPrSource).
//
// gh is asked only while the page shows: when it opens (unless asked in the last 30 s), when
// the window comes back to the front (unless asked in the last 2 minutes), and on Refresh. The
// core asks gh at most once every 10 s whatever the page does.

import { api } from "../ipc";
import { inboxKey } from "../inbox";
import type { InboxPr, InboxRepoMatch, PrInbox, PullRequest } from "../types";
import { activate, app, reportError, toast } from "./app.svelte";
import { openWebPage } from "./editor";
import { loadDetailsWith } from "./prDetails.svelte";
import type { PrTarget } from "./prCompose.svelte";
import { PrCompose } from "./prDrafts.svelte";
import type { PrSource } from "./prSource.svelte";
import { checkOutBranch } from "../menus/checkout";
import { runOp } from "./ops.svelte";
import { findRepoView } from "./repo.svelte";
import { activateWorktree } from "./worktrees.svelte";

/** Least time between two loads the page asks for when it opens, and when the window comes back. */
export const OPEN_FLOOR_MS = 30_000;
export const FOCUS_FLOOR_MS = 120_000;

export const inbox = $state({
  /** The page as last known: what gg kept, then each load's answer. */
  data: null as PrInbox | null,
  /** A load the user would see is out (the Refresh button turns). */
  loading: false,
  /** Why the core could not be asked at all (gh's own failures are in `data`). */
  error: null as string | null,
  /** When gh was last asked (Date.now()); 0: never in this run. */
  askedAt: 0,
  /** The key of the row selected in the list. */
  cursor: null as string | null,
  /** The pull request whose page shows; null: the list. */
  pr: null as InboxPr | null,
  /** Grows with each Refresh: the open pull request's details load again. */
  refreshTick: 0,
  /** The list's scroll position, kept while a pull request's page shows. */
  scrollTop: 0,
});

let kept = false;

/** Shows the page: what gg kept at once, then gh is asked (unless it was in the last 30 s). */
export function openInbox(): void {
  app.inboxOpen = true;
  inbox.pr = null;
  if (!kept) {
    kept = true;
    void api
      .prInbox(false)
      .then((data) => {
        // A load that answered meanwhile is newer.
        if (inbox.data === null) inbox.data = data;
      })
      .catch(() => {});
  }
  void refreshInbox("open");
}

/** Puts the page away: the active repository shows again (or the welcome page). */
export function closeInbox(): void {
  app.inboxOpen = false;
  inbox.pr = null;
}

/** The page's shortcut: shows it, or from it, goes back. */
export function toggleInbox(): void {
  if (app.inboxOpen) closeInbox();
  else openInbox();
}

/** Asks gh for the page again, for `why`: when it opens and on focus only past their floors; on Refresh always. */
export async function refreshInbox(why: "open" | "focus" | "user"): Promise<void> {
  if (!app.inboxOpen) return;
  const since = Date.now() - inbox.askedAt;
  if (why === "open" && since < OPEN_FLOOR_MS) return;
  if (why === "focus" && since < FOCUS_FLOOR_MS) return;
  if (why === "user") inbox.refreshTick++;
  if (inbox.loading) return;
  inbox.loading = true;
  inbox.askedAt = Date.now();
  try {
    const data = await api.prInbox(true);
    inbox.data = data;
    inbox.error = null;
    // The open pull request as the new list has it (a new title, checks), if still listed.
    if (inbox.pr) inbox.pr = data.prs.find((p) => p.key === inbox.pr!.key) ?? inbox.pr;
  } catch (e) {
    inbox.error = e instanceof Error ? e.message : String(e);
  } finally {
    inbox.loading = false;
  }
}

/** Opens `pr`'s page, recording it as seen (its activity marker goes). */
export function showInboxPr(pr: InboxPr): void {
  inbox.cursor = pr.key;
  inbox.pr = pr;
  if (pr.activity !== "none") {
    const listed = inbox.data?.prs.find((p) => p.key === pr.key);
    if (listed) listed.activity = "none";
  }
  api.prInboxSeen(pr.key).catch(() => {});
}

/** Back from a pull request's page to the list (Escape). */
export function backToList(): void {
  inbox.pr = null;
}

/** `pr` as PullRequestView shows one. */
export function asPullRequest(pr: InboxPr): PullRequest {
  return { number: pr.number, title: pr.title, url: pr.url, state: pr.draft ? "draft" : "open", head: pr.head, base: pr.base };
}

const sources = new Map<string, PrSource>();

/** Where `pr`'s details come from and its writes go: gh, by its repository's name. */
export function inboxPrSource(pr: InboxPr): PrSource {
  const key = inboxKey(pr);
  const known = sources.get(key);
  if (known) return known;
  const { host, owner, name } = pr;
  const load = (number: number, background = false) =>
    loadDetailsWith(key, number, () => api.inboxPrDetails(host, owner, name, number), background);
  const target = (number: number): PrTarget => ({
    number,
    async send(request) {
      const written = await api.inboxPrWrite(host, owner, name, request);
      // No command log for a repository here: the failure says it all (logFrom 0: no Show log).
      return written.done ? null : { error: written.message, logFrom: 0 };
    },
    reload: () => load(number),
  });
  const made: PrSource = {
    key,
    load,
    target,
    // Its repository's own drafts (they are kept by pull request number), in memory only, as
    // the side panel's are.
    compose: new PrCompose(),
    showLog: () => {},
    openPage: (url) => void openWebPage("url", url),
    openLink: (url) => void openWebPage("link", url),
    get refresh() {
      return String(inbox.refreshTick);
    },
    pushed: 0,
  };
  sources.set(key, made);
  return made;
}

/** The first open repository (in tab order) that has `pr`'s repository as a remote, if any. */
export async function findOpenRepo(pr: InboxPr): Promise<InboxRepoMatch | null> {
  let found: InboxRepoMatch[];
  try {
    found = await api.inboxRepoMatch(pr.host, pr.owner, pr.name, pr.headRepo);
  } catch {
    return null;
  }
  for (const tab of app.tabs) {
    const m = found.find((f) => f.repoId === tab.repoId);
    if (m) return m;
  }
  return null;
}

/** Shows repository `repoId`'s tab (the page goes). */
export function goToRepo(repoId: number): void {
  closeInbox();
  activate(repoId);
}

/**
 * How `pr`'s head branch can be checked out in the open repository `match` is: it is the
 * current branch already; it is checked out in another worktree (that one is made active);
 * a local branch of its name (`local`); its remote's branch, fetched before (`remote`); or on
 * the remote, not fetched yet (`fetch`: fetched into a local branch first). Null: it cannot
 * be (its head is in a repository that is no remote here, or a local branch of its name is
 * another one's).
 */
export type Checkout =
  | { kind: "current"; branch: string }
  | { kind: "worktree"; branch: string; path: string }
  | { kind: "local"; branch: string }
  | { kind: "remote"; branch: string; remoteBranch: string }
  | { kind: "fetch"; branch: string; remoteBranch: string };

export function checkoutOf(pr: InboxPr, match: InboxRepoMatch): Checkout | null {
  const view = findRepoView(match.repoId);
  if (!view || match.headRemote === null || pr.head === "") return null;
  const snap = view.snapshot;
  const branch = pr.head;
  const remoteBranch = `${match.headRemote}/${branch}`;
  if (snap.branches.includes(branch)) {
    // A fork's branch: only a local branch that follows it is the pull request's.
    if (pr.cross && view.loadedLabel(branch)?.upstream !== remoteBranch) return null;
    if (snap.head.branch === branch) return { kind: "current", branch };
    const wt = snap.worktrees.find((w) => w.branch === branch);
    if (wt) return { kind: "worktree", branch, path: wt.path };
    return { kind: "local", branch };
  }
  if (snap.remoteBranches.includes(remoteBranch)) return { kind: "remote", branch, remoteBranch };
  return { kind: "fetch", branch, remoteBranch };
}

/**
 * Checks out `pr`'s branch in repository `match` as `plan` says, with gg's own operations and
 * their safety (uncommitted changes, a merge in progress): its tab shows first, so what the
 * operation asks or says is seen there.
 */
export async function checkOutInRepo(match: InboxRepoMatch, plan: Checkout): Promise<void> {
  const view = findRepoView(match.repoId);
  if (!view) return;
  goToRepo(match.repoId);
  const worktree = view.snapshot.activeWorktree;
  switch (plan.kind) {
    case "current":
      return;
    case "worktree": {
      const wt = view.snapshot.worktrees.find((w) => w.path === plan.path);
      if (wt) activateWorktree(view, wt);
      return;
    }
    case "local":
      checkOutBranch(match.repoId, plan.branch, worktree);
      return;
    case "remote":
      await runOp(match.repoId, { op: "checkout_remote", remoteBranch: plan.remoteBranch, localName: plan.branch, expectWorktree: worktree }).catch(() => {});
      return;
    case "fetch": {
      let fetched;
      try {
        fetched = await runOp(match.repoId, { op: "fetch_into_local", remoteBranch: plan.remoteBranch, localName: plan.branch });
      } catch {
        return; // reported by runOp
      }
      if (fetched.status !== "done") return;
      checkOutBranch(match.repoId, plan.branch, view.snapshot.activeWorktree);
      return;
    }
  }
}

/** Opens `url` (a pull request's page) in the browser, saying why not when it cannot. */
export function openInBrowser(url: string): void {
  void openWebPage("url", url);
}

/** Reports a failure of the page's own (the core could not be asked). */
export function inboxFailed(e: unknown, what: string): void {
  reportError(e, what);
}

/** Says `message` as a quiet notice. */
export function inboxSays(message: string, note?: string): void {
  toast("info", message, note);
}
