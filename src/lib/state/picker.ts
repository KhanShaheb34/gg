// Opens the branch picker (⌘P, or the branch name in the repository header) over the window.

import BranchPicker from "../components/BranchPicker.svelte";
import { showDialog } from "./overlays.svelte";

/** Shows the branch picker for repository `repoId`'s active worktree. */
export function openBranchPicker(repoId: number): void {
  showDialog(BranchPicker, { repoId });
}
