// The details of the pull requests the Pull request tab has shown (pr_details: merge status,
// checks, comments), by repository and number, kept while the app runs: shown again at once,
// then asked for again. The tab asks only while it shows one (see PullRequestView); the core
// asks gh at most once every 10 s per pull request whatever the tab does.
//
// An answer is taken only while it is the latest asked for that pull request; and it goes to
// that pull request's entry, so one for another branch's pull request that comes late never
// shows on the branch shown now.

import { SvelteMap } from "svelte/reactivity";
import { api } from "../ipc";
import { checksPhrase, checksTone, type Tone } from "../prDetails";
import type { PrDetails } from "../types";
import { knownPr, subjectOf } from "./pullRequest.svelte";
import type { RepoView } from "./repo.svelte";
import { panelState } from "./worktrees.svelte";

export type DetailsEntry = {
  /** The details as last loaded (kept when a reload fails). */
  data: PrDetails | null;
  /** A load is out. */
  loading: boolean;
  /** The load out is a poll (the Refresh button does not turn for it). */
  background: boolean;
  /** Why the last load failed, in one line; null when it did not. */
  error: string | null;
  /** The number of the latest load asked for. */
  asked: number;
  /** When the latest load was asked for (Date.now()). */
  askedAt: number;
};

const key = (root: string, number: number) => `${root}\n${number}`;
const entries = new SvelteMap<string, DetailsEntry>();

/** What is known of pull request `number` of the repository at `root`, if anything. */
export function detailsOf(root: string, number: number): DetailsEntry | null {
  return entries.get(key(root, number)) ?? null;
}

/** The entry of pull request `number` of `root`, made on first use. */
function entry(root: string, number: number): DetailsEntry {
  const found = entries.get(key(root, number));
  if (found) return found;
  const made = $state<DetailsEntry>({ data: null, loading: false, background: false, error: null, asked: 0, askedAt: 0 });
  entries.set(key(root, number), made);
  return made;
}

/** The message of error `e`, in one line. */
function messageOf(e: unknown): string {
  const text = e instanceof Error ? e.message : typeof e === "object" && e !== null && "message" in e ? String((e as { message: unknown }).message) : String(e);
  return text.split("\n")[0] || "Could not load the checks and comments.";
}

/**
 * Asks the core for pull request `number` of repository `repoId` (at `root`). What was shown
 * stays shown while it loads; a failure keeps it and says why (never a notice: polls would
 * repeat it). `background`: a poll, which the Refresh button does not show.
 */
export async function loadDetails(repoId: number, root: string, number: number, background = false): Promise<void> {
  const e = entry(root, number);
  const n = ++e.asked;
  e.loading = true;
  e.background = background;
  e.askedAt = Date.now();
  try {
    const d = await api.prDetails(repoId, number);
    if (n !== e.asked) return;
    e.data = d;
    e.error = null;
  } catch (err) {
    if (n !== e.asked) return;
    e.error = messageOf(err);
  } finally {
    if (n === e.asked) e.loading = false;
  }
}

/** Whether a load the user would see is out for any pull request of the repository at `root`. */
export function detailsBusy(root: string): boolean {
  for (const [k, e] of entries) if (k.startsWith(`${root}\n`) && e.loading && !e.background) return true;
  return false;
}

/**
 * The checks of the pull request `view`'s Pull request tab is about, as last loaded, for the
 * tab's dot and tooltip: its tone (danger: one fails; warning: one runs; success: all passed)
 * and how the tooltip says it. Null when not loaded, not open, or nothing to say.
 */
export function prChecksOf(view: RepoView): { tone: Exclude<Tone, "neutral">; phrase: string } | null {
  const subject = subjectOf(view);
  if (subject === null) return null;
  const shown = panelState(view.repoId).prShown;
  const number = knownPr(view.snapshot, subject)?.number ?? (shown?.branch === subject ? shown.number : null);
  if (number === null) return null;
  const d = detailsOf(view.snapshot.rootPath, number)?.data;
  if (!d || d.state !== "open") return null;
  const tone = checksTone(d.checksSummary);
  const phrase = checksPhrase(d.checksSummary);
  return tone && tone !== "neutral" && phrase ? { tone, phrase } : null;
}

/** Forgets the details of the repository at `root` (tests; a repository closed). */
export function forgetDetails(root: string): void {
  for (const k of [...entries.keys()]) if (k.startsWith(`${root}\n`)) entries.delete(k);
}
