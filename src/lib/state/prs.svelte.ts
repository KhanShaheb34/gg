// Pull requests in the UI (spec §5.5): how a marker reads (its tooltip, whether the list it
// comes from is stale), opening a pull request (only through `open_external`, only at
// addresses the core gave), and the Create pull request dialog (which also opens GitHub's
// compare page, after pushing, when gh cannot make one).
// The core loads the open pull requests with fetches and the user's refreshes; nothing here
// asks gh for anything but `pr_suggest`.

import CreatePullRequestDialog from "../components/dialogs/CreatePullRequestDialog.svelte";
import { clockTime, formatClock, relativeTime, type DateFormat } from "../format";
import type { PrState, PullRequest, RepoSnapshot } from "../types";
import { toast } from "./app.svelte";
import { openExternal } from "./editor";
import { showDialog } from "./overlays.svelte";

/** A list of pull requests older than this is shown as of when it was loaded. */
export const STALE_AFTER_MS = 10 * 60_000;

/** The time now, a minute at a time: markers turn stale without anything else changing. */
export const clock = $state({ now: Date.now() });
if (typeof window !== "undefined") setInterval(() => (clock.now = Date.now()), 60_000);

/**
 * "as of 14:05" (with how long ago, past a day) when the markers of `prs` may be out of date:
 * the list is older than 10 minutes, or the last reload failed. Null when they are fresh, or
 * there is no list.
 */
export function staleSince(prs: PrState, now: number, format: DateFormat = "standard"): string | null {
  if (prs.status !== "ready" || prs.loadedAtMs === null) return null;
  const age = now - prs.loadedAtMs;
  if (prs.reason !== "failed" && age <= STALE_AFTER_MS) return null;
  // In the date format chosen (Settings › Appearance): "as of 12 min ago" when relative.
  if (format === "relative") return `as of ${relativeTime(prs.loadedAtMs, now)}`;
  const at = format === "locale" ? formatClock(prs.loadedAtMs, "locale") : clockTime(prs.loadedAtMs).slice(0, 5);
  return age >= 24 * 3_600_000 ? `as of ${at}, ${relativeTime(prs.loadedAtMs, now)}` : `as of ${at}`;
}

/** A marker's tooltip: the title; base ← head; draft; as of when, if stale. */
export function prTooltip(pr: PullRequest, stale: string | null): string {
  const lines = [`#${pr.number} ${pr.title}`, `${pr.base} ← ${pr.head}`];
  const notes = [pr.state === "draft" ? "Draft" : null, stale].filter((n) => n !== null);
  if (notes.length > 0) lines.push(notes.join(" · "));
  return lines.join("\n");
}

/** Opens pull request `pr` of repository `repoId` in the browser. */
export function openPullRequest(repoId: number, pr: PullRequest): void {
  void openExternal(repoId, { target: "url", url: pr.url });
}

/** Copies `url` (a pull request's, a compare page's). */
export async function copyLink(url: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(url);
  } catch {
    toast("error", "Could not copy the link.");
    return;
  }
  toast("info", "Copied the link.");
}

/** Opens the Create pull request dialog for local branch `branch` of `snap`'s repository. */
export function openCreatePullRequest(snap: RepoSnapshot, branch: string): void {
  showDialog(CreatePullRequestDialog, { repoId: snap.repoId, branch });
}
