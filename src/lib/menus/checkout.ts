// Checking out a local branch, the same way from its menu, a double click on its label and
// the "check it out instead" of the remote branch dialog: a branch behind its upstream and
// not ahead of it asks whether to fast-forward it first (spec §5.3); one level with its
// upstream, or diverged from it, or without one, is checked out directly.

import FastForwardDialog from "../components/dialogs/FastForwardDialog.svelte";
import { runOp } from "../state/ops.svelte";
import { showDialog } from "../state/overlays.svelte";
import { findRepoView } from "../state/repo.svelte";
import type { RefLabel } from "../types";

/**
 * Checks out local branch `branch` of repository `repoId`. `label`: its label as the menu
 * had it; without one, its label on the loaded rows, if any, says how far behind it is.
 */
export function checkOutBranch(repoId: number, branch: string, label?: RefLabel): void {
  const known = label ?? findRepoView(repoId)?.loadedLabel(branch);
  if (known && known.upstream !== null && known.behind > 0 && known.ahead === 0) {
    showDialog(FastForwardDialog, { repoId, branch, upstream: known.upstream, behind: known.behind });
    return;
  }
  runOp(repoId, { op: "checkout_branch", branch, fastForward: false }).catch(() => {}); // reported by runOp
}
