// One repository's side panel: whether it shows (RepoPrefs.sidePanel, kept by the core), the
// worktree selected in it, the switch underway, and which view it shows (the worktree list, or
// the commit panel of one worktree). Made with the repository's view (outside
// any reactive context, so everything reading it follows its changes); see worktrees.svelte.ts.

import { api } from "../ipc";

export class PanelState {
  /** RepoPrefs.sidePanel, as last known (shown until the core says otherwise). */
  shownPref = $state(true);
  /** The worktree selected in the list (its path), or null. */
  selected: string | null = $state(null);
  /** The worktree being made active, while the core switches. */
  switching: string | null = $state(null);
  /** Asks to move the focus into the list (bumped; the list follows it). */
  focusRequest = $state(0);
  /** The worktree whose commit panel shows (its path), or null: the worktree list. */
  commitFor: string | null = $state(null);
  /** Asks to move the focus to the commit message (bumped; the commit panel follows it). */
  commitFocus = $state(0);
  /**
   * The `commitFocus` request the commit panel last followed: kept here, not in the panel,
   * so a panel shown again (back to its tab) does not take the focus once more (final
   * review M10).
   */
  commitFocusDone = 0;
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
