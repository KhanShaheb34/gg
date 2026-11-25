// The side panel's Pull request tab: which branch it is about (its subject), what it can show
// for it (the pull request, the form to make one, or why none can be made), the forms being
// filled in (kept per repository and branch while the app runs, whatever is shown), the pull
// requests just made (shown at once, until the list the core reloads has them), and the push
// that shows the tab by itself. Everything goes through `gh` in the core; nothing here asks
// GitHub for anything.

import { SvelteMap } from "svelte/reactivity";
import type { OpOutcome, PrState, PrSuggestion, PullRequest, RepoSnapshot } from "../types";
import { app } from "./app.svelte";
import { overlays } from "./overlays.svelte";
import { findRepoView, type RepoView } from "./repo.svelte";
import { panelState, setPanelShown } from "./worktrees.svelte";

// --- The branch the tab is about --------------------------------------------------------

/**
 * The local branch the Pull request tab of `snap`'s repository is about: the branch it was
 * pinned to (`prFor`) while that is still a local branch, else the active worktree's branch;
 * null when HEAD is detached there (and nothing is pinned).
 */
export function prSubject(prFor: string | null, snap: Pick<RepoSnapshot, "branches" | "head">): string | null {
  if (prFor !== null && snap.branches.includes(prFor)) return prFor;
  return snap.head.branch;
}

/** The branch `view`'s Pull request tab is about (see `prSubject`). */
export function subjectOf(view: RepoView): string | null {
  return prSubject(panelState(view.repoId).prFor, view.snapshot);
}

/**
 * The remote a pull request goes to, as the core would guess it before asking: upstream,
 * github or origin, else the first (the core's rule when no remote is marked as the base).
 */
export function baseRemoteOf(remotes: readonly string[]): string | null {
  return ["upstream", "github", "origin"].find((r) => remotes.includes(r)) ?? remotes[0] ?? null;
}

/**
 * The repository's default branch on `remote` (the base remote when omitted): what its HEAD
 * names, else main or master when the remote has it; null when it cannot be told.
 */
export function defaultBranch(
  snap: Pick<RepoSnapshot, "remotes" | "remoteHeads" | "remoteBranches">,
  remote: string | null = baseRemoteOf(snap.remotes),
): string | null {
  if (remote === null) return null;
  const head = snap.remoteHeads[remote];
  if (head) return head;
  return ["main", "master"].find((b) => snap.remoteBranches.includes(`${remote}/${b}`)) ?? null;
}

// --- Pull requests just made -------------------------------------------------------------

const key = (root: string, branch: string) => `${root}\n${branch}`;

/**
 * Pull requests made from the tab (by repository and branch), shown at once: the core
 * reloads the list after making one, and until a list loaded since has come, this stands in
 * (`listAt`: when the list shown then was loaded).
 */
const made = new SvelteMap<string, { pr: PullRequest; listAt: number | null }>();

/** Records `pr`, just made from `branch` of the repository of `snap` (as it was then). */
export function rememberMade(snap: Pick<RepoSnapshot, "rootPath" | "prs">, branch: string, pr: PullRequest): void {
  made.set(key(snap.rootPath, branch), { pr, listAt: snap.prs.loadedAtMs });
}

/**
 * The open pull request of local branch `branch` as `snap` knows it: the list's, else one
 * just made from the tab while no list has been loaded since; null when neither has one
 * (gh may still know of one: see the suggestion's `existing`).
 */
export function knownPr(snap: Pick<RepoSnapshot, "rootPath" | "branchPrs" | "prs">, branch: string): PullRequest | null {
  const listed = snap.branchPrs[branch];
  if (listed) return listed;
  const just = made.get(key(snap.rootPath, branch));
  return just && just.listAt === snap.prs.loadedAtMs ? just.pr : null;
}

/** The number of a pull request's page (`…/pull/21` → 21), or null. */
export function prNumberOf(url: string): number | null {
  const m = /\/pull\/(\d+)(?:[/?#]|$)/.exec(url);
  return m ? Number(m[1]) : null;
}

// --- The forms being filled in ------------------------------------------------------------

/**
 * A pull request being written for a branch: what the form holds, what the user changed, and
 * the request out for it (kept here, not in the form: shown again, the form says it).
 */
export type PrDraft = {
  title: string;
  body: string;
  base: string;
  draft: boolean;
  /** Changed by the user: a suggestion arriving later leaves it as it is. */
  titleTouched: boolean;
  bodyTouched: boolean;
  baseTouched: boolean;
  /** The request is out. */
  pending: boolean;
  /** The step it is at, as the core says it (pushing, creating). */
  step: string | null;
  /** git's own latest progress line, while pushing. */
  progress: string | null;
  /** How the last request failed (gh's reason, with the log's entries). */
  failure: OpOutcome | null;
  /** A request the core refused outright (a bad title, say). */
  refused: string | null;
};

const drafts = new Map<string, PrDraft>();

/**
 * The form being filled in for local branch `branch` of the repository at `root`: the same
 * one whenever the tab shows it while the app runs (another tab, the panel hidden, another
 * repository's tab in between), until its pull request is made.
 */
export function prDraft(root: string, branch: string): PrDraft {
  const found = drafts.get(key(root, branch));
  if (found) return found;
  const d = $state<PrDraft>({
    title: "",
    body: "",
    base: "",
    draft: false,
    titleTouched: false,
    bodyTouched: false,
    baseTouched: false,
    pending: false,
    step: null,
    progress: null,
    failure: null,
    refused: null,
  });
  drafts.set(key(root, branch), d);
  return d;
}

/** Forgets the form of `branch` (its pull request was made). */
export function dropPrDraft(root: string, branch: string): void {
  drafts.delete(key(root, branch));
}

/**
 * Fills the fields of `draft` the user has not changed from `suggestion`. The base: the
 * suggested one, unless the user picked one in the same repository.
 */
export function applySuggestion(draft: PrDraft, suggestion: PrSuggestion, guessedRemote: string | null): void {
  if (!draft.titleTouched) draft.title = suggestion.title;
  if (!draft.bodyTouched) draft.body = suggestion.body;
  if (suggestion.baseRemote !== guessedRemote || !draft.baseTouched) draft.base = suggestion.baseRemote === null ? "" : suggestion.base;
}

// --- What the core suggested, while it holds -----------------------------------------------

type Kept = { stamp: string; at: number; suggestion: PrSuggestion };
const suggestions = new Map<string, Kept>();
/** A suggestion is asked for again after this long, whatever else changed. */
export const SUGGESTION_FRESH_MS = 5 * 60_000;

/**
 * What the suggestion for `branch` depends on in `snap`: the branch's commit, where it was
 * pushed, and the pull requests' list. Any of these changed, it is asked for again.
 */
export function suggestionStamp(snap: RepoSnapshot, branch: string): string {
  const tip = snap.branchTips[snap.branches.indexOf(branch)]?.hash ?? "";
  const remote = snap.remoteBranches
    .map((name, i) => (name.endsWith(`/${branch}`) ? `${name}@${snap.remoteBranchTips[i]?.hash ?? ""}` : ""))
    .filter((s) => s !== "")
    .join(",");
  return [tip, remote, snap.prs.loadedAtMs ?? "", snap.remotes.join(",")].join("|");
}

/** The suggestion kept for `branch` while `stamp` holds and it is fresh; null to ask again. */
export function keptSuggestion(root: string, branch: string, stamp: string, now = Date.now()): PrSuggestion | null {
  const kept = suggestions.get(key(root, branch));
  if (!kept || kept.stamp !== stamp || now - kept.at > SUGGESTION_FRESH_MS) return null;
  return kept.suggestion;
}

/** Keeps `suggestion` for `branch`, as of `stamp`. */
export function keepSuggestion(root: string, branch: string, stamp: string, suggestion: PrSuggestion, now = Date.now()): void {
  suggestions.set(key(root, branch), { stamp, at: now, suggestion });
}

/** The last suggestion for `branch`, whatever it was asked against (shown until a new one comes). */
export function lastSuggestion(root: string, branch: string): PrSuggestion | null {
  return suggestions.get(key(root, branch))?.suggestion ?? null;
}

/** Forgets the suggestions of the repository at `root` (the user asked to refresh). */
export function forgetSuggestions(root: string): void {
  for (const k of [...suggestions.keys()]) if (k.startsWith(`${root}\n`)) suggestions.delete(k);
}

// --- Showing the tab -----------------------------------------------------------------------

/**
 * Shows the Pull request tab of `view`'s side panel (and the panel) for local branch
 * `branch`, or the active worktree's branch (null). `focus`: asked for by the user (a menu, the
 * shortcut): the focus goes into the tab, to the title when there is a form.
 */
export function openPullRequestTab(view: RepoView, branch: string | null, focus: boolean): void {
  const state = panelState(view.repoId);
  state.prFor = branch === null || branch === view.snapshot.head.branch ? null : branch;
  state.tab = "pr";
  if (focus) state.prFocus++;
  setPanelShown(view.repoId, true);
}

/** Back from a pinned branch to the active worktree's, the focus staying in the tab. */
export function prBackToCurrent(view: RepoView): void {
  const state = panelState(view.repoId);
  state.prFor = null;
  state.prFocus++;
}

/** Why the Pull request tab cannot make one, as its states name it. */
export type PrUnavailableHere = "no_remote" | "not_github" | "no_gh" | "signed_out";

/** Why pull requests cannot be made through gh in a repository with `prs` and `remotes`, if they cannot. */
export function unavailableHere(prs: PrState, remotes: readonly string[]): PrUnavailableHere | null {
  if (remotes.length === 0) return "no_remote";
  if (prs.status !== "unavailable" || prs.reason === null || prs.reason === "failed") return null;
  return prs.reason;
}

/** The tab's tooltip: what it is, the state of its branch's pull request, its shortcut. */
export function prTabTitle(snap: RepoSnapshot, subject: string | null, keys: string): string {
  let state = "";
  if (subject !== null) {
    const pr = knownPr(snap, subject);
    if (pr) state = ` · #${pr.number} ${pr.state === "draft" ? "draft" : "open"}`;
    else if (snap.prs.status === "ready") state = " · none yet";
  }
  return `Pull request${state} (${keys})`;
}

// --- After a push ---------------------------------------------------------------------------

/** What decides whether a push shows the Pull request tab by itself. */
export type AfterPush = {
  /** Settings › Pull requests: show the tab after a push. */
  setting: boolean;
  /** A dialog (or sheet) is open: nothing behind it changes. */
  dialogOpen: boolean;
  /** The repository is the one the window shows. */
  activeRepo: boolean;
  /** The branch pushed (a local branch, by its short name). */
  branch: string;
  snap: Pick<RepoSnapshot, "prs" | "branches" | "branchPrs" | "remoteHeads" | "remotes" | "remoteBranches" | "rootPath">;
};

/**
 * Whether a push the user asked for, which went through, shows the Pull request tab for the
 * branch pushed: the setting is on, no dialog is open, the window shows the repository, its
 * pull requests are known (listed: an older list is taken as it is, as the markers do), the
 * branch is a local one with no open pull request, and it is not the default branch.
 */
export function opensAfterPush(a: AfterPush): boolean {
  if (!a.setting || a.dialogOpen || !a.activeRepo) return false;
  if (a.snap.prs.status !== "ready") return false;
  if (!a.snap.branches.includes(a.branch)) return false;
  if (knownPr(a.snap, a.branch) !== null) return false;
  return a.branch !== defaultBranch(a.snap);
}

/**
 * A push the user asked for went through: shows the tab for its branch, when it should,
 * without taking the focus. (The app has `runOp` tell it of each: `onUserPush`.)
 */
export function afterPush(repoId: number, branch: string): void {
  const view = findRepoView(repoId);
  if (!view) return;
  const go = opensAfterPush({
    setting: app.settings.openPrTabAfterPush,
    dialogOpen: overlays.dialog !== null,
    activeRepo: app.activeRepoId === repoId,
    branch,
    snap: view.snapshot,
  });
  if (go) openPullRequestTab(view, branch, false);
}
