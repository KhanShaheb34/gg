// One repository's side panel: whether it shows (RepoPrefs.sidePanel, kept by the core), the
// tab it shows (the commit panel, the pull request, or the worktree list), the worktree
// selected in the list, the switch underway, which worktree's commit panel the Commit tab
// holds, which branch the Pull request tab is about, and the drafts written there. Made with the
// repository's view (outside any reactive context, so everything reading it follows its
// changes); see worktrees.svelte.ts.

import { api } from "../ipc";
import { BranchFold } from "./branchFiles.svelte";
import { PrCompose } from "./prDrafts.svelte";

/** The side panel's tabs, in the order its tab strip shows them. */
export const SIDE_TABS = ["commit", "pr", "worktrees"] as const;
export type SideTab = (typeof SIDE_TABS)[number];

/**
 * The tab a key pressed on tab `at` of `tabs` moves to (as the ARIA tabs pattern has it:
 * arrows go round, Home and End to the ends), or null for any other key.
 */
export function tabForKey<T>(tabs: readonly T[], at: T, key: string): T | null {
  const i = tabs.indexOf(at);
  const n = tabs.length;
  if (n === 0) return null;
  switch (key) {
    case "ArrowRight":
      return tabs[(i + 1) % n];
    case "ArrowLeft":
      return tabs[(i - 1 + n) % n];
    case "Home":
      return tabs[0];
    case "End":
      return tabs[n - 1];
    default:
      return null;
  }
}

export class PanelState {
  /** RepoPrefs.sidePanel, as last known (shown until the core says otherwise). */
  shownPref = $state(true);
  /**
   * The tab the panel shows: the one last used in this repository (while the app runs), so
   * the panel shown again comes back on it. The commit panel to begin with.
   */
  tab: SideTab = $state("commit");
  /** The worktree selected in the list (its path), or null. */
  selected: string | null = $state(null);
  /** The worktree being made active, while the core switches. */
  switching: string | null = $state(null);
  /** Asks to move the focus into the list (bumped; the list follows it). */
  focusRequest = $state(0);
  /**
   * The `focusRequest` the list last followed: kept here, not in the list, so a list shown
   * again (the panel shown again, or back on its tab) does not take the focus once more.
   */
  focusDone = 0;
  /**
   * The worktree whose commit panel the Commit tab holds when it is not the active one (its
   * path), or null: the active worktree's.
   */
  commitFor: string | null = $state(null);
  /**
   * The local branch the Pull request tab is about when it is not the active worktree's
   * branch (a branch's menu asked for it), or null: the active worktree's branch, as it
   * changes.
   */
  prFor: string | null = $state(null);
  /** The user asked to refresh the pull requests (bumped): the tab asks gh about its branch again. */
  prRefresh = $state(0);
  /** Asks to move the focus into the Pull request tab (bumped; the tab follows it). */
  prFocus = $state(0);
  /** The pull request the tab last showed, and the branch it is of (for the tab's dot). */
  prShown: { branch: string; number: number } | null = $state(null);
  /** The tab's branch was pushed (bumped): its pull request's checks are asked for again. */
  prPushed = $state(0);
  /** The `prFocus` request the tab last followed (see `commitFocusDone`). */
  prFocusDone = 0;
  /** Asks to move the focus to the commit message (bumped; the commit panel follows it). */
  commitFocus = $state(0);
  /**
   * The `commitFocus` request the commit panel last followed: kept here, not in the panel,
   * so a panel shown again (back to its tab) does not take the focus once more.
   */
  commitFocusDone = 0;
  /** Whether the Commit tab's "Committed on branch" is open (never saved; see BranchFold). */
  branchFold = new BranchFold();
  /**
   * What the user writes in the Pull request tab, per pull request and thread (replies,
   * comments), and the threads being resolved: in memory only, kept while the view is.
   */
  prCompose = new PrCompose();
  /** The user chose since the view opened: the remembered choice no longer applies. */
  chosen = false;

  /** `repoId` null: a stand-in for no repository, read from nowhere. */
  constructor(repoId: number | null) {
    if (repoId !== null) void this.load(repoId);
  }

  private async load(repoId: number): Promise<void> {
    try {
      const prefs = await api.getRepoPrefs(repoId);
      if (!this.chosen) this.shownPref = prefs.sidePanel !== false;
    } catch {
      // Shown, the default.
    }
  }
}
